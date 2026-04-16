//! User group storage implementations
//!
//! Provides both persistent (ZQL via [`ZqlJsonStore`]) and in-memory storage
//! backends for user groups and their membership relationships.
//!
//! # Schema (persistent backend)
//!
//! The persistent backend owns three ZQL stores sharing a single
//! `zql::Database`:
//!
//! - `groups` — managed by the generic [`ZqlJsonStore`] adapter with a unique
//!   `name` companion index, so duplicate names surface as
//!   [`StorageError::AlreadyExists`] directly from the adapter.
//! - `group_members` — a link store keyed by `{group_id}:{user_id}` with a
//!   blob `()` value. Scanning the prefix `{group_id}:` enumerates a group's
//!   members cheaply without pulling every other group's rows.
//! - `group_members_by_user` — a reverse link store keyed by
//!   `{user_id}:{group_id}` so that `list_groups_for_user` is a prefix scan
//!   rather than a full-table scan over `group_members`.
//!
//! The three stores are kept consistent by a cascade in `delete`: the
//! membership rows (both forward and reverse) are torn down before the
//! primary row goes away. ZQL does not expose cross-store transactions, so
//! the operation is best-effort — callers that need strict atomicity should
//! avoid concurrent writers against a single group id, which matches the
//! trait contract.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::zql_json::{IndexSpec, JsonTable, ZqlJsonStore};
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

/// Name of the ZQL link store holding `{group_id}:{user_id}` memberships.
/// Values are unit-typed — the key carries all the information.
const GROUP_MEMBERS_STORE: &str = "group_members";

/// Name of the ZQL reverse link store keyed by `{user_id}:{group_id}`. Lets
/// `list_groups_for_user` run as a prefix scan instead of a full-table scan
/// across the forward link store.
const GROUP_MEMBERS_BY_USER_STORE: &str = "group_members_by_user";

/// Build the forward-link key for `(group_id, user_id)`.
fn member_key(group_id: &str, user_id: &str) -> String {
    format!("{group_id}:{user_id}")
}

/// Build the reverse-link key for `(user_id, group_id)`.
fn reverse_member_key(user_id: &str, group_id: &str) -> String {
    format!("{user_id}:{group_id}")
}

/// ZQL-backed persistent user group store.
///
/// Delegates group CRUD to a [`ZqlJsonStore<StoredUserGroup>`] configured for
/// the `groups` table and owns two secondary link stores on the same
/// database — `group_members` (keyed by `{group_id}:{user_id}`) and
/// `group_members_by_user` (keyed by `{user_id}:{group_id}`) — to track
/// memberships in both directions. Deleting a group cascades to its
/// membership rows (forward and reverse) before the primary row goes away.
pub struct ZqlGroupStore {
    inner: ZqlJsonStore<StoredUserGroup>,
}

impl ZqlGroupStore {
    /// Open or create a ZQL database at the given path. The `groups` primary
    /// store is initialised by the adapter; the `group_members` and
    /// `group_members_by_user` link stores are created lazily on first write,
    /// matching ZQL's store-on-demand model.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database connection or primary table
    /// creation fails.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let inner = ZqlJsonStore::<StoredUserGroup>::open(path, GROUPS_TABLE).await?;
        Ok(Self { inner })
    }

    /// Create a ZQL database in a temporary directory (useful for tests).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the temporary database cannot be created.
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self, StorageError> {
        let inner = ZqlJsonStore::<StoredUserGroup>::in_memory(GROUPS_TABLE).await?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl GroupStorage for ZqlGroupStore {
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
        // Preflight: if the primary row is already gone, there's no cascade
        // to run. Matches `InMemoryGroupStore::delete`.
        if self.inner.get(id).await?.is_none() {
            return Ok(false);
        }

        // Cascade: drop every forward/reverse membership row before the
        // primary record. Running forward first gives us the user ids we
        // need to key the reverse deletes.
        let mut db = self.inner.db().lock().await;

        let prefix = format!("{id}:");
        let forward: Vec<(String, ())> = db
            .scan_typed(GROUP_MEMBERS_STORE, &prefix)
            .map_err(StorageError::from)?;

        for (key, ()) in forward {
            // `key` is `{group_id}:{user_id}`. The group id is `id`, so
            // anything after the first `:` is the user id.
            let Some((_, uid)) = key.split_once(':') else {
                continue;
            };
            db.delete_typed(GROUP_MEMBERS_STORE, &key)
                .map_err(StorageError::from)?;
            let reverse = reverse_member_key(uid, id);
            db.delete_typed(GROUP_MEMBERS_BY_USER_STORE, &reverse)
                .map_err(StorageError::from)?;
        }

        drop(db);

        self.inner.delete(id).await
    }

    async fn add_member(&self, group_id: &str, user_id: &str) -> Result<(), StorageError> {
        // Preflight: the group must exist. Matches `InMemoryGroupStore` —
        // a missing group is `NotFound`, not a silently-added dangling row.
        if self.inner.get(group_id).await?.is_none() {
            return Err(StorageError::NotFound(format!("group {group_id}")));
        }

        // Double-add is idempotent: `put_typed` on an existing key is an
        // upsert with `()` as the value, so re-adding a (group, user) pair
        // leaves the store in the same observable state.
        let mut db = self.inner.db().lock().await;
        let forward = member_key(group_id, user_id);
        let reverse = reverse_member_key(user_id, group_id);

        db.put_typed(GROUP_MEMBERS_STORE, &forward, &())
            .map_err(StorageError::from)?;
        db.put_typed(GROUP_MEMBERS_BY_USER_STORE, &reverse, &())
            .map_err(StorageError::from)?;

        Ok(())
    }

    async fn remove_member(&self, group_id: &str, user_id: &str) -> Result<bool, StorageError> {
        // Preflight: the group must exist. Matches `InMemoryGroupStore` —
        // removing from an unknown group is `NotFound`, not a silent
        // `Ok(false)`.
        if self.inner.get(group_id).await?.is_none() {
            return Err(StorageError::NotFound(format!("group {group_id}")));
        }

        let mut db = self.inner.db().lock().await;
        let forward = member_key(group_id, user_id);

        // Was there a forward row to delete? ZQL has no "delete and report"
        // primitive, so peek first.
        let existed = db
            .get_typed::<()>(GROUP_MEMBERS_STORE, &forward)
            .map_err(StorageError::from)?
            .is_some();

        if existed {
            db.delete_typed(GROUP_MEMBERS_STORE, &forward)
                .map_err(StorageError::from)?;
            let reverse = reverse_member_key(user_id, group_id);
            db.delete_typed(GROUP_MEMBERS_BY_USER_STORE, &reverse)
                .map_err(StorageError::from)?;
        }

        Ok(existed)
    }

    async fn list_members(&self, group_id: &str) -> Result<Vec<String>, StorageError> {
        // Preflight: the group must exist. Matches `InMemoryGroupStore`.
        if self.inner.get(group_id).await?.is_none() {
            return Err(StorageError::NotFound(format!("group {group_id}")));
        }

        let mut db = self.inner.db().lock().await;
        let prefix = format!("{group_id}:");
        let forward: Vec<(String, ())> = db
            .scan_typed(GROUP_MEMBERS_STORE, &prefix)
            .map_err(StorageError::from)?;

        let mut users: Vec<String> = forward
            .into_iter()
            .filter_map(|(key, ())| key.split_once(':').map(|(_, uid)| uid.to_string()))
            .collect();
        users.sort();
        Ok(users)
    }

    async fn list_groups_for_user(&self, user_id: &str) -> Result<Vec<String>, StorageError> {
        let mut db = self.inner.db().lock().await;
        let prefix = format!("{user_id}:");
        let reverse: Vec<(String, ())> = db
            .scan_typed(GROUP_MEMBERS_BY_USER_STORE, &prefix)
            .map_err(StorageError::from)?;

        let mut groups: Vec<String> = reverse
            .into_iter()
            .filter_map(|(key, ())| key.split_once(':').map(|(_, gid)| gid.to_string()))
            .collect();
        groups.sort();
        Ok(groups)
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
    // ZqlGroupStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_zql_store_and_get() {
        let store = ZqlGroupStore::in_memory().await.unwrap();
        let group = StoredUserGroup::new("developers");
        let id = group.id.clone();

        store.store(&group).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("group must exist");
        assert_eq!(retrieved.name, "developers");
        assert!(retrieved.description.is_none());
    }

    #[tokio::test]
    async fn test_zql_duplicate_name() {
        let store = ZqlGroupStore::in_memory().await.unwrap();
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
    async fn test_zql_add_and_list_members() {
        let store = ZqlGroupStore::in_memory().await.unwrap();
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
    async fn test_zql_add_member_missing_group() {
        let store = ZqlGroupStore::in_memory().await.unwrap();
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
    async fn test_zql_remove_member_missing_group() {
        let store = ZqlGroupStore::in_memory().await.unwrap();
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
    async fn test_zql_remove_nonexistent_member() {
        // Removing a user that was never a member of an existing group must
        // return Ok(false), not an error — matches `InMemoryGroupStore`.
        let store = ZqlGroupStore::in_memory().await.unwrap();
        let group = StoredUserGroup::new("team");
        let gid = group.id.clone();
        store.store(&group).await.unwrap();

        let removed = store.remove_member(&gid, "never-added").await.unwrap();
        assert!(!removed, "removing a non-member must return Ok(false)");
    }

    #[tokio::test]
    async fn test_zql_double_add_member_idempotent() {
        let store = ZqlGroupStore::in_memory().await.unwrap();
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
    async fn test_zql_list_groups_for_user() {
        let store = ZqlGroupStore::in_memory().await.unwrap();

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
    async fn test_zql_delete_cascades_memberships() {
        let store = ZqlGroupStore::in_memory().await.unwrap();

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
    async fn test_zql_persistent_round_trip() {
        // Survives closing + reopening the database — the whole point of the
        // persistent backend. Membership rows must round-trip too.
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("groups.db");

        let group = StoredUserGroup::new("persist-me");
        let gid = group.id.clone();

        {
            let store = ZqlGroupStore::open(&db_path).await.unwrap();
            store.store(&group).await.unwrap();
            store.add_member(&gid, "user-a").await.unwrap();
            store.add_member(&gid, "user-b").await.unwrap();
        }

        {
            let store = ZqlGroupStore::open(&db_path).await.unwrap();
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
