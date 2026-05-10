//! Persistent storage for SDK / browser client public keys, used as
//! recipients for sealed-box secret reads. Shares the secrets `SQLite`
//! database with [`PersistentSecretsStore`](crate::PersistentSecretsStore).
//!
//! Each registered key is bound to an actor (a user or an API key) and
//! stored alongside an opaque `key_id`. Keys are never deleted — `revoke`
//! is a soft-delete that hides the key from `list_by_actor` while keeping
//! it retrievable via `get` so the actor's audit trail stays intact.
//!
//! The schema lives in the same `secrets.sqlite` file as the secrets
//! table, so callers should construct a single [`SqlitePool`] (typically
//! via [`PersistentSecretsStore::open`](crate::PersistentSecretsStore::open))
//! and hand the same pool to [`PersistentClientKeyStore::new`].

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use tracing::{debug, info};

use crate::{Result, SecretsError};

pub use zlayer_types::secrets::client_keys::{ActorKind, ClientPublicKey, PUBLIC_KEY_LEN};

/// SQL schema for the client public keys table. Idempotent — safe to run
/// on every [`PersistentClientKeyStore::new`].
const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS client_public_keys (
    key_id        TEXT PRIMARY KEY,
    actor_kind    TEXT NOT NULL CHECK(actor_kind IN ('user','api_key')),
    actor_id      TEXT NOT NULL,
    public_key    BLOB NOT NULL,
    label         TEXT,
    created_at    TEXT NOT NULL,
    last_used_at  TEXT,
    revoked_at    TEXT
);
CREATE INDEX IF NOT EXISTS idx_client_public_keys_actor
  ON client_public_keys(actor_kind, actor_id) WHERE revoked_at IS NULL;
";

/// Storage trait for SDK / browser client public keys.
#[async_trait]
pub trait ClientKeyStore: Send + Sync {
    /// Registers a new public key for `actor_kind` / `actor_id`.
    async fn register(
        &self,
        actor_kind: ActorKind,
        actor_id: &str,
        public_key: &[u8],
        label: Option<&str>,
    ) -> Result<ClientPublicKey>;

    /// Looks up a key by its `key_id`. Returns the row regardless of
    /// whether it has been revoked.
    async fn get(&self, key_id: &str) -> Result<Option<ClientPublicKey>>;

    /// Lists all *active* (non-revoked) keys for an actor, newest first.
    async fn list_by_actor(
        &self,
        actor_kind: ActorKind,
        actor_id: &str,
    ) -> Result<Vec<ClientPublicKey>>;

    /// Soft-deletes a key by setting `revoked_at`.
    async fn revoke(&self, key_id: &str) -> Result<()>;

    /// Updates the `last_used_at` timestamp on a key.
    async fn touch_last_used(&self, key_id: &str) -> Result<()>;
}

/// SQLite-backed [`ClientKeyStore`].
///
/// Constructed from a [`SqlitePool`] so it can share the same database
/// file as [`PersistentSecretsStore`](crate::PersistentSecretsStore).
pub struct PersistentClientKeyStore {
    pool: SqlitePool,
}

impl PersistentClientKeyStore {
    /// Wraps `pool` and runs the schema migration.
    ///
    /// # Errors
    ///
    /// Returns [`SecretsError::Storage`] if the migration fails.
    pub async fn new(pool: SqlitePool) -> Result<Self> {
        sqlx::query(SCHEMA)
            .execute(&pool)
            .await
            .map_err(|e| SecretsError::Storage(format!("Failed to initialize schema: {e}")))?;

        info!("Initialized client public keys schema");
        Ok(Self { pool })
    }

    /// Returns a borrowed reference to the underlying pool. Useful for
    /// callers that want to compose multiple stores against one DB file.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Generates a fresh `ck_<32-hex>` key id.
    fn generate_key_id() -> String {
        let bytes: [u8; 16] = rand::random();
        format!("ck_{}", hex::encode(bytes))
    }

    /// Format a [`DateTime<Utc>`] as RFC 3339 for `SQLite` TEXT storage.
    /// Uses millisecond precision so rapidly-issued rows still sort
    /// deterministically by `created_at`.
    fn format_timestamp(ts: DateTime<Utc>) -> String {
        ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    /// Parses an RFC 3339 timestamp from a row.
    fn parse_timestamp(s: &str) -> Result<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| SecretsError::Storage(format!("invalid timestamp {s:?}: {e}")))
    }

    /// Hydrates a [`ClientPublicKey`] from a `SELECT *` style row.
    fn row_to_key(row: &sqlx::sqlite::SqliteRow) -> Result<ClientPublicKey> {
        let key_id: String = row
            .try_get("key_id")
            .map_err(|e| SecretsError::Storage(format!("Failed to read key_id: {e}")))?;
        let actor_kind_str: String = row
            .try_get("actor_kind")
            .map_err(|e| SecretsError::Storage(format!("Failed to read actor_kind: {e}")))?;
        let actor_kind = ActorKind::from_str(&actor_kind_str)?;
        let actor_id: String = row
            .try_get("actor_id")
            .map_err(|e| SecretsError::Storage(format!("Failed to read actor_id: {e}")))?;
        let public_key: Vec<u8> = row
            .try_get("public_key")
            .map_err(|e| SecretsError::Storage(format!("Failed to read public_key: {e}")))?;
        let label: Option<String> = row
            .try_get("label")
            .map_err(|e| SecretsError::Storage(format!("Failed to read label: {e}")))?;
        let created_at_str: String = row
            .try_get("created_at")
            .map_err(|e| SecretsError::Storage(format!("Failed to read created_at: {e}")))?;
        let last_used_at_str: Option<String> = row
            .try_get("last_used_at")
            .map_err(|e| SecretsError::Storage(format!("Failed to read last_used_at: {e}")))?;
        let revoked_at_str: Option<String> = row
            .try_get("revoked_at")
            .map_err(|e| SecretsError::Storage(format!("Failed to read revoked_at: {e}")))?;

        let created_at = Self::parse_timestamp(&created_at_str)?;
        let last_used_at = match last_used_at_str {
            Some(s) => Some(Self::parse_timestamp(&s)?),
            None => None,
        };
        let revoked_at = match revoked_at_str {
            Some(s) => Some(Self::parse_timestamp(&s)?),
            None => None,
        };

        Ok(ClientPublicKey {
            key_id,
            actor_kind,
            actor_id,
            public_key,
            label,
            created_at,
            last_used_at,
            revoked_at,
        })
    }
}

#[async_trait]
impl ClientKeyStore for PersistentClientKeyStore {
    async fn register(
        &self,
        actor_kind: ActorKind,
        actor_id: &str,
        public_key: &[u8],
        label: Option<&str>,
    ) -> Result<ClientPublicKey> {
        if public_key.len() != PUBLIC_KEY_LEN {
            return Err(SecretsError::Storage(format!(
                "invalid public key length: expected {PUBLIC_KEY_LEN} bytes, got {}",
                public_key.len()
            )));
        }

        let key_id = Self::generate_key_id();
        let created_at = Utc::now();
        let created_at_str = Self::format_timestamp(created_at);
        let public_key_vec = public_key.to_vec();

        sqlx::query(
            "INSERT INTO client_public_keys \
             (key_id, actor_kind, actor_id, public_key, label, created_at, last_used_at, revoked_at) \
             VALUES (?, ?, ?, ?, ?, ?, NULL, NULL)",
        )
        .bind(&key_id)
        .bind(actor_kind.as_str())
        .bind(actor_id)
        .bind(&public_key_vec)
        .bind(label)
        .bind(&created_at_str)
        .execute(&self.pool)
        .await
        .map_err(|e| SecretsError::Storage(format!("Failed to insert client public key: {e}")))?;

        debug!(
            "Registered client public key {} for {} {}",
            key_id,
            actor_kind.as_str(),
            actor_id
        );

        Ok(ClientPublicKey {
            key_id,
            actor_kind,
            actor_id: actor_id.to_string(),
            public_key: public_key_vec,
            label: label.map(str::to_string),
            created_at,
            last_used_at: None,
            revoked_at: None,
        })
    }

    async fn get(&self, key_id: &str) -> Result<Option<ClientPublicKey>> {
        let row = sqlx::query(
            "SELECT key_id, actor_kind, actor_id, public_key, label, created_at, last_used_at, revoked_at \
             FROM client_public_keys WHERE key_id = ?",
        )
        .bind(key_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| SecretsError::Storage(format!("Failed to query client public key: {e}")))?;

        match row {
            Some(row) => Ok(Some(Self::row_to_key(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_by_actor(
        &self,
        actor_kind: ActorKind,
        actor_id: &str,
    ) -> Result<Vec<ClientPublicKey>> {
        let rows = sqlx::query(
            "SELECT key_id, actor_kind, actor_id, public_key, label, created_at, last_used_at, revoked_at \
             FROM client_public_keys \
             WHERE actor_kind = ? AND actor_id = ? AND revoked_at IS NULL \
             ORDER BY created_at DESC",
        )
        .bind(actor_kind.as_str())
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SecretsError::Storage(format!("Failed to list client public keys: {e}")))?;

        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(Self::row_to_key(row)?);
        }
        Ok(out)
    }

    async fn revoke(&self, key_id: &str) -> Result<()> {
        let now = Self::format_timestamp(Utc::now());

        let result = sqlx::query("UPDATE client_public_keys SET revoked_at = ? WHERE key_id = ?")
            .bind(&now)
            .bind(key_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                SecretsError::Storage(format!("Failed to revoke client public key: {e}"))
            })?;

        if result.rows_affected() == 0 {
            return Err(SecretsError::NotFound {
                name: key_id.to_string(),
            });
        }

        debug!("Revoked client public key {}", key_id);
        Ok(())
    }

    async fn touch_last_used(&self, key_id: &str) -> Result<()> {
        let now = Self::format_timestamp(Utc::now());

        let result = sqlx::query("UPDATE client_public_keys SET last_used_at = ? WHERE key_id = ?")
            .bind(&now)
            .bind(key_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SecretsError::Storage(format!("Failed to update last_used_at: {e}")))?;

        if result.rows_affected() == 0 {
            return Err(SecretsError::NotFound {
                name: key_id.to_string(),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_store() -> PersistentClientKeyStore {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        PersistentClientKeyStore::new(pool).await.unwrap()
    }

    #[tokio::test]
    async fn register_and_get_roundtrip() {
        let store = create_test_store().await;
        let pk = [7u8; 32];
        let registered = store
            .register(ActorKind::User, "user-123", &pk, Some("laptop"))
            .await
            .unwrap();

        assert!(registered.key_id.starts_with("ck_"));
        assert_eq!(registered.key_id.len(), 3 + 32);
        assert_eq!(registered.actor_kind, ActorKind::User);
        assert_eq!(registered.actor_id, "user-123");
        assert_eq!(registered.public_key, pk.to_vec());
        assert_eq!(registered.label.as_deref(), Some("laptop"));
        assert!(registered.last_used_at.is_none());
        assert!(registered.revoked_at.is_none());

        let fetched = store.get(&registered.key_id).await.unwrap().unwrap();
        assert_eq!(fetched.key_id, registered.key_id);
        assert_eq!(fetched.actor_kind, ActorKind::User);
        assert_eq!(fetched.actor_id, "user-123");
        assert_eq!(fetched.public_key, pk.to_vec());
        assert_eq!(fetched.label.as_deref(), Some("laptop"));

        // Unknown key id returns None.
        assert!(store.get("ck_does_not_exist").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn duplicate_key_id_errors() {
        let store = create_test_store().await;
        let pk = [3u8; 32];

        let registered = store
            .register(ActorKind::ApiKey, "api-1", &pk, None)
            .await
            .unwrap();

        // Force a second insert with the same key_id; the PRIMARY KEY
        // constraint should reject it as a Storage error.
        let result = sqlx::query(
            "INSERT INTO client_public_keys \
             (key_id, actor_kind, actor_id, public_key, label, created_at, last_used_at, revoked_at) \
             VALUES (?, 'api_key', 'api-1', ?, NULL, '2026-01-01T00:00:00Z', NULL, NULL)",
        )
        .bind(&registered.key_id)
        .bind(pk.to_vec())
        .execute(store.pool())
        .await;

        assert!(result.is_err(), "duplicate key_id must be rejected");
    }

    #[tokio::test]
    async fn list_by_actor_returns_only_active() {
        let store = create_test_store().await;
        let pk = [9u8; 32];

        let a = store
            .register(ActorKind::User, "u1", &pk, Some("a"))
            .await
            .unwrap();
        // Sleep between inserts so created_at strictly increases at
        // millisecond resolution, making the DESC ordering deterministic.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let _b = store
            .register(ActorKind::User, "u1", &pk, Some("b"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let c = store
            .register(ActorKind::User, "u1", &pk, Some("c"))
            .await
            .unwrap();
        // A different actor — must not leak in.
        let _other = store
            .register(ActorKind::User, "u2", &pk, Some("other"))
            .await
            .unwrap();
        // A different actor kind with the same id — also must not leak.
        let _other_kind = store
            .register(ActorKind::ApiKey, "u1", &pk, Some("api"))
            .await
            .unwrap();

        // Revoke `a` so only b and c remain active.
        store.revoke(&a.key_id).await.unwrap();

        let active = store.list_by_actor(ActorKind::User, "u1").await.unwrap();
        assert_eq!(active.len(), 2);
        // Newest first ordering: c was registered after b.
        assert_eq!(active[0].key_id, c.key_id);
        assert!(active.iter().all(|k| k.revoked_at.is_none()));
        assert!(active.iter().all(|k| k.actor_id == "u1"));
        assert!(active
            .iter()
            .all(|k| matches!(k.actor_kind, ActorKind::User)));
    }

    #[tokio::test]
    async fn revoke_hides_from_list_but_get_still_finds_with_revoked_at() {
        let store = create_test_store().await;
        let pk = [1u8; 32];

        let key = store
            .register(ActorKind::ApiKey, "svc-1", &pk, None)
            .await
            .unwrap();

        // Active before revoke.
        let pre = store
            .list_by_actor(ActorKind::ApiKey, "svc-1")
            .await
            .unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0].key_id, key.key_id);

        store.revoke(&key.key_id).await.unwrap();

        // Hidden from the active list.
        let post = store
            .list_by_actor(ActorKind::ApiKey, "svc-1")
            .await
            .unwrap();
        assert!(post.is_empty());

        // But `get` still returns it, with `revoked_at` populated.
        let fetched = store.get(&key.key_id).await.unwrap().unwrap();
        assert!(fetched.revoked_at.is_some());

        // Revoking an unknown key is a NotFound.
        let missing = store.revoke("ck_nope").await;
        assert!(matches!(missing, Err(SecretsError::NotFound { .. })));
    }

    #[tokio::test]
    async fn invalid_public_key_length_rejected() {
        let store = create_test_store().await;

        let too_short = [0u8; 16];
        let err = store
            .register(ActorKind::User, "u1", &too_short, None)
            .await
            .unwrap_err();
        assert!(matches!(err, SecretsError::Storage(_)));

        let too_long = [0u8; 64];
        let err = store
            .register(ActorKind::User, "u1", &too_long, None)
            .await
            .unwrap_err();
        assert!(matches!(err, SecretsError::Storage(_)));

        let empty: &[u8] = &[];
        let err = store
            .register(ActorKind::User, "u1", empty, None)
            .await
            .unwrap_err();
        assert!(matches!(err, SecretsError::Storage(_)));

        // Nothing was inserted for any of the failed registers.
        let list = store.list_by_actor(ActorKind::User, "u1").await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn touch_last_used_updates_timestamp() {
        let store = create_test_store().await;
        let pk = [2u8; 32];

        let key = store
            .register(ActorKind::User, "u1", &pk, None)
            .await
            .unwrap();
        assert!(key.last_used_at.is_none());

        store.touch_last_used(&key.key_id).await.unwrap();

        let fetched = store.get(&key.key_id).await.unwrap().unwrap();
        assert!(fetched.last_used_at.is_some());

        // Unknown id is NotFound.
        let err = store.touch_last_used("ck_nope").await.unwrap_err();
        assert!(matches!(err, SecretsError::NotFound { .. }));
    }

    #[test]
    fn actor_kind_str_roundtrip() {
        assert_eq!(ActorKind::User.as_str(), "user");
        assert_eq!(ActorKind::ApiKey.as_str(), "api_key");
        assert_eq!(ActorKind::from_str("user").unwrap(), ActorKind::User);
        assert_eq!(ActorKind::from_str("api_key").unwrap(), ActorKind::ApiKey);
        assert!(matches!(
            ActorKind::from_str("garbage"),
            Err(SecretsError::Storage(_))
        ));
    }
}
