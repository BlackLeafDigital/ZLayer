//! Project storage implementations
//!
//! Provides both persistent (`SQLite` via sqlx) and in-memory storage backends
//! for projects. A project bundles a git source, build configuration, registry
//! credential reference, linked deployments, and a default environment.
//!
//! Uniqueness rule: `name` is globally unique. The `SQLite` backend enforces
//! this with a `UNIQUE` constraint on the `name` column.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tokio::sync::RwLock;

use super::{StorageError, StoredProject};

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

/// SQLite-based persistent storage for projects using sqlx.
pub struct SqlxProjectStore {
    pool: SqlitePool,
}

impl SqlxProjectStore {
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
    /// Returns an error if the in-memory database creation or table creation
    /// fails.
    pub async fn in_memory() -> Result<Self, StorageError> {
        let pool = SqlitePool::connect(":memory:").await?;
        Self::init_schema(&pool).await?;
        Ok(Self { pool })
    }

    /// Create the `projects` and `project_deployments` tables and supporting
    /// indexes if they do not already exist.
    async fn init_schema(pool: &SqlitePool) -> Result<(), StorageError> {
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL UNIQUE,
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            ",
        )
        .execute(pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name)")
            .execute(pool)
            .await?;

        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS project_deployments (
                project_id TEXT NOT NULL,
                deployment_name TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (project_id, deployment_name)
            )
            ",
        )
        .execute(pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl ProjectStorage for SqlxProjectStore {
    async fn store(&self, project: &StoredProject) -> Result<(), StorageError> {
        // Preflight: enforce name uniqueness across different ids.
        if let Some(existing) = self.get_by_name(&project.name).await? {
            if existing.id != project.id {
                return Err(StorageError::Database(format!(
                    "UNIQUE constraint failed: projects.name \
                     (name '{}' already in use by project {})",
                    project.name, existing.id
                )));
            }
        }

        let data_json = serde_json::to_string(project)?;
        let created_at = project.created_at.to_rfc3339();
        let updated_at = project.updated_at.to_rfc3339();

        sqlx::query(
            r"
            INSERT INTO projects (id, name, data_json, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                data_json = excluded.data_json,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at
            ",
        )
        .bind(&project.id)
        .bind(&project.name)
        .bind(&data_json)
        .bind(&created_at)
        .bind(&updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredProject>, StorageError> {
        let row: Option<(String,)> = sqlx::query_as("SELECT data_json FROM projects WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some((data_json,)) => {
                let project: StoredProject = serde_json::from_str(&data_json)?;
                Ok(Some(project))
            }
            None => Ok(None),
        }
    }

    async fn get_by_name(&self, name: &str) -> Result<Option<StoredProject>, StorageError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT data_json FROM projects WHERE name = ?")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;

        match row {
            Some((data_json,)) => {
                let project: StoredProject = serde_json::from_str(&data_json)?;
                Ok(Some(project))
            }
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<StoredProject>, StorageError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT data_json FROM projects ORDER BY name ASC")
                .fetch_all(&self.pool)
                .await?;

        let mut projects = Vec::with_capacity(rows.len());
        for (data_json,) in rows {
            let project: StoredProject = serde_json::from_str(&data_json)?;
            projects.push(project);
        }

        Ok(projects)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        // Also clean up any deployment links for this project.
        sqlx::query("DELETE FROM project_deployments WHERE project_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        let result = sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn count(&self) -> Result<u64, StorageError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM projects")
            .fetch_one(&self.pool)
            .await?;

        Ok(u64::try_from(row.0).unwrap_or(0))
    }

    async fn link_deployment(
        &self,
        project_id: &str,
        deployment_name: &str,
    ) -> Result<(), StorageError> {
        let created_at = chrono::Utc::now().to_rfc3339();

        sqlx::query(
            r"
            INSERT INTO project_deployments (project_id, deployment_name, created_at)
            VALUES (?, ?, ?)
            ON CONFLICT(project_id, deployment_name) DO NOTHING
            ",
        )
        .bind(project_id)
        .bind(deployment_name)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn unlink_deployment(
        &self,
        project_id: &str,
        deployment_name: &str,
    ) -> Result<bool, StorageError> {
        let result = sqlx::query(
            "DELETE FROM project_deployments WHERE project_id = ? AND deployment_name = ?",
        )
        .bind(project_id)
        .bind(deployment_name)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn list_deployments(&self, project_id: &str) -> Result<Vec<String>, StorageError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT deployment_name FROM project_deployments WHERE project_id = ? ORDER BY deployment_name ASC",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(name,)| name).collect())
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
    // SqlxProjectStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_sqlx_store_and_get() {
        let store = SqlxProjectStore::in_memory().await.unwrap();
        let project = make_project("my-app");
        let id = project.id.clone();

        store.store(&project).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("project must exist");
        assert_eq!(retrieved.name, "my-app");
        assert_eq!(retrieved.git_branch.as_deref(), Some("main"));
    }

    #[tokio::test]
    async fn test_sqlx_get_by_name() {
        let store = SqlxProjectStore::in_memory().await.unwrap();
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
    async fn test_sqlx_list_sorted_by_name() {
        let store = SqlxProjectStore::in_memory().await.unwrap();
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
    async fn test_sqlx_delete() {
        let store = SqlxProjectStore::in_memory().await.unwrap();
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
    async fn test_sqlx_count() {
        let store = SqlxProjectStore::in_memory().await.unwrap();
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
    async fn test_sqlx_unique_name_rejects_different_id() {
        let store = SqlxProjectStore::in_memory().await.unwrap();
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
    async fn test_sqlx_update_by_id_advances_updated_at() {
        let store = SqlxProjectStore::in_memory().await.unwrap();
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
    async fn test_sqlx_link_and_list_deployments() {
        let store = SqlxProjectStore::in_memory().await.unwrap();
        let project = make_project("my-app");
        let pid = project.id.clone();
        store.store(&project).await.unwrap();

        store.link_deployment(&pid, "deploy-b").await.unwrap();
        store.link_deployment(&pid, "deploy-a").await.unwrap();

        let deps = store.list_deployments(&pid).await.unwrap();
        assert_eq!(deps, vec!["deploy-a", "deploy-b"]);
    }

    #[tokio::test]
    async fn test_sqlx_link_deployment_idempotent() {
        let store = SqlxProjectStore::in_memory().await.unwrap();
        let project = make_project("my-app");
        let pid = project.id.clone();
        store.store(&project).await.unwrap();

        store.link_deployment(&pid, "deploy-a").await.unwrap();
        store.link_deployment(&pid, "deploy-a").await.unwrap();

        let deps = store.list_deployments(&pid).await.unwrap();
        assert_eq!(deps.len(), 1);
    }

    #[tokio::test]
    async fn test_sqlx_unlink_deployment() {
        let store = SqlxProjectStore::in_memory().await.unwrap();
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
    async fn test_sqlx_list_deployments_empty() {
        let store = SqlxProjectStore::in_memory().await.unwrap();
        let deps = store.list_deployments("nonexistent-project").await.unwrap();
        assert!(deps.is_empty());
    }

    #[tokio::test]
    async fn test_sqlx_delete_cascades_deployments() {
        let store = SqlxProjectStore::in_memory().await.unwrap();
        let project = make_project("my-app");
        let pid = project.id.clone();
        store.store(&project).await.unwrap();

        store.link_deployment(&pid, "deploy-a").await.unwrap();
        store.delete(&pid).await.unwrap();

        let deps = store.list_deployments(&pid).await.unwrap();
        assert!(deps.is_empty());
    }

    #[tokio::test]
    async fn test_sqlx_persistent_storage() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("projects.db");

        let project = make_project("persist");
        let id = project.id.clone();

        // Create and populate database
        {
            let store = SqlxProjectStore::open(&db_path).await.unwrap();
            store.store(&project).await.unwrap();
            store.link_deployment(&id, "my-deploy").await.unwrap();
        }

        // Reopen and verify data persists
        {
            let store = SqlxProjectStore::open(&db_path).await.unwrap();
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
