//! Workflow storage implementations.
//!
//! Provides both persistent (`SQLite` via [`SqlxJsonStore`]) and in-memory
//! storage backends for workflows (named DAGs of steps that compose tasks,
//! project builds, deploys, and sync applies) and their execution records
//! (workflow runs). Both backends share the [`WorkflowStorage`] trait so
//! callers are backend-agnostic.
//!
//! Uniqueness rule: `name` is globally unique. The `SQLite` backend enforces
//! this at the schema level via a `UNIQUE` constraint on the peeled `name`
//! column; the in-memory backend does not enforce uniqueness (kept simple for
//! unit-test fixtures — the persistent backend is the source of truth in
//! production).
//!
//! # Schema
//!
//! The persistent backend owns two tables in the same `SQLite` file:
//!
//! - `workflows` — primary record table managed by [`SqlxJsonStore`].
//! - `workflow_runs` — secondary history table managed hand-rolled on the
//!   underlying pool. A workflow's run history lives here, indexed by
//!   `workflow_id` + `started_at` so listings come out in reverse-chronological
//!   order via an index scan rather than an in-memory sort.
//!
//! Deleting a workflow is atomic across both tables: a transaction deletes
//! every run in `workflow_runs` referencing the workflow id, then the row in
//! `workflows`. Orphaned run rows are impossible by construction.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::sqlx_json::{IndexSpec, JsonTable, SqlxJsonStore};
use super::{StorageError, StoredWorkflow, WorkflowRun};

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
    /// Persistent backends cascade-delete the workflow's run history
    /// atomically in the same transaction.
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
        // Cascade-delete run history to mirror the persistent backend.
        drop(workflows);
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

/// Static table specification for the `workflows` table.
///
/// One peeled column — `name` — with a single-column `UNIQUE` constraint so a
/// second `store` with the same name under a different id surfaces as
/// [`StorageError::AlreadyExists`] straight out of the adapter.
const WORKFLOWS_TABLE: JsonTable<StoredWorkflow> = JsonTable {
    name: "workflows",
    indexes: &[IndexSpec {
        column: "name",
        extractor: |w| Some(w.name.clone()),
        unique: true,
    }],
    unique_constraints: &[],
};

/// DDL for the secondary `workflow_runs` history table, hand-rolled against
/// the same pool as [`WORKFLOWS_TABLE`]. Not managed by [`SqlxJsonStore`]
/// because its access pattern is fundamentally different — rows are appended
/// by `record_run`, scanned by `workflow_id`, and never updated.
///
/// The composite index `(workflow_id, started_at DESC)` is the point of this
/// table: `list_runs` becomes an ordered index scan rather than a
/// fetch-plus-sort-in-memory pass like the in-memory backend does.
const WORKFLOW_RUNS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS workflow_runs (\n\
    id TEXT PRIMARY KEY NOT NULL,\n\
    workflow_id TEXT NOT NULL,\n\
    data_json TEXT NOT NULL,\n\
    started_at TEXT NOT NULL\n\
)";

const WORKFLOW_RUNS_IDX_DDL: &str = "\
CREATE INDEX IF NOT EXISTS idx_workflow_runs_workflow_id \
ON workflow_runs(workflow_id, started_at DESC)";

/// `SQLite`-backed persistent workflow store.
///
/// Delegates primary-record CRUD to [`SqlxJsonStore<StoredWorkflow>`] and owns
/// the secondary `workflow_runs` table directly on the same pool. The daemon
/// reopens the database on start-up and both workflows and their run history
/// survive restarts.
pub struct SqlxWorkflowStore {
    inner: SqlxJsonStore<StoredWorkflow>,
}

impl SqlxWorkflowStore {
    /// Open or create a `SQLite` database at the given path and initialise
    /// both the `workflows` and `workflow_runs` tables.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the database connection fails or either
    /// table (or its index) cannot be created.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let inner = SqlxJsonStore::open(path, WORKFLOWS_TABLE).await?;
        Self::init_runs_schema(&inner).await?;
        Ok(Self { inner })
    }

    /// Create an in-memory `SQLite` database (useful for tests) with both
    /// tables in place.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the in-memory database cannot be created or
    /// the secondary table cannot be initialised.
    pub async fn in_memory() -> Result<Self, StorageError> {
        let inner = SqlxJsonStore::in_memory(WORKFLOWS_TABLE).await?;
        Self::init_runs_schema(&inner).await?;
        Ok(Self { inner })
    }

    /// Create the `workflow_runs` table + its composite index on the inner
    /// store's pool. Safe to run repeatedly; both statements are
    /// `IF NOT EXISTS`.
    async fn init_runs_schema(inner: &SqlxJsonStore<StoredWorkflow>) -> Result<(), StorageError> {
        let pool = inner.pool();
        sqlx::query(WORKFLOW_RUNS_DDL).execute(pool).await?;
        sqlx::query(WORKFLOW_RUNS_IDX_DDL).execute(pool).await?;
        Ok(())
    }
}

#[async_trait]
impl WorkflowStorage for SqlxWorkflowStore {
    async fn store(&self, workflow: &StoredWorkflow) -> Result<(), StorageError> {
        self.inner.put(&workflow.id, workflow).await
    }

    async fn get(&self, id: &str) -> Result<Option<StoredWorkflow>, StorageError> {
        self.inner.get(id).await
    }

    async fn list(&self) -> Result<Vec<StoredWorkflow>, StorageError> {
        // The adapter orders by `id`; the trait contract sorts by name.
        let mut list = self.inner.list().await?;
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete(&self, id: &str) -> Result<bool, StorageError> {
        // Delete the workflow and its run history atomically so a crash mid-
        // delete cannot leave orphan rows in `workflow_runs`. Run history
        // goes first; if the workflow row turns out not to exist, the empty
        // run delete is a harmless no-op.
        let mut tx = self.inner.pool().begin().await?;

        sqlx::query("DELETE FROM workflow_runs WHERE workflow_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;

        let result = sqlx::query("DELETE FROM workflows WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    async fn record_run(&self, run: &WorkflowRun) -> Result<(), StorageError> {
        let data_json = serde_json::to_string(run)?;
        let started_at = run.started_at.to_rfc3339();
        sqlx::query(
            "INSERT INTO workflow_runs (id, workflow_id, data_json, started_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(&run.id)
        .bind(&run.workflow_id)
        .bind(&data_json)
        .bind(&started_at)
        .execute(self.inner.pool())
        .await?;
        Ok(())
    }

    async fn list_runs(&self, workflow_id: &str) -> Result<Vec<WorkflowRun>, StorageError> {
        // The composite index `(workflow_id, started_at DESC)` makes this a
        // straight index scan — no post-sort needed.
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT data_json FROM workflow_runs \
             WHERE workflow_id = ? \
             ORDER BY started_at DESC",
        )
        .bind(workflow_id)
        .fetch_all(self.inner.pool())
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (data_json,) in rows {
            let run: WorkflowRun = serde_json::from_str(&data_json)?;
            out.push(run);
        }
        Ok(out)
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
    // SqlxWorkflowStore tests
    // =========================================================================

    #[tokio::test]
    async fn test_sqlx_store_and_get() {
        let store = SqlxWorkflowStore::in_memory().await.unwrap();
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
    async fn test_sqlx_list_sorted() {
        let store = SqlxWorkflowStore::in_memory().await.unwrap();
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
    async fn test_sqlx_duplicate_name() {
        let store = SqlxWorkflowStore::in_memory().await.unwrap();

        store.store(&make_workflow("dup")).await.unwrap();

        // A fresh workflow with the same name but a different id collides on
        // the UNIQUE(name) schema constraint.
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
    async fn test_sqlx_record_run_and_list_runs() {
        let store = SqlxWorkflowStore::in_memory().await.unwrap();
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
    async fn test_sqlx_delete_cascades_runs() {
        let store = SqlxWorkflowStore::in_memory().await.unwrap();
        let wf = make_workflow("cascade");
        let wf_id = wf.id.clone();
        store.store(&wf).await.unwrap();

        store.record_run(&make_run(&wf_id)).await.unwrap();
        store.record_run(&make_run(&wf_id)).await.unwrap();
        store.record_run(&make_run(&wf_id)).await.unwrap();
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
    async fn test_sqlx_persistent_round_trip() {
        // Survives closing + reopening the database — the whole point of the
        // persistent backend, exercised across both tables.
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("workflows.db");

        let wf = make_workflow("persist-me");
        let wf_id = wf.id.clone();

        let base = Utc::now();
        let mut r1 = make_run(&wf_id);
        r1.started_at = base;
        let mut r2 = make_run(&wf_id);
        r2.started_at = base + ChronoDuration::seconds(1);

        {
            let store = SqlxWorkflowStore::open(&db_path).await.unwrap();
            store.store(&wf).await.unwrap();
            store.record_run(&r1).await.unwrap();
            store.record_run(&r2).await.unwrap();
        }

        {
            let store = SqlxWorkflowStore::open(&db_path).await.unwrap();

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
