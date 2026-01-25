//! Job execution endpoints
//!
//! Provides API endpoints for triggering jobs and querying execution status.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use agent::{JobExecution, JobExecutionId, JobExecutor, JobStatus, JobTrigger};
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};

/// Shared state containing the job executor
#[derive(Clone)]
pub struct JobState {
    pub executor: Arc<JobExecutor>,
}

/// Response after triggering a job
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TriggerJobResponse {
    /// Unique execution ID for tracking
    pub execution_id: String,
    /// Human-readable message
    pub message: String,
}

/// Job execution status response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JobExecutionResponse {
    /// Unique execution ID
    pub id: String,
    /// Name of the job
    pub job_name: String,
    /// Current status (pending, initializing, running, completed, failed, cancelled)
    pub status: String,
    /// When the job started (ISO 8601 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// When the job completed (ISO 8601 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// Exit code (if completed/failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Captured logs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<String>,
    /// How the job was triggered
    pub trigger: String,
    /// Error reason (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Duration in milliseconds (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Query parameters for listing executions
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListExecutionsQuery {
    /// Maximum number of executions to return
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Filter by status (pending, running, completed, failed)
    #[serde(default)]
    pub status: Option<String>,
}

fn default_limit() -> usize {
    50
}

/// Convert internal JobExecution to API response
fn execution_to_response(exec: &JobExecution) -> JobExecutionResponse {
    let (status_str, exit_code, error, duration_ms) = match &exec.status {
        JobStatus::Pending => ("pending".to_string(), None, None, None),
        JobStatus::Initializing => ("initializing".to_string(), None, None, None),
        JobStatus::Running => ("running".to_string(), None, None, None),
        JobStatus::Completed {
            exit_code,
            duration,
        } => (
            "completed".to_string(),
            Some(*exit_code),
            None,
            Some(duration.as_millis() as u64),
        ),
        JobStatus::Failed { reason, exit_code } => {
            ("failed".to_string(), *exit_code, Some(reason.clone()), None)
        }
        JobStatus::Cancelled => ("cancelled".to_string(), None, None, None),
    };

    let trigger_str = match &exec.trigger {
        JobTrigger::Endpoint { remote_addr } => {
            if let Some(addr) = remote_addr {
                format!("endpoint:{}", addr)
            } else {
                "endpoint".to_string()
            }
        }
        JobTrigger::Cli => "cli".to_string(),
        JobTrigger::Scheduler => "scheduler".to_string(),
        JobTrigger::Internal { reason } => format!("internal:{}", reason),
    };

    // Convert Instant to approximate ISO 8601 string
    // Note: Instant doesn't have a direct mapping to calendar time,
    // so we approximate based on elapsed time from now
    use chrono::Utc;
    let now = Utc::now();
    let started_at = exec.started_at.map(|_| now.to_rfc3339()); // Approximation
    let completed_at = exec.completed_at.map(|_| now.to_rfc3339()); // Approximation

    JobExecutionResponse {
        id: exec.id.0.clone(),
        job_name: exec.job_name.clone(),
        status: status_str,
        started_at,
        completed_at,
        exit_code,
        logs: exec.logs.clone(),
        trigger: trigger_str,
        error,
        duration_ms,
    }
}

/// POST /api/v1/jobs/{name}/trigger - Trigger a job execution
///
/// Starts a new execution of the specified job. Returns immediately with an
/// execution ID that can be used to track the job's progress.
#[utoipa::path(
    post,
    path = "/api/v1/jobs/{name}/trigger",
    params(
        ("name" = String, Path, description = "Job name"),
    ),
    responses(
        (status = 202, description = "Job triggered successfully", body = TriggerJobResponse),
        (status = 404, description = "Job not found"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Jobs"
)]
pub async fn trigger_job(
    _user: AuthUser,
    State(state): State<JobState>,
    Path(name): Path<String>,
) -> Result<(StatusCode, Json<TriggerJobResponse>)> {
    // Get the job spec
    let spec =
        state.executor.get_job_spec(&name).await.ok_or_else(|| {
            ApiError::NotFound(format!("Job '{}' not found or not registered", name))
        })?;

    // Trigger the job
    let exec_id = state
        .executor
        .trigger(&name, &spec, JobTrigger::Endpoint { remote_addr: None })
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to trigger job: {}", e)))?;

    Ok((
        StatusCode::ACCEPTED,
        Json(TriggerJobResponse {
            execution_id: exec_id.0,
            message: format!("Job '{}' triggered successfully", name),
        }),
    ))
}

/// GET /api/v1/jobs/{execution_id}/status - Get execution status
///
/// Returns the current status of a job execution, including logs if available.
#[utoipa::path(
    get,
    path = "/api/v1/jobs/{execution_id}/status",
    params(
        ("execution_id" = String, Path, description = "Execution ID"),
    ),
    responses(
        (status = 200, description = "Execution status", body = JobExecutionResponse),
        (status = 404, description = "Execution not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Jobs"
)]
pub async fn get_execution_status(
    _user: AuthUser,
    State(state): State<JobState>,
    Path(execution_id): Path<String>,
) -> Result<Json<JobExecutionResponse>> {
    let exec_id = JobExecutionId(execution_id.clone());

    let execution = state
        .executor
        .get_execution(&exec_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Execution '{}' not found", execution_id)))?;

    Ok(Json(execution_to_response(&execution)))
}

/// GET /api/v1/jobs/{name}/executions - List executions for a job
///
/// Returns a list of recent executions for the specified job.
#[utoipa::path(
    get,
    path = "/api/v1/jobs/{name}/executions",
    params(
        ("name" = String, Path, description = "Job name"),
        ListExecutionsQuery,
    ),
    responses(
        (status = 200, description = "List of executions", body = Vec<JobExecutionResponse>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Jobs"
)]
pub async fn list_job_executions(
    _user: AuthUser,
    State(state): State<JobState>,
    Path(name): Path<String>,
    Query(query): Query<ListExecutionsQuery>,
) -> Result<Json<Vec<JobExecutionResponse>>> {
    let executions = state.executor.list_executions(&name).await;

    // Filter by status if specified
    let filtered: Vec<_> = executions
        .iter()
        .filter(|e| {
            if let Some(ref status_filter) = query.status {
                let status_str = match &e.status {
                    JobStatus::Pending => "pending",
                    JobStatus::Initializing => "initializing",
                    JobStatus::Running => "running",
                    JobStatus::Completed { .. } => "completed",
                    JobStatus::Failed { .. } => "failed",
                    JobStatus::Cancelled => "cancelled",
                };
                status_str == status_filter.as_str()
            } else {
                true
            }
        })
        .take(query.limit)
        .map(execution_to_response)
        .collect();

    Ok(Json(filtered))
}

/// POST /api/v1/jobs/{execution_id}/cancel - Cancel a running execution
///
/// Attempts to cancel a running or pending job execution.
#[utoipa::path(
    post,
    path = "/api/v1/jobs/{execution_id}/cancel",
    params(
        ("execution_id" = String, Path, description = "Execution ID"),
    ),
    responses(
        (status = 200, description = "Execution cancelled"),
        (status = 404, description = "Execution not found"),
        (status = 409, description = "Execution already completed"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Jobs"
)]
pub async fn cancel_execution(
    user: AuthUser,
    State(state): State<JobState>,
    Path(execution_id): Path<String>,
) -> Result<Json<JobExecutionResponse>> {
    // Require operator role for cancellation
    user.require_role("operator")?;

    let exec_id = JobExecutionId(execution_id.clone());

    // Check if execution exists and is cancellable
    let execution = state
        .executor
        .get_execution(&exec_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Execution '{}' not found", execution_id)))?;

    // Check if it's in a cancellable state
    if matches!(
        execution.status,
        JobStatus::Completed { .. } | JobStatus::Failed { .. } | JobStatus::Cancelled
    ) {
        return Err(ApiError::Conflict(format!(
            "Execution '{}' is already in terminal state: {}",
            execution_id, execution.status
        )));
    }

    // Cancel the execution
    state
        .executor
        .cancel(&exec_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to cancel execution: {}", e)))?;

    // Get updated status
    let updated = state
        .executor
        .get_execution(&exec_id)
        .await
        .ok_or_else(|| ApiError::Internal("Execution disappeared after cancel".to_string()))?;

    Ok(Json(execution_to_response(&updated)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trigger_response_serialize() {
        let response = TriggerJobResponse {
            execution_id: "abc-123".to_string(),
            message: "Job triggered".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("abc-123"));
        assert!(json.contains("Job triggered"));
    }

    #[test]
    fn test_execution_response_serialize() {
        let response = JobExecutionResponse {
            id: "exec-123".to_string(),
            job_name: "backup".to_string(),
            status: "completed".to_string(),
            started_at: Some("2025-01-25T12:00:00Z".to_string()),
            completed_at: Some("2025-01-25T12:01:00Z".to_string()),
            exit_code: Some(0),
            logs: Some("Done!".to_string()),
            trigger: "cli".to_string(),
            error: None,
            duration_ms: Some(5000),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("exec-123"));
        assert!(json.contains("backup"));
        assert!(json.contains("completed"));
    }

    #[test]
    fn test_list_query_defaults() {
        let query: ListExecutionsQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(query.limit, 50);
        assert!(query.status.is_none());
    }
}
