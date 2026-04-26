//! Job execution DTOs.

use serde::{Deserialize, Serialize};

#[cfg(feature = "utoipa")]
use utoipa::IntoParams;

/// Response after triggering a job
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct TriggerJobResponse {
    /// Unique execution ID for tracking
    pub execution_id: String,
    /// Human-readable message
    pub message: String,
}

/// Job execution status response
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(IntoParams))]
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
