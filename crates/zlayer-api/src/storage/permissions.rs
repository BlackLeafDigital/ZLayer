//! Permission storage implementations
//!
//! Provides both persistent (ZQL) and in-memory storage backends for
//! resource-level permissions.
//!
//! The ZQL backend is hand-rolled (not built on `ZqlJsonStore`) because
//! `check()` requires filtered iteration over subject+resource matches with
//! a `MAX(level)` aggregate across exact + wildcard grants — neither of which
//! is expressible through the generic JSON-table adapter. Permissions are
//! persisted as primary records keyed by id in the `permissions` store,
//! alongside two parallel secondary-index stores keyed by subject and by
//! resource so that `list_for_subject` and `list_for_resource` stay cheap
//! prefix scans.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::{PermissionLevel, StorageError, StoredPermission, SubjectKind};

/// Name of the ZQL store holding permission records keyed by permission id.
const PERMISSIONS_STORE: &str = "permissions";

/// Name of the ZQL store holding the `(subject_kind, subject_id)` ->
/// permission-id secondary index. Keys are
/// `"{subject_kind}:{subject_id}:{permission_id}"`; values are the
/// permission id (also embedded in the key).
const PERMS_BY_SUBJECT_STORE: &str = "perms_by_subject";

/// Name of the ZQL store holding the `(resource_kind, resource_id_or_ALL)`
/// -> permission-id secondary index. Keys are
/// `"{resource_kind}:{resource_id_or_ALL}:{permission_id}"`. Wildcard grants
/// (where `resource_id` is `None`) are encoded with the literal sentinel
/// `"ALL"` in the second slot.
const PERMS_BY_RESOURCE_STORE: &str = "perms_by_resource";

/// Sentinel used in the resource-index key for wildcard grants (rows whose
/// `resource_id` is `None`). Kept short and distinct from any real id.
const WILDCARD_SENTINEL: &str = "ALL";

/// Canonical text form of a [`SubjectKind`] for secondary-index keys. Matches
/// the `#[serde(rename_all = "snake_case")]` wire form and the `Display` impl.
fn subject_kind_to_text(kind: SubjectKind) -> &'static str {
    match kind {
        SubjectKind::User => "user",
        SubjectKind::Group => "group",
    }
}

/// Build the secondary-index key for the `perms_by_subject` store.
fn subject_key(kind: SubjectKind, subject_id: &str, permission_id: &str) -> String {
    format!(
        "{}:{}:{}",
        subject_kind_to_text(kind),
        subject_id,
        permission_id
    )
}

/// Build the secondary-index prefix used to scan all permissions for a
/// given subject.
fn subject_prefix(kind: SubjectKind, subject_id: &str) -> String {
    format!("{}:{}:", subject_kind_to_text(kind), subject_id)
}

/// Build the secondary-index key for the `perms_by_resource` store.
fn resource_key(resource_kind: &str, resource_id: Option<&str>, permission_id: &str) -> String {
    let slot = resource_id.unwrap_or(WILDCARD_SENTINEL);
    format!("{resource_kind}:{slot}:{permission_id}")
}

/// Build the secondary-index prefix used to scan all permissions for a
/// given resource-kind + resource-id (wildcard or specific).
fn resource_prefix(resource_kind: &str, resource_id: Option<&str>) -> String {
    let slot = resource_id.unwrap_or(WILDCARD_SENTINEL);
    format!("{resource_kind}:{slot}:")
}

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
// ZqlPermissionStore
// -----------------------------------------------------------------------------

/// ZQL-based persistent storage for permission grants.
///
/// Bespoke implementation (does NOT use `ZqlJsonStore`) because `check()`
/// requires filtered iteration over subject+resource matches with a
/// `MAX(level)` aggregate across exact + wildcard grants. Three ZQL stores
/// back this type:
///
/// - `permissions` — primary, keyed by permission id -> `StoredPermission`.
/// - `perms_by_subject` — `"{subject_kind}:{subject_id}:{permission_id}"`
///   -> permission id. Supports cheap subject-scoped prefix scans.
/// - `perms_by_resource` — `"{resource_kind}:{resource_id_or_ALL}:{permission_id}"`
///   -> permission id. Wildcards (stored `resource_id` is `None`) are encoded
///   with the literal sentinel `"ALL"`.
///
/// Grants are keyed solely by id, matching `InMemoryPermissionStore::grant`:
/// re-granting with the same id upserts (row-fields replaced, index entries
/// refreshed); granting with a different id for the same (subject, resource)
/// tuple COEXISTS. `check()` takes the `MAX` across all matching rows so
/// duplicates resolve correctly.
pub struct ZqlPermissionStore {
    db: tokio::sync::Mutex<zql::Database>,
}

impl ZqlPermissionStore {
    /// Open or create a ZQL database at the given path.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database cannot be opened or created.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let path = path.as_ref().to_path_buf();

        let db = tokio::task::spawn_blocking(move || zql::Database::open(&path))
            .await
            .map_err(|e| StorageError::Database(format!("spawn_blocking failed: {e}")))?
            .map_err(StorageError::from)?;

        Ok(Self {
            db: tokio::sync::Mutex::new(db),
        })
    }

    /// Create a ZQL database in a temporary directory (useful for testing).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the temporary directory cannot be created
    /// or the database fails to open.
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self, StorageError> {
        let temp_dir = tempfile::tempdir()
            .map_err(|e| StorageError::Database(format!("failed to create temp dir: {e}")))?;
        let path = temp_dir.path().join("permissions_zql");

        let db = tokio::task::spawn_blocking(move || {
            let _keep = temp_dir;
            zql::Database::open(&path)
        })
        .await
        .map_err(|e| StorageError::Database(format!("spawn_blocking failed: {e}")))?
        .map_err(StorageError::from)?;

        Ok(Self {
            db: tokio::sync::Mutex::new(db),
        })
    }
}

#[async_trait]
impl PermissionStorage for ZqlPermissionStore {
    async fn grant(&self, permission: &StoredPermission) -> Result<(), StorageError> {
        let mut db = self.db.lock().await;

        // If a permission with this id already exists, tear down any stale
        // secondary-index entries whose keys may have changed (different
        // subject, different resource_kind, different resource_id, etc.)
        // before we write the refreshed ones. Two distinct ids with the same
        // subject+resource are allowed to coexist, so we never delete by
        // tuple — only by the previous row's own key shape.
        if let Some(previous) = db
            .get_typed::<StoredPermission>(PERMISSIONS_STORE, &permission.id)
            .map_err(StorageError::from)?
        {
            let prev_subject_key =
                subject_key(previous.subject_kind, &previous.subject_id, &permission.id);
            let new_subject_key = subject_key(
                permission.subject_kind,
                &permission.subject_id,
                &permission.id,
            );
            if prev_subject_key != new_subject_key {
                db.delete_typed(PERMS_BY_SUBJECT_STORE, &prev_subject_key)
                    .map_err(StorageError::from)?;
            }

            let prev_resource_key = resource_key(
                &previous.resource_kind,
                previous.resource_id.as_deref(),
                &permission.id,
            );
            let new_resource_key = resource_key(
                &permission.resource_kind,
                permission.resource_id.as_deref(),
                &permission.id,
            );
            if prev_resource_key != new_resource_key {
                db.delete_typed(PERMS_BY_RESOURCE_STORE, &prev_resource_key)
                    .map_err(StorageError::from)?;
            }
        }

        // Write the primary record.
        db.put_typed(PERMISSIONS_STORE, &permission.id, permission)
            .map_err(StorageError::from)?;

        // Refresh the secondary index entries (put is idempotent).
        let sub_key = subject_key(
            permission.subject_kind,
            &permission.subject_id,
            &permission.id,
        );
        db.put_typed(PERMS_BY_SUBJECT_STORE, &sub_key, &permission.id)
            .map_err(StorageError::from)?;

        let res_key = resource_key(
            &permission.resource_kind,
            permission.resource_id.as_deref(),
            &permission.id,
        );
        db.put_typed(PERMS_BY_RESOURCE_STORE, &res_key, &permission.id)
            .map_err(StorageError::from)?;

        Ok(())
    }

    async fn revoke(&self, id: &str) -> Result<bool, StorageError> {
        let mut db = self.db.lock().await;

        let Some(existing) = db
            .get_typed::<StoredPermission>(PERMISSIONS_STORE, id)
            .map_err(StorageError::from)?
        else {
            return Ok(false);
        };

        // Tear down the two secondary entries first, derived from the primary
        // row so the keys match what `grant` wrote.
        let sub_key = subject_key(existing.subject_kind, &existing.subject_id, id);
        db.delete_typed(PERMS_BY_SUBJECT_STORE, &sub_key)
            .map_err(StorageError::from)?;

        let res_key = resource_key(&existing.resource_kind, existing.resource_id.as_deref(), id);
        db.delete_typed(PERMS_BY_RESOURCE_STORE, &res_key)
            .map_err(StorageError::from)?;

        db.delete_typed(PERMISSIONS_STORE, id)
            .map_err(StorageError::from)
    }

    async fn list_for_subject(
        &self,
        kind: SubjectKind,
        id: &str,
    ) -> Result<Vec<StoredPermission>, StorageError> {
        let mut db = self.db.lock().await;

        let prefix = subject_prefix(kind, id);
        let index_entries: Vec<(String, String)> = db
            .scan_typed(PERMS_BY_SUBJECT_STORE, &prefix)
            .map_err(StorageError::from)?;

        let mut out: Vec<StoredPermission> = Vec::with_capacity(index_entries.len());
        for (_, perm_id) in index_entries {
            if let Some(perm) = db
                .get_typed::<StoredPermission>(PERMISSIONS_STORE, &perm_id)
                .map_err(StorageError::from)?
            {
                out.push(perm);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    async fn list_for_resource(
        &self,
        resource_kind: &str,
        resource_id: Option<&str>,
    ) -> Result<Vec<StoredPermission>, StorageError> {
        // Match InMemory semantics exactly:
        //   - Some(id) => exact match on resource_id (wildcards NOT included).
        //   - None     => wildcard grants only (resource_id IS None).
        // Wildcard-aware access resolution belongs in `check()`.
        let mut db = self.db.lock().await;

        let prefix = resource_prefix(resource_kind, resource_id);
        let index_entries: Vec<(String, String)> = db
            .scan_typed(PERMS_BY_RESOURCE_STORE, &prefix)
            .map_err(StorageError::from)?;

        let mut out: Vec<StoredPermission> = Vec::with_capacity(index_entries.len());
        for (_, perm_id) in index_entries {
            if let Some(perm) = db
                .get_typed::<StoredPermission>(PERMISSIONS_STORE, &perm_id)
                .map_err(StorageError::from)?
            {
                out.push(perm);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
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
        // Scan all of the subject's grants, filter to matching resource
        // (exact-id OR wildcard), take MAX(level), compare against
        // required_level. We don't go through `list_for_subject` because we
        // already hold the lock and want to keep the whole check under one
        // lock acquisition.
        let mut db = self.db.lock().await;

        let prefix = subject_prefix(subject_kind, subject_id);
        let index_entries: Vec<(String, String)> = db
            .scan_typed(PERMS_BY_SUBJECT_STORE, &prefix)
            .map_err(StorageError::from)?;

        let mut best: Option<PermissionLevel> = None;
        for (_, perm_id) in index_entries {
            let Some(perm) = db
                .get_typed::<StoredPermission>(PERMISSIONS_STORE, &perm_id)
                .map_err(StorageError::from)?
            else {
                continue;
            };

            if perm.resource_kind != resource_kind {
                continue;
            }

            let matches = match (perm.resource_id.as_deref(), resource_id) {
                // Wildcard grant covers any specific (or wildcard) target.
                (None, _) => true,
                // Exact-id grant only matches the same exact id.
                (Some(pid), Some(tid)) => pid == tid,
                // Exact-id grant does NOT cover a wildcard query.
                (Some(_), None) => false,
            };

            if matches {
                best = Some(match best {
                    Some(current) => current.max(perm.level),
                    None => perm.level,
                });
            }
        }

        Ok(best.is_some_and(|level| level >= required_level))
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
    // ZqlPermissionStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_zql_grant_and_list_for_subject() {
        let store = ZqlPermissionStore::in_memory().await.unwrap();
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
    async fn test_zql_duplicate_grant_same_subject_same_resource() {
        // Matches `InMemoryPermissionStore::grant` semantics: grants are keyed
        // by `id`, not by (subject, resource). Two distinct ids with the same
        // subject+resource coexist; re-granting with the same id is an
        // idempotent upsert of the row's fields.
        let store = ZqlPermissionStore::in_memory().await.unwrap();

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
    async fn test_zql_revoke() {
        let store = ZqlPermissionStore::in_memory().await.unwrap();
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

        // And the by-resource index must also be clean.
        let by_res = store
            .list_for_resource("deployment", Some("d1"))
            .await
            .unwrap();
        assert!(by_res.is_empty());
    }

    #[tokio::test]
    async fn test_zql_list_for_resource_includes_wildcards() {
        // Matches InMemory semantics: list_for_resource(kind, Some(id)) returns
        // exact-id grants only; list_for_resource(kind, None) returns wildcard
        // grants only. Wildcard-covers-specific is a `check()` concern, not a
        // listing concern.
        let store = ZqlPermissionStore::in_memory().await.unwrap();

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
    async fn test_zql_check_max_wins() {
        let store = ZqlPermissionStore::in_memory().await.unwrap();

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

        // d1: MAX(Read, Write) = Write => required=Write must pass.
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

        // d2: only the wildcard Read matches => required=Write must fail,
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
    async fn test_zql_check_no_grant_returns_false() {
        let store = ZqlPermissionStore::in_memory().await.unwrap();

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
    async fn test_zql_persistent_round_trip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("permissions_zql_db");

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
            let store = ZqlPermissionStore::open(&db_path).await.unwrap();
            store.grant(&wildcard).await.unwrap();
            store.grant(&specific).await.unwrap();
        }

        // Reopen and verify rows, listing, and check() semantics all survive.
        {
            let store = ZqlPermissionStore::open(&db_path).await.unwrap();

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
            let store = ZqlPermissionStore::open(&db_path).await.unwrap();
            let u1 = store
                .list_for_subject(SubjectKind::User, "u1")
                .await
                .unwrap();
            assert!(u1.is_empty(), "revoke must persist across reopen");
        }
    }
}
