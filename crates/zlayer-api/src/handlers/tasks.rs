//! Task CRUD and execution endpoints.
//!
//! Tasks are named runnable scripts that can be executed on demand.
//! When run, the task body is executed as a subprocess and stdout/stderr
//! are captured. Past runs are recorded and queryable.
//!
//! Tasks can be global (`project_id = None`) or project-scoped
//! (`project_id = Some(project_id)`).
//!
//! Read endpoints accept any authenticated actor; mutating endpoints
//! (create, delete, run) require the `admin` role.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;
use crate::storage::{StoredTask, TaskKind, TaskRun, TaskStorage};

/// State for task endpoints.
#[derive(Clone)]
pub struct TasksState {
    /// Underlying task storage backend.
    pub store: Arc<dyn TaskStorage>,
}

impl TasksState {
    /// Build a new state from a task store.
    #[must_use]
    pub fn new(store: Arc<dyn TaskStorage>) -> Self {
        Self { store }
    }
}

// ---- Request/response types ----

/// Query for `GET /api/v1/tasks`.
#[derive(Debug, Deserialize, Default)]
pub struct ListTasksQuery {
    /// Filter by project id. When omitted, all tasks (global + project-scoped)
    /// are returned.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// Body for `POST /api/v1/tasks`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateTaskRequest {
    /// Task name.
    pub name: String,
    /// Script type.
    pub kind: TaskKind,
    /// The script/command body.
    pub body: String,
    /// Project id scope. `None` = global task.
    #[serde(default)]
    pub project_id: Option<String>,
}

// ---- Endpoints ----

/// List tasks.
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the task store fails.
#[utoipa::path(
    get,
    path = "/api/v1/tasks",
    params(
        ("project_id" = Option<String>, Query,
            description = "Filter by project id; omit to list all tasks"),
    ),
    responses(
        (status = 200, description = "List of tasks", body = Vec<StoredTask>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Tasks"
)]
pub async fn list_tasks(
    _actor: AuthActor,
    State(state): State<TasksState>,
    Query(query): Query<ListTasksQuery>,
) -> Result<Json<Vec<StoredTask>>> {
    let tasks = state
        .store
        .list(query.project_id.as_deref())
        .await
        .map_err(|e| ApiError::Internal(format!("Task store: {e}")))?;
    Ok(Json(tasks))
}

/// Create a new task. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::BadRequest`] for an empty name or body,
/// or [`ApiError::Internal`] when the task store fails.
#[utoipa::path(
    post,
    path = "/api/v1/tasks",
    request_body = CreateTaskRequest,
    responses(
        (status = 201, description = "Task created", body = StoredTask),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Tasks"
)]
pub async fn create_task(
    actor: AuthActor,
    State(state): State<TasksState>,
    Json(req): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<StoredTask>)> {
    actor.require_admin()?;

    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest(
            "Task name cannot be empty".to_string(),
        ));
    }
    if name.len() > 256 {
        return Err(ApiError::BadRequest(
            "Task name cannot exceed 256 characters".to_string(),
        ));
    }
    if req.body.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Task body cannot be empty".to_string(),
        ));
    }

    let task = StoredTask::new(name, req.kind, &req.body, req.project_id);

    state
        .store
        .store(&task)
        .await
        .map_err(|e| ApiError::Internal(format!("Task store: {e}")))?;

    Ok((StatusCode::CREATED, Json(task)))
}

/// Fetch a single task by id.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] if no task with the given id exists,
/// or [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    get,
    path = "/api/v1/tasks/{id}",
    params(("id" = String, Path, description = "Task id")),
    responses(
        (status = 200, description = "Task", body = StoredTask),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Tasks"
)]
pub async fn get_task(
    _actor: AuthActor,
    State(state): State<TasksState>,
    Path(id): Path<String>,
) -> Result<Json<StoredTask>> {
    let task = state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Task store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Task {id} not found")))?;
    Ok(Json(task))
}

/// Delete a task. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the task does not exist, or [`ApiError::Internal`] when the
/// store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/tasks/{id}",
    params(("id" = String, Path, description = "Task id")),
    responses(
        (status = 204, description = "Task deleted"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Tasks"
)]
pub async fn delete_task(
    actor: AuthActor,
    State(state): State<TasksState>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    let deleted = state
        .store
        .delete(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Task store: {e}")))?;

    if !deleted {
        return Err(ApiError::NotFound(format!("Task {id} not found")));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Execute a task synchronously. Admin only.
///
/// Runs the task's body via `sh -c` on Unix, captures stdout/stderr,
/// and returns the completed `TaskRun`.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the task does not exist, [`ApiError::NotImplemented`] on non-Unix
/// platforms, or [`ApiError::Internal`] when the store or execution fails.
#[utoipa::path(
    post,
    path = "/api/v1/tasks/{id}/run",
    params(("id" = String, Path, description = "Task id")),
    responses(
        (status = 200, description = "Task run result", body = TaskRun),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Task not found"),
        (status = 501, description = "Not implemented on this platform"),
    ),
    security(("bearer_auth" = [])),
    tag = "Tasks"
)]
pub async fn run_task(
    actor: AuthActor,
    State(state): State<TasksState>,
    Path(id): Path<String>,
) -> Result<Json<TaskRun>> {
    actor.require_admin()?;

    let task = state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Task store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Task {id} not found")))?;

    let run = execute_task(&task).await?;

    state
        .store
        .record_run(&run)
        .await
        .map_err(|e| ApiError::Internal(format!("Task store: {e}")))?;

    Ok(Json(run))
}

/// List past runs for a task, most recent first.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] if the task does not exist,
/// or [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    get,
    path = "/api/v1/tasks/{id}/runs",
    params(("id" = String, Path, description = "Task id")),
    responses(
        (status = 200, description = "List of task runs", body = Vec<TaskRun>),
        (status = 404, description = "Task not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Tasks"
)]
pub async fn list_task_runs(
    _actor: AuthActor,
    State(state): State<TasksState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<TaskRun>>> {
    // Verify task exists
    state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Task store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Task {id} not found")))?;

    let runs = state
        .store
        .list_runs(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Task store: {e}")))?;
    Ok(Json(runs))
}

// ---- Internal helpers ----

/// Execute a task body via `sh -c` on Unix, capturing output.
///
/// On non-Unix platforms, returns 501 Not Implemented.
async fn execute_task(task: &StoredTask) -> Result<TaskRun> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let started_at = Utc::now();

    #[cfg(unix)]
    {
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&task.body)
            .output()
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to execute task: {e}")))?;

        let finished_at = Utc::now();

        Ok(TaskRun {
            id: run_id,
            task_id: task.id.clone(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            started_at,
            finished_at: Some(finished_at),
        })
    }

    #[cfg(not(unix))]
    {
        let _ = (run_id, started_at);
        Err(ApiError::NotImplemented(
            "Task execution is only supported on Unix platforms".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_request_deserialize_minimum() {
        let req: CreateTaskRequest =
            serde_json::from_str(r#"{"name":"build","kind":"bash","body":"cargo build"}"#).unwrap();
        assert_eq!(req.name, "build");
        assert_eq!(req.kind, TaskKind::Bash);
        assert_eq!(req.body, "cargo build");
        assert!(req.project_id.is_none());
    }

    #[test]
    fn test_create_request_deserialize_full() {
        let req: CreateTaskRequest = serde_json::from_str(
            r#"{"name":"build","kind":"bash","body":"cargo build","project_id":"proj-1"}"#,
        )
        .unwrap();
        assert_eq!(req.name, "build");
        assert_eq!(req.kind, TaskKind::Bash);
        assert_eq!(req.body, "cargo build");
        assert_eq!(req.project_id.as_deref(), Some("proj-1"));
    }

    #[test]
    fn test_list_query_default_is_empty() {
        let q = ListTasksQuery::default();
        assert!(q.project_id.is_none());
    }

    #[test]
    fn test_task_kind_serde_roundtrip() {
        let json = serde_json::to_string(&TaskKind::Bash).unwrap();
        assert_eq!(json, r#""bash""#);
        let parsed: TaskKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, TaskKind::Bash);
    }

    #[test]
    fn test_task_run_serialization() {
        let run = TaskRun {
            id: "run-1".to_string(),
            task_id: "task-1".to_string(),
            exit_code: Some(0),
            stdout: "ok".to_string(),
            stderr: String::new(),
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
        };
        let json = serde_json::to_string(&run).unwrap();
        let parsed: TaskRun = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "run-1");
        assert_eq!(parsed.exit_code, Some(0));
    }
}
