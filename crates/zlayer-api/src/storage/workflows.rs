//! Workflow storage implementations.
//!
//! Provides both persistent (ZQL) and in-memory storage backends for
//! workflows (named DAGs of steps that compose tasks, project builds,
//! deploys, and sync applies) and their execution records (workflow runs).
//! Both backends share the [`WorkflowStorage`] trait so callers are
//! backend-agnostic.
//!
//! Uniqueness rule: `name` is globally unique. The ZQL backend enforces this
//! via a unique index on the peeled `name` column maintained by
//! [`ZqlJsonStore`]; the in-memory backend does not enforce uniqueness (kept
//! simple for unit-test fixtures — the persistent backend is the source of
//! truth in production).
//!
//! # Layout
//!
//! The ZQL backend owns two stores in the same database:
//!
//! - `workflows` — primary record store managed by
//!   [`ZqlJsonStore<StoredWorkflow>`] with a unique index on `name`.
//! - `workflow_runs` — secondary history store managed directly against the
//!   inner adapter's database handle. Keys are
//!   `"{workflow_id}:{started_at_rfc3339}:{run_id}"` so that a prefix scan
//!   by `"{workflow_id}:"` returns every run for a workflow in one pass.
//!   The run id suffix keeps keys unique when two runs share a timestamp.
//!
//! Deleting a workflow cascades: every `workflow_runs` entry under the
//! `"{workflow_id}:"` prefix is removed alongside the primary record, under
//! the same database mutex the adapter uses for its own writes, so a
//! concurrent `list_runs` cannot observe an orphaned run.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::zql_json::{IndexSpec, JsonTable, ZqlJsonStore};
use super::{StorageError, StoredWorkflow, WorkflowRun};

/// Name of the secondary ZQL store holding workflow run history.
const WORKFLOW_RUNS_STORE: &str = "workflow_runs";

/// Build the composite run-history key. Embedding `started_at` as the second
/// segment keeps per-workflow prefix scans sortable by time without a
/// separate index; the trailing `run_id` disambiguates runs that happened in
/// the same millisecond.
fn run_key(workflow_id: &str, run: &WorkflowRun) -> String {
    format!(
        "{workflow_id}:{started_at}:{run_id}",
        started_at = run.started_at.to_rfc3339(),
        run_id = run.id,
    )
}

/// Trait for workflow storage backends.
#[async_trait]
pub trait WorkflowStorage: Send + Sync {
    /// Store (create or update) a workflow by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the
    /// write.
    async fn store(&self, workflow: &StoredWorkflow) -> Result<(), StorageError>;

    /// Fetch a workflow by id.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn get(&self, id: &str) -> Result<Option<StoredWorkflow>, StorageError>;

    /// List all workflows. Results are sorted by name ascending.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn list(&self) -> Result<Vec<StoredWorkflow>, StorageError>;

    /// Delete a workflow by id. Returns `true` if the row existed.
    ///
    /// Persistent backends cascade-delete the workflow's run history.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the delete operation fails.
    async fn delete(&self, id: &str) -> Result<bool, StorageError>;

    /// Record a completed workflow run.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] if the backing store rejects the
    /// write.
    async fn record_run(&self, run: &WorkflowRun) -> Result<(), StorageError>;

    /// List runs for a given workflow, ordered by `started_at` descending
    /// (most recent first).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the backing store fails.
    async fn list_runs(&self, workflow_id: &str) -> Result<Vec<WorkflowRun>, StorageError>;
}

/// In-memory workflow store.
pub struct InMemoryWorkflowStore {
    workflows: Arc<RwLock<HashMap<String, StoredWorkflow>>>,
    runs: Arc<RwLock<Vec<WorkflowRun>>>,
}

impl InMemoryWorkflowStore {
    /// Create a new empty in-memory workflow store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            workflows: Arc::new(RwLock::new(HashMap::new())),
            runs: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for InMemoryWorkflowStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WorkflowStorage for InMemoryWorkflowStore {
    async fn store(&self, workflow: &StoredWorkflow) -> Result<(), StorageError> {
        let mut workflows = self.workflows.write().await;
        workflows.insert(workflow.id.clone(), workflow.clone());
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<StoredWorkflow>, StorageError> {
        let workflows = self.workflows.read().await;
        Ok(workflows.get(id).cloned())
    }

    async fn list(&self) -> Result<Vec<StoredWorkflow>, StorageError> {
        let workflows = self.workflows.read().await;
        let mut list: Vec<_> = workflows.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        let mut workflows = self.workflows.write().await;
        let existed = workflows.remove(id).is_some();
        drop(workflows);
        // Cascade-delete run history to mirror the persistent backend.
        let mut runs = self.runs.write().await;
        runs.retain(|r| r.workflow_id != id);
        Ok(existed)
    }

    async fn record_run(&self, run: &WorkflowRun) -> Result<(), StorageError> {
        let mut runs = self.runs.write().await;
        runs.push(run.clone());
        Ok(())
    }

    async fn list_runs(&self, workflow_id: &str) -> Result<Vec<WorkflowRun>, StorageError> {
        let runs = self.runs.read().await;
        let mut filtered: Vec<_> = runs
            .iter()
            .filter(|r| r.workflow_id == workflow_id)
            .cloned()
            .collect();
        filtered.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(filtered)
    }
}

/// Static table specification for the primary `workflows` store.
///
/// One peeled column — `name` — declared unique so the adapter's unique
/// companion store surfaces a second `store` under a different id with the
/// same name as [`StorageError::AlreadyExists`] straight out of
/// [`ZqlJsonStore::put`].
const WORKFLOWS_TABLE: JsonTable<StoredWorkflow> = JsonTable {
    name: "workflows",
    indexes: &[IndexSpec {
        column: "name",
        extractor: |w| Some(w.name.clone()),
        unique: true,
    }],
    unique_constraints: &[],
};

/// ZQL-backed persistent workflow store.
///
/// Delegates primary-record CRUD to [`ZqlJsonStore<StoredWorkflow>`] and owns
/// the secondary `workflow_runs` store directly on the adapter's database
/// handle. Both stores live in the same ZQL database; the daemon reopens
/// that database on start-up and both workflows and their run history
/// survive restarts.
pub struct ZqlWorkflowStore {
    inner: ZqlJsonStore<StoredWorkflow>,
}

impl ZqlWorkflowStore {
    /// Open or create a ZQL database at the given path.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database cannot be opened.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let inner = ZqlJsonStore::open(path, WORKFLOWS_TABLE).await?;
        Ok(Self { inner })
    }

    /// Create a ZQL database in a temporary directory (primarily for tests).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the temp directory or database cannot be
    /// created.
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self, StorageError> {
        let inner = ZqlJsonStore::in_memory(WORKFLOWS_TABLE).await?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl WorkflowStorage for ZqlWorkflowStore {
    async fn store(&self, workflow: &StoredWorkflow) -> Result<(), StorageError> {
        self.inner.put(&workflow.id, workflow).await
    }

    async fn get(&self, id: &str) -> Result<Option<StoredWorkflow>, StorageError> {
        self.inner.get(id).await
    }

    async fn list(&self) -> Result<Vec<StoredWorkflow>, StorageError> {
        // The adapter orders by primary key (`id`); the trait contract sorts
        // by name.
        let mut list = self.inner.list().await?;
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        // Cascade-delete the workflow's run history under the same mutex the
        // primary adapter uses, so a concurrent `list_runs` cannot observe an
        // orphan row between the two deletes. The primary `delete` call takes
        // the lock itself, so the cascade runs in a narrower critical section
        // first.
        let prefix = format!("{id}:");
        {
            let mut db = self.inner.db().lock().await;
            let runs: Vec<(String, WorkflowRun)> = db
                .scan_typed(WORKFLOW_RUNS_STORE, &prefix)
                .map_err(StorageError::from)?;
            for (key, _) in runs {
                db.delete_typed(WORKFLOW_RUNS_STORE, &key)
                    .map_err(StorageError::from)?;
            }
        }

        self.inner.delete(id).await
    }

    async fn record_run(&self, run: &WorkflowRun) -> Result<(), StorageError> {
        let key = run_key(&run.workflow_id, run);
        let mut db = self.inner.db().lock().await;
        db.put_typed(WORKFLOW_RUNS_STORE, &key, run)
            .map_err(StorageError::from)?;
        Ok(())
    }

    async fn list_runs(&self, workflow_id: &str) -> Result<Vec<WorkflowRun>, StorageError> {
        let prefix = format!("{workflow_id}:");
        let mut db = self.inner.db().lock().await;
        let rows: Vec<(String, WorkflowRun)> = db
            .scan_typed(WORKFLOW_RUNS_STORE, &prefix)
            .map_err(StorageError::from)?;
        drop(db);

        // Sort by `started_at` descending — the embedded timestamp in the key
        // is lexicographically sorted ascending by ZQL's key ordering, so we
        // reverse it here to fulfil the trait contract of "most recent first".
        let mut runs: Vec<WorkflowRun> = rows.into_iter().map(|(_, r)| r).collect();
        runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(runs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{
        StepResult, StoredWorkflow, WorkflowAction, WorkflowRun, WorkflowRunStatus, WorkflowStep,
    };
    use chrono::{Duration as ChronoDuration, Utc};

    fn make_workflow(name: &str) -> StoredWorkflow {
        StoredWorkflow::new(
            name,
            vec![WorkflowStep {
                name: "step-1".to_string(),
                action: WorkflowAction::RunTask {
                    task_id: "task-1".to_string(),
                },
                on_failure: None,
            }],
        )
    }

    fn make_run(workflow_id: &str) -> WorkflowRun {
        let now = Utc::now();
        WorkflowRun {
            id: uuid::Uuid::new_v4().to_string(),
            workflow_id: workflow_id.to_string(),
            status: WorkflowRunStatus::Completed,
            step_results: vec![StepResult {
                step_name: "step-1".to_string(),
                status: "ok".to_string(),
                output: Some("done".to_string()),
            }],
            started_at: now,
            finished_at: Some(now),
        }
    }

    // =========================================================================
    // InMemoryWorkflowStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_store_and_get() {
        let store = InMemoryWorkflowStore::new();
        let wf = make_workflow("deploy-pipeline");
        let id = wf.id.clone();

        store.store(&wf).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("workflow must exist");
        assert_eq!(retrieved.name, "deploy-pipeline");
        assert_eq!(retrieved.steps.len(), 1);
    }

    #[tokio::test]
    async fn test_list_sorted() {
        let store = InMemoryWorkflowStore::new();
        store.store(&make_workflow("beta")).await.unwrap();
        store.store(&make_workflow("alpha")).await.unwrap();
        store.store(&make_workflow("gamma")).await.unwrap();

        let all = store.list().await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].name, "alpha");
        assert_eq!(all[1].name, "beta");
        assert_eq!(all[2].name, "gamma");
    }

    #[tokio::test]
    async fn test_delete() {
        let store = InMemoryWorkflowStore::new();
        let wf = make_workflow("doomed");
        let id = wf.id.clone();
        store.store(&wf).await.unwrap();

        assert!(store.delete(&id).await.unwrap());
        assert!(store.get(&id).await.unwrap().is_none());
        assert!(!store.delete(&id).await.unwrap());
    }

    #[tokio::test]
    async fn test_record_and_list_runs() {
        let store = InMemoryWorkflowStore::new();
        let wf = make_workflow("test-wf");
        let wf_id = wf.id.clone();
        store.store(&wf).await.unwrap();

        let first_run = make_run(&wf_id);
        store.record_run(&first_run).await.unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut second_run = make_run(&wf_id);
        second_run.started_at = Utc::now();
        store.record_run(&second_run).await.unwrap();

        let recorded = store.list_runs(&wf_id).await.unwrap();
        assert_eq!(recorded.len(), 2);
        // Most recent first
        assert!(recorded[0].started_at >= recorded[1].started_at);
    }

    #[tokio::test]
    async fn test_list_runs_empty() {
        let store = InMemoryWorkflowStore::new();
        let runs = store.list_runs("nonexistent").await.unwrap();
        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn test_default_trait() {
        let store = InMemoryWorkflowStore::default();
        let all = store.list().await.unwrap();
        assert!(all.is_empty());
    }

    // =========================================================================
    // ZqlWorkflowStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_zql_store_and_get() {
        let store = ZqlWorkflowStore::in_memory().await.unwrap();
        let wf = make_workflow("deploy-pipeline");
        let id = wf.id.clone();

        store.store(&wf).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().expect("workflow must exist");
        assert_eq!(retrieved.name, "deploy-pipeline");
        assert_eq!(retrieved.steps.len(), 1);

        let missing = store.get("no-such-id").await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_zql_list_sorted() {
        let store = ZqlWorkflowStore::in_memory().await.unwrap();
        store.store(&make_workflow("gamma")).await.unwrap();
        store.store(&make_workflow("alpha")).await.unwrap();
        store.store(&make_workflow("beta")).await.unwrap();

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "alpha");
        assert_eq!(list[1].name, "beta");
        assert_eq!(list[2].name, "gamma");
    }

    #[tokio::test]
    async fn test_zql_duplicate_name() {
        let store = ZqlWorkflowStore::in_memory().await.unwrap();

        store.store(&make_workflow("dup")).await.unwrap();

        // A fresh workflow with the same name but a different id collides on
        // the unique-`name` index maintained by the adapter.
        let err = store
            .store(&make_workflow("dup"))
            .await
            .expect_err("duplicate name must fail");
        match err {
            StorageError::AlreadyExists(msg) => {
                assert!(
                    msg.contains("workflows"),
                    "message should mention the table name, got: {msg}"
                );
            }
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_zql_record_run_and_list_runs() {
        let store = ZqlWorkflowStore::in_memory().await.unwrap();
        let wf = make_workflow("run-history");
        let wf_id = wf.id.clone();
        store.store(&wf).await.unwrap();

        // Three runs with distinct `started_at` — explicit offsets so the
        // ordering assertion does not depend on wall-clock resolution.
        let base = Utc::now();
        let mut r1 = make_run(&wf_id);
        r1.started_at = base;
        let mut r2 = make_run(&wf_id);
        r2.started_at = base + ChronoDuration::seconds(1);
        let mut r3 = make_run(&wf_id);
        r3.started_at = base + ChronoDuration::seconds(2);

        store.record_run(&r1).await.unwrap();
        store.record_run(&r2).await.unwrap();
        store.record_run(&r3).await.unwrap();

        // A run for a different workflow must not leak into the listing.
        let other_wf = make_workflow("other");
        store.store(&other_wf).await.unwrap();
        let mut noise = make_run(&other_wf.id);
        noise.started_at = base + ChronoDuration::seconds(5);
        store.record_run(&noise).await.unwrap();

        let listed = store.list_runs(&wf_id).await.unwrap();
        assert_eq!(listed.len(), 3, "only the three runs for wf must come back");

        // Reverse-chronological: newest (r3) first, oldest (r1) last.
        assert_eq!(listed[0].id, r3.id);
        assert_eq!(listed[1].id, r2.id);
        assert_eq!(listed[2].id, r1.id);
    }

    #[tokio::test]
    async fn test_zql_delete_cascades_runs() {
        let store = ZqlWorkflowStore::in_memory().await.unwrap();
        let wf = make_workflow("cascade");
        let wf_id = wf.id.clone();
        store.store(&wf).await.unwrap();

        // Three runs with distinct timestamps so their composite keys stay
        // unique under the `"{wf_id}:"` prefix.
        let base = Utc::now();
        let mut r1 = make_run(&wf_id);
        r1.started_at = base;
        let mut r2 = make_run(&wf_id);
        r2.started_at = base + ChronoDuration::seconds(1);
        let mut r3 = make_run(&wf_id);
        r3.started_at = base + ChronoDuration::seconds(2);

        store.record_run(&r1).await.unwrap();
        store.record_run(&r2).await.unwrap();
        store.record_run(&r3).await.unwrap();
        assert_eq!(store.list_runs(&wf_id).await.unwrap().len(), 3);

        assert!(store.delete(&wf_id).await.unwrap());
        assert!(store.get(&wf_id).await.unwrap().is_none());

        // Cascade: run history for this workflow is gone.
        let runs = store.list_runs(&wf_id).await.unwrap();
        assert!(
            runs.is_empty(),
            "deleting the workflow must cascade-delete its runs, got {} rows",
            runs.len()
        );

        // Deleting again is a no-op.
        assert!(!store.delete(&wf_id).await.unwrap());
    }

    #[tokio::test]
    async fn test_zql_persistent_round_trip() {
        // Survives closing + reopening the database — the whole point of the
        // persistent backend, exercised across both stores.
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("workflows_zql_db");

        let wf = make_workflow("persist-me");
        let wf_id = wf.id.clone();

        let base = Utc::now();
        let mut r1 = make_run(&wf_id);
        r1.started_at = base;
        let mut r2 = make_run(&wf_id);
        r2.started_at = base + ChronoDuration::seconds(1);

        {
            let store = ZqlWorkflowStore::open(&db_path).await.unwrap();
            store.store(&wf).await.unwrap();
            store.record_run(&r1).await.unwrap();
            store.record_run(&r2).await.unwrap();
        }

        {
            let store = ZqlWorkflowStore::open(&db_path).await.unwrap();

            let retrieved = store
                .get(&wf_id)
                .await
                .unwrap()
                .expect("workflow must persist across reopen");
            assert_eq!(retrieved.name, "persist-me");
            assert_eq!(retrieved.steps.len(), 1);

            let list = store.list().await.unwrap();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].id, wf_id);

            // Run history survives too, still in reverse-chronological order.
            let runs = store.list_runs(&wf_id).await.unwrap();
            assert_eq!(runs.len(), 2);
            assert_eq!(runs[0].id, r2.id);
            assert_eq!(runs[1].id, r1.id);
        }
    }
}
