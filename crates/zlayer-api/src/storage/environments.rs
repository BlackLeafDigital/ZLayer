//! Environment storage implementations
//!
//! Provides both persistent (`SQLite` via sqlx) and in-memory storage backends
//! for deployment/runtime environments. An environment is an isolated namespace
//! for secrets and (eventually) deployments. An environment optionally belongs
//! to a project — when `project_id` is `None`, the environment is global.
//!
//! Uniqueness rule: `(name, project_id)` is unique. `SQLite`'s SQL-standard
//! UNIQUE constraint treats `NULL` values as distinct, so two global
//! environments with the same name would slip past the table-level constraint.
//! We therefore enforce uniqueness in code via a `get_by_name` preflight inside
//! `store()`, mirroring how `users.rs` enforces email uniqueness.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tokio::sync::RwLock;

use super::{StorageError, StoredEnvironment};

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

/// SQLite-based persistent storage for environments using sqlx.
pub struct SqlxEnvironmentStore {
    pool: SqlitePool,
}

impl SqlxEnvironmentStore {
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

    /// Create the `environments` table and supporting indexes if they do not
    /// already exist.
    async fn init_schema(pool: &SqlitePool) -> Result<(), StorageError> {
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS environments (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                project_id TEXT,
                data_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(name, project_id)
            )
            ",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_environments_project_id ON environments(project_id)",
        )
        .execute(pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_environments_name ON environments(name)")
            .execute(pool)
            .await?;

        Ok(())
    }
}

#[async_trait]
impl EnvironmentStorage for SqlxEnvironmentStore {
    async fn store(&self, env: &StoredEnvironment) -> Result<(), StorageError> {
        // Preflight: SQLite treats NULL as distinct in UNIQUE constraints, so
        // we must enforce `(name, project_id)` uniqueness in code for the
        // global (project_id IS NULL) namespace as well as for any project
        // namespace. Mirrors the email-uniqueness preflight in `users.rs`.
        if let Some(existing) = self
            .get_by_name(&env.name, env.project_id.as_deref())
            .await?
        {
            if existing.id != env.id {
                return Err(StorageError::Database(format!(
                    "UNIQUE constraint failed: environments.(name, project_id) \
                     (name '{}' already in use in this project by environment {})",
                    env.name, existing.id
                )));
            }
        }

        let data_json = serde_json::to_string(env)?;
        let created_at = env.created_at.to_rfc3339();
        let updated_at = env.updated_at.to_rfc3339();

        sqlx::query(
            r"
            INSERT INTO environments (id, name, project_id, data_json, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                project_id = excluded.project_id,
                data_json = excluded.data_json,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at
            ",
        )
        .bind(&env.id)
        .bind(&env.name)
        .bind(env.project_id.as_ref())
        .bind(&data_json)
        .bind(&created_at)
        .bind(&updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredEnvironment>, StorageError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT data_json FROM environments WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;

        match row {
            Some((data_json,)) => {
                let env: StoredEnvironment = serde_json::from_str(&data_json)?;
                Ok(Some(env))
            }
            None => Ok(None),
        }
    }

    async fn get_by_name(
        &self,
        name: &str,
        project_id: Option<&str>,
    ) -> Result<Option<StoredEnvironment>, StorageError> {
        let row: Option<(String,)> = match project_id {
            Some(pid) => {
                sqlx::query_as(
                    "SELECT data_json FROM environments WHERE name = ? AND project_id = ?",
                )
                .bind(name)
                .bind(pid)
                .fetch_optional(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as(
                    "SELECT data_json FROM environments WHERE name = ? AND project_id IS NULL",
                )
                .bind(name)
                .fetch_optional(&self.pool)
                .await?
            }
        };

        match row {
            Some((data_json,)) => {
                let env: StoredEnvironment = serde_json::from_str(&data_json)?;
                Ok(Some(env))
            }
            None => Ok(None),
        }
    }

    async fn list(&self, project_id: Option<&str>) -> Result<Vec<StoredEnvironment>, StorageError> {
        let rows: Vec<(String,)> =
            match project_id {
                Some(pid) => {
                    sqlx::query_as(
                        "SELECT data_json FROM environments WHERE project_id = ? ORDER BY name ASC",
                    )
                    .bind(pid)
                    .fetch_all(&self.pool)
                    .await?
                }
                None => sqlx::query_as(
                    "SELECT data_json FROM environments WHERE project_id IS NULL ORDER BY name ASC",
                )
                .fetch_all(&self.pool)
                .await?,
            };

        let mut envs = Vec::with_capacity(rows.len());
        for (data_json,) in rows {
            let env: StoredEnvironment = serde_json::from_str(&data_json)?;
            envs.push(env);
        }

        Ok(envs)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let result = sqlx::query("DELETE FROM environments WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn count(&self) -> Result<u64, StorageError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM environments")
            .fetch_one(&self.pool)
            .await?;

        Ok(u64::try_from(row.0).unwrap_or(0))
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

        // Enforce UNIQUE(name, project_id) — including for the global
        // (project_id == None) namespace, which SQL would not enforce.
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
    // SqlxEnvironmentStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_sqlx_store_and_get() {
        let store = SqlxEnvironmentStore::in_memory().await.unwrap();
        let env = make_env("dev", None);
        let id = env.id.clone();

        store.store(&env).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("env must exist");
        assert_eq!(retrieved.name, "dev");
        assert!(retrieved.project_id.is_none());
    }

    #[tokio::test]
    async fn test_sqlx_get_by_name_global_and_project_distinguished() {
        let store = SqlxEnvironmentStore::in_memory().await.unwrap();
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
    async fn test_sqlx_get_by_name_nonexistent() {
        let store = SqlxEnvironmentStore::in_memory().await.unwrap();
        assert!(store.get_by_name("nope", None).await.unwrap().is_none());
        assert!(store
            .get_by_name("nope", Some("p"))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_sqlx_list_filtered_by_project() {
        let store = SqlxEnvironmentStore::in_memory().await.unwrap();
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
    async fn test_sqlx_delete() {
        let store = SqlxEnvironmentStore::in_memory().await.unwrap();
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
    async fn test_sqlx_count() {
        let store = SqlxEnvironmentStore::in_memory().await.unwrap();
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
    async fn test_sqlx_unique_name_within_project_rejects_different_id() {
        let store = SqlxEnvironmentStore::in_memory().await.unwrap();
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
    async fn test_sqlx_unique_name_global_rejects_different_id() {
        let store = SqlxEnvironmentStore::in_memory().await.unwrap();
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
    async fn test_sqlx_same_name_in_different_projects_ok() {
        let store = SqlxEnvironmentStore::in_memory().await.unwrap();
        store.store(&make_env("dev", None)).await.unwrap();
        store.store(&make_env("dev", Some("p1"))).await.unwrap();
        store.store(&make_env("dev", Some("p2"))).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn test_sqlx_update_by_id_advances_updated_at() {
        let store = SqlxEnvironmentStore::in_memory().await.unwrap();
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
    async fn test_sqlx_persistent_storage() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("environments.db");

        let env = make_env("persist", Some("proj"));
        let id = env.id.clone();

        // Create and populate database
        {
            let store = SqlxEnvironmentStore::open(&db_path).await.unwrap();
            store.store(&env).await.unwrap();
        }

        // Reopen and verify data persists
        {
            let store = SqlxEnvironmentStore::open(&db_path).await.unwrap();
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
