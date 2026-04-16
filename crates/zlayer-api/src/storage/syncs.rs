//! Sync storage implementations.
//!
//! Provides both persistent (ZQL via [`ZqlJsonStore`]) and in-memory storage
//! backends for sync resources — named records that point at a directory
//! within a project's git checkout containing `ZLayer` resource YAMLs. Both
//! backends share the [`SyncStorage`] trait so callers are backend-agnostic.
//!
//! Uniqueness rule: `name` is globally unique. The ZQL backend enforces this
//! via a `name` unique index on [`ZqlJsonStore<StoredSync>`]; the in-memory
//! backend enforces it in-process in [`InMemorySyncStore::create`].

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::zql_json::{IndexSpec, JsonTable, ZqlJsonStore};
use super::{StorageError, StoredSync};

/// Trait for persisting sync records.
#[async_trait]
pub trait SyncStorage: Send + Sync {
    /// List all syncs, sorted by name ascending.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn list(&self) -> Result<Vec<StoredSync>, StorageError>;

    /// Get a sync by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn get(&self, id: &str) -> Result<Option<StoredSync>, StorageError>;

    /// Create a new sync. Rejects duplicate names with
    /// [`StorageError::AlreadyExists`].
    ///
    /// # Errors
    ///
    /// - [`StorageError::AlreadyExists`] if a sync with the same name exists.
    /// - [`StorageError::Database`] or other variants for backend failures.
    async fn create(&self, sync_record: StoredSync) -> Result<StoredSync, StorageError>;

    /// Delete a sync by id. Returns `true` if the row existed.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn delete(&self, id: &str) -> Result<bool, StorageError>;

    /// Replace an existing sync in place. The sync must already exist.
    ///
    /// # Errors
    ///
    /// - [`StorageError::NotFound`] if no sync with `sync_record.id` exists.
    /// - [`StorageError::Database`] or other variants for backend failures.
    async fn update(&self, sync_record: StoredSync) -> Result<StoredSync, StorageError>;
}

/// In-memory sync store backed by an `Arc<RwLock<HashMap>>`.
#[derive(Debug, Clone, Default)]
pub struct InMemorySyncStore {
    store: Arc<RwLock<HashMap<String, StoredSync>>>,
}

impl InMemorySyncStore {
    /// Create a new empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SyncStorage for InMemorySyncStore {
    async fn list(&self) -> Result<Vec<StoredSync>, StorageError> {
        let map = self.store.read().await;
        let mut syncs: Vec<StoredSync> = map.values().cloned().collect();
        syncs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(syncs)
    }

    async fn get(&self, id: &str) -> Result<Option<StoredSync>, StorageError> {
        let map = self.store.read().await;
        Ok(map.get(id).cloned())
    }

    async fn create(&self, sync_record: StoredSync) -> Result<StoredSync, StorageError> {
        let mut map = self.store.write().await;
        if map.values().any(|s| s.name == sync_record.name) {
            return Err(StorageError::AlreadyExists(format!(
                "sync '{}' already exists",
                sync_record.name
            )));
        }
        map.insert(sync_record.id.clone(), sync_record.clone());
        Ok(sync_record)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut map = self.store.write().await;
        Ok(map.remove(id).is_some())
    }

    async fn update(&self, sync_record: StoredSync) -> Result<StoredSync, StorageError> {
        let mut map = self.store.write().await;
        if !map.contains_key(&sync_record.id) {
            return Err(StorageError::NotFound(format!(
                "sync '{}' not found",
                sync_record.id
            )));
        }
        map.insert(sync_record.id.clone(), sync_record.clone());
        Ok(sync_record)
    }
}

/// Static table specification for the `syncs` table.
///
/// Two peeled columns:
///
/// - `name`: unique, single-column. Enforces global name uniqueness via a
///   companion `syncs_by_name` store; a second `create` with the same name
///   surfaces as [`StorageError::AlreadyExists`] from the ZQL adapter.
/// - `project_id`: non-unique, nullable. Declared so future `project_id`-
///   scoped lookups can use [`ZqlJsonStore::list_where`] /
///   [`ZqlJsonStore::get_by_unique`] without a custom scan.
const SYNCS_TABLE: JsonTable<StoredSync> = JsonTable {
    name: "syncs",
    indexes: &[
        IndexSpec {
            column: "name",
            extractor: |s| Some(s.name.clone()),
            unique: true,
        },
        IndexSpec {
            column: "project_id",
            extractor: |s| s.project_id.clone(),
            unique: false,
        },
    ],
    unique_constraints: &[],
};

/// ZQL-backed persistent sync store.
///
/// Delegates all storage operations to a [`ZqlJsonStore<StoredSync>`]
/// configured for the `syncs` table. Records are serialised as typed values
/// into the underlying ZQL store keyed by `id`; the daemon reopens the
/// database on start-up and sync records survive restarts.
pub struct ZqlSyncStore {
    inner: ZqlJsonStore<StoredSync>,
}

impl ZqlSyncStore {
    /// Open or create a ZQL database at the given path.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database connection or table creation
    /// fails.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let inner = ZqlJsonStore::open(path, SYNCS_TABLE).await?;
        Ok(Self { inner })
    }

    /// Create a ZQL database in a temporary directory (useful for tests).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the temporary database cannot be created.
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self, StorageError> {
        let inner = ZqlJsonStore::in_memory(SYNCS_TABLE).await?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl SyncStorage for ZqlSyncStore {
    async fn list(&self) -> Result<Vec<StoredSync>, StorageError> {
        // The adapter orders by `id`; the trait contract sorts by name.
        let mut list = self.inner.list().await?;
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn get(&self, id: &str) -> Result<Option<StoredSync>, StorageError> {
        self.inner.get(id).await
    }

    async fn create(&self, sync_record: StoredSync) -> Result<StoredSync, StorageError> {
        // Preflight name-uniqueness check. The `name` column is a unique index
        // so the adapter would still reject a conflict with AlreadyExists, but
        // checking ahead of time yields a cleaner error message and avoids
        // round-tripping the upsert machinery for a known failure.
        if let Some(existing) = self.inner.get_by_unique("name", &sync_record.name).await? {
            if existing.id != sync_record.id {
                return Err(StorageError::AlreadyExists(format!(
                    "sync '{}' already exists",
                    sync_record.name
                )));
            }
        }

        self.inner.put(&sync_record.id, &sync_record).await?;
        Ok(sync_record)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        self.inner.delete(id).await
    }

    async fn update(&self, sync_record: StoredSync) -> Result<StoredSync, StorageError> {
        // Update is "replace in place" — the row must exist.
        if self.inner.get(&sync_record.id).await?.is_none() {
            return Err(StorageError::NotFound(format!(
                "sync '{}' not found",
                sync_record.id
            )));
        }

        self.inner.put(&sync_record.id, &sync_record).await?;
        Ok(sync_record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // InMemorySyncStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_create_and_list() {
        let store = InMemorySyncStore::new();

        let sync = StoredSync::new("my-sync", "deployments/");
        store.create(sync.clone()).await.unwrap();

        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "my-sync");
    }

    #[tokio::test]
    async fn test_duplicate_name_rejected() {
        let store = InMemorySyncStore::new();

        let sync1 = StoredSync::new("dup", "a/");
        store.create(sync1).await.unwrap();

        let sync2 = StoredSync::new("dup", "b/");
        let err = store.create(sync2).await.expect_err("duplicate must fail");
        match err {
            StorageError::AlreadyExists(msg) => {
                assert!(msg.contains("dup"), "message should name the sync: {msg}");
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_get_existing() {
        let store = InMemorySyncStore::new();
        let sync = StoredSync::new("find-me", "path/");
        let created = store.create(sync).await.unwrap();

        let found = store.get(&created.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "find-me");
    }

    #[tokio::test]
    async fn test_get_missing() {
        let store = InMemorySyncStore::new();
        let found = store.get("nonexistent").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_delete() {
        let store = InMemorySyncStore::new();
        let sync = StoredSync::new("deletable", "path/");
        let created = store.create(sync).await.unwrap();

        assert!(store.delete(&created.id).await.unwrap());
        assert!(!store.delete(&created.id).await.unwrap());
        assert!(store.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_update() {
        let store = InMemorySyncStore::new();
        let sync = StoredSync::new("updatable", "old/");
        let mut created = store.create(sync).await.unwrap();

        created.git_path = "new/".to_string();
        let updated = store.update(created).await.unwrap();
        assert_eq!(updated.git_path, "new/");
    }

    #[tokio::test]
    async fn test_update_missing() {
        let store = InMemorySyncStore::new();
        let sync = StoredSync::new("ghost", "path/");
        let err = store.update(sync).await.expect_err("missing must fail");
        match err {
            StorageError::NotFound(msg) => {
                assert!(msg.contains("not found"));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_default_delete_missing_is_false() {
        // Sanity check on the constructor: a fresh StoredSync must have
        // delete_missing = false (the safer default).
        let sync = StoredSync::new("x", "y/");
        assert!(!sync.delete_missing);
    }

    // =========================================================================
    // ZqlSyncStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_zql_list_and_get_roundtrip() {
        let store = ZqlSyncStore::in_memory().await.unwrap();

        let mut sync = StoredSync::new("roundtrip", "deployments/");
        sync.project_id = Some("proj-1".to_string());
        sync.auto_apply = true;
        sync.delete_missing = true;
        sync.last_applied_sha = Some("deadbeef".to_string());

        let id = sync.id.clone();
        let created = store.create(sync.clone()).await.unwrap();
        assert_eq!(created.name, "roundtrip");

        // get by id
        let fetched = store
            .get(&id)
            .await
            .unwrap()
            .expect("sync must exist after create");
        assert_eq!(fetched.name, "roundtrip");
        assert_eq!(fetched.git_path, "deployments/");
        assert_eq!(fetched.project_id.as_deref(), Some("proj-1"));
        assert!(fetched.auto_apply);
        assert!(fetched.delete_missing);
        assert_eq!(fetched.last_applied_sha.as_deref(), Some("deadbeef"));

        // list
        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
    }

    #[tokio::test]
    async fn test_zql_list_sorted_by_name() {
        let store = ZqlSyncStore::in_memory().await.unwrap();
        store.create(StoredSync::new("gamma", "g/")).await.unwrap();
        store.create(StoredSync::new("alpha", "a/")).await.unwrap();
        store.create(StoredSync::new("beta", "b/")).await.unwrap();

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "alpha");
        assert_eq!(list[1].name, "beta");
        assert_eq!(list[2].name, "gamma");
    }

    #[tokio::test]
    async fn test_zql_create_duplicate_name() {
        let store = ZqlSyncStore::in_memory().await.unwrap();

        store.create(StoredSync::new("dup", "a/")).await.unwrap();

        let err = store
            .create(StoredSync::new("dup", "b/"))
            .await
            .expect_err("duplicate name must fail");
        match err {
            StorageError::AlreadyExists(msg) => {
                assert!(
                    msg.contains("dup"),
                    "message should mention the sync name, got: {msg}"
                );
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_zql_update_nonexistent() {
        let store = ZqlSyncStore::in_memory().await.unwrap();
        let ghost = StoredSync::new("ghost", "path/");

        let err = store
            .update(ghost)
            .await
            .expect_err("update of missing row must fail");
        match err {
            StorageError::NotFound(msg) => {
                assert!(msg.contains("not found"), "got: {msg}");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_zql_update_existing() {
        let store = ZqlSyncStore::in_memory().await.unwrap();
        let sync = StoredSync::new("updatable", "old/");
        let id = sync.id.clone();
        store.create(sync.clone()).await.unwrap();

        let mut updated = sync.clone();
        updated.git_path = "new/".to_string();
        updated.delete_missing = true;
        let returned = store.update(updated).await.unwrap();
        assert_eq!(returned.git_path, "new/");
        assert!(returned.delete_missing);

        let fetched = store.get(&id).await.unwrap().unwrap();
        assert_eq!(fetched.git_path, "new/");
        assert!(fetched.delete_missing);
    }

    #[tokio::test]
    async fn test_zql_delete() {
        let store = ZqlSyncStore::in_memory().await.unwrap();
        let sync = StoredSync::new("doomed", "path/");
        let id = sync.id.clone();
        store.create(sync).await.unwrap();

        assert!(store.delete(&id).await.unwrap());
        assert!(store.get(&id).await.unwrap().is_none());
        // Repeated delete / unknown id must return Ok(false), not error.
        assert!(!store.delete(&id).await.unwrap());
        assert!(!store.delete("nonexistent-id").await.unwrap());
    }

    #[tokio::test]
    async fn test_zql_delete_missing_field_defaults_false() {
        // Construct a minimal blob with no `delete_missing` key; the
        // `#[serde(default)]` attribute must deserialise it to `false` after
        // the round-trip through the store.
        let store = ZqlSyncStore::in_memory().await.unwrap();

        let json = serde_json::json!({
            "id": "legacy-sync-id",
            "name": "legacy-sync",
            "project_id": null,
            "git_path": "deployments/",
            "auto_apply": false,
            "last_applied_sha": null,
            "created_at": "2026-04-15T12:00:00Z",
            "updated_at": "2026-04-15T12:00:00Z",
        });
        let legacy: StoredSync =
            serde_json::from_value(json).expect("legacy JSON without delete_missing must parse");
        assert!(
            !legacy.delete_missing,
            "serde default must produce false for missing field"
        );

        store.create(legacy.clone()).await.unwrap();
        let fetched = store.get(&legacy.id).await.unwrap().unwrap();
        assert!(
            !fetched.delete_missing,
            "delete_missing must round-trip as false when serialised from a default"
        );
    }

    #[tokio::test]
    async fn test_zql_delete_missing_field_persists_true() {
        let store = ZqlSyncStore::in_memory().await.unwrap();

        let mut sync = StoredSync::new("with-deletes", "deployments/");
        sync.delete_missing = true;
        let id = sync.id.clone();
        store.create(sync).await.unwrap();

        let fetched = store.get(&id).await.unwrap().unwrap();
        assert!(
            fetched.delete_missing,
            "delete_missing = true must persist through the store"
        );
    }

    #[tokio::test]
    async fn test_zql_persistent_round_trip() {
        // Survives closing + reopening the database — the whole point of the
        // persistent backend.
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("syncs.db");

        let mut sync = StoredSync::new("persist-me", "deployments/");
        sync.project_id = Some("proj-x".to_string());
        sync.delete_missing = true;
        let id = sync.id.clone();

        {
            let store = ZqlSyncStore::open(&db_path).await.unwrap();
            store.create(sync.clone()).await.unwrap();
        }

        {
            let store = ZqlSyncStore::open(&db_path).await.unwrap();
            let retrieved = store.get(&id).await.unwrap().expect("must persist");
            assert_eq!(retrieved.name, "persist-me");
            assert_eq!(retrieved.project_id.as_deref(), Some("proj-x"));
            assert!(retrieved.delete_missing);

            // List still works after reopen.
            let list = store.list().await.unwrap();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].id, id);
        }
    }
}
