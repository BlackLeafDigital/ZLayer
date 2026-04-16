//! Task storage implementations
//!
//! Provides both persistent (`SQLite` via [`SqlxJsonStore`]) and in-memory
//! storage backends for named runnable scripts (tasks) and their execution
//! records (task runs). Tasks can be global (`project_id = None`) or
//! project-scoped.
//!
//! # Schema
//!
//! The persistent backend owns two tables sharing a single `SqlitePool`:
//!
//! - `tasks` — managed by the generic [`SqlxJsonStore`] adapter with a unique
//!   `name` peeled column and a nullable `project_id` secondary index.
//! - `task_runs` — a secondary table holding run history, keyed by `id` with
//!   `task_id` + `started_at DESC` indexed for fast per-task listing.
//!
//! The two tables are kept consistent by a cascading `delete` that wraps both
//! `DELETE` statements in a single `sqlx` transaction, so a crash between the
//! child and parent delete cannot leave orphaned runs behind.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::sqlite::SqlitePool;
use tokio::sync::RwLock;

use super::sqlx_json::{IndexSpec, JsonTable, SqlxJsonStore};
use super::{StorageError, StoredTask, TaskRun};

/// Trait for task storage backends.
#[async_trait]
pub trait TaskStorage: Send + Sync {
    /// Store (create or update) a task by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the
    /// write.
    async fn store(&self, task: &StoredTask) -> Result<(), StorageError>;

    /// Fetch a task by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn get(&self, id: &str) -> Result<Option<StoredTask>, StorageError>;

    /// List all tasks, optionally filtered by project id. When `project_id`
    /// is `Some`, only tasks belonging to that project are returned. When
    /// `project_id` is `None`, all tasks are returned (both global and
    /// project-scoped). Results are sorted by name ascending.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn list(&self, project_id: Option<&str>) -> Result<Vec<StoredTask>, StorageError>;

    /// Delete a task by id. Returns `true` if the row existed.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the delete operation fails.
    async fn delete(&self, id: &str) -> Result<bool, StorageError>;

    /// Record a completed task run.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the
    /// write.
    async fn record_run(&self, run: &TaskRun) -> Result<(), StorageError>;

    /// List runs for a given task, ordered by `started_at` descending
    /// (most recent first).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn list_runs(&self, task_id: &str) -> Result<Vec<TaskRun>, StorageError>;
}

/// In-memory task store.
pub struct InMemoryTaskStore {
    tasks: Arc<RwLock<HashMap<String, StoredTask>>>,
    runs: Arc<RwLock<Vec<TaskRun>>>,
}

impl InMemoryTaskStore {
    /// Create a new empty in-memory task store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            runs: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for InMemoryTaskStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TaskStorage for InMemoryTaskStore {
    async fn store(&self, task: &StoredTask) -> Result<(), StorageError> {
        let mut tasks = self.tasks.write().await;
        tasks.insert(task.id.clone(), task.clone());
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredTask>, StorageError> {
        let tasks = self.tasks.read().await;
        Ok(tasks.get(id).cloned())
    }

    async fn list(&self, project_id: Option<&str>) -> Result<Vec<StoredTask>, StorageError> {
        let tasks = self.tasks.read().await;
        let mut list: Vec<_> = tasks
            .values()
            .filter(|t| match project_id {
                Some(pid) => t.project_id.as_deref() == Some(pid),
                None => true,
            })
            .cloned()
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut tasks = self.tasks.write().await;
        Ok(tasks.remove(id).is_some())
    }

    async fn record_run(&self, run: &TaskRun) -> Result<(), StorageError> {
        let mut runs = self.runs.write().await;
        runs.push(run.clone());
        Ok(())
    }

    async fn list_runs(&self, task_id: &str) -> Result<Vec<TaskRun>, StorageError> {
        let runs = self.runs.read().await;
        let mut filtered: Vec<_> = runs
            .iter()
            .filter(|r| r.task_id == task_id)
            .cloned()
            .collect();
        filtered.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(filtered)
    }
}

/// Static table specification for the `tasks` table.
///
/// Two peeled columns:
///
/// - `name` — unique, single-column. Matches the trait's documented behaviour
///   that task names are globally unique, and lets conflicts surface as
///   [`StorageError::AlreadyExists`] directly from the adapter.
/// - `project_id` — non-unique, nullable. Enables fast
///   `list(Some(project_id))` via `SqlxJsonStore::list_where`.
const TASKS_TABLE: JsonTable<StoredTask> = JsonTable {
    name: "tasks",
    indexes: &[
        IndexSpec {
            column: "name",
            extractor: |t| Some(t.name.clone()),
            unique: true,
        },
        IndexSpec {
            column: "project_id",
            extractor: |t| t.project_id.clone(),
            unique: false,
        },
    ],
    unique_constraints: &[],
};

/// `SQLite`-backed persistent task store.
///
/// Delegates task CRUD to a [`SqlxJsonStore<StoredTask>`] configured for the
/// `tasks` table and owns a secondary `task_runs` table on the same pool for
/// run history. Deleting a task cascades to its runs inside a single
/// transaction so partial failures cannot leave orphaned runs behind.
pub struct SqlxTaskStore {
    inner: SqlxJsonStore<StoredTask>,
}

impl SqlxTaskStore {
    /// Open or create a `SQLite` database at the given path and ensure both
    /// the `tasks` and `task_runs` tables exist.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database connection, primary table
    /// creation, or `task_runs` secondary schema cannot be established.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let inner = SqlxJsonStore::<StoredTask>::open(path, TASKS_TABLE).await?;
        Self::init_task_runs_schema(inner.pool()).await?;
        Ok(Self { inner })
    }

    /// Create an in-memory `SQLite` database (useful for tests) with both
    /// the `tasks` and `task_runs` tables in place.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the in-memory database cannot be created.
    pub async fn in_memory() -> Result<Self, StorageError> {
        let inner = SqlxJsonStore::<StoredTask>::in_memory(TASKS_TABLE).await?;
        Self::init_task_runs_schema(inner.pool()).await?;
        Ok(Self { inner })
    }

    /// Create the `task_runs` secondary table and its `(task_id,
    /// started_at DESC)` index if they don't already exist. Idempotent.
    async fn init_task_runs_schema(pool: &SqlitePool) -> Result<(), StorageError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS task_runs (\n    \
                 id TEXT PRIMARY KEY NOT NULL,\n    \
                 task_id TEXT NOT NULL,\n    \
                 data_json TEXT NOT NULL,\n    \
                 started_at TEXT NOT NULL\n\
             )",
        )
        .execute(pool)
        .await?;

        // Descending index on started_at: `list_runs` returns newest-first,
        // and SQLite can serve an ORDER BY DESC directly from this index.
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_task_runs_task_id \
             ON task_runs(task_id, started_at DESC)",
        )
        .execute(pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl TaskStorage for SqlxTaskStore {
    async fn store(&self, task: &StoredTask) -> Result<(), StorageError> {
        self.inner.put(&task.id, task).await
    }

    async fn get(&self, id: &str) -> Result<Option<StoredTask>, StorageError> {
        self.inner.get(id).await
    }

    async fn list(&self, project_id: Option<&str>) -> Result<Vec<StoredTask>, StorageError> {
        // The adapter orders by `id`; the trait contract sorts by name.
        let mut list = match project_id {
            Some(pid) => self.inner.list_where("project_id", pid).await?,
            None => self.inner.list().await?,
        };
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        // Cascade delete: runs first, then the task itself. Wrapping both
        // statements in a transaction means either both succeed or neither
        // does — the `task_runs` table can never be left referencing a
        // deleted task.
        let pool = self.inner.pool();
        let mut tx = pool.begin().await?;

        sqlx::query("DELETE FROM task_runs WHERE task_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;

        let result = sqlx::query("DELETE FROM tasks WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(result.rows_affected() > 0)
    }

    async fn record_run(&self, run: &TaskRun) -> Result<(), StorageError> {
        let data_json = serde_json::to_string(run)?;
        // `TaskRun::started_at` is a `DateTime<Utc>` (see storage/mod.rs).
        // Store the RFC-3339 form so the column is text-sortable for the
        // `ORDER BY started_at DESC` in `list_runs`.
        let started_at = run.started_at.to_rfc3339();

        sqlx::query(
            "INSERT INTO task_runs (id, task_id, data_json, started_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
                task_id = excluded.task_id, \
                data_json = excluded.data_json, \
                started_at = excluded.started_at",
        )
        .bind(&run.id)
        .bind(&run.task_id)
        .bind(&data_json)
        .bind(&started_at)
        .execute(self.inner.pool())
        .await?;

        Ok(())
    }

    async fn list_runs(&self, task_id: &str) -> Result<Vec<TaskRun>, StorageError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT data_json FROM task_runs WHERE task_id = ? ORDER BY started_at DESC",
        )
        .bind(task_id)
        .fetch_all(self.inner.pool())
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (data_json,) in rows {
            out.push(serde_json::from_str::<TaskRun>(&data_json)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{StoredTask, TaskKind, TaskRun};
    use chrono::Utc;

    fn make_task(name: &str, body: &str, project_id: Option<&str>) -> StoredTask {
        StoredTask::new(name, TaskKind::Bash, body, project_id.map(str::to_string))
    }

    fn make_run(task_id: &str, exit_code: i32) -> TaskRun {
        let now = Utc::now();
        TaskRun {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: task_id.to_string(),
            exit_code: Some(exit_code),
            stdout: "hello".to_string(),
            stderr: String::new(),
            started_at: now,
            finished_at: Some(now),
        }
    }

    #[tokio::test]
    async fn test_store_and_get() {
        let store = InMemoryTaskStore::new();
        let task = make_task("build", "cargo build", None);
        let id = task.id.clone();

        store.store(&task).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("task must exist");
        assert_eq!(retrieved.name, "build");
        assert_eq!(retrieved.body, "cargo build");
        assert!(retrieved.project_id.is_none());
    }

    #[tokio::test]
    async fn test_list_all() {
        let store = InMemoryTaskStore::new();
        store
            .store(&make_task("beta", "echo beta", None))
            .await
            .unwrap();
        store
            .store(&make_task("alpha", "echo alpha", None))
            .await
            .unwrap();
        store
            .store(&make_task("gamma", "echo gamma", Some("proj-1")))
            .await
            .unwrap();

        let all = store.list(None).await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].name, "alpha");
        assert_eq!(all[1].name, "beta");
        assert_eq!(all[2].name, "gamma");
    }

    #[tokio::test]
    async fn test_list_filtered_by_project() {
        let store = InMemoryTaskStore::new();
        store
            .store(&make_task("global", "echo global", None))
            .await
            .unwrap();
        store
            .store(&make_task("proj-task", "echo proj", Some("proj-1")))
            .await
            .unwrap();

        let filtered = store.list(Some("proj-1")).await.unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "proj-task");
    }

    #[tokio::test]
    async fn test_delete() {
        let store = InMemoryTaskStore::new();
        let task = make_task("doomed", "rm -rf /", None);
        let id = task.id.clone();
        store.store(&task).await.unwrap();

        assert!(store.delete(&id).await.unwrap());
        assert!(store.get(&id).await.unwrap().is_none());
        assert!(!store.delete(&id).await.unwrap());
    }

    #[tokio::test]
    async fn test_record_and_list_runs() {
        let store = InMemoryTaskStore::new();
        let task = make_task("test", "cargo test", None);
        let task_id = task.id.clone();
        store.store(&task).await.unwrap();

        let first_run = make_run(&task_id, 0);
        store.record_run(&first_run).await.unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut second_run = make_run(&task_id, 1);
        second_run.started_at = Utc::now();
        store.record_run(&second_run).await.unwrap();

        let recorded = store.list_runs(&task_id).await.unwrap();
        assert_eq!(recorded.len(), 2);
        // Most recent first
        assert!(recorded[0].started_at >= recorded[1].started_at);
    }

    #[tokio::test]
    async fn test_list_runs_empty() {
        let store = InMemoryTaskStore::new();
        let runs = store.list_runs("nonexistent").await.unwrap();
        assert!(runs.is_empty());
    }

    // =========================================================================
    // SqlxTaskStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_sqlx_store_and_get() {
        let store = SqlxTaskStore::in_memory().await.unwrap();
        let task = make_task("build", "cargo build", None);
        let id = task.id.clone();

        store.store(&task).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("task must exist");
        assert_eq!(retrieved.name, "build");
        assert_eq!(retrieved.body, "cargo build");
        assert_eq!(retrieved.kind, TaskKind::Bash);
        assert!(retrieved.project_id.is_none());

        // Missing id returns None, not error.
        assert!(store.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_sqlx_list_all_and_by_project() {
        let store = SqlxTaskStore::in_memory().await.unwrap();
        store
            .store(&make_task("beta", "echo beta", None))
            .await
            .unwrap();
        store
            .store(&make_task("alpha", "echo alpha", None))
            .await
            .unwrap();
        store
            .store(&make_task("gamma", "echo gamma", Some("proj-1")))
            .await
            .unwrap();
        store
            .store(&make_task("delta", "echo delta", Some("proj-2")))
            .await
            .unwrap();

        // No filter: all four tasks, sorted by name ASC.
        let all = store.list(None).await.unwrap();
        assert_eq!(all.len(), 4);
        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta", "delta", "gamma"]);

        // Filter by project.
        let proj1 = store.list(Some("proj-1")).await.unwrap();
        assert_eq!(proj1.len(), 1);
        assert_eq!(proj1[0].name, "gamma");

        let proj2 = store.list(Some("proj-2")).await.unwrap();
        assert_eq!(proj2.len(), 1);
        assert_eq!(proj2[0].name, "delta");

        let missing = store.list(Some("nope")).await.unwrap();
        assert!(missing.is_empty());
    }

    #[tokio::test]
    async fn test_sqlx_duplicate_name() {
        let store = SqlxTaskStore::in_memory().await.unwrap();
        store
            .store(&make_task("dup", "echo 1", None))
            .await
            .unwrap();

        // A different task (different id) with the same name must fail with
        // AlreadyExists from the schema-level UNIQUE(name) constraint.
        let err = store
            .store(&make_task("dup", "echo 2", None))
            .await
            .expect_err("duplicate task name must fail");
        match err {
            StorageError::AlreadyExists(msg) => {
                assert!(
                    msg.contains("tasks"),
                    "message should mention the table name, got: {msg}"
                );
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sqlx_record_run_and_list_runs() {
        let store = SqlxTaskStore::in_memory().await.unwrap();
        let task = make_task("runnable", "echo hi", None);
        let task_id = task.id.clone();
        store.store(&task).await.unwrap();

        // Three runs at explicitly-distinct timestamps so `list_runs`
        // has an unambiguous DESC ordering to assert against.
        let base = Utc::now();
        let mut run1 = make_run(&task_id, 0);
        run1.started_at = base;
        let mut run2 = make_run(&task_id, 0);
        run2.started_at = base + chrono::Duration::seconds(1);
        let mut run3 = make_run(&task_id, 0);
        run3.started_at = base + chrono::Duration::seconds(2);

        store.record_run(&run1).await.unwrap();
        store.record_run(&run2).await.unwrap();
        store.record_run(&run3).await.unwrap();

        let listed = store.list_runs(&task_id).await.unwrap();
        assert_eq!(listed.len(), 3);
        // Newest first: run3, run2, run1.
        assert_eq!(listed[0].id, run3.id);
        assert_eq!(listed[1].id, run2.id);
        assert_eq!(listed[2].id, run1.id);
        assert!(listed[0].started_at >= listed[1].started_at);
        assert!(listed[1].started_at >= listed[2].started_at);

        // Listing runs for a task with no runs returns empty, not error.
        let empty = store.list_runs("unknown-task").await.unwrap();
        assert!(empty.is_empty());

        // Upsert: re-recording a run with the same id must overwrite, not
        // duplicate.
        let mut run1_updated = run1.clone();
        run1_updated.stdout = "re-run".to_string();
        store.record_run(&run1_updated).await.unwrap();
        let after_upsert = store.list_runs(&task_id).await.unwrap();
        assert_eq!(after_upsert.len(), 3, "upsert must not create a duplicate");
        let found = after_upsert.iter().find(|r| r.id == run1.id).unwrap();
        assert_eq!(found.stdout, "re-run");
    }

    #[tokio::test]
    async fn test_sqlx_delete_cascades_runs() {
        let store = SqlxTaskStore::in_memory().await.unwrap();
        let task = make_task("doomed", "echo bye", None);
        let task_id = task.id.clone();
        store.store(&task).await.unwrap();

        // Seed a couple of runs so the cascade has something to remove.
        store.record_run(&make_run(&task_id, 0)).await.unwrap();
        store.record_run(&make_run(&task_id, 1)).await.unwrap();
        assert_eq!(store.list_runs(&task_id).await.unwrap().len(), 2);

        // Delete the task; the transaction in `SqlxTaskStore::delete`
        // should also wipe the runs.
        assert!(store.delete(&task_id).await.unwrap());
        assert!(store.get(&task_id).await.unwrap().is_none());
        assert!(
            store.list_runs(&task_id).await.unwrap().is_empty(),
            "task_runs must be cascaded on task delete"
        );

        // Deleting an already-gone task returns Ok(false), not an error,
        // and must not disturb other data.
        assert!(!store.delete(&task_id).await.unwrap());
        assert!(!store.delete("never-existed").await.unwrap());
    }

    #[tokio::test]
    async fn test_sqlx_persistent_round_trip() {
        // Survives closing + reopening the database — both the task blob
        // and its run history must come back intact.
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("tasks.db");

        let task = make_task("persist-me", "echo persist", Some("proj-x"));
        let task_id = task.id.clone();
        let run_a = {
            let mut r = make_run(&task_id, 0);
            r.started_at = Utc::now();
            r
        };
        let run_b = {
            let mut r = make_run(&task_id, 1);
            r.started_at = Utc::now() + chrono::Duration::seconds(1);
            r
        };

        {
            let store = SqlxTaskStore::open(&db_path).await.unwrap();
            store.store(&task).await.unwrap();
            store.record_run(&run_a).await.unwrap();
            store.record_run(&run_b).await.unwrap();
        }

        {
            let store = SqlxTaskStore::open(&db_path).await.unwrap();
            let retrieved = store.get(&task_id).await.unwrap().expect("must persist");
            assert_eq!(retrieved.name, "persist-me");
            assert_eq!(retrieved.body, "echo persist");
            assert_eq!(retrieved.project_id.as_deref(), Some("proj-x"));

            // Runs preserved with correct DESC ordering.
            let runs = store.list_runs(&task_id).await.unwrap();
            assert_eq!(runs.len(), 2);
            assert_eq!(runs[0].id, run_b.id);
            assert_eq!(runs[1].id, run_a.id);

            // list(None) still finds the task after reopen.
            let all = store.list(None).await.unwrap();
            assert_eq!(all.len(), 1);
            assert_eq!(all[0].id, task_id);
        }
    }
}
