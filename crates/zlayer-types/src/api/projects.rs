//! Project CRUD API DTOs.

use serde::{Deserialize, Serialize};

use crate::storage::BuildKind;

/// Body for `POST /api/v1/projects`.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateProjectRequest {
    /// Project name (globally unique).
    pub name: String,
    /// Free-form description.
    #[serde(default)]
    pub description: Option<String>,
    /// Git repository URL.
    #[serde(default)]
    pub git_url: Option<String>,
    /// Git branch to build from.
    #[serde(default)]
    pub git_branch: Option<String>,
    /// Reference to a `GitCredential` id.
    #[serde(default)]
    pub git_credential_id: Option<String>,
    /// How the project is built.
    #[serde(default)]
    pub build_kind: Option<BuildKind>,
    /// Relative build path within the repo.
    #[serde(default)]
    pub build_path: Option<String>,
    /// Relative path (inside the cloned repo) to a `DeploymentSpec` YAML
    /// that the workflow `DeployProject` action should apply.
    #[serde(default)]
    pub deploy_spec_path: Option<String>,
    /// Reference to a `RegistryCredential` id.
    #[serde(default)]
    pub registry_credential_id: Option<String>,
    /// Default environment id.
    #[serde(default)]
    pub default_environment_id: Option<String>,
    /// Enable automatic deploy on new commits.
    #[serde(default)]
    pub auto_deploy: Option<bool>,
    /// Polling interval in seconds (None = no polling).
    #[serde(default)]
    pub poll_interval_secs: Option<u64>,
}

/// Body for `PATCH /api/v1/projects/{id}`. All fields are optional.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UpdateProjectRequest {
    /// New project name.
    #[serde(default)]
    pub name: Option<String>,
    /// New description.
    #[serde(default)]
    pub description: Option<String>,
    /// New git URL.
    #[serde(default)]
    pub git_url: Option<String>,
    /// New git branch.
    #[serde(default)]
    pub git_branch: Option<String>,
    /// New git credential id.
    #[serde(default)]
    pub git_credential_id: Option<String>,
    /// New build kind.
    #[serde(default)]
    pub build_kind: Option<BuildKind>,
    /// New build path.
    #[serde(default)]
    pub build_path: Option<String>,
    /// New path (inside the cloned repo) to the `DeploymentSpec` YAML that
    /// workflow `DeployProject` actions should apply. Pass `""` to clear.
    #[serde(default)]
    pub deploy_spec_path: Option<String>,
    /// New registry credential id.
    #[serde(default)]
    pub registry_credential_id: Option<String>,
    /// New default environment id.
    #[serde(default)]
    pub default_environment_id: Option<String>,
    /// Enable or disable automatic deploy on new commits.
    #[serde(default)]
    pub auto_deploy: Option<bool>,
    /// Set polling interval in seconds. `null` disables polling.
    #[serde(default)]
    pub poll_interval_secs: Option<Option<u64>>,
}

/// Body for `POST /api/v1/projects/{id}/deployments`.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LinkDeploymentRequest {
    /// Name of the deployment to link.
    pub deployment_name: String,
}

/// Response body for `POST /api/v1/projects/{id}/pull`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProjectPullResponse {
    /// Project id the pull was performed for.
    pub project_id: String,
    /// Git URL that was cloned / fetched.
    pub git_url: String,
    /// Branch checked out in the working copy.
    pub branch: String,
    /// HEAD commit SHA after the pull.
    pub sha: String,
    /// Absolute path to the working copy on disk.
    pub path: String,
}
