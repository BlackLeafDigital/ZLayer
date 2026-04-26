#![allow(clippy::missing_errors_doc, clippy::doc_markdown)]
//! OIDC / SSO endpoints.
//!
//! - `GET /auth/oidc/providers` lists configured providers (public; no auth)
//! - `GET /auth/oidc/:provider/start` redirects the browser to the provider's
//!   authorisation URL with a fresh CSRF/PKCE pair recorded in the in-memory
//!   state store.
//! - `GET /auth/oidc/:provider/callback` consumes the state, exchanges the
//!   authorisation code for tokens, verifies the ID token, resolves the
//!   `(provider, sub)` pair to a local user (creating one if needed), and
//!   issues the same session + CSRF cookies a password login would.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::extract::cookie::CookieJar;

use crate::auth::AuthState;
use crate::error::{ApiError, Result};
use crate::handlers::auth::issue_session;
use crate::oidc::OidcProviderPublic;

pub use zlayer_types::api::oidc::*;

/// `GET /auth/oidc/providers`.
#[utoipa::path(
    get,
    path = "/auth/oidc/providers",
    responses(
        (status = 200, description = "List of configured OIDC providers",
         body = Vec<OidcProviderPublic>),
    ),
    tag = "Authentication"
)]
pub async fn list_providers(State(auth): State<AuthState>) -> Json<Vec<OidcProviderPublic>> {
    let mut providers: Vec<_> = auth
        .oidc_clients
        .values()
        .map(|c| OidcProviderPublic::from(c.config()))
        .collect();
    providers.sort_by(|a, b| a.name.cmp(&b.name));
    Json(providers)
}

/// `GET /auth/oidc/:provider/start` — 302 to the provider's authorize URL.
#[utoipa::path(
    get,
    path = "/auth/oidc/{provider}/start",
    params(("provider" = String, Path, description = "Provider slug")),
    responses(
        (status = 302, description = "Redirect to provider's authorize URL"),
        (status = 404, description = "Unknown provider"),
    ),
    tag = "Authentication"
)]
pub async fn start(
    State(auth): State<AuthState>,
    Path(provider): Path<String>,
) -> Result<impl IntoResponse> {
    let client = auth
        .oidc_clients
        .get(&provider)
        .ok_or_else(|| ApiError::NotFound(format!("OIDC provider '{provider}' not configured")))?;

    let started = client
        .start_auth()
        .await
        .map_err(|e| ApiError::Internal(format!("OIDC start: {e}")))?;

    auth.oidc_state
        .put(
            provider.clone(),
            started.csrf_token.clone(),
            started.nonce.clone(),
            started.pkce_verifier.clone(),
        )
        .await;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&started.auth_url).map_err(|e| {
            ApiError::Internal(format!(
                "OIDC: authorize URL contains invalid header chars: {e}"
            ))
        })?,
    );
    Ok((StatusCode::FOUND, headers))
}

/// `GET /auth/oidc/:provider/callback`.
#[utoipa::path(
    get,
    path = "/auth/oidc/{provider}/callback",
    params(("provider" = String, Path, description = "Provider slug")),
    responses(
        (status = 200, description = "Signed in via OIDC", body = OidcCallbackResponse),
        (status = 400, description = "Invalid callback parameters"),
        (status = 401, description = "Provider returned an error"),
        (status = 404, description = "Unknown provider"),
    ),
    tag = "Authentication"
)]
pub async fn callback(
    State(auth): State<AuthState>,
    Path(provider): Path<String>,
    Query(params): Query<CallbackParams>,
    jar: CookieJar,
) -> Result<(CookieJar, Json<OidcCallbackResponse>)> {
    if let Some(err) = params.error.as_deref() {
        let desc = params.error_description.as_deref().unwrap_or("");
        return Err(ApiError::Unauthorized(format!(
            "OIDC provider returned error: {err} {desc}"
        )));
    }

    let code = params
        .code
        .ok_or_else(|| ApiError::BadRequest("missing code".to_string()))?;
    let state = params
        .state
        .ok_or_else(|| ApiError::BadRequest("missing state".to_string()))?;

    let client = auth
        .oidc_clients
        .get(&provider)
        .ok_or_else(|| ApiError::NotFound(format!("OIDC provider '{provider}' not configured")))?;

    let payload =
        auth.oidc_state.take(&state).await.ok_or_else(|| {
            ApiError::Unauthorized("OIDC state token invalid or expired".to_string())
        })?;

    if payload.provider != provider {
        return Err(ApiError::Unauthorized(
            "OIDC state/provider mismatch".to_string(),
        ));
    }

    let identity = client
        .exchange_code(code, &payload)
        .await
        .map_err(|e| ApiError::Unauthorized(format!("OIDC exchange: {e}")))?;

    let idmgr = auth
        .identity
        .as_ref()
        .ok_or_else(|| ApiError::Internal("Identity manager not configured".to_string()))?;

    let user = idmgr
        .find_or_create_by_external_id(
            &identity.provider,
            &identity.subject,
            identity.email.as_deref(),
            identity.display_name.as_deref(),
        )
        .await?;

    if !user.is_active {
        return Err(ApiError::Forbidden("User is disabled".to_string()));
    }

    let (jar, login) = issue_session(&auth, &user, jar)?;
    Ok((
        jar,
        Json(OidcCallbackResponse {
            user: login.user,
            csrf_token: login.csrf_token,
            provider: identity.provider,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oidc::{OidcProviderConfig, StateTokenStore};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use secrecy::SecretString;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn empty_auth_state() -> AuthState {
        AuthState {
            jwt_secret: SecretString::from("test-jwt-secret-not-used".to_string()),
            credential_store: None,
            user_store: None,
            identity: None,
            oidc_clients: HashMap::new(),
            oidc_state: Arc::new(StateTokenStore::new()),
            cookie_secure: false,
        }
    }

    fn app(state: AuthState) -> Router {
        Router::new()
            .route("/auth/oidc/providers", get(list_providers))
            .route("/auth/oidc/{provider}/start", get(start))
            .route("/auth/oidc/{provider}/callback", get(callback))
            .with_state(state)
    }

    #[tokio::test]
    async fn providers_empty_when_none_configured() {
        let app = app(empty_auth_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/auth/oidc/providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"[]");
    }

    #[tokio::test]
    async fn start_unknown_provider_returns_404() {
        let app = app(empty_auth_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/auth/oidc/nope/start")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn callback_unknown_provider_returns_404() {
        let app = app(empty_auth_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/auth/oidc/nope/callback?code=x&state=y")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn callback_bubbles_provider_error_as_401() {
        let mut state = empty_auth_state();
        let cfg = OidcProviderConfig {
            name: "google".to_string(),
            display_name: "Google".to_string(),
            issuer: "https://example.com".to_string(),
            client_id: "cid".to_string(),
            client_secret: "csecret".to_string(),
            redirect_url: "https://app/cb".to_string(),
            scopes: vec!["openid".to_string()],
        };
        state
            .oidc_clients
            .insert("google".to_string(), crate::oidc::OidcClient::new(cfg));

        let app = app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/auth/oidc/google/callback?error=access_denied&error_description=user+denied")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn callback_missing_state_returns_400() {
        let mut state = empty_auth_state();
        let cfg = OidcProviderConfig {
            name: "okta".to_string(),
            display_name: "Okta".to_string(),
            issuer: "https://example.com".to_string(),
            client_id: "cid".to_string(),
            client_secret: "csecret".to_string(),
            redirect_url: "https://app/cb".to_string(),
            scopes: vec!["openid".to_string()],
        };
        state
            .oidc_clients
            .insert("okta".to_string(), crate::oidc::OidcClient::new(cfg));

        let app = app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/auth/oidc/okta/callback?code=abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn callback_expired_state_returns_401() {
        let mut state = empty_auth_state();
        let cfg = OidcProviderConfig {
            name: "okta".to_string(),
            display_name: "Okta".to_string(),
            issuer: "https://example.com".to_string(),
            client_id: "cid".to_string(),
            client_secret: "csecret".to_string(),
            redirect_url: "https://app/cb".to_string(),
            scopes: vec!["openid".to_string()],
        };
        state
            .oidc_clients
            .insert("okta".to_string(), crate::oidc::OidcClient::new(cfg));

        let app = app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/auth/oidc/okta/callback?code=abc&state=not-in-store")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
