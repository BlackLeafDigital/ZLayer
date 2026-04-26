//! Workflow DTOs.

use serde::{Deserialize, Serialize};

use crate::storage::WorkflowStep;

/// Body for `POST /api/v1/workflows`.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CreateWorkflowRequest {
    /// Workflow name.
    pub name: String,
    /// Ordered list of steps.
    pub steps: Vec<WorkflowStep>,
    /// Optional project scope.
    #[serde(default)]
    pub project_id: Option<String>,
}
