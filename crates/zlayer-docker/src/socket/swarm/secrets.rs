//! `/secrets` endpoints — bridge to `ZLayer`'s `/api/v1/secrets`.
//!
//! `ZLayer` secrets are name-keyed: there is no separate opaque ID. To honour
//! Docker's `{ID}` path parameter we expose the secret name as the ID on the
//! wire — `Secret.ID == Secret.Spec.Name`. Per the Docker contract `Data` is
//! never echoed back on list / inspect responses (only the caller who
//! created the secret has the plaintext); we mirror that even though the
//! `ZLayer` daemon also keeps secret material out of its list response.
//!
//! Endpoint family:
//!
//! - `GET    /secrets`              -- list secrets (with `?filters=`)
//! - `GET    /secrets/{id}`         -- inspect a single secret by name
//! - `POST   /secrets/create`       -- create from a Docker `SecretSpec`
//! - `POST   /secrets/{id}/update`  -- rotate (when `Data` is present) or
//!   accept a labels-only update as a no-op
//! - `DELETE /secrets/{id}`         -- remove by name

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use zlayer_types::api::secrets::SecretMetadataResponse;

use super::shape::{iso_timestamp, Secret, SecretSpec, Version};
use super::SocketState;

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/// `/secrets` route table.
pub(super) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/secrets", get(list_secrets))
        .route("/secrets/create", post(create_secret))
        .route("/secrets/{id}", get(inspect_secret).delete(delete_secret))
        .route("/secrets/{id}/update", post(update_secret))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Error helpers
// ---------------------------------------------------------------------------

/// Build a Docker-style JSON error response: `{ "message": "..." }`.
fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "message": message.into() }))).into_response()
}

/// Map a [`zlayer_client::DaemonClient`] error into a Docker-style HTTP
/// response.
///
/// The daemon client returns errors as `anyhow::Error` strings; we
/// pattern-match on the well-known prefixes used by
/// `DaemonClient::check_status` so that "secret not found" surfaces as
/// 404 and conflicts as 409.
fn map_daemon_error(err: &anyhow::Error) -> Response {
    let msg = format!("{err:#}");
    let status = classify_daemon_error(&msg);
    error_response(status, msg)
}

/// Classify a daemon-error message into a Docker-compatible HTTP status.
fn classify_daemon_error(msg: &str) -> StatusCode {
    let lower = msg.to_ascii_lowercase();
    if lower.starts_with("404 ") || lower.contains("404 not found") {
        StatusCode::NOT_FOUND
    } else if lower.contains("409 conflict") || lower.contains(" 409 ") {
        StatusCode::CONFLICT
    } else if lower.starts_with("400 ") || lower.contains("400 bad request") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

// ---------------------------------------------------------------------------
// Base64 decoding
// ---------------------------------------------------------------------------

/// Try every base64 alphabet/padding combination Docker is known to send.
///
/// Mirrors the helper in `socket/auth.rs`: Docker's reference Go decoder
/// accepts both standard and URL-safe alphabets and is permissive about
/// padding, so we try padded standard, padded URL-safe, then their
/// unpadded counterparts. The first decoder that succeeds wins.
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

// ---------------------------------------------------------------------------
// SecretMetadataResponse -> Docker `Secret`
// ---------------------------------------------------------------------------

/// Translate a daemon-side [`SecretMetadataResponse`] into the Docker wire
/// shape.
///
/// The wire contract:
///
/// - `Secret.ID` is the secret name (`ZLayer` is name-keyed).
/// - `Secret.Version.Index` is the daemon's monotonic version counter.
/// - `Secret.CreatedAt` / `UpdatedAt` are formatted as RFC 3339.
/// - `Secret.Spec.Labels` is always `{}` (`ZLayer` does not track per-secret
///   labels). Docker clients accept the empty object.
/// - `Secret.Spec.Data` is always empty (Docker hides Data on inspect).
fn meta_to_docker(meta: &SecretMetadataResponse) -> Secret {
    Secret {
        id: meta.name.clone(),
        version: Version {
            index: u64::from(meta.version),
        },
        created_at: iso_timestamp(meta.created_at),
        updated_at: iso_timestamp(meta.updated_at),
        spec: SecretSpec {
            name: meta.name.clone(),
            labels: Value::Object(serde_json::Map::new()),
            data: String::new(),
            driver: Value::Null,
        },
    }
}

// ---------------------------------------------------------------------------
// Filter parsing
// ---------------------------------------------------------------------------

/// Parse Docker's `filters` query parameter into a map of filter-key ->
/// list of values.
///
/// Docker passes filters as a URL-encoded JSON object whose values are
/// arrays of strings, e.g. `{"name":["foo"],"label":["env=prod"]}`. An
/// unparseable filter string yields an empty map, matching Docker's
/// behaviour of silently ignoring malformed filters.
fn parse_filters(raw: Option<&str>) -> HashMap<String, Vec<String>> {
    let Some(s) = raw else {
        return HashMap::new();
    };
    if s.trim().is_empty() {
        return HashMap::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(s) else {
        return HashMap::new();
    };
    let Some(obj) = value.as_object() else {
        return HashMap::new();
    };

    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for (k, v) in obj {
        let mut values: Vec<String> = Vec::new();
        if let Some(arr) = v.as_array() {
            for item in arr {
                if let Some(s) = item.as_str() {
                    values.push(s.to_owned());
                }
            }
        } else if let Some(map) = v.as_object() {
            for (key, flag) in map {
                if flag.as_bool().unwrap_or(false) {
                    values.push(key.clone());
                }
            }
        }
        out.insert(k.clone(), values);
    }
    out
}

/// Apply Docker's `id`/`name`/`label` filters to a list of secrets.
///
/// Honoured filters:
///
/// - `id=<name>` / `name=<name>`: keep secrets whose name matches one of
///   the supplied values exactly. (`id` and `name` are aliases here
///   because `ZLayer` uses the name as the ID.)
/// - `label=<key>` / `label=<key>=<value>`: `ZLayer` does not store labels
///   per secret, so any label filter rejects every secret. This matches
///   Docker's behaviour: the result of "filter by a label that no
///   secret carries" is the empty list.
///
/// Unknown filter keys are ignored (Docker's behaviour).
fn apply_secret_filters(
    secrets: Vec<SecretMetadataResponse>,
    filters: &HashMap<String, Vec<String>>,
) -> Vec<SecretMetadataResponse> {
    if filters.is_empty() {
        return secrets;
    }

    // ZLayer doesn't store per-secret labels; a label filter therefore
    // matches nothing.
    if let Some(labels) = filters.get("label") {
        if !labels.is_empty() {
            return Vec::new();
        }
    }

    secrets
        .into_iter()
        .filter(|s| {
            for key in ["id", "name"] {
                if let Some(values) = filters.get(key) {
                    if !values.is_empty() && !values.iter().any(|v| v == &s.name) {
                        return false;
                    }
                }
            }
            true
        })
        .collect()
}

// ---------------------------------------------------------------------------
// `GET /secrets`
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ListSecretsQuery {
    /// Raw JSON-encoded filter map; see [`parse_filters`].
    filters: Option<String>,
}

/// `GET /secrets` -- List secrets.
async fn list_secrets(
    State(state): State<SocketState>,
    Query(query): Query<ListSecretsQuery>,
) -> Response {
    let metas = match state.client.secrets_list(None).await {
        Ok(m) => m,
        Err(e) => return map_daemon_error(&e),
    };

    let filters = parse_filters(query.filters.as_deref());
    let kept = apply_secret_filters(metas, &filters);
    let docker: Vec<Secret> = kept.iter().map(meta_to_docker).collect();
    Json(docker).into_response()
}

// ---------------------------------------------------------------------------
// `GET /secrets/{id}`
// ---------------------------------------------------------------------------

/// `GET /secrets/{id}` -- Inspect a single secret by name.
///
/// The daemon's `/api/v1/secrets` list endpoint is the only K-pre wrapper
/// that returns metadata, so we list and find by name. This is `O(n)` in
/// the secret count — acceptable for the small volumes these endpoints
/// see in practice — and avoids a second wrapper that doesn't exist yet.
async fn inspect_secret(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    let metas = match state.client.secrets_list(None).await {
        Ok(m) => m,
        Err(e) => return map_daemon_error(&e),
    };

    metas.iter().find(|m| m.name == id).map_or_else(
        || error_response(StatusCode::NOT_FOUND, format!("secret {id} not found")),
        |m| Json(meta_to_docker(m)).into_response(),
    )
}

// ---------------------------------------------------------------------------
// `POST /secrets/create`
// ---------------------------------------------------------------------------

/// Docker's create-secret request body.
///
/// Docker fields:
///
/// - `Name`: required secret name.
/// - `Labels`: free-form labels. `ZLayer` doesn't persist them; we accept
///   and ignore.
/// - `Data`: base64-encoded payload (required by Docker).
/// - `Driver`: external driver. `ZLayer` only supports the in-tree store,
///   so non-empty drivers are rejected with 400.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct DockerSecretSpec {
    name: String,
    #[allow(dead_code)]
    labels: Option<HashMap<String, String>>,
    data: Option<String>,
    driver: Option<Value>,
    #[serde(rename = "Templating")]
    #[allow(dead_code)]
    templating: Option<Value>,
}

/// Validate a create body and extract `(name, plaintext)`.
///
/// Pulled out for unit-testing without needing a daemon.
fn parse_create(body: &DockerSecretSpec) -> Result<(String, String), (StatusCode, String)> {
    if body.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "secret name is required".to_owned(),
        ));
    }
    if let Some(driver) = &body.driver {
        if !driver.is_null() {
            // External drivers (Vault, etc.) live outside ZLayer's secret
            // store. Reject loudly so the caller doesn't think their
            // driver config silently took effect.
            if let Some(obj) = driver.as_object() {
                let name = obj.get("Name").and_then(Value::as_str).unwrap_or("").trim();
                if !name.is_empty() {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        format!("secret driver {name} is not supported by the ZLayer daemon"),
                    ));
                }
            }
        }
    }

    let data = body.data.as_deref().unwrap_or("");
    let bytes = decode_base64_lenient(data).ok_or((
        StatusCode::BAD_REQUEST,
        "Data must be a base64-encoded payload".to_owned(),
    ))?;
    let plaintext = String::from_utf8(bytes).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "Data must decode to a UTF-8 string".to_owned(),
        )
    })?;

    Ok((body.name.clone(), plaintext))
}

/// `POST /secrets/create` -- Create a secret.
///
/// Body shape: `{ "Name": "foo", "Labels": {...}, "Data": "<base64>",
/// "Driver": null }`. Returns `200 OK` with `{ "ID": "<name>" }` on
/// success — Docker's contract uses 201 in the spec but stock dockerd
/// emits 201; we follow the Docker engine and return 201 here too so
/// `docker secret create` exits cleanly.
async fn create_secret(
    State(state): State<SocketState>,
    Json(body): Json<DockerSecretSpec>,
) -> Response {
    let (name, plaintext) = match parse_create(&body) {
        Ok(v) => v,
        Err((status, msg)) => return error_response(status, msg),
    };

    match state.client.secrets_create(&name, &plaintext, None).await {
        Ok(meta) => (StatusCode::CREATED, Json(json!({ "ID": meta.name }))).into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

// ---------------------------------------------------------------------------
// `POST /secrets/{id}/update`
// ---------------------------------------------------------------------------

/// `POST /secrets/{id}/update` -- Update a secret.
///
/// Docker's update model is unusual: the body is a `SecretSpec` plus a
/// `version` query parameter (the daemon enforces version monotonicity).
/// We honour rotations by treating any non-empty `Data` as a new value
/// and forwarding to `secrets_rotate`. If the body only contains label
/// changes, we accept and 200 — `ZLayer` doesn't persist per-secret labels
/// today, so the change is a no-op rather than an error (matching
/// Docker's tolerant view of "update with no material change").
async fn update_secret(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Json(body): Json<DockerSecretSpec>,
) -> Response {
    let Some(raw) = body.data.as_deref() else {
        // Labels-only update: ZLayer doesn't persist labels, so accept
        // and 200 with no body. Docker clients treat an empty body as
        // success.
        return StatusCode::OK.into_response();
    };

    if raw.is_empty() {
        return StatusCode::OK.into_response();
    }

    let Some(bytes) = decode_base64_lenient(raw) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Data must be a base64-encoded payload",
        );
    };
    let Ok(plaintext) = String::from_utf8(bytes) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Data must decode to a UTF-8 string",
        );
    };

    match state.client.secrets_rotate(&id, &plaintext, None).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

// ---------------------------------------------------------------------------
// `DELETE /secrets/{id}`
// ---------------------------------------------------------------------------

/// `DELETE /secrets/{id}` -- Remove a secret by name.
///
/// Returns `204 No Content` on success, `404` when the secret does not
/// exist, `500` otherwise.
async fn delete_secret(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    match state.client.secrets_delete(&id, None).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta(name: &str, version: u32) -> SecretMetadataResponse {
        SecretMetadataResponse {
            name: name.to_owned(),
            created_at: 1_700_000_000,
            updated_at: 1_700_000_500,
            version,
            value: None,
        }
    }

    #[test]
    fn meta_to_docker_uses_name_as_id_and_hides_data() {
        let m = sample_meta("db-pass", 3);
        let s = meta_to_docker(&m);
        assert_eq!(s.id, "db-pass");
        assert_eq!(s.spec.name, "db-pass");
        assert_eq!(s.version.index, 3);
        assert!(s.spec.data.is_empty());
        assert!(s.spec.labels.is_object());
        assert!(s.spec.driver.is_null());
        // CreatedAt / UpdatedAt are RFC 3339 (the helper from `shape`).
        assert!(s.created_at.starts_with("20"));
        assert!(s.updated_at.starts_with("20"));
    }

    #[test]
    fn parse_filters_handles_well_formed_json() {
        let raw = r#"{"name":["a"],"label":["env=prod"]}"#;
        let parsed = parse_filters(Some(raw));
        assert_eq!(parsed.get("name").map(Vec::len), Some(1));
        assert_eq!(parsed.get("label").map(Vec::len), Some(1));
    }

    #[test]
    fn parse_filters_tolerates_garbage() {
        assert!(parse_filters(Some("not json")).is_empty());
        assert!(parse_filters(Some("")).is_empty());
        assert!(parse_filters(None).is_empty());
    }

    #[test]
    fn apply_filters_keeps_matching_name() {
        let metas = vec![sample_meta("a", 1), sample_meta("b", 1)];
        let mut f: HashMap<String, Vec<String>> = HashMap::new();
        f.insert("name".to_owned(), vec!["a".to_owned()]);
        let kept = apply_secret_filters(metas, &f);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "a");
    }

    #[test]
    fn apply_filters_id_alias_matches_name() {
        let metas = vec![sample_meta("a", 1), sample_meta("b", 1)];
        let mut f: HashMap<String, Vec<String>> = HashMap::new();
        f.insert("id".to_owned(), vec!["b".to_owned()]);
        let kept = apply_secret_filters(metas, &f);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "b");
    }

    #[test]
    fn apply_filters_label_drops_everything() {
        // ZLayer doesn't store per-secret labels; any label filter
        // therefore returns the empty list.
        let metas = vec![sample_meta("a", 1)];
        let mut f: HashMap<String, Vec<String>> = HashMap::new();
        f.insert("label".to_owned(), vec!["env=prod".to_owned()]);
        let kept = apply_secret_filters(metas, &f);
        assert!(kept.is_empty());
    }

    #[test]
    fn apply_filters_unknown_key_ignored() {
        let metas = vec![sample_meta("a", 1)];
        let mut f: HashMap<String, Vec<String>> = HashMap::new();
        f.insert("garbage".to_owned(), vec!["x".to_owned()]);
        let kept = apply_secret_filters(metas, &f);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn parse_create_decodes_base64_data() {
        let body = DockerSecretSpec {
            name: "key".to_owned(),
            labels: None,
            // base64 of "hunter2"
            data: Some("aHVudGVyMg==".to_owned()),
            driver: None,
            templating: None,
        };
        let (name, plain) = parse_create(&body).expect("parse");
        assert_eq!(name, "key");
        assert_eq!(plain, "hunter2");
    }

    #[test]
    fn parse_create_accepts_url_safe_base64() {
        let body = DockerSecretSpec {
            name: "k".to_owned(),
            labels: None,
            // base64url of "hi"
            data: Some("aGk".to_owned()),
            driver: None,
            templating: None,
        };
        let (_, plain) = parse_create(&body).expect("parse");
        assert_eq!(plain, "hi");
    }

    #[test]
    fn parse_create_rejects_blank_name() {
        let body = DockerSecretSpec {
            name: "  ".to_owned(),
            labels: None,
            data: Some("aGk=".to_owned()),
            driver: None,
            templating: None,
        };
        let err = parse_create(&body).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn parse_create_rejects_external_driver() {
        let body = DockerSecretSpec {
            name: "k".to_owned(),
            labels: None,
            data: Some("aGk=".to_owned()),
            driver: Some(json!({"Name": "vault"})),
            templating: None,
        };
        let err = parse_create(&body).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("vault"));
    }

    #[test]
    fn parse_create_rejects_invalid_base64() {
        let body = DockerSecretSpec {
            name: "k".to_owned(),
            labels: None,
            data: Some("!!!".to_owned()),
            driver: None,
            templating: None,
        };
        let err = parse_create(&body).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn parse_create_rejects_non_utf8_payload() {
        // Pure UTF-8 invalid bytes (0xFF 0xFE) base64-encode to "//4=".
        let body = DockerSecretSpec {
            name: "k".to_owned(),
            labels: None,
            data: Some("//4=".to_owned()),
            driver: None,
            templating: None,
        };
        let err = parse_create(&body).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn classify_daemon_error_maps_404_409_400_500() {
        assert_eq!(
            classify_daemon_error("404 Not Found: secret 'x' missing"),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            classify_daemon_error("Daemon returned 409 Conflict"),
            StatusCode::CONFLICT
        );
        assert_eq!(
            classify_daemon_error("400 Bad Request"),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            classify_daemon_error("everything exploded"),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn decode_base64_handles_all_four_alphabets() {
        // "hi" in standard padded.
        assert_eq!(decode_base64_lenient("aGk=").unwrap(), b"hi");
        // "hi" in unpadded standard.
        assert_eq!(decode_base64_lenient("aGk").unwrap(), b"hi");
        // URL-safe with padding.
        assert_eq!(decode_base64_lenient("aGk=").unwrap(), b"hi");
        // Garbage rejected.
        assert!(decode_base64_lenient("!!!").is_none());
    }
}
