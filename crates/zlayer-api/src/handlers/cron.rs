//! Cron job management endpoints
//!
//! Provides API endpoints for listing and managing cron jobs.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use std::sync::Arc;
use zlayer_agent::{CronJobInfo, CronScheduler};

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};

/// Shared state containing the cron scheduler
#[derive(Clone)]
pub struct CronState {
    pub scheduler: Arc<CronScheduler>,
}

/// Response for cron job information
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CronJobResponse {
    /// Job name
    pub name: String,
    /// Cron schedule expression
    pub schedule: String,
    /// Whether the job is enabled
    pub enabled: bool,
    /// When the job last ran (ISO 8601 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    /// Next scheduled run time (ISO 8601 format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run: Option<String>,
}

/// Response after triggering a cron job
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TriggerCronResponse {
    /// Execution ID for tracking
    pub execution_id: String,
    /// Human-readable message
    pub message: String,
}

/// Response for enable/disable operations
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CronStatusResponse {
    /// Job name
    pub name: String,
    /// New enabled state
    pub enabled: bool,
    /// Human-readable message
    pub message: String,
}

/// Convert internal CronJobInfo to API response
fn cron_info_to_response(info: &CronJobInfo) -> CronJobResponse {
    CronJobResponse {
        name: info.name.clone(),
        schedule: info.schedule_expr.clone(),
        enabled: info.enabled,
        last_run: info.last_run.map(|dt| dt.to_rfc3339()),
        next_run: info.next_run.map(|dt| dt.to_rfc3339()),
    }
}

/// GET /api/v1/cron - List all cron jobs
///
/// Returns a list of all registered cron jobs with their schedule information.
#[utoipa::path(
    get,
    path = "/api/v1/cron",
    responses(
        (status = 200, description = "List of cron jobs", body = Vec<CronJobResponse>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Cron"
)]
pub async fn list_cron_jobs(
    _user: AuthUser,
    State(state): State<CronState>,
) -> Result<Json<Vec<CronJobResponse>>> {
    let jobs = state.scheduler.list_jobs().await;
    let responses: Vec<CronJobResponse> = jobs.iter().map(cron_info_to_response).collect();
    Ok(Json(responses))
}

/// GET /api/v1/cron/{name} - Get cron job details
///
/// Returns detailed information about a specific cron job.
#[utoipa::path(
    get,
    path = "/api/v1/cron/{name}",
    params(
        ("name" = String, Path, description = "Cron job name"),
    ),
    responses(
        (status = 200, description = "Cron job details", body = CronJobResponse),
        (status = 404, description = "Cron job not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Cron"
)]
pub async fn get_cron_job(
    _user: AuthUser,
    State(state): State<CronState>,
    Path(name): Path<String>,
) -> Result<Json<CronJobResponse>> {
    let info = state
        .scheduler
        .get_job_info(&name)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Cron job '{}' not found", name)))?;

    Ok(Json(cron_info_to_response(&info)))
}

/// POST /api/v1/cron/{name}/trigger - Manually trigger a cron job
///
/// Triggers an immediate execution of the cron job, regardless of its schedule.
#[utoipa::path(
    post,
    path = "/api/v1/cron/{name}/trigger",
    params(
        ("name" = String, Path, description = "Cron job name"),
    ),
    responses(
        (status = 202, description = "Cron job triggered", body = TriggerCronResponse),
        (status = 404, description = "Cron job not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Cron"
)]
pub async fn trigger_cron_job(
    user: AuthUser,
    State(state): State<CronState>,
    Path(name): Path<String>,
) -> Result<(StatusCode, Json<TriggerCronResponse>)> {
    // Require operator role for triggering
    user.require_role("operator")?;

    let exec_id = state
        .scheduler
        .trigger_now(&name)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { .. } => {
                ApiError::NotFound(format!("Cron job '{}' not found", name))
            }
            _ => ApiError::Internal(format!("Failed to trigger cron job: {}", e)),
        })?;

    Ok((
        StatusCode::ACCEPTED,
        Json(TriggerCronResponse {
            execution_id: exec_id.0,
            message: format!("Cron job '{}' triggered manually", name),
        }),
    ))
}

/// PUT /api/v1/cron/{name}/enable - Enable a cron job
///
/// Enables a disabled cron job, allowing it to run on schedule.
#[utoipa::path(
    put,
    path = "/api/v1/cron/{name}/enable",
    params(
        ("name" = String, Path, description = "Cron job name"),
    ),
    responses(
        (status = 200, description = "Cron job enabled", body = CronStatusResponse),
        (status = 404, description = "Cron job not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Cron"
)]
pub async fn enable_cron_job(
    user: AuthUser,
    State(state): State<CronState>,
    Path(name): Path<String>,
) -> Result<Json<CronStatusResponse>> {
    // Require operator role
    user.require_role("operator")?;

    // Verify the job exists
    state
        .scheduler
        .get_job_info(&name)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Cron job '{}' not found", name)))?;

    // Enable the job
    state.scheduler.set_enabled(&name, true).await;

    Ok(Json(CronStatusResponse {
        name: name.clone(),
        enabled: true,
        message: format!("Cron job '{}' enabled", name),
    }))
}

/// PUT /api/v1/cron/{name}/disable - Disable a cron job
///
/// Disables a cron job, preventing it from running on schedule.
/// The job can still be manually triggered.
#[utoipa::path(
    put,
    path = "/api/v1/cron/{name}/disable",
    params(
        ("name" = String, Path, description = "Cron job name"),
    ),
    responses(
        (status = 200, description = "Cron job disabled", body = CronStatusResponse),
        (status = 404, description = "Cron job not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Cron"
)]
pub async fn disable_cron_job(
    user: AuthUser,
    State(state): State<CronState>,
    Path(name): Path<String>,
) -> Result<Json<CronStatusResponse>> {
    // Require operator role
    user.require_role("operator")?;

    // Verify the job exists
    state
        .scheduler
        .get_job_info(&name)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Cron job '{}' not found", name)))?;

    // Disable the job
    state.scheduler.set_enabled(&name, false).await;

    Ok(Json(CronStatusResponse {
        name: name.clone(),
        enabled: false,
        message: format!("Cron job '{}' disabled", name),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_job_response_serialize() {
        let response = CronJobResponse {
            name: "backup".to_string(),
            schedule: "0 0 * * * * *".to_string(),
            enabled: true,
            last_run: Some("2025-01-25T00:00:00Z".to_string()),
            next_run: Some("2025-01-26T00:00:00Z".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("backup"));
        assert!(json.contains("0 0 * * * * *"));
    }

    #[test]
    fn test_trigger_cron_response_serialize() {
        let response = TriggerCronResponse {
            execution_id: "abc-123".to_string(),
            message: "Job triggered".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("abc-123"));
    }

    #[test]
    fn test_cron_status_response_serialize() {
        let response = CronStatusResponse {
            name: "backup".to_string(),
            enabled: true,
            message: "Enabled".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("backup"));
        assert!(json.contains("true"));
    }

    #[test]
    fn test_cron_info_to_response() {
        use chrono::Utc;
        let info = CronJobInfo {
            name: "cleanup".to_string(),
            schedule_expr: "0 0 * * * * *".to_string(),
            last_run: Some(Utc::now()),
            next_run: Some(Utc::now()),
            enabled: true,
        };
        let response = cron_info_to_response(&info);
        assert_eq!(response.name, "cleanup");
        assert_eq!(response.schedule, "0 0 * * * * *");
        assert!(response.enabled);
    }
}
