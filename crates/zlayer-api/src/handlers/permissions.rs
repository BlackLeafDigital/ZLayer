//! Permission grant/revoke/list endpoints.
//!
//! Routes:
//!
//! ```text
//! GET    /api/v1/permissions?user={id}|group={id}  -> list for subject
//! POST   /api/v1/permissions                       -> grant (admin)
//! DELETE /api/v1/permissions/{id}                   -> revoke (admin)
//! ```
//!
//! Read endpoints accept any authenticated actor; mutating endpoints
//! (grant, revoke) require the `admin` role.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;
use crate::storage::{
    GroupStorage, PermissionLevel, PermissionStorage, StoredPermission, SubjectKind,
};

/// State for permission endpoints.
#[derive(Clone)]
pub struct PermissionsState {
    /// Underlying permission storage backend.
    pub store: Arc<dyn PermissionStorage>,
    /// Group storage backend (for verifying group existence).
    pub group_store: Arc<dyn GroupStorage>,
}

impl PermissionsState {
    /// Build a new state from permission and group stores.
    #[must_use]
    pub fn new(store: Arc<dyn PermissionStorage>, group_store: Arc<dyn GroupStorage>) -> Self {
        Self { store, group_store }
    }
}

// ---- Request/response types ----

/// Query parameters for `GET /api/v1/permissions`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListPermissionsQuery {
    /// Filter by user id.
    #[serde(default)]
    pub user: Option<String>,
    /// Filter by group id.
    #[serde(default)]
    pub group: Option<String>,
}

/// Body for `POST /api/v1/permissions`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GrantPermissionRequest {
    /// Whether the subject is a user or a group.
    pub subject_kind: SubjectKind,
    /// The user or group id.
    pub subject_id: String,
    /// The kind of resource (e.g. `"deployment"`, `"project"`, `"secret"`).
    pub resource_kind: String,
    /// A specific resource id, or omit for a wildcard (all resources of that kind).
    #[serde(default)]
    pub resource_id: Option<String>,
    /// The access level to grant.
    pub level: PermissionLevel,
}

// ---- Endpoints ----

/// List permissions for a subject (user or group).
///
/// Exactly one of `user` or `group` must be provided.
///
/// # Errors
///
/// Returns [`ApiError::BadRequest`] when neither or both query parameters are
/// provided, or [`ApiError::Internal`] if the store fails.
#[utoipa::path(
    get,
    path = "/api/v1/permissions",
    params(ListPermissionsQuery),
    responses(
        (status = 200, description = "List permissions", body = Vec<StoredPermission>),
        (status = 400, description = "Must provide exactly one of user or group"),
    ),
    tag = "Permissions"
)]
pub async fn list_permissions(
    _actor: AuthActor,
    State(state): State<PermissionsState>,
    Query(q): Query<ListPermissionsQuery>,
) -> Result<Json<Vec<StoredPermission>>> {
    let (kind, id) = match (q.user, q.group) {
        (Some(uid), None) => (SubjectKind::User, uid),
        (None, Some(gid)) => (SubjectKind::Group, gid),
        _ => {
            return Err(ApiError::BadRequest(
                "Provide exactly one of ?user=<id> or ?group=<id>".to_string(),
            ));
        }
    };

    let perms = state
        .store
        .list_for_subject(kind, &id)
        .await
        .map_err(|e| ApiError::Internal(format!("Permission store: {e}")))?;

    Ok(Json(perms))
}

/// Grant a permission. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::BadRequest`] when required fields are missing, or
/// [`ApiError::Internal`] if the store fails.
#[utoipa::path(
    post,
    path = "/api/v1/permissions",
    request_body = GrantPermissionRequest,
    responses(
        (status = 201, description = "Permission granted", body = StoredPermission),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
    ),
    tag = "Permissions"
)]
pub async fn grant_permission(
    actor: AuthActor,
    State(state): State<PermissionsState>,
    Json(req): Json<GrantPermissionRequest>,
) -> Result<(StatusCode, Json<StoredPermission>)> {
    actor.require_admin()?;

    if req.subject_id.trim().is_empty() {
        return Err(ApiError::BadRequest("subject_id is required".to_string()));
    }
    if req.resource_kind.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "resource_kind is required".to_string(),
        ));
    }

    // Verify group exists when granting to a group.
    if req.subject_kind == SubjectKind::Group {
        let group = state
            .group_store
            .get(&req.subject_id)
            .await
            .map_err(|e| ApiError::Internal(format!("Group store: {e}")))?;
        if group.is_none() {
            return Err(ApiError::NotFound(format!(
                "Group {} not found",
                req.subject_id
            )));
        }
    }

    let perm = StoredPermission::new(
        req.subject_kind,
        req.subject_id,
        req.resource_kind,
        req.resource_id,
        req.level,
    );

    state
        .store
        .grant(&perm)
        .await
        .map_err(|e| ApiError::Internal(format!("Permission store: {e}")))?;

    Ok((StatusCode::CREATED, Json(perm)))
}

/// Revoke a permission by id. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::NotFound`] when the permission does not exist, or
/// [`ApiError::Internal`] if the store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/permissions/{id}",
    params(("id" = String, Path, description = "Permission id")),
    responses(
        (status = 204, description = "Permission revoked"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Permission not found"),
    ),
    tag = "Permissions"
)]
pub async fn revoke_permission(
    actor: AuthActor,
    State(state): State<PermissionsState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    let revoked = state
        .store
        .revoke(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Permission store: {e}")))?;

    if !revoked {
        return Err(ApiError::NotFound(format!("Permission {id} not found")));
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grant_permission_request_deserialize() {
        let json = r#"{
            "subject_kind": "user",
            "subject_id": "u-1",
            "resource_kind": "deployment",
            "resource_id": "d-1",
            "level": "write"
        }"#;
        let req: GrantPermissionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.subject_kind, SubjectKind::User);
        assert_eq!(req.subject_id, "u-1");
        assert_eq!(req.resource_kind, "deployment");
        assert_eq!(req.resource_id.as_deref(), Some("d-1"));
        assert_eq!(req.level, PermissionLevel::Write);
    }

    #[test]
    fn test_grant_permission_request_wildcard() {
        let json = r#"{
            "subject_kind": "group",
            "subject_id": "g-1",
            "resource_kind": "project",
            "level": "read"
        }"#;
        let req: GrantPermissionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.subject_kind, SubjectKind::Group);
        assert!(req.resource_id.is_none());
        assert_eq!(req.level, PermissionLevel::Read);
    }

    #[test]
    fn test_list_permissions_query_user() {
        let q: ListPermissionsQuery = serde_json::from_str(r#"{"user": "u-1"}"#).unwrap();
        assert_eq!(q.user.as_deref(), Some("u-1"));
        assert!(q.group.is_none());
    }

    #[test]
    fn test_list_permissions_query_group() {
        let q: ListPermissionsQuery = serde_json::from_str(r#"{"group": "g-1"}"#).unwrap();
        assert!(q.user.is_none());
        assert_eq!(q.group.as_deref(), Some("g-1"));
    }
}
