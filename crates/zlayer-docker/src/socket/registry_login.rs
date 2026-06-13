//! Docker Engine API `POST /auth` (docker login).
//!
//! Real dockerd validates the submitted credentials against the named
//! registry and answers `200 {"Status": "Login Succeeded", ...}` or
//! `401 {"message": "..."}`. Before this module existed the shim had no
//! `/auth` route at all, so `docker login` (and Komodo's registry login
//! probe) fell through to axum's bare 404 — an empty non-JSON body that
//! Docker clients then failed to parse.
//!
//! Validation follows the registry v2 handshake:
//!
//! 1. `GET https://{host}/v2/` — a `2xx` means the registry needs no
//!    auth; a `401` carries a `WWW-Authenticate` challenge.
//! 2. `Bearer realm=...,service=...` challenges are answered by fetching
//!    a token from the realm with HTTP basic credentials; `Basic`
//!    challenges by retrying `/v2/` with basic credentials.
//! 3. A `401`/`403` from the challenge endpoint means bad credentials; a
//!    `2xx` means the login is valid.
//!
//! On success the credential is also persisted in the daemon's registry
//! credential store (`POST /api/v1/credentials/registry`), replacing any
//! existing entry for the same registry, so native zlayer pulls pick the
//! credential up too. Persistence is best-effort: dockerd's `/auth`
//! contract is *validation*, so a store failure logs a warning but does
//! not turn a proven-valid login into an error.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};

use super::auth::RegistryAuth;
use super::system::error_response;
use super::SocketState;

/// `/auth` route.
pub(crate) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/auth", post(auth_login))
        .with_state(state)
}

/// Outcome of probing a registry with the submitted credentials.
#[derive(Debug)]
enum LoginOutcome {
    /// Credentials accepted (or the registry requires no auth).
    Success,
    /// The registry rejected the credentials.
    BadCredentials(String),
    /// The registry could not be reached or answered out-of-protocol.
    Unreachable(String),
}

/// `POST /auth` — validate registry credentials (docker login).
async fn auth_login(State(state): State<SocketState>, body: Bytes) -> Response {
    // Parse the AuthConfig body leniently: the docker CLI sends
    // `application/json`, but other clients omit the Content-Type, so we
    // take raw bytes instead of the strict `Json` extractor.
    let auth: RegistryAuth = match serde_json::from_slice(&body) {
        Ok(a) => a,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("invalid auth config body: {e}"),
            );
        }
    };

    let username = auth.username.as_deref().unwrap_or("").trim().to_string();
    // docker login submits username+password; treat an identity token as
    // the password when no password is present (registries accept PATs
    // and OAuth tokens as basic-auth passwords).
    let secret = auth
        .password
        .as_deref()
        .filter(|p| !p.is_empty())
        .or(auth.identity_token.as_deref())
        .unwrap_or("")
        .to_string();
    if username.is_empty() || secret.is_empty() {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "login requires a username and a password (or identity token)",
        );
    }

    let server_address = auth.server_address.as_deref().unwrap_or("");
    let ping_host = registry_ping_host(server_address);

    match validate_registry_login(&ping_host, &username, &secret).await {
        LoginOutcome::Success => {}
        LoginOutcome::BadCredentials(msg) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                format!("login attempt to https://{ping_host}/v2/ failed: {msg}"),
            );
        }
        LoginOutcome::Unreachable(msg) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("error contacting registry https://{ping_host}/v2/: {msg}"),
            );
        }
    }

    // Persist (replace-then-create) so native zlayer pulls use the
    // credential too. Best-effort by design — see module docs.
    let storage_registry = registry_storage_key(server_address);
    persist_credential(&state, &storage_registry, &username, &secret).await;

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "Status": "Login Succeeded",
            "IdentityToken": "",
        })),
    )
        .into_response()
}

/// Store the validated credential in the daemon, replacing any existing
/// entry for the same registry so re-login rotates the stored secret.
async fn persist_credential(state: &SocketState, registry: &str, username: &str, secret: &str) {
    match state.client.list_registry_credentials().await {
        Ok(existing) => {
            for cred in existing {
                let same_registry = cred
                    .get("registry")
                    .and_then(|v| v.as_str())
                    .is_some_and(|r| r == registry);
                if !same_registry {
                    continue;
                }
                if let Some(id) = cred.get("id").and_then(|v| v.as_str()) {
                    if let Err(e) = state.client.delete_registry_credential(id).await {
                        tracing::warn!(
                            registry,
                            id,
                            error = %e,
                            "docker /auth: could not replace existing registry credential"
                        );
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                registry,
                error = %e,
                "docker /auth: could not list registry credentials before store"
            );
        }
    }

    let body = serde_json::json!({
        "registry": registry,
        "username": username,
        "password": secret,
        "auth_type": "basic",
    });
    if let Err(e) = state.client.create_registry_credential(&body).await {
        tracing::warn!(
            registry,
            error = %e,
            "docker /auth: login validated but storing the credential in the daemon failed"
        );
    }
}

/// Probe `host` with the registry v2 handshake using `username`/`secret`.
async fn validate_registry_login(host: &str, username: &str, secret: &str) -> LoginOutcome {
    let client = match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(e) => return LoginOutcome::Unreachable(format!("building HTTP client: {e}")),
    };

    // Plain-HTTP fallback only for loopback registries (mirrors Docker's
    // implicit insecure-registry handling for localhost).
    let mut bases = vec![format!("https://{host}")];
    if is_loopback_host(host) {
        bases.push(format!("http://{host}"));
    }

    let mut last_err = String::new();
    for base in bases {
        match probe_base(&client, &base, username, secret).await {
            Ok(outcome) => return outcome,
            Err(e) => last_err = e,
        }
    }
    LoginOutcome::Unreachable(last_err)
}

/// Run the v2 handshake against one base URL. `Err` means "this base was
/// unreachable, try the next"; `Ok` carries the definitive outcome.
async fn probe_base(
    client: &reqwest::Client,
    base: &str,
    username: &str,
    secret: &str,
) -> Result<LoginOutcome, String> {
    let ping_url = format!("{base}/v2/");
    let resp = client
        .get(&ping_url)
        .send()
        .await
        .map_err(|e| format!("{e}"))?;

    let status = resp.status();
    if status.is_success() {
        // Open registry: nothing to validate against, login trivially OK.
        return Ok(LoginOutcome::Success);
    }
    if status != reqwest::StatusCode::UNAUTHORIZED {
        return Ok(LoginOutcome::Unreachable(format!(
            "unexpected status {status} from {ping_url}"
        )));
    }

    let challenge = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_default();

    if let Some(params) = parse_bearer_challenge(&challenge) {
        let Some(realm) = params.realm else {
            return Ok(LoginOutcome::Unreachable(format!(
                "bearer challenge from {ping_url} has no realm"
            )));
        };
        let mut token_url = url::Url::parse(&realm)
            .map_err(|e| format!("bearer realm {realm} is not a valid URL: {e}"))?;
        {
            let mut pairs = token_url.query_pairs_mut();
            if let Some(service) = params.service.as_deref() {
                pairs.append_pair("service", service);
            }
            pairs.append_pair("account", username);
        }
        let token_resp = client
            .get(token_url)
            .basic_auth(username, Some(secret))
            .send()
            .await
            .map_err(|e| format!("{e}"))?;
        let token_status = token_resp.status();
        if token_status.is_success() {
            return Ok(LoginOutcome::Success);
        }
        if token_status == reqwest::StatusCode::UNAUTHORIZED
            || token_status == reqwest::StatusCode::FORBIDDEN
        {
            return Ok(LoginOutcome::BadCredentials(format!(
                "incorrect username or password (token endpoint answered {token_status})"
            )));
        }
        return Ok(LoginOutcome::Unreachable(format!(
            "token endpoint {realm} answered {token_status}"
        )));
    }

    // Basic challenge (or no recognisable challenge): retry the ping with
    // basic credentials.
    let basic_resp = client
        .get(&ping_url)
        .basic_auth(username, Some(secret))
        .send()
        .await
        .map_err(|e| format!("{e}"))?;
    let basic_status = basic_resp.status();
    if basic_status.is_success() {
        return Ok(LoginOutcome::Success);
    }
    if basic_status == reqwest::StatusCode::UNAUTHORIZED
        || basic_status == reqwest::StatusCode::FORBIDDEN
    {
        return Ok(LoginOutcome::BadCredentials(
            "incorrect username or password".to_string(),
        ));
    }
    Ok(LoginOutcome::Unreachable(format!(
        "unexpected status {basic_status} from {ping_url} with basic credentials"
    )))
}

/// Parameters of a `WWW-Authenticate: Bearer ...` challenge.
#[derive(Debug, Default, PartialEq, Eq)]
struct BearerChallenge {
    realm: Option<String>,
    service: Option<String>,
}

/// Parse a `Bearer realm="...",service="..."` challenge. Returns `None`
/// when the header doesn't carry the `Bearer` scheme.
fn parse_bearer_challenge(header: &str) -> Option<BearerChallenge> {
    let rest = header.trim();
    let lower = rest.to_ascii_lowercase();
    let params = if let Some(stripped) = lower.strip_prefix("bearer") {
        // Preserve the original casing of the parameter VALUES by
        // slicing the untouched string at the same offset.
        let offset = rest.len() - stripped.len();
        &rest[offset..]
    } else {
        return None;
    };

    let mut out = BearerChallenge::default();
    for part in params.split(',') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().trim_matches('"');
        match key.as_str() {
            "realm" => out.realm = Some(value.to_string()),
            "service" => out.service = Some(value.to_string()),
            _ => {}
        }
    }
    Some(out)
}

/// Host to run the v2 handshake against for a docker-login
/// `serveraddress`. Docker Hub aliases (including the legacy
/// `https://index.docker.io/v1/` form the docker CLI sends) all resolve
/// to `registry-1.docker.io`; everything else is the bare `host[:port]`.
fn registry_ping_host(server_address: &str) -> String {
    let host = strip_scheme_and_path(server_address);
    if host.is_empty() || is_docker_hub_alias(&host) {
        "registry-1.docker.io".to_string()
    } else {
        host
    }
}

/// Registry key used when persisting the credential in the daemon's
/// store. Mirrors the convention of `zlayer credential registry add`
/// (bare hostname); Hub aliases collapse to `docker.io`, which the
/// daemon's docker-config sync re-normalises to the canonical
/// `https://index.docker.io/v1/` auths key.
fn registry_storage_key(server_address: &str) -> String {
    let host = strip_scheme_and_path(server_address);
    if host.is_empty() || is_docker_hub_alias(&host) {
        "docker.io".to_string()
    } else {
        host
    }
}

/// Drop a `scheme://` prefix and any `/path` suffix from a registry
/// address, leaving `host[:port]`.
fn strip_scheme_and_path(address: &str) -> String {
    let no_scheme = address
        .trim()
        .strip_prefix("https://")
        .or_else(|| address.trim().strip_prefix("http://"))
        .unwrap_or(address.trim());
    no_scheme
        .split_once('/')
        .map_or(no_scheme, |(host, _)| host)
        .to_string()
}

/// Hub spellings that all mean "Docker Hub".
fn is_docker_hub_alias(host: &str) -> bool {
    matches!(
        host,
        "docker.io" | "index.docker.io" | "registry-1.docker.io" | "registry.hub.docker.com"
    )
}

/// True for hosts where a plain-HTTP fallback is acceptable (local dev
/// registries).
fn is_loopback_host(host: &str) -> bool {
    let bare = host.split(':').next().unwrap_or(host);
    bare == "localhost" || bare == "127.0.0.1" || bare == "::1" || bare == "[::1]"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_challenge_parses_realm_and_service() {
        let parsed = parse_bearer_challenge(
            r#"Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:foo:pull""#,
        )
        .expect("bearer scheme");
        assert_eq!(parsed.realm.as_deref(), Some("https://ghcr.io/token"));
        assert_eq!(parsed.service.as_deref(), Some("ghcr.io"));
    }

    #[test]
    fn bearer_challenge_case_insensitive_scheme() {
        let parsed = parse_bearer_challenge(r#"bearer realm="https://auth.example/t""#)
            .expect("lowercase scheme accepted");
        assert_eq!(parsed.realm.as_deref(), Some("https://auth.example/t"));
    }

    #[test]
    fn basic_challenge_is_not_bearer() {
        assert!(parse_bearer_challenge(r#"Basic realm="registry""#).is_none());
        assert!(parse_bearer_challenge("").is_none());
    }

    #[test]
    fn ping_host_maps_hub_aliases() {
        for addr in [
            "",
            "docker.io",
            "index.docker.io",
            "registry-1.docker.io",
            "https://index.docker.io/v1/",
        ] {
            assert_eq!(registry_ping_host(addr), "registry-1.docker.io", "{addr}");
        }
        assert_eq!(registry_ping_host("ghcr.io"), "ghcr.io");
        assert_eq!(
            registry_ping_host("https://forge.example.com/v2/"),
            "forge.example.com"
        );
        assert_eq!(registry_ping_host("localhost:5000"), "localhost:5000");
    }

    #[test]
    fn storage_key_collapses_hub_aliases() {
        for addr in ["", "https://index.docker.io/v1/", "docker.io"] {
            assert_eq!(registry_storage_key(addr), "docker.io", "{addr}");
        }
        assert_eq!(registry_storage_key("ghcr.io"), "ghcr.io");
        assert_eq!(registry_storage_key("localhost:5000"), "localhost:5000");
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_host("localhost:5000"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(!is_loopback_host("ghcr.io"));
        assert!(!is_loopback_host("registry.internal:5000"));
    }

    /// Full handshake against an in-process mock registry: bearer
    /// challenge, then the token endpoint accepts one credential pair
    /// and rejects everything else.
    #[tokio::test]
    async fn validate_login_bearer_flow() {
        use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // good credentials: alice / hunter2 (base64 of "alice:hunter2")
        let expected = format!(
            "Basic {}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"alice:hunter2")
        );
        let expected_for_token = expected.clone();

        let app = Router::new()
            .route(
                "/v2/",
                axum::routing::get(move || {
                    let realm = format!("http://{addr}/token");
                    async move {
                        (
                            StatusCode::UNAUTHORIZED,
                            [(
                                WWW_AUTHENTICATE,
                                format!(r#"Bearer realm="{realm}",service="mock""#),
                            )],
                            "",
                        )
                    }
                }),
            )
            .route(
                "/token",
                axum::routing::get(move |headers: axum::http::HeaderMap| {
                    let expected = expected_for_token.clone();
                    async move {
                        let authz = headers
                            .get(AUTHORIZATION)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("");
                        if authz == expected {
                            (StatusCode::OK, r#"{"token":"mock-token"}"#).into_response()
                        } else {
                            (StatusCode::UNAUTHORIZED, "").into_response()
                        }
                    }
                }),
            );

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let host = addr.to_string();
        // 127.0.0.1 gets the plain-HTTP fallback, so the mock needs no TLS.
        match validate_registry_login(&host, "alice", "hunter2").await {
            LoginOutcome::Success => {}
            other => panic!("expected Success, got {other:?}"),
        }
        match validate_registry_login(&host, "alice", "wrong").await {
            LoginOutcome::BadCredentials(_) => {}
            other => panic!("expected BadCredentials, got {other:?}"),
        }
    }

    /// An open registry (200 on /v2/) accepts any login.
    #[tokio::test]
    async fn validate_login_open_registry() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route("/v2/", axum::routing::get(|| async { "{}" }));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        match validate_registry_login(&addr.to_string(), "anyone", "anything").await {
            LoginOutcome::Success => {}
            other => panic!("expected Success, got {other:?}"),
        }
    }
}
