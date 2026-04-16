//! Wire types for the Tasks page.
//!
//! Mirror of `zlayer_api::storage::{StoredTask, TaskRun}`, but defined
//! locally so the hydrate/WASM build doesn't need `zlayer-api` (which is
//! SSR-only).
//!
//! Field shape was cross-checked against
//! `crates/zlayer-api/src/storage/mod.rs` (`StoredTask`, `TaskRun`) and
//! `crates/zlayer-api/src/handlers/tasks.rs` (`CreateTaskRequest`). The
//! daemon serialises `TaskKind::Bash` as the string `"bash"` (via
//! `#[serde(rename_all = "snake_case")]`) and `DateTime<Utc>` as
//! RFC-3339, so receiving them here as `String` round-trips cleanly.
//!
//! NOTE: the daemon does NOT expose an `update_task` / PATCH endpoint.
//! The UI therefore only supports create / delete / run; edits are done
//! by deleting and recreating.

use serde::{Deserialize, Serialize};

/// A stored task returned by `/api/v1/tasks`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireTask {
    /// UUID identifier.
    pub id: String,
    /// Task name (unique).
    pub name: String,
    /// Script kind — the daemon currently only emits `"bash"`.
    pub kind: String,
    /// The script/command body (bash source text).
    pub body: String,
    /// Project-scope id. `None` = global task.
    #[serde(default)]
    pub project_id: Option<String>,
    /// RFC-3339 creation timestamp.
    pub created_at: String,
    /// RFC-3339 last-updated timestamp.
    pub updated_at: String,
}

/// A recorded task run returned by `/api/v1/tasks/{id}/run` or
/// `/api/v1/tasks/{id}/runs`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireTaskRun {
    /// UUID identifier of this run.
    pub id: String,
    /// The task that was executed.
    pub task_id: String,
    /// Process exit code; `None` if the task could not be started.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Captured standard output.
    #[serde(default)]
    pub stdout: String,
    /// Captured standard error.
    #[serde(default)]
    pub stderr: String,
    /// RFC-3339 start timestamp.
    pub started_at: String,
    /// RFC-3339 finish timestamp; `None` if the run has not finished.
    #[serde(default)]
    pub finished_at: Option<String>,
}

/// Request body for `manager_create_task`. Mirrors the daemon's
/// `CreateTaskRequest`.
///
/// `kind` is a free-form string to match the on-the-wire JSON; it should
/// currently always be `"bash"`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WireTaskSpec {
    /// Task name.
    pub name: String,
    /// Script kind — `"bash"` for now.
    pub kind: String,
    /// The script/command body.
    pub body: String,
    /// Project id scope. `None` = global task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}
