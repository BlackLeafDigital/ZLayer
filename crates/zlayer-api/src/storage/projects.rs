//! Project storage implementations
//!
//! Provides both persistent (ZQL) and in-memory storage backends for projects.
//! A project bundles a git source, build configuration, registry credential
//! reference, linked deployments, and a default environment.
//!
//! Uniqueness rule: `name` is globally unique. The persistent backend enforces
//! this with a parallel ZQL store keyed by name that records the owning project
//! id, mirroring how `users.rs` enforces email uniqueness with a
//! `users_by_email` index store.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::{StorageError, StoredProject};

/// Name of the ZQL store holding project records (keyed by project id).
const PROJECTS_STORE: &str = "projects";

/// Name of the ZQL store holding the name -> project-id secondary index.
/// Keys are project names; values are the owning project id.
const PROJECTS_BY_NAME_STORE: &str = "projects_by_name";

/// Name of the ZQL store holding project-deployment links.
/// Keys are `{project_id}:{deployment_name}` strings; values are the
/// deployment name (for easy scanning by prefix).
const PROJECT_DEPLOYMENTS_STORE: &str = "project_deployments";

/// Build the deployment-link key for a given `(project_id, deployment_name)`.
fn deployment_key(project_id: &str, deployment_name: &str) -> String {
    format!("{project_id}:{deployment_name}")
}

/// Trait for project storage backends.
#[async_trait]
pub trait ProjectStorage: Send + Sync {
    /// Store (create or update) a project by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the
    /// write, including when the name uniqueness constraint would be violated
    /// by a different id.
    async fn store(&self, project: &StoredProject) -> Result<(), StorageError>;

    /// Fetch a project by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails or the record cannot
    /// be deserialized.
    async fn get(&self, id: &str) -> Result<Option<StoredProject>, StorageError>;

    /// Fetch a project by name. Matching is exact (case-sensitive).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails or the record cannot
    /// be deserialized.
    async fn get_by_name(&self, name: &str) -> Result<Option<StoredProject>, StorageError>;

    /// List all projects sorted by name ascending.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails or any record cannot
    /// be deserialized.
    async fn list(&self) -> Result<Vec<StoredProject>, StorageError>;

    /// Delete a project by id. Returns `true` if the row existed.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the delete operation fails.
    async fn delete(&self, id: &str) -> Result<bool, StorageError>;

    /// Total project count.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the count query fails.
    async fn count(&self) -> Result<u64, StorageError>;

    /// Link a deployment to a project.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the write fails.
    async fn link_deployment(
        &self,
        project_id: &str,
        deployment_name: &str,
    ) -> Result<(), StorageError>;

    /// Unlink a deployment from a project. Returns `true` if the link existed.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the delete fails.
    async fn unlink_deployment(
        &self,
        project_id: &str,
        deployment_name: &str,
    ) -> Result<bool, StorageError>;

    /// List all deployment names linked to a project, sorted ascending.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the query fails.
    async fn list_deployments(&self, project_id: &str) -> Result<Vec<String>, StorageError>;
}

/// ZQL-based persistent storage for projects.
///
/// Primary records are keyed by project id in the `projects` store. A parallel
/// `projects_by_name` store maps name -> project id so that the name-uniqueness
/// invariant can be enforced and `get_by_name` stays cheap. Deployment links
/// live in a `project_deployments` store keyed by `{project_id}:{deployment_name}`.
pub struct ZqlProjectStore {
    db: tokio::sync::Mutex<zql::Database>,
}

impl ZqlProjectStore {
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
        let path = temp_dir.path().join("projects_zql");

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
impl ProjectStorage for ZqlProjectStore {
    async fn store(&self, project: &StoredProject) -> Result<(), StorageError> {
        let mut db = self.db.lock().await;

        // 1) Enforce UNIQUE name: reject if a different id already owns it.
        if let Some(existing_id) = db
            .get_typed::<String>(PROJECTS_BY_NAME_STORE, &project.name)
            .map_err(StorageError::from)?
        {
            if existing_id != project.id {
                return Err(StorageError::Database(format!(
                    "UNIQUE constraint failed: projects.name \
                     (name '{}' already in use by project {})",
                    project.name, existing_id
                )));
            }
        }

        // 2) If this is an update and the name changed, tear down the old
        //    name -> id mapping before writing the new one.
        if let Some(previous) = db
            .get_typed::<StoredProject>(PROJECTS_STORE, &project.id)
            .map_err(StorageError::from)?
        {
            if previous.name != project.name {
                db.delete_typed(PROJECTS_BY_NAME_STORE, &previous.name)
                    .map_err(StorageError::from)?;
            }
        }

        // 3) Write the primary record and refresh the secondary index.
        db.put_typed(PROJECTS_STORE, &project.id, project)
            .map_err(StorageError::from)?;
        db.put_typed(PROJECTS_BY_NAME_STORE, &project.name, &project.id)
            .map_err(StorageError::from)?;

        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredProject>, StorageError> {
        let mut db = self.db.lock().await;
        db.get_typed(PROJECTS_STORE, id).map_err(StorageError::from)
    }

    async fn get_by_name(&self, name: &str) -> Result<Option<StoredProject>, StorageError> {
        let mut db = self.db.lock().await;

        let Some(project_id) = db
            .get_typed::<String>(PROJECTS_BY_NAME_STORE, name)
            .map_err(StorageError::from)?
        else {
            return Ok(None);
        };

        db.get_typed(PROJECTS_STORE, &project_id)
            .map_err(StorageError::from)
    }

    async fn list(&self) -> Result<Vec<StoredProject>, StorageError> {
        let mut db = self.db.lock().await;
        let all: Vec<(String, StoredProject)> = db
            .scan_typed(PROJECTS_STORE, "")
            .map_err(StorageError::from)?;

        let mut projects: Vec<StoredProject> = all.into_iter().map(|(_, p)| p).collect();
        projects.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(projects)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut db = self.db.lock().await;

        // Look up the project so we can clean up the secondary index.
        let Some(existing) = db
            .get_typed::<StoredProject>(PROJECTS_STORE, id)
            .map_err(StorageError::from)?
        else {
            return Ok(false);
        };

        // Clean up deployment links by scanning the prefix.
        let prefix = format!("{id}:");
        let links: Vec<(String, String)> = db
            .scan_typed(PROJECT_DEPLOYMENTS_STORE, &prefix)
            .map_err(StorageError::from)?;
        for (key, _) in links {
            db.delete_typed(PROJECT_DEPLOYMENTS_STORE, &key)
                .map_err(StorageError::from)?;
        }

        db.delete_typed(PROJECTS_BY_NAME_STORE, &existing.name)
            .map_err(StorageError::from)?;
        db.delete_typed(PROJECTS_STORE, id)
            .map_err(StorageError::from)
    }

    async fn count(&self) -> Result<u64, StorageError> {
        let mut db = self.db.lock().await;
        let all: Vec<(String, StoredProject)> = db
            .scan_typed(PROJECTS_STORE, "")
            .map_err(StorageError::from)?;
        Ok(all.len() as u64)
    }

    async fn link_deployment(
        &self,
        project_id: &str,
        deployment_name: &str,
    ) -> Result<(), StorageError> {
        let mut db = self.db.lock().await;
        let key = deployment_key(project_id, deployment_name);
        db.put_typed(
            PROJECT_DEPLOYMENTS_STORE,
            &key,
            &deployment_name.to_string(),
        )
        .map_err(StorageError::from)?;
        Ok(())
    }

    async fn unlink_deployment(
        &self,
        project_id: &str,
        deployment_name: &str,
    ) -> Result<bool, StorageError> {
        let mut db = self.db.lock().await;
        let key = deployment_key(project_id, deployment_name);

        // Check if it exists first.
        let existed = db
            .get_typed::<String>(PROJECT_DEPLOYMENTS_STORE, &key)
            .map_err(StorageError::from)?
            .is_some();

        if existed {
            db.delete_typed(PROJECT_DEPLOYMENTS_STORE, &key)
                .map_err(StorageError::from)?;
        }

        Ok(existed)
    }

    async fn list_deployments(&self, project_id: &str) -> Result<Vec<String>, StorageError> {
        let mut db = self.db.lock().await;
        let prefix = format!("{project_id}:");
        let links: Vec<(String, String)> = db
            .scan_typed(PROJECT_DEPLOYMENTS_STORE, &prefix)
            .map_err(StorageError::from)?;

        let mut names: Vec<String> = links.into_iter().map(|(_, name)| name).collect();
        names.sort();
        Ok(names)
    }
}

/// In-memory project store for tests.
pub struct InMemoryProjectStore {
    projects: Arc<RwLock<HashMap<String, StoredProject>>>,
    /// Maps `project_id` -> set of deployment names.
    deployments: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl InMemoryProjectStore {
    /// Create a new empty in-memory project store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            projects: Arc::new(RwLock::new(HashMap::new())),
            deployments: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryProjectStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProjectStorage for InMemoryProjectStore {
    async fn store(&self, project: &StoredProject) -> Result<(), StorageError> {
        let mut projects = self.projects.write().await;

        // Enforce UNIQUE name constraint: reject if another id already owns
        // this name.
        if let Some(conflict) = projects
            .values()
            .find(|existing| existing.id != project.id && existing.name == project.name)
        {
            return Err(StorageError::Database(format!(
                "UNIQUE constraint failed: projects.name \
                 (name '{}' already in use by project {})",
                project.name, conflict.id
            )));
        }

        projects.insert(project.id.clone(), project.clone());
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredProject>, StorageError> {
        let projects = self.projects.read().await;
        Ok(projects.get(id).cloned())
    }

    async fn get_by_name(&self, name: &str) -> Result<Option<StoredProject>, StorageError> {
        let projects = self.projects.read().await;
        Ok(projects.values().find(|p| p.name == name).cloned())
    }

    async fn list(&self) -> Result<Vec<StoredProject>, StorageError> {
        let projects = self.projects.read().await;
        let mut list: Vec<_> = projects.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut projects = self.projects.write().await;
        let existed = projects.remove(id).is_some();

        if existed {
            let mut deployments = self.deployments.write().await;
            deployments.remove(id);
        }

        Ok(existed)
    }

    async fn count(&self) -> Result<u64, StorageError> {
        let projects = self.projects.read().await;
        Ok(projects.len() as u64)
    }

    async fn link_deployment(
        &self,
        project_id: &str,
        deployment_name: &str,
    ) -> Result<(), StorageError> {
        let mut deployments = self.deployments.write().await;
        let entry = deployments
            .entry(project_id.to_string())
            .or_insert_with(Vec::new);

        if !entry.contains(&deployment_name.to_string()) {
            entry.push(deployment_name.to_string());
        }

        Ok(())
    }

    async fn unlink_deployment(
        &self,
        project_id: &str,
        deployment_name: &str,
    ) -> Result<bool, StorageError> {
        let mut deployments = self.deployments.write().await;
        let Some(entry) = deployments.get_mut(project_id) else {
            return Ok(false);
        };

        let before = entry.len();
        entry.retain(|n| n != deployment_name);
        Ok(entry.len() < before)
    }

    async fn list_deployments(&self, project_id: &str) -> Result<Vec<String>, StorageError> {
        let deployments = self.deployments.read().await;
        let mut names = deployments.get(project_id).cloned().unwrap_or_default();
        names.sort();
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::BuildKind;

    fn make_project(name: &str) -> StoredProject {
        StoredProject::new(name)
    }

    // =========================================================================
    // InMemoryProjectStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_inmemory_store_and_get() {
        let store = InMemoryProjectStore::new();
        let project = make_project("my-app");
        let id = project.id.clone();

        store.store(&project).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("project must exist");
        assert_eq!(retrieved.name, "my-app");
        assert!(retrieved.description.is_none());
        assert_eq!(retrieved.git_branch.as_deref(), Some("main"));
    }

    #[tokio::test]
    async fn test_inmemory_get_by_name() {
        let store = InMemoryProjectStore::new();
        let project = make_project("my-app");
        store.store(&project).await.unwrap();

        let found = store
            .get_by_name("my-app")
            .await
            .unwrap()
            .expect("project must exist");
        assert_eq!(found.id, project.id);

        let missing = store.get_by_name("no-such-project").await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_inmemory_list_sorted_by_name() {
        let store = InMemoryProjectStore::new();
        store.store(&make_project("charlie")).await.unwrap();
        store.store(&make_project("alpha")).await.unwrap();
        store.store(&make_project("bravo")).await.unwrap();

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "alpha");
        assert_eq!(list[1].name, "bravo");
        assert_eq!(list[2].name, "charlie");
    }

    #[tokio::test]
    async fn test_inmemory_delete() {
        let store = InMemoryProjectStore::new();
        let project = make_project("doomed");
        let id = project.id.clone();
        store.store(&project).await.unwrap();

        let deleted = store.delete(&id).await.unwrap();
        assert!(deleted);
        assert!(store.get(&id).await.unwrap().is_none());

        let deleted_again = store.delete(&id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_inmemory_count() {
        let store = InMemoryProjectStore::new();
        assert_eq!(store.count().await.unwrap(), 0);

        store.store(&make_project("a")).await.unwrap();
        store.store(&make_project("b")).await.unwrap();
        let third = make_project("c");
        let third_id = third.id.clone();
        store.store(&third).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 3);

        store.delete(&third_id).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_inmemory_unique_name_rejects_different_id() {
        let store = InMemoryProjectStore::new();
        let first = make_project("dup");
        store.store(&first).await.unwrap();

        let second = make_project("dup");
        let err = store.store(&second).await.expect_err("should fail");
        match err {
            StorageError::Database(_) => {}
            other => panic!("expected Database error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_inmemory_update_advances_updated_at() {
        let store = InMemoryProjectStore::new();
        let mut project = make_project("my-app");
        let id = project.id.clone();
        let original_updated = project.updated_at;
        store.store(&project).await.unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        project.description = Some("updated description".to_string());
        project.updated_at = chrono::Utc::now();
        store.store(&project).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(
            retrieved.description.as_deref(),
            Some("updated description")
        );
        assert!(retrieved.updated_at > original_updated);
    }

    #[tokio::test]
    async fn test_inmemory_link_and_list_deployments() {
        let store = InMemoryProjectStore::new();
        let project = make_project("my-app");
        let pid = project.id.clone();
        store.store(&project).await.unwrap();

        store.link_deployment(&pid, "deploy-b").await.unwrap();
        store.link_deployment(&pid, "deploy-a").await.unwrap();

        let deps = store.list_deployments(&pid).await.unwrap();
        assert_eq!(deps, vec!["deploy-a", "deploy-b"]);
    }

    #[tokio::test]
    async fn test_inmemory_link_deployment_idempotent() {
        let store = InMemoryProjectStore::new();
        let project = make_project("my-app");
        let pid = project.id.clone();
        store.store(&project).await.unwrap();

        store.link_deployment(&pid, "deploy-a").await.unwrap();
        store.link_deployment(&pid, "deploy-a").await.unwrap();

        let deps = store.list_deployments(&pid).await.unwrap();
        assert_eq!(deps.len(), 1);
    }

    #[tokio::test]
    async fn test_inmemory_unlink_deployment() {
        let store = InMemoryProjectStore::new();
        let project = make_project("my-app");
        let pid = project.id.clone();
        store.store(&project).await.unwrap();

        store.link_deployment(&pid, "deploy-a").await.unwrap();
        store.link_deployment(&pid, "deploy-b").await.unwrap();

        let removed = store.unlink_deployment(&pid, "deploy-a").await.unwrap();
        assert!(removed);

        let removed_again = store.unlink_deployment(&pid, "deploy-a").await.unwrap();
        assert!(!removed_again);

        let deps = store.list_deployments(&pid).await.unwrap();
        assert_eq!(deps, vec!["deploy-b"]);
    }

    #[tokio::test]
    async fn test_inmemory_list_deployments_empty() {
        let store = InMemoryProjectStore::new();
        let deps = store.list_deployments("nonexistent-project").await.unwrap();
        assert!(deps.is_empty());
    }

    #[tokio::test]
    async fn test_inmemory_delete_cascades_deployments() {
        let store = InMemoryProjectStore::new();
        let project = make_project("my-app");
        let pid = project.id.clone();
        store.store(&project).await.unwrap();

        store.link_deployment(&pid, "deploy-a").await.unwrap();
        store.delete(&pid).await.unwrap();

        let deps = store.list_deployments(&pid).await.unwrap();
        assert!(deps.is_empty());
    }

    #[tokio::test]
    async fn test_inmemory_build_kind_roundtrip() {
        let store = InMemoryProjectStore::new();
        let mut project = make_project("builder");
        project.build_kind = Some(BuildKind::Dockerfile);
        project.build_path = Some("./Dockerfile".to_string());
        store.store(&project).await.unwrap();

        let retrieved = store.get(&project.id).await.unwrap().unwrap();
        assert_eq!(retrieved.build_kind, Some(BuildKind::Dockerfile));
        assert_eq!(retrieved.build_path.as_deref(), Some("./Dockerfile"));
    }

    #[tokio::test]
    async fn test_inmemory_all_build_kinds() {
        let kinds = [
            BuildKind::Dockerfile,
            BuildKind::Compose,
            BuildKind::ZImagefile,
            BuildKind::Spec,
        ];
        for kind in &kinds {
            let json = serde_json::to_string(kind).unwrap();
            let roundtrip: BuildKind = serde_json::from_str(&json).unwrap();
            assert_eq!(*kind, roundtrip);
        }
    }

    // =========================================================================
    // ZqlProjectStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_zql_store_and_get() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        let project = make_project("my-app");
        let id = project.id.clone();

        store.store(&project).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("project must exist");
        assert_eq!(retrieved.name, "my-app");
        assert_eq!(retrieved.git_branch.as_deref(), Some("main"));
    }

    #[tokio::test]
    async fn test_zql_get_by_name() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        let project = make_project("my-app");
        store.store(&project).await.unwrap();

        let found = store
            .get_by_name("my-app")
            .await
            .unwrap()
            .expect("project must exist");
        assert_eq!(found.id, project.id);

        let missing = store.get_by_name("no-such-project").await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_zql_list_sorted_by_name() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        store.store(&make_project("charlie")).await.unwrap();
        store.store(&make_project("alpha")).await.unwrap();
        store.store(&make_project("bravo")).await.unwrap();

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "alpha");
        assert_eq!(list[1].name, "bravo");
        assert_eq!(list[2].name, "charlie");
    }

    #[tokio::test]
    async fn test_zql_delete() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        let project = make_project("doomed");
        let id = project.id.clone();
        store.store(&project).await.unwrap();

        let deleted = store.delete(&id).await.unwrap();
        assert!(deleted);
        assert!(store.get(&id).await.unwrap().is_none());

        let deleted_again = store.delete(&id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_zql_count() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);

        store.store(&make_project("a")).await.unwrap();
        store.store(&make_project("b")).await.unwrap();
        let third = make_project("c");
        let third_id = third.id.clone();
        store.store(&third).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 3);

        store.delete(&third_id).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_zql_unique_name_rejects_different_id() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        let first = make_project("dup");
        store.store(&first).await.unwrap();

        let second = make_project("dup");
        let err = store.store(&second).await.expect_err("should fail");
        match err {
            StorageError::Database(_) => {}
            other => panic!("expected Database error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_zql_update_by_id_advances_updated_at() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        let mut project = make_project("my-app");
        let id = project.id.clone();
        let original_updated = project.updated_at;
        store.store(&project).await.unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        project.description = Some("updated".to_string());
        project.updated_at = chrono::Utc::now();
        store.store(&project).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.description.as_deref(), Some("updated"));
        assert!(retrieved.updated_at > original_updated);
    }

    #[tokio::test]
    async fn test_zql_rename_clears_old_index_entry() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        let mut project = make_project("old-name");
        let id = project.id.clone();
        store.store(&project).await.unwrap();

        // Rename the project in place — the old name index slot must be
        // reclaimable by a different project.
        project.name = "new-name".to_string();
        project.updated_at = chrono::Utc::now();
        store.store(&project).await.unwrap();

        // Another project can now claim the old name.
        let new_proj = make_project("old-name");
        store
            .store(&new_proj)
            .await
            .expect("old name must be free after rename");

        let by_new = store
            .get_by_name("new-name")
            .await
            .unwrap()
            .expect("renamed lookup must work");
        assert_eq!(by_new.id, id);

        let by_old = store
            .get_by_name("old-name")
            .await
            .unwrap()
            .expect("new project with old name must exist");
        assert_eq!(by_old.id, new_proj.id);
    }

    #[tokio::test]
    async fn test_zql_link_and_list_deployments() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        let project = make_project("my-app");
        let pid = project.id.clone();
        store.store(&project).await.unwrap();

        store.link_deployment(&pid, "deploy-b").await.unwrap();
        store.link_deployment(&pid, "deploy-a").await.unwrap();

        let deps = store.list_deployments(&pid).await.unwrap();
        assert_eq!(deps, vec!["deploy-a", "deploy-b"]);
    }

    #[tokio::test]
    async fn test_zql_link_deployment_idempotent() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        let project = make_project("my-app");
        let pid = project.id.clone();
        store.store(&project).await.unwrap();

        store.link_deployment(&pid, "deploy-a").await.unwrap();
        store.link_deployment(&pid, "deploy-a").await.unwrap();

        let deps = store.list_deployments(&pid).await.unwrap();
        assert_eq!(deps.len(), 1);
    }

    #[tokio::test]
    async fn test_zql_unlink_deployment() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        let project = make_project("my-app");
        let pid = project.id.clone();
        store.store(&project).await.unwrap();

        store.link_deployment(&pid, "deploy-a").await.unwrap();
        store.link_deployment(&pid, "deploy-b").await.unwrap();

        let removed = store.unlink_deployment(&pid, "deploy-a").await.unwrap();
        assert!(removed);

        let removed_again = store.unlink_deployment(&pid, "deploy-a").await.unwrap();
        assert!(!removed_again);

        let deps = store.list_deployments(&pid).await.unwrap();
        assert_eq!(deps, vec!["deploy-b"]);
    }

    #[tokio::test]
    async fn test_zql_list_deployments_empty() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        let deps = store.list_deployments("nonexistent-project").await.unwrap();
        assert!(deps.is_empty());
    }

    #[tokio::test]
    async fn test_zql_delete_cascades_deployments() {
        let store = ZqlProjectStore::in_memory().await.unwrap();
        let project = make_project("my-app");
        let pid = project.id.clone();
        store.store(&project).await.unwrap();

        store.link_deployment(&pid, "deploy-a").await.unwrap();
        store.delete(&pid).await.unwrap();

        let deps = store.list_deployments(&pid).await.unwrap();
        assert!(deps.is_empty());
    }

    #[tokio::test]
    async fn test_zql_persistent_storage() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("projects_zql_db");

        let project = make_project("persist");
        let id = project.id.clone();

        // Create and populate database
        {
            let store = ZqlProjectStore::open(&db_path).await.unwrap();
            store.store(&project).await.unwrap();
            store.link_deployment(&id, "my-deploy").await.unwrap();
        }

        // Reopen and verify data persists
        {
            let store = ZqlProjectStore::open(&db_path).await.unwrap();
            let retrieved = store.get(&id).await.unwrap().expect("project must persist");
            assert_eq!(retrieved.name, "persist");

            let by_name = store
                .get_by_name("persist")
                .await
                .unwrap()
                .expect("name lookup must work after reopen");
            assert_eq!(by_name.id, id);

            let deps = store.list_deployments(&id).await.unwrap();
            assert_eq!(deps, vec!["my-deploy"]);
        }
    }
}
