//! Environment storage implementations
//!
//! Provides both persistent (ZQL) and in-memory storage backends for
//! deployment/runtime environments. An environment is an isolated namespace
//! for secrets and (eventually) deployments. An environment optionally belongs
//! to a project — when `project_id` is `None`, the environment is global.
//!
//! Uniqueness rule: `(name, project_id)` is unique. The persistent backend
//! enforces this with a parallel ZQL store keyed by `{project_id_or_empty}:{name}`
//! that records the owning environment id, mirroring how `users.rs` enforces
//! email uniqueness with a `users_by_email` index store.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::{StorageError, StoredEnvironment};

/// Name of the ZQL store holding environment records (keyed by environment id).
const ENVIRONMENTS_STORE: &str = "environments";

/// Name of the ZQL store holding the `(project_id, name) -> environment-id`
/// secondary index. Keys are `{project_id_or_empty}:{name}` strings; values are
/// the owning environment id.
const ENVIRONMENTS_BY_NAME_PROJECT_STORE: &str = "environments_by_name_project";

/// Build the secondary-index key for a given `(project_id, name)` pair.
/// `project_id = None` is encoded as an empty string, which is fine because
/// environment names are non-empty in practice.
fn index_key(project_id: Option<&str>, name: &str) -> String {
    format!("{}:{}", project_id.unwrap_or(""), name)
}

/// Trait for environment storage backends.
#[async_trait]
pub trait EnvironmentStorage: Send + Sync {
    /// Store (create or update) an environment by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the
    /// write, including when the `(name, project_id)` uniqueness constraint
    /// would be violated by a different id.
    async fn store(&self, env: &StoredEnvironment) -> Result<(), StorageError>;

    /// Fetch an environment by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails or the record cannot
    /// be deserialized.
    async fn get(&self, id: &str) -> Result<Option<StoredEnvironment>, StorageError>;

    /// Fetch an environment by `(name, project_id)`. Matching is exact
    /// (case-sensitive). `project_id = None` matches the global namespace.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails or the record cannot
    /// be deserialized.
    async fn get_by_name(
        &self,
        name: &str,
        project_id: Option<&str>,
    ) -> Result<Option<StoredEnvironment>, StorageError>;

    /// List environments, optionally filtered by `project_id`. When
    /// `project_id` is `Some`, only environments belonging to that project are
    /// returned. When `project_id` is `None`, only global environments
    /// (`project_id IS NULL`) are returned. Results are sorted by name
    /// ascending.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails or any record cannot
    /// be deserialized.
    async fn list(&self, project_id: Option<&str>) -> Result<Vec<StoredEnvironment>, StorageError>;

    /// Delete an environment by id. Returns `true` if the row existed.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the delete operation fails.
    async fn delete(&self, id: &str) -> Result<bool, StorageError>;

    /// Total environment count (across all projects, including global).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the count query fails.
    async fn count(&self) -> Result<u64, StorageError>;
}

/// ZQL-based persistent storage for environments.
///
/// Primary records are keyed by environment id in the `environments` store.
/// A parallel `environments_by_name_project` store maps
/// `{project_id_or_empty}:{name}` -> environment id so that the
/// `(name, project_id)` uniqueness invariant can be enforced and `get_by_name`
/// stays cheap.
pub struct ZqlEnvironmentStore {
    db: tokio::sync::Mutex<zql::Database>,
}

impl ZqlEnvironmentStore {
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
        let path = temp_dir.path().join("environments_zql");

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
impl EnvironmentStorage for ZqlEnvironmentStore {
    async fn store(&self, env: &StoredEnvironment) -> Result<(), StorageError> {
        let mut db = self.db.lock().await;

        let new_key = index_key(env.project_id.as_deref(), &env.name);

        // 1) Enforce UNIQUE(name, project_id): reject if a different id already
        //    owns the same `(project_id, name)` key.
        if let Some(existing_id) = db
            .get_typed::<String>(ENVIRONMENTS_BY_NAME_PROJECT_STORE, &new_key)
            .map_err(StorageError::from)?
        {
            if existing_id != env.id {
                return Err(StorageError::Database(format!(
                    "UNIQUE constraint failed: environments.(name, project_id) \
                     (name '{}' already in use in this project by environment {})",
                    env.name, existing_id
                )));
            }
        }

        // 2) If this is an update and the `(project_id, name)` changed, tear
        //    down the old index entry before writing the new one.
        if let Some(previous) = db
            .get_typed::<StoredEnvironment>(ENVIRONMENTS_STORE, &env.id)
            .map_err(StorageError::from)?
        {
            let previous_key = index_key(previous.project_id.as_deref(), &previous.name);
            if previous_key != new_key {
                db.delete_typed(ENVIRONMENTS_BY_NAME_PROJECT_STORE, &previous_key)
                    .map_err(StorageError::from)?;
            }
        }

        // 3) Write the primary record and refresh the secondary index.
        db.put_typed(ENVIRONMENTS_STORE, &env.id, env)
            .map_err(StorageError::from)?;
        db.put_typed(ENVIRONMENTS_BY_NAME_PROJECT_STORE, &new_key, &env.id)
            .map_err(StorageError::from)?;

        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredEnvironment>, StorageError> {
        let mut db = self.db.lock().await;
        db.get_typed(ENVIRONMENTS_STORE, id)
            .map_err(StorageError::from)
    }

    async fn get_by_name(
        &self,
        name: &str,
        project_id: Option<&str>,
    ) -> Result<Option<StoredEnvironment>, StorageError> {
        let key = index_key(project_id, name);
        let mut db = self.db.lock().await;

        let Some(env_id) = db
            .get_typed::<String>(ENVIRONMENTS_BY_NAME_PROJECT_STORE, &key)
            .map_err(StorageError::from)?
        else {
            return Ok(None);
        };

        db.get_typed(ENVIRONMENTS_STORE, &env_id)
            .map_err(StorageError::from)
    }

    async fn list(&self, project_id: Option<&str>) -> Result<Vec<StoredEnvironment>, StorageError> {
        let mut db = self.db.lock().await;
        let all: Vec<(String, StoredEnvironment)> = db
            .scan_typed(ENVIRONMENTS_STORE, "")
            .map_err(StorageError::from)?;

        let mut envs: Vec<StoredEnvironment> = all
            .into_iter()
            .map(|(_, e)| e)
            .filter(|e| e.project_id.as_deref() == project_id)
            .collect();
        envs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(envs)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut db = self.db.lock().await;

        // Look up the env so we can clean up the secondary index.
        let Some(existing) = db
            .get_typed::<StoredEnvironment>(ENVIRONMENTS_STORE, id)
            .map_err(StorageError::from)?
        else {
            return Ok(false);
        };

        let key = index_key(existing.project_id.as_deref(), &existing.name);
        db.delete_typed(ENVIRONMENTS_BY_NAME_PROJECT_STORE, &key)
            .map_err(StorageError::from)?;
        db.delete_typed(ENVIRONMENTS_STORE, id)
            .map_err(StorageError::from)
    }

    async fn count(&self) -> Result<u64, StorageError> {
        let mut db = self.db.lock().await;
        let all: Vec<(String, StoredEnvironment)> = db
            .scan_typed(ENVIRONMENTS_STORE, "")
            .map_err(StorageError::from)?;
        Ok(all.len() as u64)
    }
}

/// In-memory environment store for tests.
pub struct InMemoryEnvironmentStore {
    envs: Arc<RwLock<HashMap<String, StoredEnvironment>>>,
}

impl InMemoryEnvironmentStore {
    /// Create a new empty in-memory environment store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            envs: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryEnvironmentStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EnvironmentStorage for InMemoryEnvironmentStore {
    async fn store(&self, env: &StoredEnvironment) -> Result<(), StorageError> {
        let mut envs = self.envs.write().await;

        if let Some(conflict) = envs.values().find(|existing| {
            existing.id != env.id
                && existing.name == env.name
                && existing.project_id == env.project_id
        }) {
            return Err(StorageError::Database(format!(
                "UNIQUE constraint failed: environments.(name, project_id) \
                 (name '{}' already in use in this project by environment {})",
                env.name, conflict.id
            )));
        }

        envs.insert(env.id.clone(), env.clone());
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredEnvironment>, StorageError> {
        let envs = self.envs.read().await;
        Ok(envs.get(id).cloned())
    }

    async fn get_by_name(
        &self,
        name: &str,
        project_id: Option<&str>,
    ) -> Result<Option<StoredEnvironment>, StorageError> {
        let envs = self.envs.read().await;
        Ok(envs
            .values()
            .find(|env| env.name == name && env.project_id.as_deref() == project_id)
            .cloned())
    }

    async fn list(&self, project_id: Option<&str>) -> Result<Vec<StoredEnvironment>, StorageError> {
        let envs = self.envs.read().await;
        let mut list: Vec<_> = envs
            .values()
            .filter(|env| env.project_id.as_deref() == project_id)
            .cloned()
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut envs = self.envs.write().await;
        Ok(envs.remove(id).is_some())
    }

    async fn count(&self) -> Result<u64, StorageError> {
        let envs = self.envs.read().await;
        Ok(envs.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_env(name: &str, project_id: Option<&str>) -> StoredEnvironment {
        StoredEnvironment::new(name, project_id.map(str::to_string))
    }

    // =========================================================================
    // InMemoryEnvironmentStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_inmemory_store_and_get() {
        let store = InMemoryEnvironmentStore::new();
        let env = make_env("dev", None);
        let id = env.id.clone();

        store.store(&env).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("env must exist");
        assert_eq!(retrieved.name, "dev");
        assert!(retrieved.project_id.is_none());
    }

    #[tokio::test]
    async fn test_inmemory_get_by_name_global() {
        let store = InMemoryEnvironmentStore::new();
        store.store(&make_env("dev", None)).await.unwrap();

        let found = store.get_by_name("dev", None).await.unwrap();
        assert!(found.is_some());
        assert!(found.unwrap().project_id.is_none());

        // A project-scoped environment with the same name should not match
        // a global lookup.
        store.store(&make_env("dev", Some("proj-1"))).await.unwrap();
        let still_global = store.get_by_name("dev", None).await.unwrap();
        assert!(still_global.unwrap().project_id.is_none());
    }

    #[tokio::test]
    async fn test_inmemory_get_by_name_with_project() {
        let store = InMemoryEnvironmentStore::new();
        store
            .store(&make_env("staging", Some("proj-a")))
            .await
            .unwrap();
        store
            .store(&make_env("staging", Some("proj-b")))
            .await
            .unwrap();

        let a = store
            .get_by_name("staging", Some("proj-a"))
            .await
            .unwrap()
            .expect("env in proj-a must exist");
        assert_eq!(a.project_id.as_deref(), Some("proj-a"));

        let b = store
            .get_by_name("staging", Some("proj-b"))
            .await
            .unwrap()
            .expect("env in proj-b must exist");
        assert_eq!(b.project_id.as_deref(), Some("proj-b"));
        assert_ne!(a.id, b.id);
    }

    #[tokio::test]
    async fn test_inmemory_get_by_name_nonexistent() {
        let store = InMemoryEnvironmentStore::new();
        let missing = store.get_by_name("nope", None).await.unwrap();
        assert!(missing.is_none());

        let missing_in_project = store.get_by_name("nope", Some("p")).await.unwrap();
        assert!(missing_in_project.is_none());
    }

    #[tokio::test]
    async fn test_inmemory_list_filtered_by_project() {
        let store = InMemoryEnvironmentStore::new();
        store.store(&make_env("dev", None)).await.unwrap();
        store.store(&make_env("prod", None)).await.unwrap();
        store.store(&make_env("dev", Some("p1"))).await.unwrap();
        store.store(&make_env("staging", Some("p1"))).await.unwrap();
        store.store(&make_env("dev", Some("p2"))).await.unwrap();

        let globals = store.list(None).await.unwrap();
        assert_eq!(globals.len(), 2);
        assert_eq!(globals[0].name, "dev");
        assert_eq!(globals[1].name, "prod");

        let p1 = store.list(Some("p1")).await.unwrap();
        assert_eq!(p1.len(), 2);
        assert_eq!(p1[0].name, "dev");
        assert_eq!(p1[1].name, "staging");

        let p2 = store.list(Some("p2")).await.unwrap();
        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0].name, "dev");
    }

    #[tokio::test]
    async fn test_inmemory_delete() {
        let store = InMemoryEnvironmentStore::new();
        let env = make_env("doomed", None);
        let id = env.id.clone();
        store.store(&env).await.unwrap();

        let deleted = store.delete(&id).await.unwrap();
        assert!(deleted);
        assert!(store.get(&id).await.unwrap().is_none());

        let deleted_again = store.delete(&id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_inmemory_count() {
        let store = InMemoryEnvironmentStore::new();
        assert_eq!(store.count().await.unwrap(), 0);

        store.store(&make_env("a", None)).await.unwrap();
        store.store(&make_env("b", Some("p"))).await.unwrap();
        let third = make_env("c", None);
        let third_id = third.id.clone();
        store.store(&third).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 3);

        store.delete(&third_id).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_inmemory_unique_name_within_project_rejects_different_id() {
        let store = InMemoryEnvironmentStore::new();
        let first = make_env("dev", Some("proj"));
        store.store(&first).await.unwrap();

        let second = make_env("dev", Some("proj"));
        let err = store.store(&second).await.expect_err("should fail");
        match err {
            StorageError::Database(_) => {}
            other => panic!("expected Database error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_inmemory_unique_name_global_rejects_different_id() {
        let store = InMemoryEnvironmentStore::new();
        let first = make_env("dev", None);
        store.store(&first).await.unwrap();

        let second = make_env("dev", None);
        let err = store.store(&second).await.expect_err("should fail");
        match err {
            StorageError::Database(_) => {}
            other => panic!("expected Database error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_inmemory_same_name_in_different_projects_ok() {
        let store = InMemoryEnvironmentStore::new();
        store.store(&make_env("dev", None)).await.unwrap();
        store.store(&make_env("dev", Some("p1"))).await.unwrap();
        store.store(&make_env("dev", Some("p2"))).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn test_inmemory_update_advances_updated_at() {
        let store = InMemoryEnvironmentStore::new();
        let mut env = make_env("dev", None);
        let id = env.id.clone();
        let original_updated = env.updated_at;
        store.store(&env).await.unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        env.description = Some("updated description".to_string());
        env.updated_at = chrono::Utc::now();
        store.store(&env).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(
            retrieved.description.as_deref(),
            Some("updated description")
        );
        assert!(retrieved.updated_at > original_updated);
    }

    // =========================================================================
    // ZqlEnvironmentStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_zql_store_and_get() {
        let store = ZqlEnvironmentStore::in_memory().await.unwrap();
        let env = make_env("dev", None);
        let id = env.id.clone();

        store.store(&env).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("env must exist");
        assert_eq!(retrieved.name, "dev");
        assert!(retrieved.project_id.is_none());
    }

    #[tokio::test]
    async fn test_zql_get_by_name_global_and_project_distinguished() {
        let store = ZqlEnvironmentStore::in_memory().await.unwrap();
        store.store(&make_env("dev", None)).await.unwrap();
        store.store(&make_env("dev", Some("p1"))).await.unwrap();

        let global = store
            .get_by_name("dev", None)
            .await
            .unwrap()
            .expect("global dev must exist");
        assert!(global.project_id.is_none());

        let scoped = store
            .get_by_name("dev", Some("p1"))
            .await
            .unwrap()
            .expect("p1 dev must exist");
        assert_eq!(scoped.project_id.as_deref(), Some("p1"));
        assert_ne!(global.id, scoped.id);
    }

    #[tokio::test]
    async fn test_zql_get_by_name_nonexistent() {
        let store = ZqlEnvironmentStore::in_memory().await.unwrap();
        assert!(store.get_by_name("nope", None).await.unwrap().is_none());
        assert!(store
            .get_by_name("nope", Some("p"))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_zql_list_filtered_by_project() {
        let store = ZqlEnvironmentStore::in_memory().await.unwrap();
        store.store(&make_env("dev", None)).await.unwrap();
        store.store(&make_env("prod", None)).await.unwrap();
        store.store(&make_env("dev", Some("p1"))).await.unwrap();
        store.store(&make_env("staging", Some("p1"))).await.unwrap();

        let globals = store.list(None).await.unwrap();
        assert_eq!(globals.len(), 2);
        assert_eq!(globals[0].name, "dev");
        assert_eq!(globals[1].name, "prod");

        let p1 = store.list(Some("p1")).await.unwrap();
        assert_eq!(p1.len(), 2);
        assert_eq!(p1[0].name, "dev");
        assert_eq!(p1[1].name, "staging");
    }

    #[tokio::test]
    async fn test_zql_delete() {
        let store = ZqlEnvironmentStore::in_memory().await.unwrap();
        let env = make_env("doomed", None);
        let id = env.id.clone();
        store.store(&env).await.unwrap();

        let deleted = store.delete(&id).await.unwrap();
        assert!(deleted);
        assert!(store.get(&id).await.unwrap().is_none());

        let deleted_again = store.delete(&id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_zql_count() {
        let store = ZqlEnvironmentStore::in_memory().await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);

        store.store(&make_env("a", None)).await.unwrap();
        store.store(&make_env("b", Some("p"))).await.unwrap();
        let third = make_env("c", None);
        let third_id = third.id.clone();
        store.store(&third).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 3);

        store.delete(&third_id).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_zql_unique_name_within_project_rejects_different_id() {
        let store = ZqlEnvironmentStore::in_memory().await.unwrap();
        let first = make_env("dev", Some("proj"));
        store.store(&first).await.unwrap();

        let second = make_env("dev", Some("proj"));
        let err = store.store(&second).await.expect_err("should fail");
        match err {
            StorageError::Database(_) => {}
            other => panic!("expected Database error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_zql_unique_name_global_rejects_different_id() {
        let store = ZqlEnvironmentStore::in_memory().await.unwrap();
        let first = make_env("dev", None);
        store.store(&first).await.unwrap();

        let second = make_env("dev", None);
        let err = store.store(&second).await.expect_err("should fail");
        match err {
            StorageError::Database(_) => {}
            other => panic!("expected Database error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_zql_same_name_in_different_projects_ok() {
        let store = ZqlEnvironmentStore::in_memory().await.unwrap();
        store.store(&make_env("dev", None)).await.unwrap();
        store.store(&make_env("dev", Some("p1"))).await.unwrap();
        store.store(&make_env("dev", Some("p2"))).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn test_zql_update_by_id_advances_updated_at() {
        let store = ZqlEnvironmentStore::in_memory().await.unwrap();
        let mut env = make_env("dev", None);
        let id = env.id.clone();
        let original_updated = env.updated_at;
        store.store(&env).await.unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        env.description = Some("updated".to_string());
        env.updated_at = chrono::Utc::now();
        store.store(&env).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.description.as_deref(), Some("updated"));
        assert!(retrieved.updated_at > original_updated);
    }

    #[tokio::test]
    async fn test_zql_rename_clears_old_index_entry() {
        let store = ZqlEnvironmentStore::in_memory().await.unwrap();
        let mut env = make_env("dev", None);
        let id = env.id.clone();
        store.store(&env).await.unwrap();

        // Rename the environment in place — the old `(None, "dev")` index slot
        // must be reclaimable by a different environment.
        env.name = "renamed".to_string();
        env.updated_at = chrono::Utc::now();
        store.store(&env).await.unwrap();

        // Another environment can now claim the old "dev" name.
        let new_dev = make_env("dev", None);
        store
            .store(&new_dev)
            .await
            .expect("old name must be free after rename");

        let by_new = store
            .get_by_name("renamed", None)
            .await
            .unwrap()
            .expect("renamed lookup must work");
        assert_eq!(by_new.id, id);

        let by_old = store
            .get_by_name("dev", None)
            .await
            .unwrap()
            .expect("new dev must exist");
        assert_eq!(by_old.id, new_dev.id);
    }

    #[tokio::test]
    async fn test_zql_persistent_storage() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("environments_zql_db");

        let env = make_env("persist", Some("proj"));
        let id = env.id.clone();

        // Create and populate database
        {
            let store = ZqlEnvironmentStore::open(&db_path).await.unwrap();
            store.store(&env).await.unwrap();
        }

        // Reopen and verify data persists
        {
            let store = ZqlEnvironmentStore::open(&db_path).await.unwrap();
            let retrieved = store.get(&id).await.unwrap().expect("env must persist");
            assert_eq!(retrieved.name, "persist");
            assert_eq!(retrieved.project_id.as_deref(), Some("proj"));

            let by_name = store
                .get_by_name("persist", Some("proj"))
                .await
                .unwrap()
                .expect("name lookup must work after reopen");
            assert_eq!(by_name.id, id);
        }
    }
}
