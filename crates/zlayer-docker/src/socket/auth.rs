//! Decoder for Docker Engine API `X-Registry-Auth` headers.
//!
//! The Docker CLI and other Docker-compatible clients pass registry
//! credentials as a base64-encoded JSON document in the `X-Registry-Auth`
//! HTTP header. The reference Go implementation (`engine-api`) accepts
//! either the standard or URL-safe base64 alphabets and tolerates missing
//! padding, so we mirror that behavior here.
//!
//! The decoded JSON has shape:
//! ```json
//! {
//!     "username": "...",
//!     "password": "...",
//!     "email": "...",
//!     "serveraddress": "...",
//!     "identitytoken": "..."
//! }
//! ```
//!
//! Any field may be absent. Either `password` or `identitytoken` (or both)
//! is supplied depending on whether the client is using basic credentials
//! or an OAuth-style identity token. `serveraddress` is the canonical
//! registry hostname.

use std::collections::HashMap;

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Name of the Docker Engine API authentication header.
pub const X_REGISTRY_AUTH_HEADER: &str = "X-Registry-Auth";

/// Name of the Docker Engine API multi-registry authentication header.
///
/// Sent by `docker build` and similar build calls when the build pulls base
/// images from multiple registries that each need their own credentials. The
/// value is the same shape as [`X_REGISTRY_AUTH_HEADER`] except the decoded
/// JSON is a map from `serveraddress` -> [`RegistryAuth`] payload.
pub const X_REGISTRY_CONFIG_HEADER: &str = "X-Registry-Config";

/// Decoded contents of an `X-Registry-Auth` header.
///
/// All fields are optional because Docker clients vary in which they
/// supply: basic-auth flows populate `username` and `password`, while
/// OAuth/identity-token flows populate `identity_token` and may omit
/// the password entirely.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct RegistryAuth {
    /// Registry username (basic auth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Registry password (basic auth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// Email address associated with the account (legacy field, often empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    /// Canonical registry hostname (e.g. `registry-1.docker.io`).
    #[serde(
        default,
        rename = "serveraddress",
        skip_serializing_if = "Option::is_none"
    )]
    pub server_address: Option<String>,

    /// OAuth identity token, used in lieu of a password for federated logins.
    #[serde(
        default,
        rename = "identitytoken",
        skip_serializing_if = "Option::is_none"
    )]
    pub identity_token: Option<String>,
}

/// Error returned when an `X-Registry-Auth` header cannot be decoded.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthDecodeError {
    /// The header value is not valid base64 in either standard or URL-safe alphabets.
    #[error("X-Registry-Auth header is not valid base64")]
    NotBase64,

    /// The decoded bytes do not parse as the expected JSON shape.
    #[error("X-Registry-Auth payload is not valid JSON")]
    InvalidJson,

    /// The header value is not valid UTF-8.
    #[error("X-Registry-Auth header is not valid UTF-8")]
    Utf8,
}

/// Decode the `X-Registry-Auth` header from a request, if present.
///
/// Returns `Ok(None)` when the header is absent, empty, or whitespace-only.
/// Returns `Ok(Some(_))` with parsed credentials when the header decodes
/// successfully, mirroring Docker's lenient base64 handling (standard
/// alphabet first, URL-safe fallback, padding inferred when missing).
///
/// # Errors
///
/// Returns [`AuthDecodeError`] when the header is present but cannot be
/// decoded as base64, the decoded bytes are not UTF-8, or the resulting
/// JSON does not match [`RegistryAuth`].
pub fn decode_x_registry_auth(
    headers: &http::HeaderMap,
) -> Result<Option<RegistryAuth>, AuthDecodeError> {
    let Some(raw) = headers.get(X_REGISTRY_AUTH_HEADER) else {
        return Ok(None);
    };

    let value = raw.to_str().map_err(|_| AuthDecodeError::Utf8)?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let decoded = decode_base64_lenient(trimmed).ok_or(AuthDecodeError::NotBase64)?;

    // Some Docker clients send `{}` to mean "no credentials". Preserve the
    // typed `Some(default)` return so callers can distinguish "header present
    // but empty" from "header absent".
    let auth: RegistryAuth =
        serde_json::from_slice(&decoded).map_err(|_| AuthDecodeError::InvalidJson)?;
    Ok(Some(auth))
}

/// Decode the `X-Registry-Config` header from a request, if present.
///
/// `X-Registry-Config` is the multi-registry counterpart to
/// [`X_REGISTRY_AUTH_HEADER`]: a base64-encoded JSON object whose keys are
/// canonical registry hostnames (matching the `serveraddress` field of each
/// nested entry) and whose values are individual [`RegistryAuth`] payloads.
/// `docker build` emits this header so the daemon can authenticate against
/// every registry referenced by `FROM` lines in a Dockerfile.
///
/// Returns `Ok(empty map)` when the header is absent, empty, or
/// whitespace-only — callers can distinguish "no auth" from "auth provided"
/// by checking `is_empty()` on the result.
///
/// # Errors
///
/// Returns [`AuthDecodeError`] when the header is present but cannot be
/// decoded as base64, the decoded bytes are not UTF-8, or the resulting JSON
/// does not match `HashMap<String, RegistryAuth>`.
pub fn decode_x_registry_config(
    headers: &http::HeaderMap,
) -> Result<HashMap<String, RegistryAuth>, AuthDecodeError> {
    let Some(raw) = headers.get(X_REGISTRY_CONFIG_HEADER) else {
        return Ok(HashMap::new());
    };

    let value = raw.to_str().map_err(|_| AuthDecodeError::Utf8)?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(HashMap::new());
    }

    let decoded = decode_base64_lenient(trimmed).ok_or(AuthDecodeError::NotBase64)?;
    let parsed: HashMap<String, RegistryAuth> =
        serde_json::from_slice(&decoded).map_err(|_| AuthDecodeError::InvalidJson)?;
    Ok(parsed)
}

/// Try every base64 alphabet/padding combination Docker is known to send.
///
/// The reference Go decoder (`base64.URLEncoding` with `RawStdEncoding`
/// fallback) accepts both alphabets and is permissive about padding, so
/// we attempt: padded standard, padded URL-safe, then their unpadded
/// counterparts. The first decoder that succeeds wins.
fn decode_base64_lenient(value: &str) -> Option<Vec<u8>> {
    if let Ok(bytes) = STANDARD.decode(value) {
        return Some(bytes);
    }
    if let Ok(bytes) = URL_SAFE.decode(value) {
        return Some(bytes);
    }
    if let Ok(bytes) = STANDARD_NO_PAD.decode(value) {
        return Some(bytes);
    }
    if let Ok(bytes) = URL_SAFE_NO_PAD.decode(value) {
        return Some(bytes);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    use http::{HeaderMap, HeaderValue};

    fn header_with(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            X_REGISTRY_AUTH_HEADER,
            HeaderValue::from_str(value).unwrap(),
        );
        headers
    }

    #[test]
    fn missing_header_returns_none() {
        let headers = HeaderMap::new();
        assert_eq!(decode_x_registry_auth(&headers).unwrap(), None);
    }

    #[test]
    fn empty_header_returns_none() {
        let headers = header_with("");
        assert_eq!(decode_x_registry_auth(&headers).unwrap(), None);
    }

    #[test]
    fn whitespace_only_header_returns_none() {
        let headers = header_with("   \t  ");
        assert_eq!(decode_x_registry_auth(&headers).unwrap(), None);
    }

    #[test]
    fn decodes_full_payload_with_all_fields() {
        let json = r#"{
            "username": "alice",
            "password": "hunter2",
            "email": "alice@example.com",
            "serveraddress": "registry-1.docker.io",
            "identitytoken": "tok-abc"
        }"#;
        let encoded = STANDARD.encode(json.as_bytes());
        let headers = header_with(&encoded);

        let auth = decode_x_registry_auth(&headers).unwrap().unwrap();
        assert_eq!(auth.username.as_deref(), Some("alice"));
        assert_eq!(auth.password.as_deref(), Some("hunter2"));
        assert_eq!(auth.email.as_deref(), Some("alice@example.com"));
        assert_eq!(auth.server_address.as_deref(), Some("registry-1.docker.io"));
        assert_eq!(auth.identity_token.as_deref(), Some("tok-abc"));
    }

    #[test]
    fn decodes_identity_token_only_payload() {
        let json = r#"{"identitytoken":"oauth-token-xyz","serveraddress":"ghcr.io"}"#;
        let encoded = STANDARD.encode(json.as_bytes());
        let headers = header_with(&encoded);

        let auth = decode_x_registry_auth(&headers).unwrap().unwrap();
        assert_eq!(auth.username, None);
        assert_eq!(auth.password, None);
        assert_eq!(auth.email, None);
        assert_eq!(auth.server_address.as_deref(), Some("ghcr.io"));
        assert_eq!(auth.identity_token.as_deref(), Some("oauth-token-xyz"));
    }

    #[test]
    fn decodes_url_safe_unpadded_alphabet() {
        // Bytes chosen so the standard alphabet would emit `+` and `/`,
        // forcing the URL-safe fallback path to be exercised.
        let json = r#"{"username":"u","password":"p?>?>"}"#;
        let encoded = URL_SAFE_NO_PAD.encode(json.as_bytes());
        let headers = header_with(&encoded);

        let auth = decode_x_registry_auth(&headers).unwrap().unwrap();
        assert_eq!(auth.username.as_deref(), Some("u"));
        assert_eq!(auth.password.as_deref(), Some("p?>?>"));
    }

    #[test]
    fn empty_json_object_is_default_struct() {
        let encoded = STANDARD.encode(b"{}");
        let headers = header_with(&encoded);
        let auth = decode_x_registry_auth(&headers).unwrap().unwrap();
        assert_eq!(auth, RegistryAuth::default());
    }

    #[test]
    fn malformed_base64_returns_not_base64() {
        // `!!!!` is not valid in either alphabet.
        let headers = header_with("!!!!not-base64!!!!");
        let err = decode_x_registry_auth(&headers).unwrap_err();
        assert_eq!(err, AuthDecodeError::NotBase64);
    }

    #[test]
    fn valid_base64_with_invalid_json_returns_invalid_json() {
        // Decodes cleanly to bytes that aren't a JSON object.
        let encoded = STANDARD.encode(b"not really json at all");
        let headers = header_with(&encoded);
        let err = decode_x_registry_auth(&headers).unwrap_err();
        assert_eq!(err, AuthDecodeError::InvalidJson);
    }

    fn header_with_config(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            X_REGISTRY_CONFIG_HEADER,
            HeaderValue::from_str(value).unwrap(),
        );
        headers
    }

    #[test]
    fn x_registry_config_missing_returns_empty_map() {
        let headers = HeaderMap::new();
        assert!(decode_x_registry_config(&headers).unwrap().is_empty());
    }

    #[test]
    fn x_registry_config_empty_value_returns_empty_map() {
        let headers = header_with_config("");
        assert!(decode_x_registry_config(&headers).unwrap().is_empty());
    }

    #[test]
    fn x_registry_config_decodes_multiple_registries() {
        // `docker build` sends one entry per registry that the build will
        // pull from. Verify the decoder preserves the per-registry mapping.
        let json = r#"{
            "registry-1.docker.io": {
                "username": "alice",
                "password": "hunter2",
                "serveraddress": "registry-1.docker.io"
            },
            "ghcr.io": {
                "identitytoken": "tok",
                "serveraddress": "ghcr.io"
            }
        }"#;
        let encoded = STANDARD.encode(json.as_bytes());
        let headers = header_with_config(&encoded);

        let map = decode_x_registry_config(&headers).unwrap();
        assert_eq!(map.len(), 2);

        let docker = map.get("registry-1.docker.io").expect("docker registry");
        assert_eq!(docker.username.as_deref(), Some("alice"));
        assert_eq!(docker.password.as_deref(), Some("hunter2"));

        let ghcr = map.get("ghcr.io").expect("ghcr registry");
        assert_eq!(ghcr.identity_token.as_deref(), Some("tok"));
    }

    #[test]
    fn x_registry_config_malformed_base64_returns_error() {
        let headers = header_with_config("!!!not base64!!!");
        let err = decode_x_registry_config(&headers).unwrap_err();
        assert_eq!(err, AuthDecodeError::NotBase64);
    }

    #[test]
    fn x_registry_config_invalid_json_returns_error() {
        let encoded = STANDARD.encode(b"\"not an object\"");
        let headers = header_with_config(&encoded);
        let err = decode_x_registry_config(&headers).unwrap_err();
        assert_eq!(err, AuthDecodeError::InvalidJson);
    }

    #[test]
    fn registry_auth_roundtrips_via_serde() {
        let auth = RegistryAuth {
            username: Some("alice".into()),
            password: None,
            email: None,
            server_address: Some("ghcr.io".into()),
            identity_token: Some("tok".into()),
        };
        let json = serde_json::to_string(&auth).unwrap();
        // Confirm rename attributes serialize back to the Docker field names.
        assert!(json.contains("\"serveraddress\":\"ghcr.io\""));
        assert!(json.contains("\"identitytoken\":\"tok\""));
        let parsed: RegistryAuth = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, auth);
    }
}
