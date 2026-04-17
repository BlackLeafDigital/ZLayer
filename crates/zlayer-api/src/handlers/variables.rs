//! Variable CRUD endpoints.
//!
//! Variables are plaintext key-value pairs for template substitution in
//! deployment specs. Unlike secrets, variable values are NOT encrypted and
//! are fully visible in API responses.
//!
//! Variables can be global (`scope = None`) or project-scoped
//! (`scope = Some(project_id)`).
//!
//! Read endpoints accept any authenticated actor; mutating endpoints require
//! the `admin` role.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;
use crate::storage::{StoredVariable, VariableStorage};

/// State for variable endpoints.
#[derive(Clone)]
pub struct VariableState {
    /// Underlying variable storage backend.
    pub store: Arc<dyn VariableStorage>,
}

impl VariableState {
    /// Build a new state from a variable store.
    #[must_use]
    pub fn new(store: Arc<dyn VariableStorage>) -> Self {
        Self { store }
    }
}

// ---- Request/response types ----

/// Query for `GET /api/v1/variables`.
///
/// `scope` selects the namespace:
///   - omitted -> list global variables only (`scope IS NULL`).
///   - any other value -> list variables belonging to that project/scope id.
#[derive(Debug, Deserialize, Default)]
pub struct ListVariablesQuery {
    #[serde(default)]
    pub scope: Option<String>,
}

/// Body for `POST /api/v1/variables`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateVariableRequest {
    /// Variable name (e.g. `"APP_VERSION"`). Must be unique within the chosen
    /// scope.
    pub name: String,
    /// Plaintext value.
    pub value: String,
    /// Project id scope. `None` = global variable.
    #[serde(default)]
    pub scope: Option<String>,
}

/// Body for `PATCH /api/v1/variables/{id}`. All fields are optional.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateVariableRequest {
    /// New variable name. Will be re-checked for uniqueness.
    #[serde(default)]
    pub name: Option<String>,
    /// New value.
    #[serde(default)]
    pub value: Option<String>,
}

// ---- Endpoints ----

/// List variables.
///
/// Filter rules (see [`ListVariablesQuery`]):
///
/// - `?scope={id}` -> variables belonging to that scope.
/// - no `scope` query -> only global variables (`scope IS NULL`).
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the variable store fails.
#[utoipa::path(
    get,
    path = "/api/v1/variables",
    params(
        ("scope" = Option<String>, Query,
            description = "Scope (project id) to filter by; omit for globals only"),
    ),
    responses(
        (status = 200, description = "List of variables", body = Vec<StoredVariable>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Variables"
)]
pub async fn list_variables(
    _actor: AuthActor,
    State(state): State<VariableState>,
    Query(query): Query<ListVariablesQuery>,
) -> Result<Json<Vec<StoredVariable>>> {
    let vars = state
        .store
        .list(query.scope.as_deref())
        .await
        .map_err(|e| ApiError::Internal(format!("Variable store: {e}")))?;
    Ok(Json(vars))
}

/// Create a new variable. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::BadRequest`] for an empty name,
/// [`ApiError::Conflict`] when the `(name, scope)` is already used, or
/// [`ApiError::Internal`] when the variable store fails.
#[utoipa::path(
    post,
    path = "/api/v1/variables",
    request_body = CreateVariableRequest,
    responses(
        (status = 201, description = "Variable created", body = StoredVariable),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
        (status = 409, description = "Name already used in the given scope"),
    ),
    security(("bearer_auth" = [])),
    tag = "Variables"
)]
pub async fn create_variable(
    actor: AuthActor,
    State(state): State<VariableState>,
    Json(req): Json<CreateVariableRequest>,
) -> Result<(StatusCode, Json<StoredVariable>)> {
    actor.require_admin()?;

    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest(
            "Variable name cannot be empty".to_string(),
        ));
    }
    if name.len() > 256 {
        return Err(ApiError::BadRequest(
            "Variable name cannot exceed 256 characters".to_string(),
        ));
    }

    // Conflict preflight
    if state
        .store
        .get_by_name(name, req.scope.as_deref())
        .await
        .map_err(|e| ApiError::Internal(format!("Variable store: {e}")))?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Variable '{name}' already exists in this scope"
        )));
    }

    let var = StoredVariable::new(name, &req.value, req.scope);

    state
        .store
        .store(&var)
        .await
        .map_err(|e| ApiError::Internal(format!("Variable store: {e}")))?;

    Ok((StatusCode::CREATED, Json(var)))
}

/// Fetch a single variable by id.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] if no variable with the given id exists,
/// or [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    get,
    path = "/api/v1/variables/{id}",
    params(("id" = String, Path, description = "Variable id")),
    responses(
        (status = 200, description = "Variable", body = StoredVariable),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Variables"
)]
pub async fn get_variable(
    _actor: AuthActor,
    State(state): State<VariableState>,
    Path(id): Path<String>,
) -> Result<Json<StoredVariable>> {
    let var = state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Variable store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Variable {id} not found")))?;
    Ok(Json(var))
}

/// Update a variable's name and/or value. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the variable does not exist, [`ApiError::BadRequest`] for an
/// empty replacement name, [`ApiError::Conflict`] when the new name collides
/// with another variable in the same scope, or [`ApiError::Internal`]
/// when the store fails.
#[utoipa::path(
    patch,
    path = "/api/v1/variables/{id}",
    params(("id" = String, Path, description = "Variable id")),
    request_body = UpdateVariableRequest,
    responses(
        (status = 200, description = "Updated variable", body = StoredVariable),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Name collides with another variable"),
    ),
    security(("bearer_auth" = [])),
    tag = "Variables"
)]
pub async fn update_variable(
    actor: AuthActor,
    State(state): State<VariableState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateVariableRequest>,
) -> Result<Json<StoredVariable>> {
    actor.require_admin()?;

    let mut var = state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Variable store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Variable {id} not found")))?;

    if let Some(new_name) = req.name {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Err(ApiError::BadRequest(
                "Variable name cannot be empty".to_string(),
            ));
        }
        if trimmed.len() > 256 {
            return Err(ApiError::BadRequest(
                "Variable name cannot exceed 256 characters".to_string(),
            ));
        }
        if trimmed != var.name {
            // Make sure the new name doesn't collide with a different var in
            // the same scope.
            if let Some(existing) = state
                .store
                .get_by_name(trimmed, var.scope.as_deref())
                .await
                .map_err(|e| ApiError::Internal(format!("Variable store: {e}")))?
            {
                if existing.id != var.id {
                    return Err(ApiError::Conflict(format!(
                        "Variable '{trimmed}' already exists in this scope"
                    )));
                }
            }
            var.name = trimmed.to_string();
        }
    }

    if let Some(new_value) = req.value {
        var.value = new_value;
    }

    var.updated_at = chrono::Utc::now();

    state
        .store
        .store(&var)
        .await
        .map_err(|e| ApiError::Internal(format!("Variable store: {e}")))?;

    Ok(Json(var))
}

/// Delete a variable. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the variable does not exist, or [`ApiError::Internal`] when the
/// store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/variables/{id}",
    params(("id" = String, Path, description = "Variable id")),
    responses(
        (status = 204, description = "Variable deleted"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Variables"
)]
pub async fn delete_variable(
    actor: AuthActor,
    State(state): State<VariableState>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    let deleted = state
        .store
        .delete(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Variable store: {e}")))?;

    if !deleted {
        return Err(ApiError::NotFound(format!("Variable {id} not found")));
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_request_deserialize_minimum() {
        let req: CreateVariableRequest =
            serde_json::from_str(r#"{"name":"FOO","value":"bar"}"#).unwrap();
        assert_eq!(req.name, "FOO");
        assert_eq!(req.value, "bar");
        assert!(req.scope.is_none());
    }

    #[test]
    fn test_create_request_deserialize_full() {
        let req: CreateVariableRequest =
            serde_json::from_str(r#"{"name":"FOO","value":"bar","scope":"proj-1"}"#).unwrap();
        assert_eq!(req.name, "FOO");
        assert_eq!(req.value, "bar");
        assert_eq!(req.scope.as_deref(), Some("proj-1"));
    }

    #[test]
    fn test_update_request_deserialize_partial() {
        let req: UpdateVariableRequest = serde_json::from_str("{}").unwrap();
        assert!(req.name.is_none());
        assert!(req.value.is_none());

        let req: UpdateVariableRequest = serde_json::from_str(r#"{"value":"new"}"#).unwrap();
        assert!(req.name.is_none());
        assert_eq!(req.value.as_deref(), Some("new"));
    }

    #[test]
    fn test_list_query_default_is_empty() {
        let q = ListVariablesQuery::default();
        assert!(q.scope.is_none());
    }

    #[test]
    fn test_list_query_construct_with_scope() {
        let q = ListVariablesQuery {
            scope: Some("p1".to_string()),
        };
        assert_eq!(q.scope.as_deref(), Some("p1"));
    }
}
