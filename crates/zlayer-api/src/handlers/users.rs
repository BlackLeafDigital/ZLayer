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
use crate::storage::{StoredUser, UserRole, UserStorage};

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

    if req.email.trim().is_empty() || req.password.is_empty() {
        return Err(ApiError::BadRequest(
            "Email and password are required".to_string(),
        ));
    }

    let email_lc = req.email.to_lowercase();
    let store = user_store(&auth)?;
    let creds = cred_store(&auth)?;

    if store
        .get_by_email(&email_lc)
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Email '{email_lc}' is already registered"
        )));
    }

    let display_name = req.display_name.unwrap_or_else(|| email_lc.clone());
    let user = StoredUser::new(email_lc.clone(), display_name, req.role);

    store
        .store(&user)
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?;

    creds
        .create_api_key(&email_lc, &req.password, &[req.role.as_str()])
        .await
        .map_err(|e| ApiError::Internal(format!("Credential store: {e}")))?;

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

    let store = user_store(&auth)?;
    let mut user = store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("User {id} not found")))?;

    if let Some(name) = req.display_name {
        user.display_name = name;
    }
    if let Some(role) = req.role {
        user.role = role;
    }
    if let Some(active) = req.is_active {
        user.is_active = active;
    }
    user.updated_at = chrono::Utc::now();

    store
        .store(&user)
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?;

    // Role changes propagate to the credential-store roles at the next
    // password set. Login already reads roles from the user store, so this
    // is intentionally a no-op here.
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

    let store = user_store(&auth)?;
    let creds = cred_store(&auth)?;

    let user = store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("User {id} not found")))?;

    // Delete credential first (keyed by email) so a later re-create of the
    // same email is not blocked by a stale hash.
    if creds
        .exists(&user.email)
        .await
        .map_err(|e| ApiError::Internal(format!("Credential store: {e}")))?
    {
        creds
            .delete_api_key(&user.email)
            .await
            .map_err(|e| ApiError::Internal(format!("Credential store: {e}")))?;
    }

    let deleted = store
        .delete(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?;
    if !deleted {
        return Err(ApiError::NotFound(format!("User {id} not found")));
    }

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
}
