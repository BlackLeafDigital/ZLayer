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

pub use zlayer_types::api::permissions::*;

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};

use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;
use crate::storage::{GroupStorage, PermissionStorage, StoredPermission, SubjectKind};

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

/// List permissions granted on a specific resource.
///
/// When `id` is supplied, returns exact-resource grants; when omitted, returns
/// wildcard grants for the given `kind`.
///
/// # Errors
///
/// Returns [`ApiError::Internal`] on store failure.
#[utoipa::path(
    get,
    path = "/api/v1/permissions/by-resource",
    params(ListByResourceQuery),
    responses(
        (status = 200, description = "Grants on the resource", body = Vec<StoredPermission>),
    ),
    tag = "Permissions"
)]
pub async fn list_permissions_by_resource(
    _actor: AuthActor,
    State(state): State<PermissionsState>,
    Query(q): Query<ListByResourceQuery>,
) -> Result<Json<Vec<StoredPermission>>> {
    let perms = state
        .store
        .list_for_resource(&q.kind, q.id.as_deref())
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
