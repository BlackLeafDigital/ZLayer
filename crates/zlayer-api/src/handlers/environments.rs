//! Environment CRUD endpoints.
//!
//! An environment is an isolated namespace for secrets (and, eventually,
//! deployments). Optionally belongs to a project — when `project_id` is
//! `None`, the environment is global.
//!
//! Read endpoints accept any authenticated actor; mutating endpoints require
//! the `admin` role. Deletion is non-cascading: the operator must purge
//! secrets explicitly via `DELETE /api/v1/secrets?env={id}&all=true` before
//! an environment with secrets can be removed.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{ApiError, Result};
use crate::handlers::secrets::{env_scope, SecretsState};
use crate::handlers::users::AuthActor;
use crate::storage::{EnvironmentStorage, StoredEnvironment};

/// State for environment endpoints.
#[derive(Clone)]
pub struct EnvironmentsState {
    /// Underlying environment storage backend.
    pub store: Arc<dyn EnvironmentStorage>,
}

impl EnvironmentsState {
    /// Build a new state from an environment store.
    #[must_use]
    pub fn new(store: Arc<dyn EnvironmentStorage>) -> Self {
        Self { store }
    }
}

/// Composite state used by the environments router. Carries the environment
/// store plus the secrets state so cascade-delete safety checks can count
/// secrets attached to an environment without a second store handle.
#[derive(Clone)]
pub struct EnvironmentsRouterState {
    pub envs: EnvironmentsState,
    pub secrets: SecretsState,
}

impl EnvironmentsRouterState {
    /// Build a router state from the two backing stores.
    #[must_use]
    pub fn new(envs: EnvironmentsState, secrets: SecretsState) -> Self {
        Self { envs, secrets }
    }
}

// ---- Request/response types ----

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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateEnvironmentRequest {
    /// New display name. Will be re-checked for uniqueness.
    #[serde(default)]
    pub name: Option<String>,
    /// New description. Pass `Some("")` to clear, omit to leave unchanged.
    #[serde(default)]
    pub description: Option<String>,
}

// ---- Endpoints ----

/// List environments.
///
/// Filter rules (see [`ListEnvironmentsQuery`]):
///
/// - `?project={id}` → environments belonging to that project.
/// - `?project=*` → every environment across every project + globals,
///   merged and sorted by name.
/// - no `project` query → only global environments (`project_id IS NULL`).
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the environment store fails.
#[utoipa::path(
    get,
    path = "/api/v1/environments",
    params(
        ("project" = Option<String>, Query,
            description = "Project id to filter by; '*' lists all; omit for globals only"),
    ),
    responses(
        (status = 200, description = "List of environments", body = Vec<StoredEnvironment>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Environments"
)]
pub async fn list_environments(
    _actor: AuthActor,
    State(state): State<EnvironmentsRouterState>,
    Query(query): Query<ListEnvironmentsQuery>,
) -> Result<Json<Vec<StoredEnvironment>>> {
    let envs = match query.project.as_deref() {
        Some("*") => list_all_environments(&*state.envs.store).await?,
        Some(pid) => state
            .envs
            .store
            .list(Some(pid))
            .await
            .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?,
        None => state
            .envs
            .store
            .list(None)
            .await
            .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?,
    };
    Ok(Json(envs))
}

/// Create a new environment. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::BadRequest`] for an empty name,
/// [`ApiError::Conflict`] when the `(name, project_id)` is already used, or
/// [`ApiError::Internal`] when the environment store fails.
#[utoipa::path(
    post,
    path = "/api/v1/environments",
    request_body = CreateEnvironmentRequest,
    responses(
        (status = 201, description = "Environment created", body = StoredEnvironment),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
        (status = 409, description = "Name already used in the given project"),
    ),
    security(("bearer_auth" = [])),
    tag = "Environments"
)]
pub async fn create_environment(
    actor: AuthActor,
    State(state): State<EnvironmentsRouterState>,
    Json(req): Json<CreateEnvironmentRequest>,
) -> Result<(StatusCode, Json<StoredEnvironment>)> {
    actor.require_admin()?;

    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest(
            "Environment name cannot be empty".to_string(),
        ));
    }
    if name.len() > 256 {
        return Err(ApiError::BadRequest(
            "Environment name cannot exceed 256 characters".to_string(),
        ));
    }

    // Conflict preflight — the storage layer also enforces this, but a
    // 409 is friendlier than mapping a Database error to 500.
    if state
        .envs
        .store
        .get_by_name(name, req.project_id.as_deref())
        .await
        .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Environment '{name}' already exists in this project scope"
        )));
    }

    let mut env = StoredEnvironment::new(name, req.project_id);
    env.description = req
        .description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());

    state
        .envs
        .store
        .store(&env)
        .await
        .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?;

    Ok((StatusCode::CREATED, Json(env)))
}

/// Fetch a single environment by id.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] if no environment with the given id exists,
/// or [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    get,
    path = "/api/v1/environments/{id}",
    params(("id" = String, Path, description = "Environment id")),
    responses(
        (status = 200, description = "Environment", body = StoredEnvironment),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Environments"
)]
pub async fn get_environment(
    _actor: AuthActor,
    State(state): State<EnvironmentsRouterState>,
    Path(id): Path<String>,
) -> Result<Json<StoredEnvironment>> {
    let env = state
        .envs
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Environment {id} not found")))?;
    Ok(Json(env))
}

/// Rename / re-describe an environment. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the environment does not exist, [`ApiError::BadRequest`] for an
/// empty replacement name, [`ApiError::Conflict`] when the new name collides
/// with another environment in the same scope, or [`ApiError::Internal`]
/// when the store fails.
#[utoipa::path(
    patch,
    path = "/api/v1/environments/{id}",
    params(("id" = String, Path, description = "Environment id")),
    request_body = UpdateEnvironmentRequest,
    responses(
        (status = 200, description = "Updated environment", body = StoredEnvironment),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Name collides with another environment"),
    ),
    security(("bearer_auth" = [])),
    tag = "Environments"
)]
pub async fn update_environment(
    actor: AuthActor,
    State(state): State<EnvironmentsRouterState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateEnvironmentRequest>,
) -> Result<Json<StoredEnvironment>> {
    actor.require_admin()?;

    let mut env = state
        .envs
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Environment {id} not found")))?;

    if let Some(new_name) = req.name {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Err(ApiError::BadRequest(
                "Environment name cannot be empty".to_string(),
            ));
        }
        if trimmed.len() > 256 {
            return Err(ApiError::BadRequest(
                "Environment name cannot exceed 256 characters".to_string(),
            ));
        }
        if trimmed != env.name {
            // Make sure the new name doesn't collide with a different env in
            // the same project scope.
            if let Some(existing) = state
                .envs
                .store
                .get_by_name(trimmed, env.project_id.as_deref())
                .await
                .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?
            {
                if existing.id != env.id {
                    return Err(ApiError::Conflict(format!(
                        "Environment '{trimmed}' already exists in this project scope"
                    )));
                }
            }
            env.name = trimmed.to_string();
        }
    }

    if let Some(desc) = req.description {
        let trimmed = desc.trim();
        env.description = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }

    env.updated_at = chrono::Utc::now();

    state
        .envs
        .store
        .store(&env)
        .await
        .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?;

    Ok(Json(env))
}

/// Delete an environment. Admin only.
///
/// Non-cascading: the operator must purge any secrets attached to the
/// environment before deletion (typically via
/// `DELETE /api/v1/secrets?env={id}&all=true`). If the environment still
/// has secrets, this endpoint returns 409.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the environment does not exist, [`ApiError::Conflict`] when secrets
/// remain, or [`ApiError::Internal`] when a backing store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/environments/{id}",
    params(("id" = String, Path, description = "Environment id")),
    responses(
        (status = 204, description = "Environment deleted"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Environment still has secrets"),
    ),
    security(("bearer_auth" = [])),
    tag = "Environments"
)]
pub async fn delete_environment(
    actor: AuthActor,
    State(state): State<EnvironmentsRouterState>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    let env = state
        .envs
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Environment {id} not found")))?;

    let scope = env_scope(&env);
    let secret_count = state
        .secrets
        .store
        .list_secrets(&scope)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list secrets: {e}")))?
        .len();

    if secret_count > 0 {
        return Err(ApiError::Conflict(format!(
            "Environment {id} still has {secret_count} secret(s); \
             purge them with DELETE /api/v1/secrets?env={id}&all=true first"
        )));
    }

    let deleted = state
        .envs
        .store
        .delete(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?;

    if !deleted {
        return Err(ApiError::NotFound(format!("Environment {id} not found")));
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---- Helpers ----

/// Collect every environment in the store across all projects + global.
///
/// The current `EnvironmentStorage` trait only supports filtered listing
/// (`list(None)` for globals or `list(Some(pid))` for one project) plus
/// `count()` for the total. There is no `list_all` accessor and no way to
/// enumerate distinct project ids through the trait alone, so a true
/// "list everything" must be served by walking the per-project ids the
/// caller knows about. Since this handler has no source of project ids
/// independent of the store, we list globals and consult `count()` to
/// detect the presence of unenumerable project rows. When project rows
/// exist, we return [`ApiError::ServiceUnavailable`] with a precise
/// explanation rather than silently returning a misleading subset; callers
/// should pass `?project={id}` for the specific project they want.
async fn list_all_environments(store: &dyn EnvironmentStorage) -> Result<Vec<StoredEnvironment>> {
    let mut all = store
        .list(None)
        .await
        .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?;

    let total = store
        .count()
        .await
        .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?;

    let global_len = u64::try_from(all.len()).unwrap_or(u64::MAX);
    if total > global_len {
        return Err(ApiError::ServiceUnavailable(format!(
            "Cannot enumerate {} project-scoped environment(s) without a \
             specific project id; pass ?project={{id}} to list one project",
            total - global_len
        )));
    }

    all.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryEnvironmentStore;

    #[test]
    fn test_create_request_deserialize_minimum() {
        let req: CreateEnvironmentRequest = serde_json::from_str(r#"{"name":"dev"}"#).unwrap();
        assert_eq!(req.name, "dev");
        assert!(req.project_id.is_none());
        assert!(req.description.is_none());
    }

    #[test]
    fn test_create_request_deserialize_full() {
        let req: CreateEnvironmentRequest = serde_json::from_str(
            r#"{"name":"prod","project_id":"proj-1","description":"production"}"#,
        )
        .unwrap();
        assert_eq!(req.name, "prod");
        assert_eq!(req.project_id.as_deref(), Some("proj-1"));
        assert_eq!(req.description.as_deref(), Some("production"));
    }

    #[test]
    fn test_update_request_deserialize_partial() {
        let req: UpdateEnvironmentRequest = serde_json::from_str("{}").unwrap();
        assert!(req.name.is_none());
        assert!(req.description.is_none());

        let req: UpdateEnvironmentRequest = serde_json::from_str(r#"{"name":"renamed"}"#).unwrap();
        assert_eq!(req.name.as_deref(), Some("renamed"));
        assert!(req.description.is_none());
    }

    #[test]
    fn test_list_query_default_is_empty() {
        let q = ListEnvironmentsQuery::default();
        assert!(q.project.is_none());
    }

    #[test]
    fn test_list_query_construct_with_project() {
        let q = ListEnvironmentsQuery {
            project: Some("p1".to_string()),
        };
        assert_eq!(q.project.as_deref(), Some("p1"));
    }

    #[test]
    fn test_list_query_construct_wildcard() {
        let q = ListEnvironmentsQuery {
            project: Some("*".to_string()),
        };
        assert_eq!(q.project.as_deref(), Some("*"));
    }

    #[tokio::test]
    async fn test_list_all_globals_only_returns_them_sorted() {
        let store = InMemoryEnvironmentStore::new();
        store
            .store(&StoredEnvironment::new("prod", None))
            .await
            .unwrap();
        store
            .store(&StoredEnvironment::new("dev", None))
            .await
            .unwrap();

        let all = list_all_environments(&store).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "dev");
        assert_eq!(all[1].name, "prod");
    }

    #[tokio::test]
    async fn test_list_all_with_unreachable_rows_errors() {
        let store = InMemoryEnvironmentStore::new();
        store
            .store(&StoredEnvironment::new("dev", None))
            .await
            .unwrap();
        store
            .store(&StoredEnvironment::new("dev", Some("p1".to_string())))
            .await
            .unwrap();

        // The trait cannot enumerate distinct project ids, so a true
        // "list all" with project rows present must explicitly fail rather
        // than silently dropping them.
        let err = list_all_environments(&store).await.unwrap_err();
        assert!(matches!(err, ApiError::ServiceUnavailable(_)));
    }
}
