#![allow(clippy::missing_errors_doc, clippy::doc_markdown)]
//! Storage for OIDC identity links (`(provider, subject) → user_id`).
//!
//! One [`OidcIdentity`] row is inserted the first time a user signs in via a
//! given provider; subsequent sign-ins look up the same row via
//! `get_by_external` and reuse the linked `user_id`. The uniqueness
//! constraint on `(provider, subject)` enforces the one-subject-one-user
//! invariant.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use tokio::sync::RwLock;

use super::StorageError;

// `OidcIdentity` (struct + `::new` constructor) now lives in
// `zlayer_types::storage`. Re-exported here so `use ...storage::oidc_identities::OidcIdentity`
// imports keep resolving alongside the trait/impl types defined below.
pub use zlayer_types::storage::OidcIdentity;

#[async_trait]
pub trait OidcIdentityStorage: Send + Sync {
    /// Upsert a row by `(provider, subject)`. If the composite already exists,
    /// updates the matching row's `user_id`, `email_at_link`, `updated_at`.
    async fn store(&self, identity: &OidcIdentity) -> Result<(), StorageError>;

    /// Lookup by provider + subject — the hot path on every OIDC sign-in.
    async fn get_by_external(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<OidcIdentity>, StorageError>;

    /// All providers linked to a given user. Used by future "linked accounts"
    /// views in the Manager UI.
    async fn list_for_user(&self, user_id: &str) -> Result<Vec<OidcIdentity>, StorageError>;

    /// Unlink one identity row. Returns `true` if a row was removed.
    async fn delete(&self, id: &str) -> Result<bool, StorageError>;
}

// ---- SQLite-backed implementation ----

pub struct SqlxOidcIdentityStore {
    pool: SqlitePool,
}

impl SqlxOidcIdentityStore {
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let path_str = path.as_ref().display().to_string();
        let connection_string = format!("sqlite:{path_str}?mode=rwc");
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await?;
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await?;
        sqlx::query("PRAGMA busy_timeout=5000")
            .execute(&pool)
            .await?;
        Self::init_schema(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn in_memory() -> Result<Self, StorageError> {
        let pool = SqlitePool::connect(":memory:").await?;
        Self::init_schema(&pool).await?;
        Ok(Self { pool })
    }

    async fn init_schema(pool: &SqlitePool) -> Result<(), StorageError> {
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS oidc_identities (
                id TEXT PRIMARY KEY NOT NULL,
                user_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                subject TEXT NOT NULL,
                email_at_link TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(provider, subject)
            )
            ",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_oidc_identities_user_id ON oidc_identities(user_id)",
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    fn row_to_identity(row: &sqlx::sqlite::SqliteRow) -> Result<OidcIdentity, StorageError> {
        let created_at: String = row.try_get("created_at")?;
        let updated_at: String = row.try_get("updated_at")?;
        Ok(OidcIdentity {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            provider: row.try_get("provider")?,
            subject: row.try_get("subject")?,
            email_at_link: row.try_get("email_at_link")?,
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map_err(|e| StorageError::Serialization(e.to_string()))?
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&updated_at)
                .map_err(|e| StorageError::Serialization(e.to_string()))?
                .with_timezone(&Utc),
        })
    }
}

#[async_trait]
impl OidcIdentityStorage for SqlxOidcIdentityStore {
    async fn store(&self, identity: &OidcIdentity) -> Result<(), StorageError> {
        sqlx::query(
            r"
            INSERT INTO oidc_identities
                (id, user_id, provider, subject, email_at_link, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(provider, subject) DO UPDATE SET
                user_id = excluded.user_id,
                email_at_link = excluded.email_at_link,
                updated_at = excluded.updated_at
            ",
        )
        .bind(&identity.id)
        .bind(&identity.user_id)
        .bind(&identity.provider)
        .bind(&identity.subject)
        .bind(identity.email_at_link.as_deref())
        .bind(identity.created_at.to_rfc3339())
        .bind(identity.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_by_external(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<OidcIdentity>, StorageError> {
        let row = sqlx::query(
            r"SELECT id, user_id, provider, subject, email_at_link, created_at, updated_at
              FROM oidc_identities WHERE provider = ? AND subject = ?",
        )
        .bind(provider)
        .bind(subject)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(Self::row_to_identity).transpose()
    }

    async fn list_for_user(&self, user_id: &str) -> Result<Vec<OidcIdentity>, StorageError> {
        let rows = sqlx::query(
            r"SELECT id, user_id, provider, subject, email_at_link, created_at, updated_at
              FROM oidc_identities WHERE user_id = ? ORDER BY provider",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(Self::row_to_identity).collect()
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let result = sqlx::query("DELETE FROM oidc_identities WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

// ---- In-memory implementation ----

pub struct InMemoryOidcIdentityStore {
    inner: RwLock<HashMap<String, OidcIdentity>>,
}

impl InMemoryOidcIdentityStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryOidcIdentityStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OidcIdentityStorage for InMemoryOidcIdentityStore {
    async fn store(&self, identity: &OidcIdentity) -> Result<(), StorageError> {
        let mut map = self.inner.write().await;
        // Enforce UNIQUE(provider, subject) by searching for an existing row
        // with the same composite and reusing its id.
        let existing_id = map
            .values()
            .find(|v| v.provider == identity.provider && v.subject == identity.subject)
            .map(|v| v.id.clone());

        let mut to_store = identity.clone();
        if let Some(id) = existing_id {
            to_store.id = id;
            to_store.updated_at = Utc::now();
        }
        map.insert(to_store.id.clone(), to_store);
        Ok(())
    }

    async fn get_by_external(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<OidcIdentity>, StorageError> {
        let map = self.inner.read().await;
        Ok(map
            .values()
            .find(|v| v.provider == provider && v.subject == subject)
            .cloned())
    }

    async fn list_for_user(&self, user_id: &str) -> Result<Vec<OidcIdentity>, StorageError> {
        let map = self.inner.read().await;
        let mut out: Vec<_> = map
            .values()
            .filter(|v| v.user_id == user_id)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.provider.cmp(&b.provider));
        Ok(out)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut map = self.inner.write().await;
        Ok(map.remove(id).is_some())
    }
}

// Allow `Arc<dyn OidcIdentityStorage>` to be used transparently — matches
// the pattern for every other store in this crate.
#[async_trait]
impl OidcIdentityStorage for Arc<dyn OidcIdentityStorage> {
    async fn store(&self, identity: &OidcIdentity) -> Result<(), StorageError> {
        (**self).store(identity).await
    }
    async fn get_by_external(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<OidcIdentity>, StorageError> {
        (**self).get_by_external(provider, subject).await
    }
    async fn list_for_user(&self, user_id: &str) -> Result<Vec<OidcIdentity>, StorageError> {
        (**self).list_for_user(user_id).await
    }
    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        (**self).delete(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn make() -> SqlxOidcIdentityStore {
        SqlxOidcIdentityStore::in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn store_and_get_roundtrip_sqlx() {
        let s = make().await;
        let ident = OidcIdentity::new("user-1", "google", "sub-abc", Some("a@b.com".into()));
        s.store(&ident).await.unwrap();
        let fetched = s.get_by_external("google", "sub-abc").await.unwrap();
        assert_eq!(fetched.unwrap().user_id, "user-1");
    }

    #[tokio::test]
    async fn upsert_by_provider_subject_sqlx() {
        let s = make().await;
        let first = OidcIdentity::new("user-1", "google", "sub-abc", None);
        s.store(&first).await.unwrap();

        // Same (provider, subject), different user_id — upsert overrides.
        let reassign = OidcIdentity::new("user-2", "google", "sub-abc", Some("x@y".into()));
        s.store(&reassign).await.unwrap();

        let got = s
            .get_by_external("google", "sub-abc")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.user_id, "user-2");
        assert_eq!(got.email_at_link.as_deref(), Some("x@y"));
    }

    #[tokio::test]
    async fn list_for_user_sqlx() {
        let s = make().await;
        s.store(&OidcIdentity::new("u1", "google", "s1", None))
            .await
            .unwrap();
        s.store(&OidcIdentity::new("u1", "okta", "s2", None))
            .await
            .unwrap();
        s.store(&OidcIdentity::new("u2", "google", "s3", None))
            .await
            .unwrap();

        let u1 = s.list_for_user("u1").await.unwrap();
        assert_eq!(u1.len(), 2);
        let providers: Vec<_> = u1.iter().map(|i| i.provider.as_str()).collect();
        assert_eq!(providers, vec!["google", "okta"]);
    }

    #[tokio::test]
    async fn delete_sqlx() {
        let s = make().await;
        let ident = OidcIdentity::new("u", "p", "s", None);
        let id = ident.id.clone();
        s.store(&ident).await.unwrap();
        assert!(s.delete(&id).await.unwrap());
        assert!(!s.delete(&id).await.unwrap());
    }

    #[tokio::test]
    async fn in_memory_parity() {
        let s = InMemoryOidcIdentityStore::new();
        let first = OidcIdentity::new("u1", "google", "sub-abc", None);
        s.store(&first).await.unwrap();

        let second = OidcIdentity::new("u2", "google", "sub-abc", Some("z@z".into()));
        s.store(&second).await.unwrap();

        let got = s
            .get_by_external("google", "sub-abc")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.user_id, "u2");
        assert_eq!(got.email_at_link.as_deref(), Some("z@z"));
    }
}
