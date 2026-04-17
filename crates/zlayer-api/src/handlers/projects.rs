//! Project CRUD endpoints.
//!
//! A project bundles a git source, build configuration, registry credential
//! reference, linked deployments, and a default environment.
//!
//! Read endpoints accept any authenticated actor; mutating endpoints require
//! the `admin` role. Deletion cascade-removes deployment links.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;
use crate::storage::{BuildKind, ProjectStorage, StoredProject};

/// State for project endpoints.
///
/// In addition to the project store, this state optionally carries a
/// [`GitCredentialStore`] for resolving git credentials at pull time, and
/// a root directory under which each project's working copy is cloned.
/// Handlers that do not require git access (list, create, get, …) ignore
/// those fields; the pull endpoint short-circuits to anonymous auth when
/// `git_creds` is `None`.
#[derive(Clone)]
pub struct ProjectState {
    /// Underlying project storage backend.
    pub store: Arc<dyn ProjectStorage>,
    /// Optional Git credential store for resolving `git_credential_id`
    /// references on pull. When `None`, only anonymous pulls are possible.
    pub git_creds: Option<
        Arc<zlayer_secrets::GitCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>>,
    >,
    /// Root directory for cloned project workspaces. Each project clones
    /// to `{clone_root}/{project_id}`.
    pub clone_root: std::path::PathBuf,
}

impl ProjectState {
    /// Build a new state from a project store alone.
    ///
    /// Git credentials are left unset and the clone root defaults to a
    /// subdirectory of the system temp dir. Use [`ProjectState::with_git`]
    /// when you need functional pulls with authentication.
    #[must_use]
    pub fn new(store: Arc<dyn ProjectStorage>) -> Self {
        Self {
            store,
            git_creds: None,
            clone_root: std::env::temp_dir().join("zlayer-projects"),
        }
    }

    /// Build a state wired up for git pulls.
    ///
    /// `git_creds` resolves `StoredProject::git_credential_id` references
    /// to their underlying PAT / SSH key. `clone_root` is the parent dir
    /// for per-project working copies.
    #[must_use]
    pub fn with_git(
        store: Arc<dyn ProjectStorage>,
        git_creds: Arc<
            zlayer_secrets::GitCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>,
        >,
        clone_root: std::path::PathBuf,
    ) -> Self {
        Self {
            store,
            git_creds: Some(git_creds),
            clone_root,
        }
    }
}

// ---- Request/response types ----

/// Body for `POST /api/v1/projects`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LinkDeploymentRequest {
    /// Name of the deployment to link.
    pub deployment_name: String,
}

/// Response body for `POST /api/v1/projects/{id}/pull`.
#[derive(Debug, Serialize, ToSchema)]
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

// ---- Endpoints ----

/// List all projects.
///
/// Any authenticated user can list projects.
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the project store fails.
#[utoipa::path(
    get,
    path = "/api/v1/projects",
    responses(
        (status = 200, description = "List of projects", body = Vec<StoredProject>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Projects"
)]
pub async fn list_projects(
    _actor: AuthActor,
    State(state): State<ProjectState>,
) -> Result<Json<Vec<StoredProject>>> {
    let projects = state
        .store
        .list()
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?;
    Ok(Json(projects))
}

/// Create a new project. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::BadRequest`] for an empty name,
/// [`ApiError::Conflict`] when the name is already used, or
/// [`ApiError::Internal`] when the project store fails.
#[utoipa::path(
    post,
    path = "/api/v1/projects",
    request_body = CreateProjectRequest,
    responses(
        (status = 201, description = "Project created", body = StoredProject),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
        (status = 409, description = "Name already used"),
    ),
    security(("bearer_auth" = [])),
    tag = "Projects"
)]
pub async fn create_project(
    actor: AuthActor,
    State(state): State<ProjectState>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<StoredProject>)> {
    actor.require_admin()?;

    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest(
            "Project name cannot be empty".to_string(),
        ));
    }
    if name.len() > 256 {
        return Err(ApiError::BadRequest(
            "Project name cannot exceed 256 characters".to_string(),
        ));
    }

    // Conflict preflight.
    if state
        .store
        .get_by_name(name)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Project '{name}' already exists"
        )));
    }

    let mut project = StoredProject::new(name);
    project.description = req
        .description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());
    project.git_url = req.git_url;
    if let Some(branch) = req.git_branch {
        project.git_branch = Some(branch);
    }
    project.git_credential_id = req.git_credential_id;
    project.build_kind = req.build_kind;
    project.build_path = req.build_path;
    project.deploy_spec_path = req.deploy_spec_path.and_then(|p| {
        let trimmed = p.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });
    project.registry_credential_id = req.registry_credential_id;
    project.default_environment_id = req.default_environment_id;
    if let Some(auto) = req.auto_deploy {
        project.auto_deploy = auto;
    }
    project.poll_interval_secs = req.poll_interval_secs;
    project.owner_id = Some(actor.user_id.clone());

    state
        .store
        .store(&project)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?;

    Ok((StatusCode::CREATED, Json(project)))
}

/// Fetch a single project by id.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] if no project with the given id exists,
/// or [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{id}",
    params(("id" = String, Path, description = "Project id")),
    responses(
        (status = 200, description = "Project", body = StoredProject),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Projects"
)]
pub async fn get_project(
    _actor: AuthActor,
    State(state): State<ProjectState>,
    Path(id): Path<String>,
) -> Result<Json<StoredProject>> {
    let project = state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Project {id} not found")))?;
    Ok(Json(project))
}

/// Update a project. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the project does not exist, [`ApiError::BadRequest`] for an empty
/// replacement name, [`ApiError::Conflict`] when the new name collides, or
/// [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    patch,
    path = "/api/v1/projects/{id}",
    params(("id" = String, Path, description = "Project id")),
    request_body = UpdateProjectRequest,
    responses(
        (status = 200, description = "Updated project", body = StoredProject),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Name collides with another project"),
    ),
    security(("bearer_auth" = [])),
    tag = "Projects"
)]
#[allow(clippy::too_many_lines)]
pub async fn update_project(
    actor: AuthActor,
    State(state): State<ProjectState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateProjectRequest>,
) -> Result<Json<StoredProject>> {
    actor.require_admin()?;

    let mut project = state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Project {id} not found")))?;

    if let Some(new_name) = req.name {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Err(ApiError::BadRequest(
                "Project name cannot be empty".to_string(),
            ));
        }
        if trimmed.len() > 256 {
            return Err(ApiError::BadRequest(
                "Project name cannot exceed 256 characters".to_string(),
            ));
        }
        if trimmed != project.name {
            if let Some(existing) = state
                .store
                .get_by_name(trimmed)
                .await
                .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?
            {
                if existing.id != project.id {
                    return Err(ApiError::Conflict(format!(
                        "Project '{trimmed}' already exists"
                    )));
                }
            }
            project.name = trimmed.to_string();
        }
    }

    if let Some(desc) = req.description {
        let trimmed = desc.trim();
        project.description = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }

    if let Some(url) = req.git_url {
        project.git_url = if url.trim().is_empty() {
            None
        } else {
            Some(url)
        };
    }
    if let Some(branch) = req.git_branch {
        project.git_branch = if branch.trim().is_empty() {
            None
        } else {
            Some(branch)
        };
    }
    if let Some(cred_id) = req.git_credential_id {
        project.git_credential_id = if cred_id.trim().is_empty() {
            None
        } else {
            Some(cred_id)
        };
    }
    if let Some(kind) = req.build_kind {
        project.build_kind = Some(kind);
    }
    if let Some(path) = req.build_path {
        project.build_path = if path.trim().is_empty() {
            None
        } else {
            Some(path)
        };
    }
    if let Some(spec_path) = req.deploy_spec_path {
        project.deploy_spec_path = if spec_path.trim().is_empty() {
            None
        } else {
            Some(spec_path.trim().to_string())
        };
    }
    if let Some(reg_id) = req.registry_credential_id {
        project.registry_credential_id = if reg_id.trim().is_empty() {
            None
        } else {
            Some(reg_id)
        };
    }
    if let Some(env_id) = req.default_environment_id {
        project.default_environment_id = if env_id.trim().is_empty() {
            None
        } else {
            Some(env_id)
        };
    }
    if let Some(auto) = req.auto_deploy {
        project.auto_deploy = auto;
    }
    if let Some(interval) = req.poll_interval_secs {
        project.poll_interval_secs = interval;
    }

    project.updated_at = chrono::Utc::now();

    state
        .store
        .store(&project)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?;

    Ok(Json(project))
}

/// Delete a project. Admin only. Cascade-removes deployment links.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the project does not exist, or [`ApiError::Internal`] when the
/// store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{id}",
    params(("id" = String, Path, description = "Project id")),
    responses(
        (status = 204, description = "Project deleted"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Projects"
)]
pub async fn delete_project(
    actor: AuthActor,
    State(state): State<ProjectState>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    let deleted = state
        .store
        .delete(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?;

    if !deleted {
        return Err(ApiError::NotFound(format!("Project {id} not found")));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// List deployment names linked to a project.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] when the project does not exist,
/// or [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{id}/deployments",
    params(("id" = String, Path, description = "Project id")),
    responses(
        (status = 200, description = "Linked deployment names", body = Vec<String>),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Projects"
)]
pub async fn list_project_deployments(
    _actor: AuthActor,
    State(state): State<ProjectState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<String>>> {
    // Verify the project exists.
    state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Project {id} not found")))?;

    let names = state
        .store
        .list_deployments(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?;

    Ok(Json(names))
}

/// Link a deployment to a project.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] when the project does not exist,
/// [`ApiError::BadRequest`] for an empty deployment name, or
/// [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{id}/deployments",
    params(("id" = String, Path, description = "Project id")),
    request_body = LinkDeploymentRequest,
    responses(
        (status = 201, description = "Deployment linked"),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Projects"
)]
pub async fn link_project_deployment(
    _actor: AuthActor,
    State(state): State<ProjectState>,
    Path(id): Path<String>,
    Json(req): Json<LinkDeploymentRequest>,
) -> Result<StatusCode> {
    let name = req.deployment_name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest(
            "Deployment name cannot be empty".to_string(),
        ));
    }

    // Verify the project exists.
    state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Project {id} not found")))?;

    state
        .store
        .link_deployment(&id, name)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?;

    Ok(StatusCode::CREATED)
}

/// Unlink a deployment from a project.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] when the project or link does not exist,
/// or [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{id}/deployments/{name}",
    params(
        ("id" = String, Path, description = "Project id"),
        ("name" = String, Path, description = "Deployment name"),
    ),
    responses(
        (status = 204, description = "Deployment unlinked"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Projects"
)]
pub async fn unlink_project_deployment(
    _actor: AuthActor,
    State(state): State<ProjectState>,
    Path((id, name)): Path<(String, String)>,
) -> Result<StatusCode> {
    // Verify the project exists.
    state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Project {id} not found")))?;

    let removed = state
        .store
        .unlink_deployment(&id, &name)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?;

    if !removed {
        return Err(ApiError::NotFound(format!(
            "Deployment link '{name}' not found in project {id}"
        )));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Clone the project's git repository (or fast-forward pull if the working
/// copy already exists) into `{clone_root}/{project_id}` and return the
/// resulting HEAD SHA.
///
/// Authentication is resolved from `git_credential_id`: when set, the
/// matching [`GitCredential`](zlayer_secrets::GitCredential) is looked up in
/// the [`GitCredentialStore`](zlayer_secrets::GitCredentialStore) and its
/// kind determines whether we build a PAT or SSH auth config. When the
/// project has no credential id, or the state has no git credential store
/// attached, we fall back to anonymous auth.
///
/// Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the project does not exist, [`ApiError::BadRequest`] when the
/// project has no `git_url` or its `git_credential_id` cannot be resolved,
/// or [`ApiError::Internal`] on any underlying git / credential store
/// failure.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{id}/pull",
    params(("id" = String, Path, description = "Project id")),
    responses(
        (status = 200, description = "Pull succeeded", body = ProjectPullResponse),
        (status = 400, description = "Project has no git URL configured"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Project not found"),
        (status = 500, description = "Git operation failed"),
    ),
    security(("bearer_auth" = [])),
    tag = "Projects"
)]
pub async fn pull_project(
    actor: AuthActor,
    State(state): State<ProjectState>,
    Path(id): Path<String>,
) -> Result<Json<ProjectPullResponse>> {
    actor.require_admin()?;

    let project = state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Project {id} not found")))?;

    let git_url = project
        .git_url
        .clone()
        .ok_or_else(|| ApiError::BadRequest("Project has no git_url configured".to_string()))?;
    let branch = project
        .git_branch
        .clone()
        .unwrap_or_else(|| "main".to_string());

    let outcome =
        ensure_project_clone(&project, &state.clone_root, state.git_creds.as_ref()).await?;

    Ok(Json(ProjectPullResponse {
        project_id: project.id,
        git_url,
        branch,
        sha: outcome.sha,
        path: outcome.path.to_string_lossy().to_string(),
    }))
}

/// Outcome of [`ensure_project_clone`] — the absolute on-disk path to the
/// working copy and the HEAD SHA after the clone / fast-forward.
#[derive(Debug, Clone)]
pub(crate) struct ClonedProject {
    /// Absolute path to the project's working copy (`{clone_root}/{project_id}`).
    pub path: std::path::PathBuf,
    /// HEAD SHA after the clone / fast-forward pull.
    pub sha: String,
}

/// Ensure the project's git working copy exists on disk and is fast-forwarded
/// to the latest commit on the configured branch. Fresh clones are placed in
/// `{clone_root}/{project_id}`; existing clones are pulled in place.
///
/// Authentication is resolved from the project's `git_credential_id` when both
/// the id and a credential store are available — otherwise the pull falls back
/// to anonymous auth.
///
/// This is shared between the HTTP `/projects/{id}/pull` handler and the
/// workflow `BuildProject` action so both paths produce identical side
/// effects.
///
/// # Errors
///
/// Returns [`ApiError::BadRequest`] when the project has no `git_url` or
/// its `git_credential_id` cannot be resolved, or [`ApiError::Internal`]
/// on any underlying git / credential store failure.
pub(crate) async fn ensure_project_clone(
    project: &StoredProject,
    clone_root: &std::path::Path,
    git_creds: Option<
        &Arc<zlayer_secrets::GitCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>>,
    >,
) -> Result<ClonedProject> {
    let git_url = project
        .git_url
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("Project has no git_url configured".to_string()))?;
    let branch = project.git_branch.as_deref().unwrap_or("main");

    // Resolve auth. If the project pins a credential id *and* we have a
    // credential store, fetch both metadata and value; otherwise fall back
    // to anonymous.
    let auth = match (&project.git_credential_id, git_creds) {
        (Some(cred_id), Some(git_store)) => {
            let cred = git_store
                .get(cred_id)
                .await
                .map_err(|e| ApiError::Internal(format!("Credential lookup: {e}")))?
                .ok_or_else(|| {
                    ApiError::BadRequest(format!("Git credential {cred_id} not found"))
                })?;
            let value = git_store
                .get_value(cred_id)
                .await
                .map_err(|e| ApiError::Internal(format!("Credential value: {e}")))?;
            match cred.kind {
                zlayer_secrets::GitCredentialKind::Pat => {
                    zlayer_git::GitAuth::pat(&cred.name, value.expose())
                }
                zlayer_secrets::GitCredentialKind::SshKey => {
                    zlayer_git::GitAuth::ssh_key(value.expose())
                }
            }
        }
        _ => zlayer_git::GitAuth::anonymous(),
    };

    let dest = clone_root.join(&project.id);
    let sha = if dest.exists() {
        zlayer_git::pull_ff(&dest, branch, &auth)
            .await
            .map_err(|e| ApiError::Internal(format!("Git pull failed: {e}")))?
    } else {
        // Ensure the parent of `dest` exists so git can create the clone
        // directory. `git clone` itself refuses to create intermediate
        // directories.
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::Internal(format!("Failed to create clone root: {e}")))?;
        }
        zlayer_git::clone_repo(git_url, branch, &auth, &dest)
            .await
            .map_err(|e| ApiError::Internal(format!("Git clone failed: {e}")))?
    };

    Ok(ClonedProject { path: dest, sha })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_project_request_deserialize_minimum() {
        let req: CreateProjectRequest = serde_json::from_str(r#"{"name":"my-app"}"#).unwrap();
        assert_eq!(req.name, "my-app");
        assert!(req.description.is_none());
        assert!(req.git_url.is_none());
        assert!(req.build_kind.is_none());
    }

    #[test]
    fn test_create_project_request_deserialize_full() {
        let req: CreateProjectRequest = serde_json::from_str(
            r#"{
                "name": "my-app",
                "description": "My application",
                "git_url": "https://github.com/user/repo",
                "git_branch": "develop",
                "git_credential_id": "cred-1",
                "build_kind": "dockerfile",
                "build_path": "./Dockerfile",
                "registry_credential_id": "reg-1",
                "default_environment_id": "env-1"
            }"#,
        )
        .unwrap();
        assert_eq!(req.name, "my-app");
        assert_eq!(req.description.as_deref(), Some("My application"));
        assert_eq!(req.git_url.as_deref(), Some("https://github.com/user/repo"));
        assert_eq!(req.git_branch.as_deref(), Some("develop"));
        assert_eq!(req.build_kind, Some(BuildKind::Dockerfile));
    }

    #[test]
    fn test_update_project_request_deserialize_partial() {
        let req: UpdateProjectRequest = serde_json::from_str("{}").unwrap();
        assert!(req.name.is_none());
        assert!(req.description.is_none());
        assert!(req.git_url.is_none());

        let req: UpdateProjectRequest = serde_json::from_str(r#"{"name":"renamed"}"#).unwrap();
        assert_eq!(req.name.as_deref(), Some("renamed"));
        assert!(req.description.is_none());
    }

    #[test]
    fn test_link_deployment_request_deserialize() {
        let req: LinkDeploymentRequest =
            serde_json::from_str(r#"{"deployment_name":"my-deploy"}"#).unwrap();
        assert_eq!(req.deployment_name, "my-deploy");
    }
}
