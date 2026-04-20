//! User CRUD endpoints.
//!
//! All endpoints require authentication; mutating endpoints require the
//! `admin` role. Exception: `POST /api/v1/users/{id}/password` is allowed
//! self-service by a non-admin when they provide their current password.

use axum::{
    extract::{FromRequestParts, Path, State},
    http::{request::Parts, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{AuthState, AuthUser, SessionAuthUser};
use crate::error::{ApiError, Result};
use crate::handlers::auth::UserView;
use crate::storage::{UserRole, UserStorage};

/// Authentication actor — either a Bearer-token user or a cookie-session user.
///
/// Handlers take this extractor so the same endpoints work for both API
/// clients (`Authorization: Bearer …`) and browser sessions (cookie).
#[derive(Debug, Clone)]
pub struct AuthActor {
    pub user_id: String,
    pub roles: Vec<String>,
    pub email: Option<String>,
}

impl AuthActor {
    /// Check if the actor carries the given role. `admin` short-circuits and
    /// grants every role.
    #[must_use]
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role || r == "admin")
    }

    /// Return `Ok(())` when the actor is an admin; otherwise `Forbidden`.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Forbidden`] when the actor does not have the
    /// `admin` role.
    pub fn require_admin(&self) -> Result<()> {
        if self.has_role("admin") {
            Ok(())
        } else {
            Err(ApiError::Forbidden("Admin role required".to_string()))
        }
    }

    /// Check whether the actor has at least `required_level` on the given
    /// environment. Admin role short-circuits to allow.
    ///
    /// Used by secrets handlers to gate env-scoped reads/writes on per-env
    /// RBAC without requiring full admin.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Forbidden`] if the actor is neither admin nor
    /// holds a sufficient permission grant on the env. Returns
    /// [`ApiError::Internal`] if the permission store fails.
    pub async fn require_env_access(
        &self,
        perm_store: &(dyn crate::storage::PermissionStorage + 'static),
        env_id: &str,
        required_level: crate::storage::PermissionLevel,
    ) -> Result<()> {
        if self.has_role("admin") {
            return Ok(());
        }
        let allowed = perm_store
            .check(
                crate::storage::SubjectKind::User,
                &self.user_id,
                "environment",
                Some(env_id),
                required_level,
            )
            .await
            .map_err(|e| ApiError::Internal(format!("Permission store: {e}")))?;
        if allowed {
            Ok(())
        } else {
            Err(ApiError::Forbidden(format!(
                "Missing {required_level:?} on environment {env_id}"
            )))
        }
    }
}

impl<S> FromRequestParts<S> for AuthActor
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        // Try Bearer first (API clients).
        if let Ok(user) = AuthUser::from_request_parts(parts, state).await {
            return Ok(AuthActor {
                user_id: user.claims.sub,
                roles: user.claims.roles,
                email: user.claims.email,
            });
        }
        // Fall back to session cookie (browser).
        let session = SessionAuthUser::from_request_parts(parts, state).await?;
        Ok(AuthActor {
            user_id: session.claims.sub,
            roles: session.claims.roles,
            email: session.claims.email,
        })
    }
}

// ---- Request/response types ----

/// Body for `POST /api/v1/users`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateUserRequest {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub role: UserRole,
}

/// Body for `PATCH /api/v1/users/{id}`. All fields are optional so a caller
/// can update a single attribute without touching the others.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateUserRequest {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub role: Option<UserRole>,
    #[serde(default)]
    pub is_active: Option<bool>,
}

/// Body for `POST /api/v1/users/{id}/password`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SetPasswordRequest {
    /// New password.
    pub new_password: String,
    /// Required when the caller is setting their own password (non-admin
    /// path). Admins changing someone else's password may omit this.
    #[serde(default)]
    pub current_password: Option<String>,
}

// ---- Helpers ----

fn user_store(auth: &AuthState) -> Result<&std::sync::Arc<dyn UserStorage>> {
    auth.user_store
        .as_ref()
        .ok_or_else(|| ApiError::Internal("User store not configured".to_string()))
}

fn cred_store(
    auth: &AuthState,
) -> Result<
    &std::sync::Arc<
        zlayer_secrets::CredentialStore<std::sync::Arc<zlayer_secrets::PersistentSecretsStore>>,
    >,
> {
    auth.credential_store
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Credential store not configured".to_string()))
}

fn identity(auth: &AuthState) -> Result<&std::sync::Arc<crate::identity::IdentityManager>> {
    auth.identity
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Identity manager not configured".to_string()))
}

// ---- Endpoints ----

/// List all users. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin, or
/// [`ApiError::Internal`] if the user store fails.
#[utoipa::path(
    get,
    path = "/api/v1/users",
    responses(
        (status = 200, description = "List users", body = Vec<UserView>),
        (status = 403, description = "Admin role required"),
    ),
    tag = "Users"
)]
pub async fn list_users(
    actor: AuthActor,
    State(auth): State<AuthState>,
) -> Result<Json<Vec<UserView>>> {
    actor.require_admin()?;
    let users = user_store(&auth)?
        .list()
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?;
    Ok(Json(users.iter().map(UserView::from).collect()))
}

/// Create a new user. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::BadRequest`] when the body is incomplete,
/// [`ApiError::Conflict`] when the email is already registered, or
/// [`ApiError::Internal`] if a backing store fails.
#[utoipa::path(
    post,
    path = "/api/v1/users",
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "User created", body = UserView),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
        (status = 409, description = "Email already in use"),
    ),
    tag = "Users"
)]
pub async fn create_user(
    actor: AuthActor,
    State(auth): State<AuthState>,
    Json(req): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserView>)> {
    actor.require_admin()?;

    let identity = identity(&auth)?;
    let email_lc = req.email.trim().to_lowercase();
    let display_name = req.display_name.unwrap_or_else(|| email_lc.clone());
    let user = identity
        .create_user(&email_lc, display_name, req.role, &req.password)
        .await?;
    Ok((StatusCode::CREATED, Json(UserView::from(&user))))
}

/// Fetch a single user. Admins can read any record; regular users can read
/// only their own.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when a non-admin tries to read a
/// different user, [`ApiError::NotFound`] when no such user exists, or
/// [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    get,
    path = "/api/v1/users/{id}",
    params(("id" = String, Path, description = "User id")),
    responses(
        (status = 200, description = "User", body = UserView),
        (status = 404, description = "Not found"),
    ),
    tag = "Users"
)]
pub async fn get_user(
    actor: AuthActor,
    State(auth): State<AuthState>,
    Path(id): Path<String>,
) -> Result<Json<UserView>> {
    // Non-admins can read their own record.
    if !actor.has_role("admin") && actor.user_id != id {
        return Err(ApiError::Forbidden("Cannot read other users".to_string()));
    }

    let user = user_store(&auth)?
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("User {id} not found")))?;
    Ok(Json(UserView::from(&user)))
}

/// Update a user's mutable fields. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::NotFound`] when the user does not exist, or
/// [`ApiError::Internal`] if the user store fails.
#[utoipa::path(
    patch,
    path = "/api/v1/users/{id}",
    params(("id" = String, Path, description = "User id")),
    request_body = UpdateUserRequest,
    responses(
        (status = 200, description = "Updated user", body = UserView),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    tag = "Users"
)]
pub async fn update_user(
    actor: AuthActor,
    State(auth): State<AuthState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<Json<UserView>> {
    actor.require_admin()?;

    // Role changes go through IdentityManager so the credential-store roles
    // stay in sync with the user-store role. Other field updates stay on
    // the user store directly.
    if let Some(role) = req.role {
        identity(&auth)?.set_role(&id, role).await?;
    }

    let store = user_store(&auth)?;
    let mut user = store
        .get(&id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("User {id} not found")))?;

    let mut dirty = false;
    if let Some(name) = req.display_name {
        user.display_name = name;
        dirty = true;
    }
    if let Some(active) = req.is_active {
        user.is_active = active;
        dirty = true;
    }
    if dirty {
        user.updated_at = chrono::Utc::now();
        store.store(&user).await?;
    }

    Ok(Json(UserView::from(&user)))
}

/// Delete a user. Admin only. Callers cannot delete their own account.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::BadRequest`] when an admin tries to delete themselves,
/// [`ApiError::NotFound`] when the user does not exist, or
/// [`ApiError::Internal`] if a backing store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/users/{id}",
    params(("id" = String, Path, description = "User id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    tag = "Users"
)]
pub async fn delete_user(
    actor: AuthActor,
    State(auth): State<AuthState>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    if actor.user_id == id {
        return Err(ApiError::BadRequest(
            "Cannot delete your own account".to_string(),
        ));
    }

    identity(&auth)?.delete_user(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Set a user's password. Admins may change any user's password; regular
/// users may only change their own, and must supply `current_password`.
///
/// # Errors
///
/// Returns [`ApiError::BadRequest`] for missing fields,
/// [`ApiError::Unauthorized`] for an incorrect current password,
/// [`ApiError::Forbidden`] when the caller is not allowed to change this
/// user's password, [`ApiError::NotFound`] when the user does not exist,
/// or [`ApiError::Internal`] if a backing store fails.
#[utoipa::path(
    post,
    path = "/api/v1/users/{id}/password",
    params(("id" = String, Path, description = "User id")),
    request_body = SetPasswordRequest,
    responses(
        (status = 204, description = "Password updated"),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Current password incorrect (self-service path)"),
        (status = 403, description = "Not permitted"),
        (status = 404, description = "Not found"),
    ),
    tag = "Users"
)]
pub async fn set_password(
    actor: AuthActor,
    State(auth): State<AuthState>,
    Path(id): Path<String>,
    Json(req): Json<SetPasswordRequest>,
) -> Result<StatusCode> {
    if req.new_password.is_empty() {
        return Err(ApiError::BadRequest("new_password is required".to_string()));
    }

    let store = user_store(&auth)?;
    let creds = cred_store(&auth)?;

    let user = store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("User {id} not found")))?;

    let is_self = actor.user_id == id;
    let is_admin = actor.has_role("admin");

    if !is_self && !is_admin {
        return Err(ApiError::Forbidden(
            "Cannot change password for another user".to_string(),
        ));
    }

    // Self-service requires current_password verification, even for admins
    // changing their own password (defense in depth).
    if is_self {
        let current = req.current_password.clone().ok_or_else(|| {
            ApiError::BadRequest("current_password required when changing own password".to_string())
        })?;
        let roles = creds
            .validate(&user.email, &current)
            .await
            .map_err(|e| ApiError::Internal(format!("Credential store: {e}")))?;
        if roles.is_none() {
            return Err(ApiError::Unauthorized(
                "current_password is incorrect".to_string(),
            ));
        }
    }

    creds
        .create_api_key(&user.email, &req.new_password, &[user.role.as_str()])
        .await
        .map_err(|e| ApiError::Internal(format!("Credential store: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_user_request_deserialize() {
        let json = r#"{"email": "a@b.com", "password": "pw", "role": "user"}"#;
        let req: CreateUserRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.email, "a@b.com");
        assert_eq!(req.role, UserRole::User);
        assert!(req.display_name.is_none());
    }

    #[test]
    fn test_create_user_request_admin_role() {
        let json = r#"{"email":"a@b.com","password":"pw","role":"admin","display_name":"A"}"#;
        let req: CreateUserRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.role, UserRole::Admin);
        assert_eq!(req.display_name.as_deref(), Some("A"));
    }

    #[test]
    fn test_update_user_request_partial() {
        // All fields optional — empty body is valid
        let req: UpdateUserRequest = serde_json::from_str("{}").unwrap();
        assert!(req.display_name.is_none());
        assert!(req.role.is_none());
        assert!(req.is_active.is_none());
    }

    #[test]
    fn test_set_password_request_deserialize() {
        let req: SetPasswordRequest =
            serde_json::from_str(r#"{"new_password":"np","current_password":"cp"}"#).unwrap();
        assert_eq!(req.new_password, "np");
        assert_eq!(req.current_password.as_deref(), Some("cp"));

        let req_admin: SetPasswordRequest =
            serde_json::from_str(r#"{"new_password":"np"}"#).unwrap();
        assert!(req_admin.current_password.is_none());
    }

    #[test]
    fn test_auth_actor_has_role() {
        let actor = AuthActor {
            user_id: "u1".into(),
            roles: vec!["user".into()],
            email: None,
        };
        assert!(actor.has_role("user"));
        assert!(!actor.has_role("admin"));

        let admin = AuthActor {
            user_id: "u2".into(),
            roles: vec!["admin".into()],
            email: None,
        };
        // admin short-circuits everything
        assert!(admin.has_role("admin"));
        assert!(admin.has_role("user"));
        assert!(admin.has_role("anything"));
    }

    #[test]
    fn test_require_admin_returns_forbidden_when_missing() {
        let actor = AuthActor {
            user_id: "u1".into(),
            roles: vec!["user".into()],
            email: None,
        };
        let err = actor.require_admin().unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(_)));
    }

    #[tokio::test]
    async fn test_require_env_access_admin_shortcut() {
        use crate::storage::{InMemoryPermissionStore, PermissionLevel};

        let admin = AuthActor {
            user_id: "admin-1".into(),
            roles: vec!["admin".into()],
            email: None,
        };
        let store = InMemoryPermissionStore::new();

        // Admin short-circuits even when the permission store is empty.
        admin
            .require_env_access(&store, "env-xyz", PermissionLevel::Write)
            .await
            .expect("admin should always be allowed");
    }

    #[tokio::test]
    async fn test_require_env_access_explicit_grant() {
        use crate::storage::{
            InMemoryPermissionStore, PermissionLevel, PermissionStorage, StoredPermission,
            SubjectKind,
        };

        let actor = AuthActor {
            user_id: "u1".into(),
            roles: vec!["user".into()],
            email: None,
        };
        let store = InMemoryPermissionStore::new();

        // Grant u1 Read on env "env-1".
        let grant = StoredPermission::new(
            SubjectKind::User,
            "u1",
            "environment",
            Some("env-1".to_string()),
            PermissionLevel::Read,
        );
        <InMemoryPermissionStore as PermissionStorage>::grant(&store, &grant)
            .await
            .unwrap();

        // Read on env-1 is allowed.
        actor
            .require_env_access(&store, "env-1", PermissionLevel::Read)
            .await
            .expect("read grant should allow read");

        // Write on env-1 is denied with Forbidden.
        let err = actor
            .require_env_access(&store, "env-1", PermissionLevel::Write)
            .await
            .unwrap_err();
        assert!(
            matches!(err, ApiError::Forbidden(_)),
            "expected Forbidden, got {err:?}"
        );
    }
}
