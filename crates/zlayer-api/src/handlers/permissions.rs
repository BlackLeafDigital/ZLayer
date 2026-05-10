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

/// Canonical `resource_kind` values accepted by the permissions API.
///
/// `StoredPermission::resource_kind` is a free-form `String` at the storage
/// layer so future kinds can be added without a schema migration; the handler
/// is the only place that gates the wire format. Phase 1 of the secrets/RBAC
/// milestone adds `"secret"` (handler-side authorization for secrets) and
/// `"node"` (per-node grants for node-affined secrets / cluster operations).
const KNOWN_RESOURCE_KINDS: &[&str] = &["environment", "deployment", "project", "secret", "node"];

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

    // Reject unknown resource_kind values. The storage layer is permissive
    // (free-form String) so the handler is the policy gate.
    if !KNOWN_RESOURCE_KINDS.contains(&req.resource_kind.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "unknown resource_kind: {} (expected one of: {})",
            req.resource_kind,
            KNOWN_RESOURCE_KINDS.join(", "),
        )));
    }

    // Light shape validation for resource_id by kind. Wildcards (None) are
    // always allowed; the only constraint is that, when an id is supplied,
    // it follows the storage convention for that kind.
    match req.resource_kind.as_str() {
        "node" => {
            if let Some(rid) = &req.resource_id {
                if uuid::Uuid::parse_str(rid).is_err() {
                    return Err(ApiError::BadRequest(format!(
                        "resource_kind=node requires resource_id to be a node UUID; got: {rid}"
                    )));
                }
            }
        }
        "secret" => {
            if let Some(rid) = &req.resource_id {
                let (scope, name) = rid.split_once(':').ok_or_else(|| {
                    ApiError::BadRequest(
                        "resource_kind=secret requires resource_id of form 'scope:name'"
                            .to_string(),
                    )
                })?;
                if scope.is_empty() || name.is_empty() {
                    return Err(ApiError::BadRequest(
                        "resource_kind=secret resource_id parts (scope, name) must be non-empty"
                            .to_string(),
                    ));
                }
            }
        }
        _ => {}
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
    use crate::storage::{InMemoryGroupStore, InMemoryPermissionStore, PermissionLevel};
    use axum::Json;

    fn admin_actor() -> AuthActor {
        AuthActor {
            user_id: "admin-1".into(),
            roles: vec!["admin".into()],
            email: None,
        }
    }

    fn state() -> PermissionsState {
        PermissionsState::new(
            Arc::new(InMemoryPermissionStore::new()),
            Arc::new(InMemoryGroupStore::new()),
        )
    }

    fn req(kind: &str, id: Option<&str>) -> GrantPermissionRequest {
        GrantPermissionRequest {
            subject_kind: SubjectKind::User,
            subject_id: "u-1".into(),
            resource_kind: kind.into(),
            resource_id: id.map(str::to_string),
            level: PermissionLevel::Read,
        }
    }

    #[tokio::test]
    async fn grant_with_resource_kind_secret_accepted() {
        let r = grant_permission(
            admin_actor(),
            State(state()),
            Json(req("secret", Some("default:foo"))),
        )
        .await
        .expect("secret grant with valid scope:name should succeed");
        assert_eq!(r.0, StatusCode::CREATED);
        assert_eq!(r.1.resource_kind, "secret");
        assert_eq!(r.1.resource_id.as_deref(), Some("default:foo"));
    }

    #[tokio::test]
    async fn grant_with_resource_kind_node_accepted() {
        let node_id = uuid::Uuid::new_v4().to_string();
        let r = grant_permission(
            admin_actor(),
            State(state()),
            Json(req("node", Some(&node_id))),
        )
        .await
        .expect("node grant with valid UUID should succeed");
        assert_eq!(r.0, StatusCode::CREATED);
        assert_eq!(r.1.resource_kind, "node");
        assert_eq!(r.1.resource_id.as_deref(), Some(node_id.as_str()));
    }

    #[tokio::test]
    async fn grant_with_resource_kind_node_wildcard_accepted() {
        let r = grant_permission(admin_actor(), State(state()), Json(req("node", None)))
            .await
            .expect("wildcard node grant should succeed");
        assert_eq!(r.0, StatusCode::CREATED);
        assert_eq!(r.1.resource_kind, "node");
        assert!(r.1.resource_id.is_none());
    }

    #[tokio::test]
    async fn grant_with_unknown_resource_kind_rejected() {
        let err = grant_permission(
            admin_actor(),
            State(state()),
            Json(req("banana", Some("anything"))),
        )
        .await
        .expect_err("unknown resource_kind must be rejected");
        match err {
            ApiError::BadRequest(msg) => assert!(
                msg.contains("unknown resource_kind") && msg.contains("banana"),
                "unexpected error message: {msg}"
            ),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn grant_secret_with_malformed_resource_id_rejected() {
        let err = grant_permission(
            admin_actor(),
            State(state()),
            Json(req("secret", Some("nocolon"))),
        )
        .await
        .expect_err("secret grant without ':' must be rejected");
        match err {
            ApiError::BadRequest(msg) => assert!(
                msg.contains("scope:name"),
                "unexpected error message: {msg}"
            ),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        // Empty halves are also rejected.
        for bad in ["scope:", ":name", ":"] {
            let err = grant_permission(
                admin_actor(),
                State(state()),
                Json(req("secret", Some(bad))),
            )
            .await
            .expect_err("secret grant with empty scope or name must be rejected");
            assert!(
                matches!(err, ApiError::BadRequest(_)),
                "expected BadRequest for {bad:?}, got {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn grant_node_with_non_uuid_resource_id_rejected() {
        let err = grant_permission(
            admin_actor(),
            State(state()),
            Json(req("node", Some("not-a-uuid"))),
        )
        .await
        .expect_err("node grant with non-UUID resource_id must be rejected");
        match err {
            ApiError::BadRequest(msg) => assert!(
                msg.contains("node UUID") && msg.contains("not-a-uuid"),
                "unexpected error message: {msg}"
            ),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }
}
