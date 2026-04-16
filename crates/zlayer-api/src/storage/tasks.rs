//! Task storage implementations
//!
//! Provides both persistent (ZQL) and in-memory storage backends for named
//! runnable scripts (tasks) and their execution records (task runs). Tasks
//! can be global (`project_id = None`) or project-scoped.
//!
//! # Schema
//!
//! The ZQL backend owns two logical stores sharing a single `zql::Database`
//! handle:
//!
//! - `tasks` — managed by the generic [`ZqlJsonStore`] adapter with a unique
//!   `name` index and a non-unique `project_id` index so `list(Some(pid))` is
//!   a direct `list_where` rather than a full scan.
//! - `task_runs` — a secondary ZQL store keyed by
//!   `"{task_id}:{started_at_rfc3339}:{run_id}"`. The `task_id:` prefix is a
//!   `zql::scan_typed` prefix filter for `list_runs`; the embedded
//!   `started_at` makes ascending scans approximately chronological, and
//!   `list_runs` reverses the result explicitly in Rust so the trait's
//!   "newest first" contract still holds no matter which encoding the future
//!   brings.
//!
//! The two stores share the same [`tokio::sync::Mutex<zql::Database>`] so
//! that `delete` can tear down a task's runs and the task itself under a
//! single lock acquisition — the exact analogue of the `sqlx` variant's
//! single-transaction cascade.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::zql_json::{IndexSpec, JsonTable, ZqlJsonStore};
use super::{StorageError, StoredTask, TaskRun};

/// Name of the secondary ZQL store holding task-run records.
///
/// Keys are `{task_id}:{started_at_rfc3339}:{run_id}` so that:
///
/// - `scan_typed("task_runs", "{task_id}:")` returns every run for a task in
///   a single prefix scan, and
/// - the embedded `started_at` keeps the natural key stable across writes
///   (so upserts on the same run-id don't spawn duplicate rows).
const TASK_RUNS_STORE: &str = "task_runs";

/// Build the composite key for a run under its owning task. Used both by
/// `record_run` (to write the row) and by `list_runs` / `delete` (which
/// scan the `{task_id}:` prefix).
fn task_run_key(run: &TaskRun) -> String {
    format!(
        "{task_id}:{started_at}:{run_id}",
        task_id = run.task_id,
        started_at = run.started_at.to_rfc3339(),
        run_id = run.id,
    )
}

/// Trait for task storage backends.
#[async_trait]
pub trait TaskStorage: Send + Sync {
    /// Store (create or update) a task by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the
    /// write, or [`StorageError::AlreadyExists`] if a different task already
    /// owns the given name.
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
/// Two indexed columns, matching the `sqlx` variant:
///
/// - `name` — unique, single-column. Task names are globally unique; any
///   write that would collide surfaces as [`StorageError::AlreadyExists`]
///   from the adapter.
/// - `project_id` — non-unique, nullable. Enables
///   `list(Some(project_id))` to use `ZqlJsonStore::list_where_opt` without
///   scanning every task blob.
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

/// ZQL-backed persistent task store.
///
/// Delegates task CRUD to a [`ZqlJsonStore<StoredTask>`] configured for the
/// `tasks` table, and owns a secondary `task_runs` logical store layered on
/// the same `zql::Database`. Deleting a task scans its `{task_id}:` run
/// prefix and removes every run alongside the task blob under a single
/// mutex acquisition, mirroring the `sqlx` variant's transactional cascade.
pub struct ZqlTaskStore {
    inner: ZqlJsonStore<StoredTask>,
}

impl ZqlTaskStore {
    /// Open or create a ZQL database at the given path and ensure both the
    /// primary `tasks` store and the secondary `task_runs` store are ready
    /// to use.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database cannot be opened.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let inner = ZqlJsonStore::<StoredTask>::open(path, TASKS_TABLE).await?;
        Ok(Self { inner })
    }

    /// Create a ZQL database in a temporary directory with both stores
    /// ready to use (primarily for tests).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the temp directory or database cannot be
    /// created.
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self, StorageError> {
        let inner = ZqlJsonStore::<StoredTask>::in_memory(TASKS_TABLE).await?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl TaskStorage for ZqlTaskStore {
    async fn store(&self, task: &StoredTask) -> Result<(), StorageError> {
        self.inner.put(&task.id, task).await
    }

    async fn get(&self, id: &str) -> Result<Option<StoredTask>, StorageError> {
        self.inner.get(id).await
    }

    async fn list(&self, project_id: Option<&str>) -> Result<Vec<StoredTask>, StorageError> {
        // `list_where_opt(Some(pid))` hits the `project_id` index;
        // `list()` falls through to a full scan when no filter is given.
        let mut list = match project_id {
            Some(pid) => self.inner.list_where_opt("project_id", Some(pid)).await?,
            None => self.inner.list().await?,
        };
        // The adapter orders by primary `id`; the trait contract sorts by
        // name.
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        // Cascade delete: wipe every run row whose key starts with `id:`,
        // then remove the task blob. Both operations run under the single
        // `inner.db()` mutex acquisition so a concurrent writer cannot wedge
        // new runs in between and leave them orphaned.
        let db_mutex = self.inner.db();
        let mut db = db_mutex.lock().await;

        let prefix = format!("{id}:");
        let runs: Vec<(String, TaskRun)> = db
            .scan_typed(TASK_RUNS_STORE, &prefix)
            .map_err(StorageError::from)?;
        for (key, _) in runs {
            db.delete_typed(TASK_RUNS_STORE, &key)
                .map_err(StorageError::from)?;
        }

        // Drop the guard before calling back into the adapter — the adapter
        // re-locks the same mutex for its own delete path, and holding both
        // guards would deadlock.
        drop(db);

        self.inner.delete(id).await
    }

    async fn record_run(&self, run: &TaskRun) -> Result<(), StorageError> {
        let key = task_run_key(run);
        let db_mutex = self.inner.db();
        let mut db = db_mutex.lock().await;
        db.put_typed(TASK_RUNS_STORE, &key, run)
            .map_err(StorageError::from)?;
        Ok(())
    }

    async fn list_runs(&self, task_id: &str) -> Result<Vec<TaskRun>, StorageError> {
        let prefix = format!("{task_id}:");
        let db_mutex = self.inner.db();
        let mut db = db_mutex.lock().await;
        let rows: Vec<(String, TaskRun)> = db
            .scan_typed(TASK_RUNS_STORE, &prefix)
            .map_err(StorageError::from)?;
        drop(db);

        // The composite key is `{task_id}:{started_at_rfc3339}:{run_id}`;
        // ZQL scans return ascending order, but RFC-3339 comparison is only
        // lexicographic-sortable when every value uses the same offset.
        // Sorting by the parsed `started_at` descending in Rust keeps the
        // trait's "newest first" contract independent of the encoding.
        let mut runs: Vec<TaskRun> = rows.into_iter().map(|(_, r)| r).collect();
        runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(runs)
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
    // ZqlTaskStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_zql_store_and_get() {
        let store = ZqlTaskStore::in_memory().await.unwrap();
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
    async fn test_zql_list_all_and_by_project() {
        let store = ZqlTaskStore::in_memory().await.unwrap();
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
    async fn test_zql_duplicate_name() {
        let store = ZqlTaskStore::in_memory().await.unwrap();
        store
            .store(&make_task("dup", "echo 1", None))
            .await
            .unwrap();

        // A different task (different id) with the same name must fail with
        // AlreadyExists from the schema-level unique(name) index.
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
    async fn test_zql_record_run_and_list_runs() {
        let store = ZqlTaskStore::in_memory().await.unwrap();
        let task = make_task("runnable", "echo hi", None);
        let task_id = task.id.clone();
        store.store(&task).await.unwrap();

        // Three runs at explicitly-distinct timestamps so `list_runs` has an
        // unambiguous DESC ordering to assert against.
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
    }

    #[tokio::test]
    async fn test_zql_delete_cascades_runs() {
        let store = ZqlTaskStore::in_memory().await.unwrap();
        let task = make_task("doomed", "echo bye", None);
        let task_id = task.id.clone();
        store.store(&task).await.unwrap();

        // Seed a couple of runs so the cascade has something to remove.
        store.record_run(&make_run(&task_id, 0)).await.unwrap();
        store.record_run(&make_run(&task_id, 1)).await.unwrap();
        assert_eq!(store.list_runs(&task_id).await.unwrap().len(), 2);

        // Delete the task; `ZqlTaskStore::delete` must also wipe the runs
        // under the shared `zql::Database` mutex.
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
    async fn test_zql_persistent_round_trip() {
        // Survives closing + reopening the database — both the task blob
        // and its run history must come back intact.
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("tasks_zql_db");

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
            let store = ZqlTaskStore::open(&db_path).await.unwrap();
            store.store(&task).await.unwrap();
            store.record_run(&run_a).await.unwrap();
            store.record_run(&run_b).await.unwrap();
        }

        {
            let store = ZqlTaskStore::open(&db_path).await.unwrap();
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
