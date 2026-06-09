//! Reqwest-based [`SecretsClient`] implementation backed by the `ZLayer`
//! Secrets HTTP API.
//!
//! This is the concrete client that lets a consumer (`ZBilling`, `ZRegistry`,
//! …) read secrets out of a running `ZLayer` Secrets daemon (`zlayer serve
//! --secrets-only`) instead of process env vars. It implements the shared
//! [`SecretsClient`] trait from [`secrets_client_api`], so swapping an
//! `EnvVarSecretsClient` for a `ZLayerSecretsClient` is a one-line change at
//! the construction site.
//!
//! ## Wire protocol
//!
//! A `get(scope, name)` issues:
//!
//! ```text
//! GET {base_url}/api/v1/secrets/{name}?scope={scope.as_str()}&reveal=true
//! Authorization: Bearer {token}
//! ```
//!
//! against the route served by `zlayer_api::handlers::secrets::get_secret_metadata`.
//! The mapping is:
//!
//! - `200 OK` with `{"value": "<plaintext>", ...}` → `Ok(Some(Secret))`.
//! - `200 OK` with `value == null` → the daemon returned metadata but
//!   withheld the plaintext (e.g. the bearer identity lacks reveal rights on
//!   a legacy, non-env scope). That is a backend authorization failure for a
//!   client whose entire job is to read values, so it maps to
//!   `Err(SecretsError::ReadFailed(_))` rather than silently looking like a
//!   miss.
//! - `404 Not Found` → `Ok(None)` (absent secret; consumer decides if that
//!   is fatal).
//! - `401` / `403` → `Err(SecretsError::ReadFailed(_))` (auth/permission).
//! - any other non-success status, or a transport error → `Err(ReadFailed)`.
//!
//! ## Scope mapping
//!
//! [`SecretScope`] maps to the `ZLayer` scope string via
//! [`SecretScope::as_str`], i.e. `Stripe → "stripe"`, `License → "license"`,
//! `ZRelay → "zrelay"`, `Postgres → "postgres"`, `Custom(s) → s`. The daemon
//! treats the scope as an opaque namespace, so any deployment that writes its
//! secrets under those scope strings (`zlayer secret set --scope stripe …`)
//! is readable through this client unchanged.

use async_trait::async_trait;
use secrets_client_api::{Secret, SecretScope, SecretsClient, SecretsError};
use serde::Deserialize;

/// Minimal projection of the daemon's `SecretMetadataResponse`. We only need
/// the plaintext `value`, which is populated when the request carried
/// `reveal=true` and the bearer identity is authorized to reveal.
#[derive(Debug, Deserialize)]
struct SecretRevealResponse {
    /// Present (and non-null) only when the daemon revealed the plaintext.
    #[serde(default)]
    value: Option<String>,
}

/// Client for the `ZLayer` Secrets HTTP API.
///
/// Cheap to clone — the inner [`reqwest::Client`] is `Arc`-backed and shares a
/// connection pool across clones. Hold one per process and share it.
#[derive(Clone)]
pub struct ZLayerSecretsClient {
    http: reqwest::Client,
    /// Base URL of the `ZLayer` Secrets API, with any trailing slash trimmed
    /// (e.g. `https://secrets.blackleafdigital.com`).
    base_url: String,
    /// Bearer token presented on every request (`Authorization: Bearer …`).
    bearer_token: String,
}

impl ZLayerSecretsClient {
    /// Construct a client against `base_url`, authenticating every request
    /// with `bearer_token`.
    ///
    /// `base_url` should be the scheme + host (+ optional port) of the
    /// `ZLayer` Secrets endpoint, e.g. `https://secrets.blackleafdigital.com`. A
    /// trailing slash is tolerated and trimmed.
    pub fn new(base_url: impl Into<String>, bearer_token: impl Into<String>) -> Self {
        Self::with_http_client(reqwest::Client::new(), base_url, bearer_token)
    }

    /// Construct a client reusing a caller-provided [`reqwest::Client`].
    ///
    /// Use this when the consumer already maintains a configured client
    /// (custom timeouts, proxy, root certs, …) and wants the secrets client
    /// to share that pool.
    pub fn with_http_client(
        http: reqwest::Client,
        base_url: impl Into<String>,
        bearer_token: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            http,
            base_url,
            bearer_token: bearer_token.into(),
        }
    }

    /// Build the reveal URL for `scope` + `name`.
    fn reveal_url(&self, scope: &SecretScope, name: &str) -> String {
        // `name` is a path segment; percent-encode the few characters that
        // would otherwise break the URL. Secret names are typically
        // `[a-z0-9-]`, but a defensive encode keeps `/`, `?`, `#`, space, and
        // `%` from corrupting the request line.
        let encoded_name = encode_path_segment(name);
        let encoded_scope = encode_query_value(scope.as_str());
        format!(
            "{}/api/v1/secrets/{}?scope={}&reveal=true",
            self.base_url, encoded_name, encoded_scope
        )
    }
}

#[async_trait]
impl SecretsClient for ZLayerSecretsClient {
    async fn get(&self, scope: &SecretScope, name: &str) -> Result<Option<Secret>, SecretsError> {
        let url = self.reveal_url(scope, name);

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.bearer_token)
            .send()
            .await
            .map_err(|e| {
                SecretsError::ReadFailed(format!(
                    "request to ZLayer Secrets failed (scope={}, name={name}): {e}",
                    scope.as_str()
                ))
            })?;

        let status = resp.status();

        // Absent secret → Ok(None). The consumer decides whether absence is
        // an error.
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !status.is_success() {
            // Pull the body for diagnostics, but never assume it is the
            // secret. Auth (401/403) and server (5xx) errors land here.
            let body = resp.text().await.unwrap_or_default();
            return Err(SecretsError::ReadFailed(format!(
                "ZLayer Secrets returned {status} for scope={}, name={name}: {}",
                scope.as_str(),
                truncate_for_log(&body),
            )));
        }

        let parsed: SecretRevealResponse = resp.json().await.map_err(|e| {
            SecretsError::ReadFailed(format!(
                "failed to decode ZLayer Secrets response (scope={}, name={name}): {e}",
                scope.as_str()
            ))
        })?;

        match parsed.value {
            Some(plaintext) => Ok(Some(Secret::new(plaintext))),
            // 200 but the daemon withheld the plaintext: the identity could
            // see the metadata but not reveal. For a value-reading client
            // that is an authorization failure, not a miss.
            None => Err(SecretsError::ReadFailed(format!(
                "ZLayer Secrets revealed no value for scope={}, name={name} \
                 (bearer identity lacks reveal permission?)",
                scope.as_str()
            ))),
        }
    }
}

/// Percent-encode the characters that would break a single URL **path
/// segment**. Intentionally conservative: it leaves the common
/// `unreserved` set (`A-Z a-z 0-9 - . _ ~`) untouched and encodes everything
/// else, which is correct for an opaque secret name used as one path segment.
fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex_upper(b >> 4));
            out.push(hex_upper(b & 0x0f));
        }
    }
    out
}

/// Percent-encode a query-string **value**. Same policy as
/// [`encode_path_segment`] — scopes are short identifiers, so encoding the
/// non-unreserved bytes is both correct and cheap.
fn encode_query_value(s: &str) -> String {
    encode_path_segment(s)
}

/// RFC 3986 `unreserved` set.
fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

fn hex_upper(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'A' + (nibble - 10)) as char,
    }
}

/// Clamp a body snippet so error logs from a misbehaving upstream don't blow
/// up. Never used for the secret value itself (which only comes from the
/// parsed `value` field on success).
fn truncate_for_log(s: &str) -> String {
    const MAX: usize = 256;
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…", &s[..MAX])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn get_returns_secret_on_200_with_value() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/v1/secrets/api-key"))
            .and(query_param("scope", "stripe"))
            .and(query_param("reveal", "true"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({ "name": "api-key", "value": "sk_live_abc" }),
                ),
            )
            .mount(&server)
            .await;

        let client = ZLayerSecretsClient::new(server.uri(), "test-token");
        let got = client
            .get(&SecretScope::Stripe, "api-key")
            .await
            .expect("get should succeed");
        assert_eq!(got.as_ref().map(Secret::expose), Some("sk_live_abc"));
    }

    #[tokio::test]
    async fn get_returns_none_on_404() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/v1/secrets/missing"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(serde_json::json!({ "error": "Secret 'missing' not found" })),
            )
            .mount(&server)
            .await;

        let client = ZLayerSecretsClient::new(server.uri(), "test-token");
        let got = client
            .get(&SecretScope::Stripe, "missing")
            .await
            .expect("404 should map to Ok(None)");
        assert!(got.is_none(), "expected None for 404, got {got:?}");
    }

    #[tokio::test]
    async fn get_maps_custom_scope_to_query() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/v1/secrets/token"))
            .and(query_param("scope", "zregistry"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "name": "token", "value": "v" })),
            )
            .mount(&server)
            .await;

        let client = ZLayerSecretsClient::new(server.uri(), "t");
        let got = client
            .get(&SecretScope::Custom("zregistry".into()), "token")
            .await
            .expect("custom scope should resolve");
        assert_eq!(got.as_ref().map(Secret::expose), Some("v"));
    }

    #[tokio::test]
    async fn get_errors_when_value_withheld_on_200() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/v1/secrets/api-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "name": "api-key", "value": null })),
            )
            .mount(&server)
            .await;

        let client = ZLayerSecretsClient::new(server.uri(), "t");
        let err = client
            .get(&SecretScope::Stripe, "api-key")
            .await
            .expect_err("withheld value should be an error");
        assert!(matches!(err, SecretsError::ReadFailed(_)));
    }

    #[tokio::test]
    async fn get_errors_on_403() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/api/v1/secrets/api-key"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&server)
            .await;

        let client = ZLayerSecretsClient::new(server.uri(), "t");
        let err = client
            .get(&SecretScope::Stripe, "api-key")
            .await
            .expect_err("403 should be an error");
        assert!(matches!(err, SecretsError::ReadFailed(_)));
    }

    #[test]
    fn reveal_url_trims_trailing_slash_and_encodes() {
        let client = ZLayerSecretsClient::new("https://secrets.example.com/", "tok");
        let url = client.reveal_url(&SecretScope::License, "signing-key");
        assert_eq!(
            url,
            "https://secrets.example.com/api/v1/secrets/signing-key?scope=license&reveal=true"
        );
    }

    #[test]
    fn reveal_url_percent_encodes_unsafe_name() {
        let client = ZLayerSecretsClient::new("http://h", "t");
        let url = client.reveal_url(&SecretScope::Custom("a/b".into()), "x y");
        assert_eq!(url, "http://h/api/v1/secrets/x%20y?scope=a%2Fb&reveal=true");
    }
}
