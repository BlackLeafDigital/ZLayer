//! Environment DTOs.
//!
//! Request/response types for the `/api/v1/environments` CRUD endpoints. The
//! runtime state structs (`EnvironmentsState`, `EnvironmentsRouterState`) and
//! the axum handler functions remain in the API crate.

use serde::{Deserialize, Serialize};

/// Query for `GET /api/v1/environments`.
///
/// `project` selects the namespace:
///   - omitted → list global environments only (`project_id IS NULL`).
///   - `*` → list every environment across all projects + global.
///   - any other value → list environments owned by that project id.
#[derive(Debug, Deserialize, Default)]
pub struct ListEnvironmentsQuery {
    #[serde(default)]
    pub project: Option<String>,
}

/// Body for `POST /api/v1/environments`.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateEnvironmentRequest {
    /// Display name. Must be unique within the chosen `project_id` namespace.
    pub name: String,
    /// Owning project id. `None` = global environment.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Free-form description shown in the UI.
    #[serde(default)]
    pub description: Option<String>,
}

/// Body for `PATCH /api/v1/environments/{id}`. All fields are optional.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UpdateEnvironmentRequest {
    /// New display name. Will be re-checked for uniqueness.
    #[serde(default)]
    pub name: Option<String>,
    /// New description. Pass `Some("")` to clear, omit to leave unchanged.
    #[serde(default)]
    pub description: Option<String>,
}
