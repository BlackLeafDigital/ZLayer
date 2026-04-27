//! Cron job DTOs.

use serde::{Deserialize, Serialize};

/// Response for cron job information
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TriggerCronResponse {
    /// Execution ID for tracking
    pub execution_id: String,
    /// Human-readable message
    pub message: String,
}

/// Response for enable/disable operations
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CronStatusResponse {
    /// Job name
    pub name: String,
    /// New enabled state
    pub enabled: bool,
    /// Human-readable message
    pub message: String,
}
