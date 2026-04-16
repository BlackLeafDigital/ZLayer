//! Permission storage implementations
//!
//! Provides both persistent (`SQLite` via sqlx) and in-memory storage backends
//! for resource-level permissions.
//!
//! This store is hand-rolled (not built on `SqlxJsonStore`) because
//! `check()` needs SQL-side wildcard matching (`resource_id IS NULL`) and a
//! `MAX(level)` aggregate across matching rows — neither of which is
//! expressible through the generic JSON-table adapter. All query columns
//! (`subject_kind`, `subject_id`, `resource_kind`, `resource_id`, `level`) are
//! first-class indexed `SQLite` columns; the original record is also stored as
//! `data_json` so `StoredPermission` can grow new fields without schema
//! migrations.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tokio::sync::RwLock;

use super::{PermissionLevel, StorageError, StoredPermission, SubjectKind};

/// Trait for permission storage backends.
#[async_trait]
pub trait PermissionStorage: Send + Sync {
    /// Grant a permission. If a permission with the same id already exists it is
    /// overwritten.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the write.
    async fn grant(&self, permission: &StoredPermission) -> Result<(), StorageError>;

    /// Revoke a permission by id. Returns `true` if the permission existed.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the revoke operation fails.
    async fn revoke(&self, id: &str) -> Result<bool, StorageError>;

    /// List all permissions for a given subject (user or group).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn list_for_subject(
        &self,
        kind: SubjectKind,
        id: &str,
    ) -> Result<Vec<StoredPermission>, StorageError>;

    /// List all permissions for a given resource kind and optionally a specific
    /// resource id.
    ///
    /// When `resource_id` is `Some(id)`, returns grants whose `resource_id`
    /// exactly equals `id` (wildcard grants are NOT included — use
    /// [`PermissionStorage::check`] for wildcard-aware access resolution).
    /// When `resource_id` is `None`, returns only wildcard grants (rows where
    /// the stored `resource_id` is `None`).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn list_for_resource(
        &self,
        resource_kind: &str,
        resource_id: Option<&str>,
    ) -> Result<Vec<StoredPermission>, StorageError>;

    /// Check whether a subject has at least `required_level` on the specified
    /// resource. Returns `true` if the effective permission level is >=
    /// `required_level`.
    ///
    /// When `resource_id` is `Some`, both an exact-resource grant and a
    /// wildcard grant (where `resource_id` is `None`) are considered.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn check(
        &self,
        subject_kind: SubjectKind,
        subject_id: &str,
        resource_kind: &str,
        resource_id: Option<&str>,
        required_level: PermissionLevel,
    ) -> Result<bool, StorageError>;
}

// -----------------------------------------------------------------------------
// Column encoders
// -----------------------------------------------------------------------------

/// Canonical text form of a [`SubjectKind`] for SQL storage. Matches the
/// `#[serde(rename_all = "snake_case")]` wire form and the `Display` impl.
fn subject_kind_to_text(kind: SubjectKind) -> &'static str {
    match kind {
        SubjectKind::User => "user",
        SubjectKind::Group => "group",
    }
}

/// Integer form of a [`PermissionLevel`] for SQL storage. Order matches the
/// derived `Ord` impl (higher = more privilege) so `MAX(level)` at the SQL
/// layer is the effective highest grant.
fn level_to_int(level: PermissionLevel) -> i64 {
    match level {
        PermissionLevel::None => 0,
        PermissionLevel::Read => 1,
        PermissionLevel::Execute => 2,
        PermissionLevel::Write => 3,
    }
}

// -----------------------------------------------------------------------------
// SqlxPermissionStore
// -----------------------------------------------------------------------------

/// SQLite-based persistent storage for permission grants using sqlx.
///
/// Schema is hand-rolled (not via `SqlxJsonStore`) because the access-check
/// path needs SQL-side wildcard matching and a `MAX(level)` aggregate. All
/// query columns are indexed; the full record is additionally serialized into
/// `data_json` so new `StoredPermission` fields don't require schema
/// migrations.
pub struct SqlxPermissionStore {
    pool: SqlitePool,
}

impl SqlxPermissionStore {
    /// Open or create a `SQLite` database at the given path.
    ///
    /// # Errors
    ///
    /// Returns an error if the database connection or table creation fails.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let path_str = path.as_ref().display().to_string();
        let connection_string = format!("sqlite:{path_str}?mode=rwc");

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await?;

        // Enable WAL mode for better concurrent access
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await?;
        sqlx::query("PRAGMA busy_timeout=5000")
            .execute(&pool)
            .await?;

        Self::init_schema(&pool).await?;

        Ok(Self { pool })
    }

    /// Create an in-memory `SQLite` database (useful for testing).
    ///
    /// # Errors
    ///
    /// Returns an error if the in-memory database creation or table creation fails.
    pub async fn in_memory() -> Result<Self, StorageError> {
        let pool = SqlitePool::connect(":memory:").await?;
        Self::init_schema(&pool).await?;
        Ok(Self { pool })
    }

    /// Create the `permissions` table and supporting indexes if they do not
    /// already exist. The indexes cover the three query shapes used by the
    /// trait:
    ///
    /// - `idx_perm_subject` — `list_for_subject`.
    /// - `idx_perm_resource` — `list_for_resource`.
    /// - `idx_perm_check` — the `check` aggregate, which filters by subject and
    ///   resource together and then takes `MAX(level)`.
    ///
    /// Note: deliberately no unique constraint on
    /// `(subject_kind, subject_id, resource_kind, resource_id)` — the
    /// in-memory store keys solely by `id` and allows two distinct grants
    /// with the same subject+resource but different ids, so the persistent
    /// store matches that. `check()` uses `MAX(level)` so duplicates are
    /// still handled correctly.
    async fn init_schema(pool: &SqlitePool) -> Result<(), StorageError> {
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS permissions (
                id TEXT PRIMARY KEY NOT NULL,
                subject_kind TEXT NOT NULL,
                subject_id TEXT NOT NULL,
                resource_kind TEXT NOT NULL,
                resource_id TEXT,
                level INTEGER NOT NULL,
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            ",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_perm_subject \
             ON permissions(subject_kind, subject_id)",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_perm_resource \
             ON permissions(resource_kind, resource_id)",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_perm_check \
             ON permissions(subject_kind, subject_id, resource_kind, resource_id)",
        )
        .execute(pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl PermissionStorage for SqlxPermissionStore {
    async fn grant(&self, permission: &StoredPermission) -> Result<(), StorageError> {
        let data_json = serde_json::to_string(permission)?;
        let subject_kind = subject_kind_to_text(permission.subject_kind);
        let level = level_to_int(permission.level);
        let created_at = permission.created_at.to_rfc3339();

        // Upsert by id, matching `InMemoryPermissionStore::grant` which keys
        // purely by id (so re-granting with the same id overwrites level /
        // resource / everything). Two distinct ids with the same
        // subject+resource are allowed to coexist; `check()` takes the MAX.
        sqlx::query(
            r"
            INSERT INTO permissions
                (id, subject_kind, subject_id, resource_kind, resource_id,
                 level, data_json, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                subject_kind = excluded.subject_kind,
                subject_id = excluded.subject_id,
                resource_kind = excluded.resource_kind,
                resource_id = excluded.resource_id,
                level = excluded.level,
                data_json = excluded.data_json,
                created_at = excluded.created_at
            ",
        )
        .bind(&permission.id)
        .bind(subject_kind)
        .bind(&permission.subject_id)
        .bind(&permission.resource_kind)
        .bind(permission.resource_id.as_deref())
        .bind(level)
        .bind(&data_json)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn revoke(&self, id: &str) -> Result<bool, StorageError> {
        let result = sqlx::query("DELETE FROM permissions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn list_for_subject(
        &self,
        kind: SubjectKind,
        id: &str,
    ) -> Result<Vec<StoredPermission>, StorageError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT data_json FROM permissions \
             WHERE subject_kind = ? AND subject_id = ? \
             ORDER BY id ASC",
        )
        .bind(subject_kind_to_text(kind))
        .bind(id)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (data_json,) in rows {
            let permission: StoredPermission = serde_json::from_str(&data_json)?;
            out.push(permission);
        }
        Ok(out)
    }

    async fn list_for_resource(
        &self,
        resource_kind: &str,
        resource_id: Option<&str>,
    ) -> Result<Vec<StoredPermission>, StorageError> {
        // Match InMemory semantics exactly:
        //   - Some(id) => exact match on resource_id (wildcards NOT included).
        //   - None     => wildcard grants only (resource_id IS NULL).
        // Wildcard-aware access resolution belongs in `check()`.
        let rows: Vec<(String,)> = match resource_id {
            Some(rid) => {
                sqlx::query_as(
                    "SELECT data_json FROM permissions \
                     WHERE resource_kind = ? AND resource_id = ? \
                     ORDER BY id ASC",
                )
                .bind(resource_kind)
                .bind(rid)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as(
                    "SELECT data_json FROM permissions \
                     WHERE resource_kind = ? AND resource_id IS NULL \
                     ORDER BY id ASC",
                )
                .bind(resource_kind)
                .fetch_all(&self.pool)
                .await?
            }
        };

        let mut out = Vec::with_capacity(rows.len());
        for (data_json,) in rows {
            let permission: StoredPermission = serde_json::from_str(&data_json)?;
            out.push(permission);
        }
        Ok(out)
    }

    async fn check(
        &self,
        subject_kind: SubjectKind,
        subject_id: &str,
        resource_kind: &str,
        resource_id: Option<&str>,
        required_level: PermissionLevel,
    ) -> Result<bool, StorageError> {
        // MAX(level) across (exact-resource grants) ∪ (wildcard grants for
        // this subject+resource_kind). `fetch_one` returns a single row whose
        // value is `NULL` when there are no matching rows — hence `Option`.
        let row: (Option<i64>,) = sqlx::query_as(
            r"
            SELECT MAX(level) FROM permissions
            WHERE subject_kind = ?
              AND subject_id = ?
              AND resource_kind = ?
              AND (resource_id IS NULL OR resource_id = ?)
            ",
        )
        .bind(subject_kind_to_text(subject_kind))
        .bind(subject_id)
        .bind(resource_kind)
        .bind(resource_id)
        .fetch_one(&self.pool)
        .await?;

        let required = level_to_int(required_level);
        Ok(row.0.is_some_and(|max_level| max_level >= required))
    }
}

// -----------------------------------------------------------------------------
// InMemoryPermissionStore
// -----------------------------------------------------------------------------

/// In-memory permission store.
pub struct InMemoryPermissionStore {
    permissions: Arc<RwLock<HashMap<String, StoredPermission>>>,
}

impl InMemoryPermissionStore {
    /// Create a new empty in-memory permission store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            permissions: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryPermissionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PermissionStorage for InMemoryPermissionStore {
    async fn grant(&self, permission: &StoredPermission) -> Result<(), StorageError> {
        let mut perms = self.permissions.write().await;
        perms.insert(permission.id.clone(), permission.clone());
        Ok(())
    }

    async fn revoke(&self, id: &str) -> Result<bool, StorageError> {
        let mut perms = self.permissions.write().await;
        Ok(perms.remove(id).is_some())
    }

    async fn list_for_subject(
        &self,
        kind: SubjectKind,
        id: &str,
    ) -> Result<Vec<StoredPermission>, StorageError> {
        let perms = self.permissions.read().await;
        let mut list: Vec<_> = perms
            .values()
            .filter(|p| p.subject_kind == kind && p.subject_id == id)
            .cloned()
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(list)
    }

    async fn list_for_resource(
        &self,
        resource_kind: &str,
        resource_id: Option<&str>,
    ) -> Result<Vec<StoredPermission>, StorageError> {
        let perms = self.permissions.read().await;
        let mut list: Vec<_> = perms
            .values()
            .filter(|p| {
                p.resource_kind == resource_kind
                    && match resource_id {
                        Some(rid) => p.resource_id.as_deref() == Some(rid),
                        None => p.resource_id.is_none(),
                    }
            })
            .cloned()
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(list)
    }

    async fn check(
        &self,
        subject_kind: SubjectKind,
        subject_id: &str,
        resource_kind: &str,
        resource_id: Option<&str>,
        required_level: PermissionLevel,
    ) -> Result<bool, StorageError> {
        let perms = self.permissions.read().await;

        let best = perms
            .values()
            .filter(|p| {
                p.subject_kind == subject_kind
                    && p.subject_id == subject_id
                    && p.resource_kind == resource_kind
                    && (
                        // Exact resource match
                        p.resource_id.as_deref() == resource_id
                        // Wildcard grant (resource_id == None) covers any specific resource
                        || p.resource_id.is_none()
                    )
            })
            .map(|p| p.level)
            .max();

        Ok(best.is_some_and(|level| level >= required_level))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StoredPermission;

    fn make_perm(
        subject_kind: SubjectKind,
        subject_id: &str,
        resource_kind: &str,
        resource_id: Option<&str>,
        level: PermissionLevel,
    ) -> StoredPermission {
        StoredPermission::new(
            subject_kind,
            subject_id,
            resource_kind,
            resource_id.map(String::from),
            level,
        )
    }

    // =========================================================================
    // InMemoryPermissionStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_grant_and_list_for_subject() {
        let store = InMemoryPermissionStore::new();
        let p1 = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d1"),
            PermissionLevel::Read,
        );
        let p2 = make_perm(
            SubjectKind::User,
            "u1",
            "project",
            None,
            PermissionLevel::Write,
        );
        let p3 = make_perm(
            SubjectKind::Group,
            "g1",
            "deployment",
            Some("d1"),
            PermissionLevel::Execute,
        );

        store.grant(&p1).await.unwrap();
        store.grant(&p2).await.unwrap();
        store.grant(&p3).await.unwrap();

        let user_perms = store
            .list_for_subject(SubjectKind::User, "u1")
            .await
            .unwrap();
        assert_eq!(user_perms.len(), 2);

        let group_perms = store
            .list_for_subject(SubjectKind::Group, "g1")
            .await
            .unwrap();
        assert_eq!(group_perms.len(), 1);
        assert_eq!(group_perms[0].level, PermissionLevel::Execute);
    }

    #[tokio::test]
    async fn test_revoke() {
        let store = InMemoryPermissionStore::new();
        let p = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d1"),
            PermissionLevel::Read,
        );
        let pid = p.id.clone();
        store.grant(&p).await.unwrap();

        let revoked = store.revoke(&pid).await.unwrap();
        assert!(revoked);

        let revoked_again = store.revoke(&pid).await.unwrap();
        assert!(!revoked_again);

        let perms = store
            .list_for_subject(SubjectKind::User, "u1")
            .await
            .unwrap();
        assert!(perms.is_empty());
    }

    #[tokio::test]
    async fn test_list_for_resource_exact() {
        let store = InMemoryPermissionStore::new();
        let p1 = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d1"),
            PermissionLevel::Read,
        );
        let p2 = make_perm(
            SubjectKind::User,
            "u2",
            "deployment",
            Some("d1"),
            PermissionLevel::Write,
        );
        let p3 = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d2"),
            PermissionLevel::Read,
        );

        store.grant(&p1).await.unwrap();
        store.grant(&p2).await.unwrap();
        store.grant(&p3).await.unwrap();

        let d1_perms = store
            .list_for_resource("deployment", Some("d1"))
            .await
            .unwrap();
        assert_eq!(d1_perms.len(), 2);

        let d2_perms = store
            .list_for_resource("deployment", Some("d2"))
            .await
            .unwrap();
        assert_eq!(d2_perms.len(), 1);
    }

    #[tokio::test]
    async fn test_list_for_resource_wildcard() {
        let store = InMemoryPermissionStore::new();
        let p1 = make_perm(
            SubjectKind::User,
            "u1",
            "project",
            None,
            PermissionLevel::Write,
        );
        let p2 = make_perm(
            SubjectKind::User,
            "u1",
            "project",
            Some("p1"),
            PermissionLevel::Read,
        );

        store.grant(&p1).await.unwrap();
        store.grant(&p2).await.unwrap();

        // Wildcard query: resource_id=None only matches wildcard grants.
        let wildcard = store.list_for_resource("project", None).await.unwrap();
        assert_eq!(wildcard.len(), 1);
        assert!(wildcard[0].resource_id.is_none());
    }

    #[tokio::test]
    async fn test_check_exact_match() {
        let store = InMemoryPermissionStore::new();
        let p = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d1"),
            PermissionLevel::Write,
        );
        store.grant(&p).await.unwrap();

        // Write >= Read => true
        assert!(store
            .check(
                SubjectKind::User,
                "u1",
                "deployment",
                Some("d1"),
                PermissionLevel::Read
            )
            .await
            .unwrap());

        // Write >= Write => true
        assert!(store
            .check(
                SubjectKind::User,
                "u1",
                "deployment",
                Some("d1"),
                PermissionLevel::Write
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_check_wildcard_covers_specific() {
        let store = InMemoryPermissionStore::new();
        // Wildcard grant: all deployments
        let p = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            None,
            PermissionLevel::Execute,
        );
        store.grant(&p).await.unwrap();

        // Wildcard should cover a specific resource
        assert!(store
            .check(
                SubjectKind::User,
                "u1",
                "deployment",
                Some("d99"),
                PermissionLevel::Read
            )
            .await
            .unwrap());

        assert!(store
            .check(
                SubjectKind::User,
                "u1",
                "deployment",
                Some("d99"),
                PermissionLevel::Execute
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_check_insufficient_level() {
        let store = InMemoryPermissionStore::new();
        let p = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d1"),
            PermissionLevel::Read,
        );
        store.grant(&p).await.unwrap();

        // Read < Write => false
        assert!(!store
            .check(
                SubjectKind::User,
                "u1",
                "deployment",
                Some("d1"),
                PermissionLevel::Write
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_check_no_permission() {
        let store = InMemoryPermissionStore::new();
        assert!(!store
            .check(
                SubjectKind::User,
                "u1",
                "deployment",
                Some("d1"),
                PermissionLevel::Read
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_check_wrong_subject() {
        let store = InMemoryPermissionStore::new();
        let p = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d1"),
            PermissionLevel::Write,
        );
        store.grant(&p).await.unwrap();

        // Different user has no access
        assert!(!store
            .check(
                SubjectKind::User,
                "u2",
                "deployment",
                Some("d1"),
                PermissionLevel::Read
            )
            .await
            .unwrap());

        // Group with same id string has no access (different kind)
        assert!(!store
            .check(
                SubjectKind::Group,
                "u1",
                "deployment",
                Some("d1"),
                PermissionLevel::Read
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_permission_level_ordering() {
        assert!(PermissionLevel::None < PermissionLevel::Read);
        assert!(PermissionLevel::Read < PermissionLevel::Execute);
        assert!(PermissionLevel::Execute < PermissionLevel::Write);
    }

    #[tokio::test]
    async fn test_grant_overwrite() {
        let store = InMemoryPermissionStore::new();
        let mut p = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d1"),
            PermissionLevel::Read,
        );
        let pid = p.id.clone();
        store.grant(&p).await.unwrap();

        // Overwrite with higher level using same id
        p.level = PermissionLevel::Write;
        store.grant(&p).await.unwrap();

        let perms = store
            .list_for_subject(SubjectKind::User, "u1")
            .await
            .unwrap();
        assert_eq!(perms.len(), 1);
        assert_eq!(perms[0].id, pid);
        assert_eq!(perms[0].level, PermissionLevel::Write);
    }

    // =========================================================================
    // SqlxPermissionStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_sqlx_grant_and_list_for_subject() {
        let store = SqlxPermissionStore::in_memory().await.unwrap();
        let p1 = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d1"),
            PermissionLevel::Read,
        );
        let p2 = make_perm(
            SubjectKind::User,
            "u1",
            "project",
            None,
            PermissionLevel::Write,
        );
        let p3 = make_perm(
            SubjectKind::Group,
            "g1",
            "deployment",
            Some("d1"),
            PermissionLevel::Execute,
        );

        store.grant(&p1).await.unwrap();
        store.grant(&p2).await.unwrap();
        store.grant(&p3).await.unwrap();

        let user_perms = store
            .list_for_subject(SubjectKind::User, "u1")
            .await
            .unwrap();
        assert_eq!(user_perms.len(), 2);

        let group_perms = store
            .list_for_subject(SubjectKind::Group, "g1")
            .await
            .unwrap();
        assert_eq!(group_perms.len(), 1);
        assert_eq!(group_perms[0].level, PermissionLevel::Execute);
        assert_eq!(group_perms[0].subject_id, "g1");

        // Cross-kind isolation: a User named "g1" should not see the Group grant.
        let cross = store
            .list_for_subject(SubjectKind::User, "g1")
            .await
            .unwrap();
        assert!(cross.is_empty());
    }

    #[tokio::test]
    async fn test_sqlx_duplicate_grant_same_subject_same_resource() {
        // Matches `InMemoryPermissionStore::grant` semantics: grants are keyed
        // by `id`, not by (subject, resource). Two distinct ids with the same
        // subject+resource coexist; re-granting with the same id is an
        // idempotent upsert of the row's fields.
        let store = SqlxPermissionStore::in_memory().await.unwrap();

        // Two distinct grants, same subject+resource, different ids.
        let a = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d1"),
            PermissionLevel::Read,
        );
        let b = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d1"),
            PermissionLevel::Write,
        );
        assert_ne!(a.id, b.id);

        store.grant(&a).await.unwrap();
        store.grant(&b).await.unwrap(); // must NOT return AlreadyExists

        let perms = store
            .list_for_subject(SubjectKind::User, "u1")
            .await
            .unwrap();
        assert_eq!(perms.len(), 2, "both distinct-id grants must coexist");

        // Re-grant with same id upserts (no error, row count unchanged).
        let mut c = a.clone();
        c.level = PermissionLevel::Execute;
        store.grant(&c).await.unwrap();

        let perms_after = store
            .list_for_subject(SubjectKind::User, "u1")
            .await
            .unwrap();
        assert_eq!(
            perms_after.len(),
            2,
            "same-id re-grant must upsert, not insert"
        );
        let upserted = perms_after.iter().find(|p| p.id == a.id).unwrap();
        assert_eq!(upserted.level, PermissionLevel::Execute);
    }

    #[tokio::test]
    async fn test_sqlx_revoke() {
        let store = SqlxPermissionStore::in_memory().await.unwrap();
        let p = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d1"),
            PermissionLevel::Read,
        );
        let pid = p.id.clone();
        store.grant(&p).await.unwrap();

        let revoked = store.revoke(&pid).await.unwrap();
        assert!(revoked);

        let revoked_again = store.revoke(&pid).await.unwrap();
        assert!(!revoked_again);

        let perms = store
            .list_for_subject(SubjectKind::User, "u1")
            .await
            .unwrap();
        assert!(perms.is_empty());
    }

    #[tokio::test]
    async fn test_sqlx_list_for_resource_includes_wildcards() {
        // Matches InMemory semantics: list_for_resource(kind, Some(id)) returns
        // exact-id grants only; list_for_resource(kind, None) returns wildcard
        // grants only. Wildcard-covers-specific is a `check()` concern, not a
        // listing concern.
        let store = SqlxPermissionStore::in_memory().await.unwrap();

        let wildcard = make_perm(
            SubjectKind::User,
            "u1",
            "project",
            None,
            PermissionLevel::Write,
        );
        let specific = make_perm(
            SubjectKind::User,
            "u1",
            "project",
            Some("p1"),
            PermissionLevel::Read,
        );
        let other_kind = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            None,
            PermissionLevel::Read,
        );

        store.grant(&wildcard).await.unwrap();
        store.grant(&specific).await.unwrap();
        store.grant(&other_kind).await.unwrap();

        // Wildcard query returns only the project wildcard grant (not the
        // specific one, and not the deployment-kind grant).
        let wc = store.list_for_resource("project", None).await.unwrap();
        assert_eq!(wc.len(), 1);
        assert!(wc[0].resource_id.is_none());
        assert_eq!(wc[0].resource_kind, "project");

        // Exact-id query returns only the specific grant.
        let exact = store
            .list_for_resource("project", Some("p1"))
            .await
            .unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].resource_id.as_deref(), Some("p1"));
    }

    #[tokio::test]
    async fn test_sqlx_check_max_wins() {
        let store = SqlxPermissionStore::in_memory().await.unwrap();

        // Wildcard Read across all deployments.
        let wildcard = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            None,
            PermissionLevel::Read,
        );
        // Explicit Write on d1 only.
        let specific = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            Some("d1"),
            PermissionLevel::Write,
        );
        store.grant(&wildcard).await.unwrap();
        store.grant(&specific).await.unwrap();

        // d1: MAX(Read, Write) = Write → required=Write must pass.
        assert!(store
            .check(
                SubjectKind::User,
                "u1",
                "deployment",
                Some("d1"),
                PermissionLevel::Write
            )
            .await
            .unwrap());

        // d2: only the wildcard Read matches → required=Write must fail,
        // required=Read must pass.
        assert!(!store
            .check(
                SubjectKind::User,
                "u1",
                "deployment",
                Some("d2"),
                PermissionLevel::Write
            )
            .await
            .unwrap());
        assert!(store
            .check(
                SubjectKind::User,
                "u1",
                "deployment",
                Some("d2"),
                PermissionLevel::Read
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_sqlx_check_no_grant_returns_false() {
        let store = SqlxPermissionStore::in_memory().await.unwrap();

        // Nothing granted at all.
        assert!(!store
            .check(
                SubjectKind::User,
                "u1",
                "deployment",
                Some("d1"),
                PermissionLevel::Read
            )
            .await
            .unwrap());

        // Grant something for a different user and a different kind; the
        // target user still has nothing that matches.
        store
            .grant(&make_perm(
                SubjectKind::User,
                "other",
                "deployment",
                Some("d1"),
                PermissionLevel::Write,
            ))
            .await
            .unwrap();
        store
            .grant(&make_perm(
                SubjectKind::User,
                "u1",
                "project",
                Some("d1"),
                PermissionLevel::Write,
            ))
            .await
            .unwrap();

        assert!(!store
            .check(
                SubjectKind::User,
                "u1",
                "deployment",
                Some("d1"),
                PermissionLevel::Read
            )
            .await
            .unwrap());

        // Group with same id string is a different subject; must not match.
        store
            .grant(&make_perm(
                SubjectKind::Group,
                "u1",
                "deployment",
                Some("d1"),
                PermissionLevel::Write,
            ))
            .await
            .unwrap();
        assert!(!store
            .check(
                SubjectKind::User,
                "u1",
                "deployment",
                Some("d1"),
                PermissionLevel::Read
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_sqlx_persistent_round_trip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("permissions.db");

        let wildcard = make_perm(
            SubjectKind::User,
            "u1",
            "deployment",
            None,
            PermissionLevel::Read,
        );
        let specific = make_perm(
            SubjectKind::Group,
            "g1",
            "project",
            Some("p1"),
            PermissionLevel::Write,
        );
        let wildcard_id = wildcard.id.clone();
        let specific_id = specific.id.clone();

        // Populate.
        {
            let store = SqlxPermissionStore::open(&db_path).await.unwrap();
            store.grant(&wildcard).await.unwrap();
            store.grant(&specific).await.unwrap();
        }

        // Reopen and verify rows, listing, and check() semantics all survive.
        {
            let store = SqlxPermissionStore::open(&db_path).await.unwrap();

            let u1 = store
                .list_for_subject(SubjectKind::User, "u1")
                .await
                .unwrap();
            assert_eq!(u1.len(), 1);
            assert_eq!(u1[0].id, wildcard_id);
            assert!(u1[0].resource_id.is_none());
            assert_eq!(u1[0].level, PermissionLevel::Read);

            let g1 = store
                .list_for_subject(SubjectKind::Group, "g1")
                .await
                .unwrap();
            assert_eq!(g1.len(), 1);
            assert_eq!(g1[0].id, specific_id);
            assert_eq!(g1[0].resource_id.as_deref(), Some("p1"));
            assert_eq!(g1[0].level, PermissionLevel::Write);

            // Wildcard still covers an arbitrary specific deployment.
            assert!(store
                .check(
                    SubjectKind::User,
                    "u1",
                    "deployment",
                    Some("d42"),
                    PermissionLevel::Read
                )
                .await
                .unwrap());

            // Specific grant still permits the matching project.
            assert!(store
                .check(
                    SubjectKind::Group,
                    "g1",
                    "project",
                    Some("p1"),
                    PermissionLevel::Write
                )
                .await
                .unwrap());

            // And a revoke persists too.
            assert!(store.revoke(&wildcard_id).await.unwrap());
        }
        {
            let store = SqlxPermissionStore::open(&db_path).await.unwrap();
            let u1 = store
                .list_for_subject(SubjectKind::User, "u1")
                .await
                .unwrap();
            assert!(u1.is_empty(), "revoke must persist across reopen");
        }
    }
}
