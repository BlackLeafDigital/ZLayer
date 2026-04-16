//! User group CRUD and membership endpoints.
//!
//! Routes:
//!
//! ```text
//! GET    /api/v1/groups                      -> list
//! POST   /api/v1/groups                      -> create (admin)
//! GET    /api/v1/groups/{id}                 -> get
//! PATCH  /api/v1/groups/{id}                 -> update name/description (admin)
//! DELETE /api/v1/groups/{id}                 -> delete (admin)
//! POST   /api/v1/groups/{id}/members         -> add member (admin)
//! DELETE /api/v1/groups/{id}/members/{user_id} -> remove member (admin)
//! ```
//!
//! Read endpoints accept any authenticated actor; mutating endpoints
//! (create, update, delete, add/remove member) require the `admin` role.

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
use crate::storage::{GroupStorage, StoredUserGroup};

/// State for group endpoints.
#[derive(Clone)]
pub struct GroupsState {
    /// Underlying group storage backend.
    pub store: Arc<dyn GroupStorage>,
}

impl GroupsState {
    /// Build a new state from a group store.
    #[must_use]
    pub fn new(store: Arc<dyn GroupStorage>) -> Self {
        Self { store }
    }
}

// ---- Request/response types ----

/// Body for `POST /api/v1/groups`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateGroupRequest {
    /// Group name.
    pub name: String,
    /// Free-form description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Body for `PATCH /api/v1/groups/{id}`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateGroupRequest {
    /// Updated name.
    #[serde(default)]
    pub name: Option<String>,
    /// Updated description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Body for `POST /api/v1/groups/{id}/members`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AddMemberRequest {
    /// The user id to add.
    pub user_id: String,
}

/// Response for member list queries.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GroupMembersResponse {
    /// Group id.
    pub group_id: String,
    /// List of user ids in the group.
    pub members: Vec<String>,
}

// ---- Endpoints ----

/// List all groups.
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the group store fails.
#[utoipa::path(
    get,
    path = "/api/v1/groups",
    responses(
        (status = 200, description = "List groups", body = Vec<StoredUserGroup>),
    ),
    tag = "Groups"
)]
pub async fn list_groups(
    _actor: AuthActor,
    State(state): State<GroupsState>,
) -> Result<Json<Vec<StoredUserGroup>>> {
    let groups = state
        .store
        .list()
        .await
        .map_err(|e| ApiError::Internal(format!("Group store: {e}")))?;
    Ok(Json(groups))
}

/// Create a new group. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::BadRequest`] when the name is empty, or
/// [`ApiError::Internal`] if the store fails.
#[utoipa::path(
    post,
    path = "/api/v1/groups",
    request_body = CreateGroupRequest,
    responses(
        (status = 201, description = "Group created", body = StoredUserGroup),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
    ),
    tag = "Groups"
)]
pub async fn create_group(
    actor: AuthActor,
    State(state): State<GroupsState>,
    Json(req): Json<CreateGroupRequest>,
) -> Result<(StatusCode, Json<StoredUserGroup>)> {
    actor.require_admin()?;

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("Group name is required".to_string()));
    }

    let mut group = StoredUserGroup::new(req.name);
    group.description = req.description;

    state
        .store
        .store(&group)
        .await
        .map_err(|e| ApiError::Internal(format!("Group store: {e}")))?;

    Ok((StatusCode::CREATED, Json(group)))
}

/// Fetch a single group by id.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] when no such group exists, or
/// [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    get,
    path = "/api/v1/groups/{id}",
    params(("id" = String, Path, description = "Group id")),
    responses(
        (status = 200, description = "Group", body = StoredUserGroup),
        (status = 404, description = "Not found"),
    ),
    tag = "Groups"
)]
pub async fn get_group(
    _actor: AuthActor,
    State(state): State<GroupsState>,
    Path(id): Path<String>,
) -> Result<Json<StoredUserGroup>> {
    let group = state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Group store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Group {id} not found")))?;
    Ok(Json(group))
}

/// Update a group's name and/or description. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::NotFound`] when the group does not exist, or
/// [`ApiError::Internal`] if the store fails.
#[utoipa::path(
    patch,
    path = "/api/v1/groups/{id}",
    params(("id" = String, Path, description = "Group id")),
    request_body = UpdateGroupRequest,
    responses(
        (status = 200, description = "Updated group", body = StoredUserGroup),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    tag = "Groups"
)]
pub async fn update_group(
    actor: AuthActor,
    State(state): State<GroupsState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateGroupRequest>,
) -> Result<Json<StoredUserGroup>> {
    actor.require_admin()?;

    let mut group = state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Group store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Group {id} not found")))?;

    if let Some(name) = req.name {
        group.name = name;
    }
    if let Some(desc) = req.description {
        group.description = Some(desc);
    }
    group.updated_at = chrono::Utc::now();

    state
        .store
        .store(&group)
        .await
        .map_err(|e| ApiError::Internal(format!("Group store: {e}")))?;

    Ok(Json(group))
}

/// Delete a group. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::NotFound`] when the group does not exist, or
/// [`ApiError::Internal`] if the store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/groups/{id}",
    params(("id" = String, Path, description = "Group id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    tag = "Groups"
)]
pub async fn delete_group(
    actor: AuthActor,
    State(state): State<GroupsState>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    let deleted = state
        .store
        .delete(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Group store: {e}")))?;
    if !deleted {
        return Err(ApiError::NotFound(format!("Group {id} not found")));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Add a member to a group. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::BadRequest`] when `user_id` is empty,
/// [`ApiError::NotFound`] when the group does not exist, or
/// [`ApiError::Internal`] if the store fails.
#[utoipa::path(
    post,
    path = "/api/v1/groups/{id}/members",
    params(("id" = String, Path, description = "Group id")),
    request_body = AddMemberRequest,
    responses(
        (status = 204, description = "Member added"),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Group not found"),
    ),
    tag = "Groups"
)]
pub async fn add_member(
    actor: AuthActor,
    State(state): State<GroupsState>,
    Path(id): Path<String>,
    Json(req): Json<AddMemberRequest>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    if req.user_id.trim().is_empty() {
        return Err(ApiError::BadRequest("user_id is required".to_string()));
    }

    state
        .store
        .add_member(&id, &req.user_id)
        .await
        .map_err(|e| match e {
            crate::storage::StorageError::NotFound(msg) => ApiError::NotFound(msg),
            other => ApiError::Internal(format!("Group store: {other}")),
        })?;

    Ok(StatusCode::NO_CONTENT)
}

/// Remove a member from a group. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::NotFound`] when the group or membership does not exist, or
/// [`ApiError::Internal`] if the store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/groups/{id}/members/{user_id}",
    params(
        ("id" = String, Path, description = "Group id"),
        ("user_id" = String, Path, description = "User id to remove"),
    ),
    responses(
        (status = 204, description = "Member removed"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Group or membership not found"),
    ),
    tag = "Groups"
)]
pub async fn remove_member(
    actor: AuthActor,
    State(state): State<GroupsState>,
    Path((id, user_id)): Path<(String, String)>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    let removed = state
        .store
        .remove_member(&id, &user_id)
        .await
        .map_err(|e| match e {
            crate::storage::StorageError::NotFound(msg) => ApiError::NotFound(msg),
            other => ApiError::Internal(format!("Group store: {other}")),
        })?;

    if !removed {
        return Err(ApiError::NotFound(format!(
            "User {user_id} is not a member of group {id}"
        )));
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_group_request_deserialize() {
        let json = r#"{"name": "developers"}"#;
        let req: CreateGroupRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "developers");
        assert!(req.description.is_none());
    }

    #[test]
    fn test_create_group_request_with_description() {
        let json = r#"{"name": "devs", "description": "Developer team"}"#;
        let req: CreateGroupRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "devs");
        assert_eq!(req.description.as_deref(), Some("Developer team"));
    }

    #[test]
    fn test_update_group_request_partial() {
        let req: UpdateGroupRequest = serde_json::from_str("{}").unwrap();
        assert!(req.name.is_none());
        assert!(req.description.is_none());
    }

    #[test]
    fn test_add_member_request_deserialize() {
        let json = r#"{"user_id": "u-123"}"#;
        let req: AddMemberRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.user_id, "u-123");
    }
}
