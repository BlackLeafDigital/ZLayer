//! User group storage implementations
//!
//! Provides both persistent (`SQLite` via [`SqlxJsonStore`]) and in-memory
//! storage backends for user groups and their membership relationships.
//!
//! # Schema (persistent backend)
//!
//! The persistent backend owns two tables sharing a single `SqlitePool`:
//!
//! - `groups` — managed by the generic [`SqlxJsonStore`] adapter with a unique
//!   `name` peeled column so duplicate names surface as
//!   [`StorageError::AlreadyExists`] directly from the adapter.
//! - `group_members` — a secondary link table keyed by the composite
//!   `(group_id, user_id)` primary key, with `(user_id)` indexed for fast
//!   reverse lookup in [`GroupStorage::list_groups_for_user`].
//!
//! The two tables are kept consistent by a cascading `delete` that wraps both
//! `DELETE` statements in a single `sqlx` transaction, so a crash between the
//! child and parent delete cannot leave orphaned membership rows behind.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::sqlite::SqlitePool;
use tokio::sync::RwLock;

use super::sqlx_json::{IndexSpec, JsonTable, SqlxJsonStore};
use super::{StorageError, StoredUserGroup};

/// Trait for user group storage backends.
#[async_trait]
pub trait GroupStorage: Send + Sync {
    /// Store (create or update) a group.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the write.
    async fn store(&self, group: &StoredUserGroup) -> Result<(), StorageError>;

    /// Fetch a group by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn get(&self, id: &str) -> Result<Option<StoredUserGroup>, StorageError>;

    /// List all groups sorted by name (ascending).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn list(&self) -> Result<Vec<StoredUserGroup>, StorageError>;

    /// Delete a group by id. Returns `true` if the group existed.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the delete operation fails.
    async fn delete(&self, id: &str) -> Result<bool, StorageError>;

    /// Add a user to a group.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::NotFound`] if the group does not exist.
    async fn add_member(&self, group_id: &str, user_id: &str) -> Result<(), StorageError>;

    /// Remove a user from a group. Returns `true` if the user was a member.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::NotFound`] if the group does not exist.
    async fn remove_member(&self, group_id: &str, user_id: &str) -> Result<bool, StorageError>;

    /// List all user ids that belong to a group.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::NotFound`] if the group does not exist.
    async fn list_members(&self, group_id: &str) -> Result<Vec<String>, StorageError>;

    /// List all group ids that a user belongs to.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn list_groups_for_user(&self, user_id: &str) -> Result<Vec<String>, StorageError>;
}

/// In-memory group store.
pub struct InMemoryGroupStore {
    groups: Arc<RwLock<HashMap<String, StoredUserGroup>>>,
    /// `group_id` -> set of `user_id`s
    members: Arc<RwLock<HashMap<String, HashSet<String>>>>,
}

impl InMemoryGroupStore {
    /// Create a new empty in-memory group store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            groups: Arc::new(RwLock::new(HashMap::new())),
            members: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryGroupStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl GroupStorage for InMemoryGroupStore {
    async fn store(&self, group: &StoredUserGroup) -> Result<(), StorageError> {
        let mut groups = self.groups.write().await;
        groups.insert(group.id.clone(), group.clone());
        // Ensure the membership set exists for this group.
        let mut members = self.members.write().await;
        members.entry(group.id.clone()).or_default();
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredUserGroup>, StorageError> {
        let groups = self.groups.read().await;
        Ok(groups.get(id).cloned())
    }

    async fn list(&self) -> Result<Vec<StoredUserGroup>, StorageError> {
        let groups = self.groups.read().await;
        let mut list: Vec<_> = groups.values().cloned().collect();
        list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut groups = self.groups.write().await;
        let existed = groups.remove(id).is_some();
        if existed {
            let mut members = self.members.write().await;
            members.remove(id);
        }
        Ok(existed)
    }

    async fn add_member(&self, group_id: &str, user_id: &str) -> Result<(), StorageError> {
        let groups = self.groups.read().await;
        if !groups.contains_key(group_id) {
            return Err(StorageError::NotFound(format!("group {group_id}")));
        }
        drop(groups);

        let mut members = self.members.write().await;
        members
            .entry(group_id.to_string())
            .or_default()
            .insert(user_id.to_string());
        Ok(())
    }

    async fn remove_member(&self, group_id: &str, user_id: &str) -> Result<bool, StorageError> {
        let groups = self.groups.read().await;
        if !groups.contains_key(group_id) {
            return Err(StorageError::NotFound(format!("group {group_id}")));
        }
        drop(groups);

        let mut members = self.members.write().await;
        let removed = members
            .get_mut(group_id)
            .is_some_and(|set| set.remove(user_id));
        Ok(removed)
    }

    async fn list_members(&self, group_id: &str) -> Result<Vec<String>, StorageError> {
        let groups = self.groups.read().await;
        if !groups.contains_key(group_id) {
            return Err(StorageError::NotFound(format!("group {group_id}")));
        }
        drop(groups);

        let members = self.members.read().await;
        let mut list: Vec<_> = members
            .get(group_id)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default();
        list.sort();
        Ok(list)
    }

    async fn list_groups_for_user(&self, user_id: &str) -> Result<Vec<String>, StorageError> {
        let members = self.members.read().await;
        let mut group_ids: Vec<_> = members
            .iter()
            .filter_map(|(gid, set)| {
                if set.contains(user_id) {
                    Some(gid.clone())
                } else {
                    None
                }
            })
            .collect();
        group_ids.sort();
        Ok(group_ids)
    }
}

/// Static table specification for the `groups` table.
///
/// One peeled column: `name`, unique and single-column. Matches the existing
/// in-memory store semantics (groups are distinguished by opaque `id`, but
/// `name` uniqueness is enforced at the schema level so duplicate names
/// surface as [`StorageError::AlreadyExists`] directly from the adapter).
const GROUPS_TABLE: JsonTable<StoredUserGroup> = JsonTable {
    name: "groups",
    indexes: &[IndexSpec {
        column: "name",
        extractor: |g| Some(g.name.clone()),
        unique: true,
    }],
    unique_constraints: &[],
};

/// `SQLite`-backed persistent user group store.
///
/// Delegates group CRUD to a [`SqlxJsonStore<StoredUserGroup>`] configured for
/// the `groups` table and owns a secondary `group_members` link table on the
/// same pool to track `(group_id, user_id)` pairs. Deleting a group cascades
/// to its memberships inside a single transaction so partial failures cannot
/// leave orphaned membership rows behind.
pub struct SqlxGroupStore {
    inner: SqlxJsonStore<StoredUserGroup>,
}

impl SqlxGroupStore {
    /// Open or create a `SQLite` database at the given path and ensure both
    /// the `groups` and `group_members` tables exist.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database connection, primary table
    /// creation, or `group_members` secondary schema cannot be established.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let inner = SqlxJsonStore::<StoredUserGroup>::open(path, GROUPS_TABLE).await?;
        Self::init_group_members_schema(inner.pool()).await?;
        Ok(Self { inner })
    }

    /// Create an in-memory `SQLite` database (useful for tests) with both
    /// the `groups` and `group_members` tables in place.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the in-memory database cannot be created.
    pub async fn in_memory() -> Result<Self, StorageError> {
        let inner = SqlxJsonStore::<StoredUserGroup>::in_memory(GROUPS_TABLE).await?;
        Self::init_group_members_schema(inner.pool()).await?;
        Ok(Self { inner })
    }

    /// Create the `group_members` secondary table and its `(user_id)` index
    /// if they don't already exist. Idempotent.
    ///
    /// The composite `(group_id, user_id)` primary key enforces per-pair
    /// uniqueness at the schema level — so a second `add_member` with the
    /// same pair can be made idempotent via `INSERT OR IGNORE`, matching the
    /// in-memory backend's `HashSet::insert` semantics. The `idx_group_members_user`
    /// index covers the reverse `list_groups_for_user` lookup so it doesn't
    /// have to full-scan the table.
    async fn init_group_members_schema(pool: &SqlitePool) -> Result<(), StorageError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS group_members (\n    \
                 group_id TEXT NOT NULL,\n    \
                 user_id TEXT NOT NULL,\n    \
                 added_at TEXT NOT NULL,\n    \
                 PRIMARY KEY (group_id, user_id)\n\
             )",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_group_members_user \
             ON group_members(user_id)",
        )
        .execute(pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl GroupStorage for SqlxGroupStore {
    async fn store(&self, group: &StoredUserGroup) -> Result<(), StorageError> {
        self.inner.put(&group.id, group).await
    }

    async fn get(&self, id: &str) -> Result<Option<StoredUserGroup>, StorageError> {
        self.inner.get(id).await
    }

    async fn list(&self) -> Result<Vec<StoredUserGroup>, StorageError> {
        // The adapter orders by `id`; the trait contract sorts by
        // lower-cased name to match the in-memory backend.
        let mut list = self.inner.list().await?;
        list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        // Cascade delete: membership rows first, then the group itself.
        // Wrapping both statements in a transaction means either both succeed
        // or neither does — the `group_members` table can never be left
        // referencing a deleted group.
        let pool = self.inner.pool();
        let mut tx = pool.begin().await?;

        sqlx::query("DELETE FROM group_members WHERE group_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;

        let result = sqlx::query("DELETE FROM groups WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(result.rows_affected() > 0)
    }

    async fn add_member(&self, group_id: &str, user_id: &str) -> Result<(), StorageError> {
        // Preflight: the group must exist. Matches `InMemoryGroupStore` which
        // errors with `NotFound` rather than silently creating a dangling
        // membership row.
        if self.inner.get(group_id).await?.is_none() {
            return Err(StorageError::NotFound(format!("group {group_id}")));
        }

        // `INSERT OR IGNORE` makes double-add idempotent, matching
        // `InMemoryGroupStore` (which uses a `HashSet` and silently
        // deduplicates).
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR IGNORE INTO group_members (group_id, user_id, added_at) \
             VALUES (?, ?, ?)",
        )
        .bind(group_id)
        .bind(user_id)
        .bind(&now)
        .execute(self.inner.pool())
        .await?;

        Ok(())
    }

    async fn remove_member(&self, group_id: &str, user_id: &str) -> Result<bool, StorageError> {
        // Preflight: the group must exist. Matches `InMemoryGroupStore` —
        // removing from an unknown group is a `NotFound`, not a silent
        // `Ok(false)`.
        if self.inner.get(group_id).await?.is_none() {
            return Err(StorageError::NotFound(format!("group {group_id}")));
        }

        let result = sqlx::query("DELETE FROM group_members WHERE group_id = ? AND user_id = ?")
            .bind(group_id)
            .bind(user_id)
            .execute(self.inner.pool())
            .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn list_members(&self, group_id: &str) -> Result<Vec<String>, StorageError> {
        // Preflight: the group must exist. Matches `InMemoryGroupStore`.
        if self.inner.get(group_id).await?.is_none() {
            return Err(StorageError::NotFound(format!("group {group_id}")));
        }

        // Ordered by `added_at ASC` so the oldest memberships come first;
        // the in-memory store sorts alphabetically, but the trait only says
        // "all user ids that belong to a group" without a specific order —
        // the specific order is covered by test assertions that use
        // `contains` rather than indexed compares on the sqlx tests.
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT user_id FROM group_members WHERE group_id = ? \
             ORDER BY added_at ASC",
        )
        .bind(group_id)
        .fetch_all(self.inner.pool())
        .await?;

        Ok(rows)
    }

    async fn list_groups_for_user(&self, user_id: &str) -> Result<Vec<String>, StorageError> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT group_id FROM group_members WHERE user_id = ? \
             ORDER BY added_at ASC",
        )
        .bind(user_id)
        .fetch_all(self.inner.pool())
        .await?;

        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StoredUserGroup;

    #[tokio::test]
    async fn test_store_and_get() {
        let store = InMemoryGroupStore::new();
        let group = StoredUserGroup::new("developers");
        let id = group.id.clone();

        store.store(&group).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("group must exist");
        assert_eq!(retrieved.name, "developers");
        assert!(retrieved.description.is_none());
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let store = InMemoryGroupStore::new();
        let result = store.get("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_list_sorted_by_name() {
        let store = InMemoryGroupStore::new();
        store.store(&StoredUserGroup::new("charlie")).await.unwrap();
        store.store(&StoredUserGroup::new("alpha")).await.unwrap();
        store.store(&StoredUserGroup::new("bravo")).await.unwrap();

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "alpha");
        assert_eq!(list[1].name, "bravo");
        assert_eq!(list[2].name, "charlie");
    }

    #[tokio::test]
    async fn test_delete() {
        let store = InMemoryGroupStore::new();
        let group = StoredUserGroup::new("to-delete");
        let id = group.id.clone();
        store.store(&group).await.unwrap();

        let deleted = store.delete(&id).await.unwrap();
        assert!(deleted);
        assert!(store.get(&id).await.unwrap().is_none());

        let deleted_again = store.delete(&id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_delete_removes_membership() {
        let store = InMemoryGroupStore::new();
        let group = StoredUserGroup::new("ephemeral");
        let gid = group.id.clone();
        store.store(&group).await.unwrap();
        store.add_member(&gid, "user-1").await.unwrap();

        store.delete(&gid).await.unwrap();

        // After deletion, listing members for that group should fail.
        let err = store.list_members(&gid).await.unwrap_err();
        assert!(matches!(err, StorageError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_add_and_list_members() {
        let store = InMemoryGroupStore::new();
        let group = StoredUserGroup::new("team");
        let gid = group.id.clone();
        store.store(&group).await.unwrap();

        store.add_member(&gid, "user-b").await.unwrap();
        store.add_member(&gid, "user-a").await.unwrap();

        let members = store.list_members(&gid).await.unwrap();
        assert_eq!(members, vec!["user-a", "user-b"]);
    }

    #[tokio::test]
    async fn test_add_member_idempotent() {
        let store = InMemoryGroupStore::new();
        let group = StoredUserGroup::new("team");
        let gid = group.id.clone();
        store.store(&group).await.unwrap();

        store.add_member(&gid, "user-1").await.unwrap();
        store.add_member(&gid, "user-1").await.unwrap();

        let members = store.list_members(&gid).await.unwrap();
        assert_eq!(members.len(), 1);
    }

    #[tokio::test]
    async fn test_add_member_nonexistent_group() {
        let store = InMemoryGroupStore::new();
        let err = store.add_member("nonexistent", "user-1").await.unwrap_err();
        assert!(matches!(err, StorageError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_remove_member() {
        let store = InMemoryGroupStore::new();
        let group = StoredUserGroup::new("team");
        let gid = group.id.clone();
        store.store(&group).await.unwrap();

        store.add_member(&gid, "user-1").await.unwrap();
        let removed = store.remove_member(&gid, "user-1").await.unwrap();
        assert!(removed);

        let removed_again = store.remove_member(&gid, "user-1").await.unwrap();
        assert!(!removed_again);

        let members = store.list_members(&gid).await.unwrap();
        assert!(members.is_empty());
    }

    #[tokio::test]
    async fn test_remove_member_nonexistent_group() {
        let store = InMemoryGroupStore::new();
        let err = store
            .remove_member("nonexistent", "user-1")
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_list_members_nonexistent_group() {
        let store = InMemoryGroupStore::new();
        let err = store.list_members("nonexistent").await.unwrap_err();
        assert!(matches!(err, StorageError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_list_groups_for_user() {
        let store = InMemoryGroupStore::new();

        let g1 = StoredUserGroup::new("alpha");
        let g1_id = g1.id.clone();
        let g2 = StoredUserGroup::new("bravo");
        let g2_id = g2.id.clone();
        let g3 = StoredUserGroup::new("charlie");
        let g3_id = g3.id.clone();

        store.store(&g1).await.unwrap();
        store.store(&g2).await.unwrap();
        store.store(&g3).await.unwrap();

        store.add_member(&g1_id, "user-x").await.unwrap();
        store.add_member(&g3_id, "user-x").await.unwrap();

        let groups = store.list_groups_for_user("user-x").await.unwrap();
        assert_eq!(groups.len(), 2);
        assert!(groups.contains(&g1_id));
        assert!(groups.contains(&g3_id));
        assert!(!groups.contains(&g2_id));
    }

    #[tokio::test]
    async fn test_list_groups_for_user_none() {
        let store = InMemoryGroupStore::new();
        let groups = store.list_groups_for_user("ghost").await.unwrap();
        assert!(groups.is_empty());
    }

    #[tokio::test]
    async fn test_update_group() {
        let store = InMemoryGroupStore::new();
        let mut group = StoredUserGroup::new("original");
        let id = group.id.clone();
        store.store(&group).await.unwrap();

        group.name = "renamed".to_string();
        group.description = Some("updated description".to_string());
        store.store(&group).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.name, "renamed");
        assert_eq!(
            retrieved.description.as_deref(),
            Some("updated description")
        );
    }

    // =========================================================================
    // SqlxGroupStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_sqlx_store_and_get() {
        let store = SqlxGroupStore::in_memory().await.unwrap();
        let group = StoredUserGroup::new("developers");
        let id = group.id.clone();

        store.store(&group).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("group must exist");
        assert_eq!(retrieved.name, "developers");
        assert!(retrieved.description.is_none());
    }

    #[tokio::test]
    async fn test_sqlx_duplicate_name() {
        let store = SqlxGroupStore::in_memory().await.unwrap();
        store.store(&StoredUserGroup::new("ops")).await.unwrap();

        let err = store
            .store(&StoredUserGroup::new("ops"))
            .await
            .expect_err("duplicate name must fail");
        match err {
            StorageError::AlreadyExists(msg) => {
                assert!(
                    msg.contains("groups"),
                    "message should mention the table name, got: {msg}"
                );
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sqlx_add_and_list_members() {
        let store = SqlxGroupStore::in_memory().await.unwrap();
        let group = StoredUserGroup::new("team");
        let gid = group.id.clone();
        store.store(&group).await.unwrap();

        store.add_member(&gid, "user-a").await.unwrap();
        store.add_member(&gid, "user-b").await.unwrap();

        let members = store.list_members(&gid).await.unwrap();
        assert_eq!(members.len(), 2);
        assert!(members.contains(&"user-a".to_string()));
        assert!(members.contains(&"user-b".to_string()));
    }

    #[tokio::test]
    async fn test_sqlx_add_member_missing_group() {
        let store = SqlxGroupStore::in_memory().await.unwrap();
        let err = store
            .add_member("ghost-group", "user-1")
            .await
            .expect_err("missing group must fail");
        match err {
            StorageError::NotFound(msg) => {
                assert!(msg.contains("ghost-group"), "got: {msg}");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sqlx_remove_member_missing_group() {
        let store = SqlxGroupStore::in_memory().await.unwrap();
        let err = store
            .remove_member("ghost-group", "user-1")
            .await
            .expect_err("missing group must fail");
        match err {
            StorageError::NotFound(msg) => {
                assert!(msg.contains("ghost-group"), "got: {msg}");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sqlx_remove_nonexistent_member() {
        // Removing a user that was never a member of an existing group must
        // return Ok(false), not an error — matches `InMemoryGroupStore`.
        let store = SqlxGroupStore::in_memory().await.unwrap();
        let group = StoredUserGroup::new("team");
        let gid = group.id.clone();
        store.store(&group).await.unwrap();

        let removed = store.remove_member(&gid, "never-added").await.unwrap();
        assert!(!removed, "removing a non-member must return Ok(false)");
    }

    #[tokio::test]
    async fn test_sqlx_double_add_member_idempotent() {
        let store = SqlxGroupStore::in_memory().await.unwrap();
        let group = StoredUserGroup::new("team");
        let gid = group.id.clone();
        store.store(&group).await.unwrap();

        store.add_member(&gid, "user-1").await.unwrap();
        // Second add of the same pair must succeed silently (idempotent).
        store.add_member(&gid, "user-1").await.unwrap();

        let members = store.list_members(&gid).await.unwrap();
        assert_eq!(
            members.len(),
            1,
            "double-add must not create a duplicate membership row"
        );
        assert_eq!(members[0], "user-1");
    }

    #[tokio::test]
    async fn test_sqlx_list_groups_for_user() {
        let store = SqlxGroupStore::in_memory().await.unwrap();

        let g1 = StoredUserGroup::new("alpha");
        let g1_id = g1.id.clone();
        let g2 = StoredUserGroup::new("bravo");
        let g2_id = g2.id.clone();
        store.store(&g1).await.unwrap();
        store.store(&g2).await.unwrap();

        store.add_member(&g1_id, "user-x").await.unwrap();
        store.add_member(&g2_id, "user-x").await.unwrap();

        let groups = store.list_groups_for_user("user-x").await.unwrap();
        assert_eq!(groups.len(), 2);
        assert!(groups.contains(&g1_id));
        assert!(groups.contains(&g2_id));
    }

    #[tokio::test]
    async fn test_sqlx_delete_cascades_memberships() {
        let store = SqlxGroupStore::in_memory().await.unwrap();

        let g1 = StoredUserGroup::new("doomed");
        let g1_id = g1.id.clone();
        let g2 = StoredUserGroup::new("survivor");
        let g2_id = g2.id.clone();
        store.store(&g1).await.unwrap();
        store.store(&g2).await.unwrap();

        // user-shared sits in both groups; the cascade should strip only the
        // `doomed` membership, leaving the `survivor` one intact.
        store.add_member(&g1_id, "user-shared").await.unwrap();
        store.add_member(&g1_id, "user-only-doomed").await.unwrap();
        store.add_member(&g2_id, "user-shared").await.unwrap();

        let deleted = store.delete(&g1_id).await.unwrap();
        assert!(deleted);

        // The doomed group's row is gone.
        assert!(store.get(&g1_id).await.unwrap().is_none());

        // `user-shared` is still in `survivor`, NOT in `doomed`.
        let shared_groups = store.list_groups_for_user("user-shared").await.unwrap();
        assert_eq!(shared_groups, vec![g2_id.clone()]);

        // The user that was only in the doomed group has no memberships left.
        let orphan_groups = store
            .list_groups_for_user("user-only-doomed")
            .await
            .unwrap();
        assert!(
            orphan_groups.is_empty(),
            "cascade must strip all memberships for the deleted group"
        );

        // list_members on the deleted group must now NotFound, since the
        // preflight check runs against the (now-missing) groups row.
        let err = store.list_members(&g1_id).await.unwrap_err();
        assert!(matches!(err, StorageError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_sqlx_persistent_round_trip() {
        // Survives closing + reopening the database — the whole point of the
        // persistent backend. Membership rows must round-trip too.
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("groups.db");

        let group = StoredUserGroup::new("persist-me");
        let gid = group.id.clone();

        {
            let store = SqlxGroupStore::open(&db_path).await.unwrap();
            store.store(&group).await.unwrap();
            store.add_member(&gid, "user-a").await.unwrap();
            store.add_member(&gid, "user-b").await.unwrap();
        }

        {
            let store = SqlxGroupStore::open(&db_path).await.unwrap();
            let retrieved = store.get(&gid).await.unwrap().expect("must persist");
            assert_eq!(retrieved.name, "persist-me");

            let members = store.list_members(&gid).await.unwrap();
            assert_eq!(members.len(), 2);
            assert!(members.contains(&"user-a".to_string()));
            assert!(members.contains(&"user-b".to_string()));

            let groups_for_a = store.list_groups_for_user("user-a").await.unwrap();
            assert_eq!(groups_for_a, vec![gid.clone()]);
        }
    }
}
