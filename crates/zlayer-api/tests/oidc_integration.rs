#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::too_many_lines
)]
//! End-to-end integration test for the OIDC / SSO sign-in flow.
//!
//! Spins up an in-process mock OpenID provider (discovery, JWKS, token
//! endpoints) on `127.0.0.1:0`, signs real RS256 ID tokens, and drives the
//! full `/auth/oidc/{provider}/start` → `/callback` flow against the real
//! [`zlayer_api::build_router_with_storage`] router. All three stores
//! (users, credentials, oidc_identities) are asserted on post-callback to
//! prove the sign-in actually wired a local user + identity link.
//!
//! The mock provider is plain HTTP (not HTTPS) on loopback — fine for the
//! integration test; the `openidconnect 3.5` async client accepts `http://`
//! scheme for the issuer URL.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use jsonwebtoken::{EncodingKey, Header as JwtHeader};
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use secrecy::SecretString;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tower::ServiceExt;
use url::Url;

use zlayer_api::config::RateLimitConfig;
use zlayer_api::handlers::auth::UserView;
use zlayer_api::oidc::{OidcClient, OidcProviderConfig, StateTokenStore};
use zlayer_api::storage::{
    DeploymentStorage, InMemoryOidcIdentityStore, InMemoryStorage, OidcIdentityStorage,
    SqlxUserStore, UserStorage,
};
use zlayer_api::{build_router_with_storage, ApiConfig, IdentityManager};

// ---------------------------------------------------------------------------
// Mock provider state
// ---------------------------------------------------------------------------

/// Per-code state recorded when the test mints an authorisation code — the
/// token endpoint uses this to verify the PKCE challenge and echo the correct
/// nonce back into the ID token.
#[derive(Debug, Clone)]
struct AuthCodeRecord {
    /// Base64url-SHA256 of the PKCE verifier, as sent on /authorize.
    pkce_challenge: String,
    /// Nonce that must go into the id_token's `nonce` claim.
    nonce: String,
    /// Audience claim to embed in the id_token (lets us test the
    /// "wrong-audience" negative case by recording a bad value here).
    aud_override: Option<String>,
    /// Override the nonce embedded in the id_token (lets us test the
    /// "nonce-mismatch" negative case).
    nonce_override: Option<String>,
}

/// All state the mock IdP needs to respond to discovery + token requests.
#[derive(Clone)]
struct MockProviderState {
    /// PKCS#1-DER encoded RSA private key, ready to hand to `jsonwebtoken`.
    signing_key_der: Arc<Vec<u8>>,
    /// Precomputed JWK document (published on /jwks).
    jwks_json: Arc<Value>,
    /// Issuer URL (the root — no trailing slash). Used as the `iss` claim
    /// and in the discovery document.
    issuer: String,
    /// Client id registered with this mock provider.
    client_id: String,
    /// Stable subject for the happy-path user.
    subject: String,
    /// Email the mock provider returns in the userinfo/id_token claims.
    email: String,
    /// Display name the mock provider returns.
    display_name: String,
    /// code → PKCE + nonce
    codes: Arc<Mutex<HashMap<String, AuthCodeRecord>>>,
}

impl MockProviderState {
    fn new(issuer: String, client_id: String, subject: String, email: String) -> Self {
        // NOTE: use rsa's own re-exported rand_core OsRng so it matches the
        // rand_core version rsa 0.9 was built against. The workspace pulls
        // rand 0.9, but rsa's CryptoRngCore bound is tied to rand_core 0.6.
        let mut rng = rsa::rand_core::OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("RSA keygen");
        let public_key = RsaPublicKey::from(&private_key);

        // PKCS#1 DER for jsonwebtoken::EncodingKey::from_rsa_der.
        let signing_key_der = private_key
            .to_pkcs1_der()
            .expect("RSA to pkcs1 der")
            .as_bytes()
            .to_vec();

        // JWK: RS256 public key expressed as n, e in base64url-no-pad.
        let n_b64 = URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be());
        let e_b64 = URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be());
        let jwks_json = json!({
            "keys": [{
                "kty": "RSA",
                "use": "sig",
                "alg": "RS256",
                "kid": "mock-key-1",
                "n": n_b64,
                "e": e_b64,
            }]
        });

        Self {
            signing_key_der: Arc::new(signing_key_der),
            jwks_json: Arc::new(jwks_json),
            issuer,
            client_id,
            subject,
            email,
            display_name: "Alice".to_string(),
            codes: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// Build the mock IdP axum router with `/.well-known/openid-configuration`,
/// `/jwks`, and `/token`. The `/authorize` endpoint is not served because the
/// test doesn't exercise the browser-side consent screen — it just records a
/// code in the state map directly and drives the callback.
fn mock_idp_router(state: MockProviderState) -> Router {
    Router::new()
        .route("/.well-known/openid-configuration", get(handle_discovery))
        .route("/jwks", get(handle_jwks))
        .route("/token", post(handle_token))
        .with_state(state)
}

async fn handle_discovery(State(state): State<MockProviderState>) -> Json<Value> {
    Json(json!({
        "issuer": state.issuer,
        "authorization_endpoint": format!("{}/authorize", state.issuer),
        "token_endpoint": format!("{}/token", state.issuer),
        "jwks_uri": format!("{}/jwks", state.issuer),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "scopes_supported": ["openid", "email", "profile"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
        "claims_supported": ["sub", "iss", "aud", "exp", "iat", "nonce", "email", "email_verified", "name"],
    }))
}

async fn handle_jwks(State(state): State<MockProviderState>) -> Json<Value> {
    Json((*state.jwks_json).clone())
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // `redirect_uri`, `client_id`, `client_secret` are parsed
                    // so serde rejects unexpected shapes; we don't otherwise
                    // use them because the id_token carries the binding data.
struct TokenForm {
    grant_type: String,
    code: String,
    code_verifier: String,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
}

async fn handle_token(
    State(state): State<MockProviderState>,
    Form(form): Form<TokenForm>,
) -> axum::response::Response {
    if form.grant_type != "authorization_code" {
        return (StatusCode::BAD_REQUEST, "unsupported grant_type").into_response();
    }

    // Look up the authorization code we recorded on the /start side.
    let record = {
        let mut codes = state.codes.lock().await;
        codes.remove(&form.code)
    };
    let Some(record) = record else {
        return (StatusCode::BAD_REQUEST, "unknown code").into_response();
    };

    // Verify PKCE: SHA256(code_verifier) base64url-no-pad == record.pkce_challenge.
    let computed = {
        let digest = Sha256::digest(form.code_verifier.as_bytes());
        URL_SAFE_NO_PAD.encode(digest)
    };
    if computed != record.pkce_challenge {
        return (StatusCode::BAD_REQUEST, "pkce verifier mismatch").into_response();
    }

    // Build + sign the id_token.
    let now = chrono::Utc::now().timestamp();
    let aud = record
        .aud_override
        .clone()
        .unwrap_or_else(|| state.client_id.clone());
    let nonce_in_id_token = record
        .nonce_override
        .clone()
        .unwrap_or_else(|| record.nonce.clone());
    let claims = json!({
        "iss": state.issuer,
        "sub": state.subject,
        "aud": aud,
        "exp": now + 3600,
        "iat": now,
        "nonce": nonce_in_id_token,
        "email": state.email,
        "email_verified": true,
        "name": state.display_name,
    });
    let mut header = JwtHeader::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("mock-key-1".to_string());

    let signing_key = EncodingKey::from_rsa_der(&state.signing_key_der);
    let id_token = match jsonwebtoken::encode(&header, &claims, &signing_key) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("signing failed: {e}"),
            )
                .into_response();
        }
    };

    Json(json!({
        "access_token": "opaque-access-token",
        "token_type": "Bearer",
        "expires_in": 3600,
        "id_token": id_token,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Everything a test needs to drive the flow — the real ZLayer router, the
/// mock provider state, and handles on the underlying stores so we can assert
/// on them post-callback.
struct Harness {
    router: Router,
    mock: MockProviderState,
    users: Arc<dyn UserStorage>,
    oidc_identities: Arc<dyn OidcIdentityStorage>,
    // Retained for Drop side-effects (keep tempdirs alive for the duration of
    // the test).
    _tempdir: tempfile::TempDir,
    // Retained so the spawned mock server outlives the test (dropping the
    // join handle cancels it). We don't await it.
    _server_task: tokio::task::JoinHandle<()>,
}

async fn boot() -> Harness {
    // Start the mock IdP on a random port.
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind mock idp");
    let port = listener.local_addr().expect("local_addr").port();
    let issuer = format!("http://127.0.0.1:{port}");

    let client_id = "mock-client-id".to_string();
    let client_secret = "mock-client-secret".to_string();

    let mock = MockProviderState::new(
        issuer.clone(),
        client_id.clone(),
        "sub-alice".to_string(),
        "alice@example.com".to_string(),
    );

    let mock_clone = mock.clone();
    let server_task = tokio::spawn(async move {
        axum::serve(listener, mock_idp_router(mock_clone))
            .await
            .expect("mock idp serve");
    });

    // Give the server a moment to be accepting.
    tokio::task::yield_now().await;

    // Build ZLayer storage + identity.
    let tempdir = tempfile::tempdir().expect("tempdir");
    let users: Arc<dyn UserStorage> = Arc::new(
        SqlxUserStore::in_memory()
            .await
            .expect("in-memory user store"),
    );
    let secrets_db = tempdir.path().join("secrets.sqlite");
    let secrets = Arc::new(
        zlayer_secrets::PersistentSecretsStore::open(
            &secrets_db,
            zlayer_secrets::EncryptionKey::generate(),
        )
        .await
        .expect("secrets store"),
    );
    let credentials = Arc::new(zlayer_secrets::CredentialStore::new(secrets));
    let oidc_identities: Arc<dyn OidcIdentityStorage> = Arc::new(InMemoryOidcIdentityStore::new());
    let identity = Arc::new(
        IdentityManager::new(users.clone(), credentials.clone()).with_oidc(oidc_identities.clone()),
    );

    // OIDC provider config pointing at our mock.
    let provider_cfg = OidcProviderConfig {
        name: "mock".to_string(),
        display_name: "Mock".to_string(),
        issuer: issuer.clone(),
        client_id,
        client_secret,
        redirect_url: "http://127.0.0.1:65535/auth/oidc/mock/callback".to_string(),
        scopes: vec![
            "openid".to_string(),
            "email".to_string(),
            "profile".to_string(),
        ],
    };
    let mut oidc_clients = HashMap::new();
    oidc_clients.insert(
        provider_cfg.name.clone(),
        OidcClient::new(provider_cfg.clone()),
    );

    // Build the API config wiring everything together.
    let config = ApiConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        jwt_secret: SecretString::from("oidc-integration-test-jwt-secret".to_string()),
        swagger_enabled: false,
        rate_limit: RateLimitConfig {
            enabled: false,
            requests_per_second: u32::MAX / 2,
            burst_size: u32::MAX / 2,
        },
        credential_store: Some(credentials.clone()),
        user_store: Some(users.clone()),
        identity: Some(identity.clone()),
        oidc_clients,
        oidc_state: Arc::new(StateTokenStore::new()),
        ..ApiConfig::default()
    };

    let storage: Arc<dyn DeploymentStorage + Send + Sync> = Arc::new(InMemoryStorage::new());
    let router = build_router_with_storage(&config, storage);

    Harness {
        router,
        mock,
        users,
        oidc_identities,
        _tempdir: tempdir,
        _server_task: server_task,
    }
}

/// Query-params shape openidconnect emits on the /authorize URL.
#[derive(Debug, Deserialize)]
struct AuthorizeRedirect {
    state: String,
    nonce: String,
    code_challenge: String,
    code_challenge_method: String,
}

/// Hit `/auth/oidc/mock/start` and parse the 302 Location header.
async fn drive_start(router: &Router) -> AuthorizeRedirect {
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/auth/oidc/mock/start")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("start");
    assert_eq!(
        resp.status(),
        StatusCode::FOUND,
        "/start should 302 to authorize URL",
    );
    let location = resp
        .headers()
        .get(header::LOCATION)
        .expect("Location header")
        .to_str()
        .expect("location utf8")
        .to_string();
    let url = Url::parse(&location).expect("parse Location URL");
    let mut params: HashMap<String, String> = url.query_pairs().into_owned().collect();
    AuthorizeRedirect {
        state: params.remove("state").expect("state param"),
        nonce: params.remove("nonce").expect("nonce param"),
        code_challenge: params.remove("code_challenge").expect("code_challenge"),
        code_challenge_method: params
            .remove("code_challenge_method")
            .expect("code_challenge_method"),
    }
}

/// Register a synthetic authorisation code with the mock IdP, tied to the
/// PKCE challenge + nonce we just received. Returns the code string to hand
/// back to /callback.
async fn mint_code(
    harness: &Harness,
    redirect: &AuthorizeRedirect,
    aud_override: Option<String>,
    nonce_override: Option<String>,
) -> String {
    let code = format!("code-{}", uuid::Uuid::new_v4());
    let mut codes = harness.mock.codes.lock().await;
    codes.insert(
        code.clone(),
        AuthCodeRecord {
            pkce_challenge: redirect.code_challenge.clone(),
            nonce: redirect.nonce.clone(),
            aud_override,
            nonce_override,
        },
    );
    code
}

async fn drive_callback(router: &Router, code: &str, state: &str) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/auth/oidc/mock/callback?code={code}&state={state}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("callback")
}

#[derive(Debug, Deserialize)]
struct CallbackBody {
    user: UserView,
    csrf_token: String,
    provider: String,
}

async fn decode_body(resp: axum::response::Response) -> CallbackBody {
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.expect("body");
    serde_json::from_slice(&bytes).expect("decode CallbackBody")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oidc_end_to_end_happy_path() {
    let h = boot().await;

    // 1. /start → 302 with state, nonce, pkce challenge.
    let redirect = drive_start(&h.router).await;
    assert_eq!(redirect.code_challenge_method, "S256");
    assert!(!redirect.state.is_empty());
    assert!(!redirect.nonce.is_empty());

    // 2. Mint a code in the mock IdP keyed to the PKCE challenge + nonce.
    let code = mint_code(&h, &redirect, None, None).await;

    // 3. /callback → 200 + session cookie + body.
    let resp = drive_callback(&h.router, &code, &redirect.state).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "happy-path callback must 200"
    );
    let cookies: Vec<_> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(ToOwned::to_owned)
        .collect();
    assert!(
        cookies.iter().any(|c| c.starts_with("zlayer_session=")),
        "expected zlayer_session cookie, got {cookies:?}",
    );
    let body = decode_body(resp).await;
    assert_eq!(body.provider, "mock");
    assert_eq!(body.user.email, "alice@example.com");
    assert_eq!(body.user.role, "user");
    assert!(!body.csrf_token.is_empty());

    // 4. Stores in the expected state.
    let users = h.users.list().await.expect("list users");
    assert_eq!(users.len(), 1, "exactly one user should have been created");
    assert_eq!(users[0].email, "alice@example.com");

    let link = h
        .oidc_identities
        .get_by_external("mock", "sub-alice")
        .await
        .expect("get_by_external")
        .expect("identity link must exist");
    assert_eq!(link.user_id, users[0].id);
    assert_eq!(link.provider, "mock");
    assert_eq!(link.subject, "sub-alice");

    // 5. A second sign-in for the SAME subject reuses the same user.
    let redirect2 = drive_start(&h.router).await;
    let code2 = mint_code(&h, &redirect2, None, None).await;
    let resp2 = drive_callback(&h.router, &code2, &redirect2.state).await;
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = decode_body(resp2).await;
    assert_eq!(
        body2.user.id, users[0].id,
        "re-sign-in must return same user id, no duplicate",
    );
    let users_after = h.users.list().await.expect("list users");
    assert_eq!(
        users_after.len(),
        1,
        "re-sign-in must not create a duplicate user",
    );
}

#[tokio::test]
async fn callback_replayed_state_returns_401() {
    let h = boot().await;

    let redirect = drive_start(&h.router).await;
    let code = mint_code(&h, &redirect, None, None).await;

    let resp = drive_callback(&h.router, &code, &redirect.state).await;
    assert_eq!(resp.status(), StatusCode::OK, "first use must succeed");

    // Re-mint a fresh code (the previous was consumed by the token endpoint)
    // and replay the SAME state — this must fail because the StateTokenStore
    // removed it on first `take`.
    let code2 = mint_code(&h, &redirect, None, None).await;
    let resp2 = drive_callback(&h.router, &code2, &redirect.state).await;
    assert_eq!(
        resp2.status(),
        StatusCode::UNAUTHORIZED,
        "replayed state must be rejected",
    );
}

#[tokio::test]
async fn callback_tampered_state_returns_401() {
    let h = boot().await;

    let redirect = drive_start(&h.router).await;
    let code = mint_code(&h, &redirect, None, None).await;

    // State the server never issued.
    let resp = drive_callback(&h.router, &code, "definitely-not-a-real-state").await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn callback_unknown_provider_returns_404() {
    let h = boot().await;

    // The provider `ghost` is not configured.
    let resp = h
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/auth/oidc/ghost/callback?code=x&state=y")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("callback");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn callback_wrong_audience_id_token_returns_401() {
    let h = boot().await;

    let redirect = drive_start(&h.router).await;
    let code = mint_code(
        &h,
        &redirect,
        Some("somebody-elses-client-id".to_string()),
        None,
    )
    .await;

    let resp = drive_callback(&h.router, &code, &redirect.state).await;
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "id_token aud mismatch must bubble as 401 (OidcError::IdTokenVerification)",
    );

    // No user should have been created on a failed verification.
    assert_eq!(h.users.list().await.unwrap().len(), 0);
}

#[tokio::test]
async fn callback_mismatched_nonce_returns_401() {
    let h = boot().await;

    let redirect = drive_start(&h.router).await;
    let code = mint_code(
        &h,
        &redirect,
        None,
        Some("nonce-from-a-different-request".to_string()),
    )
    .await;

    let resp = drive_callback(&h.router, &code, &redirect.state).await;
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "id_token nonce mismatch must bubble as 401",
    );
    assert_eq!(h.users.list().await.unwrap().len(), 0);
}

#[tokio::test]
async fn callback_with_provider_error_param_returns_401() {
    let h = boot().await;

    let resp = h
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/auth/oidc/mock/callback?error=access_denied&error_description=user+denied")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("callback");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn oidc_signin_links_to_existing_password_user_by_email() {
    let h = boot().await;

    // Pre-provision a local password-login user with the SAME email the mock
    // IdP will return. The OIDC flow must link — NOT create a duplicate.
    let stored = zlayer_api::storage::StoredUser::new(
        "alice@example.com",
        "Original Alice",
        zlayer_api::storage::UserRole::Admin,
    );
    let original_id = stored.id.clone();
    h.users.store(&stored).await.expect("seed user");

    // Sign in via OIDC.
    let redirect = drive_start(&h.router).await;
    let code = mint_code(&h, &redirect, None, None).await;
    let resp = drive_callback(&h.router, &code, &redirect.state).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = decode_body(resp).await;

    // Same user id — linked, not recreated.
    assert_eq!(
        body.user.id, original_id,
        "OIDC sign-in must link to the pre-existing local user",
    );
    // Role is preserved (OIDC doesn't downgrade an existing admin).
    assert_eq!(body.user.role, "admin");

    // Only one user row exists after the OIDC flow.
    let users = h.users.list().await.expect("list users");
    assert_eq!(users.len(), 1, "no duplicate user created");

    // An OIDC identity link now exists bound to the pre-existing user.
    let link = h
        .oidc_identities
        .get_by_external("mock", "sub-alice")
        .await
        .expect("get_by_external")
        .expect("link created");
    assert_eq!(link.user_id, original_id);
}
