//! Authentication endpoints

use axum::{extract::State, http::StatusCode, Json};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use utoipa::ToSchema;

use secrecy::ExposeSecret;

use crate::auth::{create_token, create_token_with_email, AuthState};
use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;
use crate::middleware::cookies::{
    clear_csrf_cookie, clear_session_cookie, csrf_cookie, generate_csrf_token, session_cookie,
    SESSION_TTL,
};
use crate::storage::{StoredUser, UserRole};

/// Token request
#[derive(Debug, Deserialize, ToSchema)]
pub struct TokenRequest {
    /// API key or username
    pub api_key: String,
    /// API secret or password
    pub api_secret: String,
}

/// Token response
#[derive(Debug, Serialize, ToSchema)]
pub struct TokenResponse {
    /// JWT access token
    pub access_token: String,
    /// Token type (always "Bearer")
    pub token_type: String,
    /// Expiration in seconds
    pub expires_in: u64,
}

/// Get an access token.
///
/// # Errors
///
/// Returns an error if credentials are invalid or token creation fails.
#[utoipa::path(
    post,
    path = "/auth/token",
    request_body = TokenRequest,
    responses(
        (status = 200, description = "Token created", body = TokenResponse),
        (status = 401, description = "Invalid credentials"),
    ),
    tag = "Authentication"
)]
pub async fn get_token(
    State(auth): State<AuthState>,
    Json(request): Json<TokenRequest>,
) -> Result<Json<TokenResponse>> {
    // Validate against the credential store if available
    if let Some(cred_store) = &auth.credential_store {
        match cred_store
            .validate(&request.api_key, &request.api_secret)
            .await
        {
            Ok(Some(roles)) => {
                let expiry = Duration::from_secs(3600);
                let token = create_token(
                    auth.jwt_secret.expose_secret(),
                    &request.api_key,
                    expiry,
                    roles,
                )?;

                return Ok(Json(TokenResponse {
                    access_token: token,
                    token_type: "Bearer".to_string(),
                    expires_in: expiry.as_secs(),
                }));
            }
            Ok(None) => {
                // Invalid credentials -- fall through to error
            }
            Err(e) => {
                tracing::error!(error = %e, "Credential store error during authentication");
                return Err(ApiError::Internal(
                    "Authentication backend error".to_string(),
                ));
            }
        }
    } else {
        // Dev fallback: accept "dev"/"dev-secret" when no credential store is configured
        if request.api_key == "dev" && request.api_secret == "dev-secret" {
            tracing::warn!("Using dev credentials -- NOT SAFE FOR PRODUCTION");
            let expiry = Duration::from_secs(3600);
            let token = create_token(
                auth.jwt_secret.expose_secret(),
                "dev",
                expiry,
                vec!["admin".to_string()],
            )?;

            return Ok(Json(TokenResponse {
                access_token: token,
                token_type: "Bearer".to_string(),
                expires_in: expiry.as_secs(),
            }));
        }
    }

    Err(ApiError::Unauthorized("Invalid credentials".to_string()))
}

/// Request for `POST /auth/bootstrap`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BootstrapRequest {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// Response shape used by login + bootstrap + me.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UserView {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub is_active: bool,
    #[schema(value_type = Option<String>, example = "2026-04-15T12:00:00Z")]
    pub last_login_at: Option<DateTime<Utc>>,
}

impl From<&StoredUser> for UserView {
    fn from(u: &StoredUser) -> Self {
        Self {
            id: u.id.clone(),
            email: u.email.clone(),
            display_name: u.display_name.clone(),
            role: u.role.to_string(),
            is_active: u.is_active,
            last_login_at: u.last_login_at,
        }
    }
}

/// Response for `/auth/login` and `/auth/bootstrap`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LoginResponse {
    pub user: UserView,
    pub csrf_token: String,
}

/// Bootstrap the very first admin user. Returns 409 if any user exists.
///
/// # Errors
///
/// Returns an error if the user store or credential store is not configured,
/// if any user already exists, if the request is missing required fields, or
/// if the underlying stores fail.
#[utoipa::path(
    post,
    path = "/auth/bootstrap",
    request_body = BootstrapRequest,
    responses(
        (status = 201, description = "Admin user created", body = LoginResponse),
        (status = 409, description = "Bootstrap already completed"),
        (status = 400, description = "Invalid email or password"),
        (status = 503, description = "User store not configured"),
    ),
    tag = "Authentication"
)]
pub async fn bootstrap(
    State(auth): State<AuthState>,
    jar: CookieJar,
    Json(req): Json<BootstrapRequest>,
) -> Result<(StatusCode, CookieJar, Json<LoginResponse>)> {
    let user_store = auth
        .user_store
        .as_ref()
        .ok_or_else(|| ApiError::Internal("User store not configured".to_string()))?;
    let cred_store = auth
        .credential_store
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Credential store not configured".to_string()))?;

    if req.email.trim().is_empty() || req.password.is_empty() {
        return Err(ApiError::BadRequest(
            "Email and password are required".to_string(),
        ));
    }

    let count = user_store
        .count()
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?;
    if count > 0 {
        return Err(ApiError::Conflict(
            "Bootstrap already completed; sign in instead".to_string(),
        ));
    }

    // Create the admin user record
    let email_lc = req.email.to_lowercase();
    let display_name = req.display_name.unwrap_or_else(|| email_lc.clone());
    let user = StoredUser::new(email_lc.clone(), display_name, UserRole::Admin);
    user_store
        .store(&user)
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?;

    // Store the password hash keyed by email
    cred_store
        .create_api_key(&email_lc, &req.password, &[UserRole::Admin.as_str()])
        .await
        .map_err(|e| ApiError::Internal(format!("Credential store: {e}")))?;

    let (jar, body) = issue_session(&auth, &user, jar)?;
    Ok((StatusCode::CREATED, jar, Json(body)))
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// Sign in an existing user.
///
/// # Errors
///
/// Returns an error if credentials are invalid, the user is disabled, or any
/// backing store fails.
#[utoipa::path(
    post,
    path = "/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Logged in", body = LoginResponse),
        (status = 401, description = "Invalid credentials"),
        (status = 403, description = "User disabled"),
    ),
    tag = "Authentication"
)]
pub async fn login(
    State(auth): State<AuthState>,
    jar: CookieJar,
    Json(req): Json<LoginRequest>,
) -> Result<(CookieJar, Json<LoginResponse>)> {
    let user_store = auth
        .user_store
        .as_ref()
        .ok_or_else(|| ApiError::Internal("User store not configured".to_string()))?;
    let cred_store = auth
        .credential_store
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Credential store not configured".to_string()))?;

    let email_lc = req.email.to_lowercase();

    let Some(mut user) = user_store
        .get_by_email(&email_lc)
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?
    else {
        return Err(ApiError::Unauthorized("Invalid credentials".to_string()));
    };

    if !user.is_active {
        return Err(ApiError::Forbidden("User is disabled".to_string()));
    }

    let roles = cred_store
        .validate(&email_lc, &req.password)
        .await
        .map_err(|e| ApiError::Internal(format!("Credential store: {e}")))?;
    if roles.is_none() {
        return Err(ApiError::Unauthorized("Invalid credentials".to_string()));
    }

    user.touch_login();
    user_store
        .store(&user)
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?;

    let (jar, body) = issue_session(&auth, &user, jar)?;
    Ok((jar, Json(body)))
}

/// Clear the session + CSRF cookies.
#[utoipa::path(
    post,
    path = "/auth/logout",
    responses((status = 204, description = "Logged out")),
    tag = "Authentication"
)]
pub fn logout(jar: CookieJar) -> (StatusCode, CookieJar) {
    let jar = jar.add(clear_session_cookie()).add(clear_csrf_cookie());
    (StatusCode::NO_CONTENT, jar)
}

/// Return the currently signed-in user.
///
/// # Errors
///
/// Returns an error if the user store is not configured, if the user no longer
/// exists, or if the store fails.
#[utoipa::path(
    get,
    path = "/auth/me",
    responses(
        (status = 200, description = "Current user", body = UserView),
        (status = 401, description = "Not signed in"),
    ),
    tag = "Authentication"
)]
pub async fn me(actor: AuthActor, State(auth): State<AuthState>) -> Result<Json<UserView>> {
    let user_store = auth
        .user_store
        .as_ref()
        .ok_or_else(|| ApiError::Internal("User store not configured".to_string()))?;
    let user = user_store
        .get(&actor.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("User store: {e}")))?
        .ok_or_else(|| ApiError::Unauthorized("User no longer exists".to_string()))?;
    Ok(Json(UserView::from(&user)))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CsrfResponse {
    pub csrf_token: String,
}

/// Rotate the CSRF double-submit token for the current session.
#[utoipa::path(
    get,
    path = "/auth/csrf",
    responses((status = 200, description = "Rotated CSRF token", body = CsrfResponse)),
    tag = "Authentication"
)]
pub fn csrf(
    _actor: AuthActor,
    State(auth): State<AuthState>,
    jar: CookieJar,
) -> (CookieJar, Json<CsrfResponse>) {
    let token = generate_csrf_token();
    let jar = jar.add(csrf_cookie(token.clone(), auth.cookie_secure));
    (jar, Json(CsrfResponse { csrf_token: token }))
}

/// Build a fresh session cookie + CSRF cookie and the login response body.
fn issue_session(
    auth: &AuthState,
    user: &StoredUser,
    jar: CookieJar,
) -> Result<(CookieJar, LoginResponse)> {
    let ttl_secs = u64::try_from(SESSION_TTL.whole_seconds()).unwrap_or(24 * 3600);
    let expiry = Duration::from_secs(ttl_secs);
    let jwt = create_token_with_email(
        auth.jwt_secret.expose_secret(),
        &user.id,
        expiry,
        vec![user.role.as_str().to_string()],
        Some(user.email.clone()),
    )?;
    let csrf = generate_csrf_token();

    let jar = jar
        .add(session_cookie(jwt, auth.cookie_secure))
        .add(csrf_cookie(csrf.clone(), auth.cookie_secure));

    Ok((
        jar,
        LoginResponse {
            user: UserView::from(user),
            csrf_token: csrf,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_request_deserialize() {
        let json = r#"{"api_key": "test", "api_secret": "secret"}"#;
        let request: TokenRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.api_key, "test");
        assert_eq!(request.api_secret, "secret");
    }

    #[test]
    fn test_token_response_serialize() {
        let response = TokenResponse {
            access_token: "token123".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("token123"));
        assert!(json.contains("Bearer"));
    }

    #[test]
    fn test_bootstrap_request_deserialize() {
        let json = r#"{"email": "a@b.com", "password": "pw"}"#;
        let req: BootstrapRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.email, "a@b.com");
        assert_eq!(req.password, "pw");
        assert!(req.display_name.is_none());
    }

    #[test]
    fn test_bootstrap_request_deserialize_with_display_name() {
        let json = r#"{"email": "a@b.com", "password": "pw", "display_name": "Alice"}"#;
        let req: BootstrapRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.display_name.as_deref(), Some("Alice"));
    }

    #[test]
    fn test_login_request_deserialize() {
        let json = r#"{"email": "user@example.com", "password": "hunter2"}"#;
        let req: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.email, "user@example.com");
        assert_eq!(req.password, "hunter2");
    }

    #[test]
    fn test_user_view_from_stored_user() {
        let stored = StoredUser::new("Foo@Bar.com", "Foo Bar", UserRole::Admin);
        let view = UserView::from(&stored);

        assert_eq!(view.id, stored.id);
        // StoredUser::new lowercases the email on construction
        assert_eq!(view.email, "foo@bar.com");
        assert_eq!(view.display_name, "Foo Bar");
        assert_eq!(view.role, "admin");
        assert!(view.is_active);
        assert!(view.last_login_at.is_none());
    }

    #[test]
    fn test_user_view_role_regular_user() {
        let stored = StoredUser::new("u@x.com", "U", UserRole::User);
        let view = UserView::from(&stored);
        assert_eq!(view.role, "user");
    }

    #[test]
    fn test_login_response_serialize() {
        let stored = StoredUser::new("a@b.com", "Alice", UserRole::Admin);
        let response = LoginResponse {
            user: UserView::from(&stored),
            csrf_token: "csrf-abc-123".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"user\""));
        assert!(json.contains("\"csrf_token\""));
        assert!(json.contains("csrf-abc-123"));
        assert!(json.contains("a@b.com"));
        assert!(json.contains("\"role\":\"admin\""));
    }

    #[test]
    fn test_csrf_response_serialize() {
        let response = CsrfResponse {
            csrf_token: "token-xyz".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"csrf_token\":\"token-xyz\""));
    }
}
