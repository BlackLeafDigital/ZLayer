//! `/configs` endpoints — Swarm config bridge.
//!
//! Docker Configs are unencrypted, name-keyed key-value blobs intended for
//! configuration data that should not be baked into the image. `ZLayer`'s
//! closest analogue is the global / project-scoped variables surface
//! (`/api/v1/variables`), which is also unencrypted and name-keyed. These
//! handlers bridge the Docker Configs wire shape onto that surface so
//! Docker-aware tooling (Docker CLI, Portainer, `docker config ls`) can
//! observe and manage `ZLayer` variables.
//!
//! # Mapping
//!
//! | Docker Config field | `ZLayer` source                               |
//! |---------------------|-----------------------------------------------|
//! | `ID`                | `StoredVariable.id`                           |
//! | `Version.Index`     | always `0` (variables don't carry a Raft idx) |
//! | `CreatedAt`         | `StoredVariable.created_at` (RFC 3339)        |
//! | `UpdatedAt`         | `StoredVariable.updated_at` (RFC 3339)        |
//! | `Spec.Name`         | `StoredVariable.name`                         |
//! | `Spec.Labels`       | always empty (variables have no labels)       |
//! | `Spec.Data`         | base64-encoded `StoredVariable.value`         |
//! | `Spec.Templating`   | always null (`ZLayer` does not template)      |
//!
//! Unlike Secrets, Docker DOES include `Spec.Data` on inspect/list — so we
//! base64-encode the variable's plaintext value into every read response.
//!
//! # Filters
//!
//! `GET /configs?filters=...` honours `id`, `name`, and `label` filters in
//! the same shape as `/nodes`. `label` always returns no matches (variables
//! carry no labels); `id` and `name` use exact-match against the variable's
//! id and name fields.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use chrono::SecondsFormat;
use serde::Deserialize;
use serde_json::Value;
use zlayer_types::storage::StoredVariable;

use super::shape::{Config, ConfigSpec, Version};
use crate::socket::system::error_response;
use crate::socket::SocketState;

/// `/configs` route table.
pub(super) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/configs", get(list_configs))
        .route("/configs/create", post(create_config))
        .route("/configs/{id}", get(inspect_config).delete(delete_config))
        .route("/configs/{id}/update", post(update_config))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Query / body shapes
// ---------------------------------------------------------------------------

/// Query parameters for `GET /configs`. Docker passes filters as a single
/// URL-encoded JSON object whose values are arrays of strings, e.g.
/// `?filters={"name":["my-config"]}`.
#[derive(Debug, Deserialize, Default)]
struct ListQuery {
    #[serde(default)]
    filters: Option<String>,
}

/// Body of `POST /configs/create`. Docker's `ConfigSpec` carries `Name`,
/// `Labels` (ignored — variables have no per-resource labels), `Data`
/// (base64-encoded plaintext), and `Templating` (ignored — `ZLayer` does
/// not template variable values).
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct CreateBody {
    name: String,
    #[allow(dead_code)]
    labels: Option<HashMap<String, String>>,
    /// Base64-encoded plaintext payload. Required.
    data: Option<String>,
    #[allow(dead_code)]
    templating: Option<Value>,
}

/// Body of `POST /configs/{id}/update`. Docker sends the full updated
/// `ConfigSpec`; we honour `Data` (rotates the variable's value) and
/// silently ignore `Labels` / `Templating`.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct UpdateBody {
    #[allow(dead_code)]
    name: Option<String>,
    #[allow(dead_code)]
    labels: Option<HashMap<String, String>>,
    data: Option<String>,
    #[allow(dead_code)]
    templating: Option<Value>,
}

// ---------------------------------------------------------------------------
// Filter parsing
// ---------------------------------------------------------------------------

/// Parse Docker's URL-encoded JSON filter blob into a multi-valued map.
///
/// Returns `Ok(map)` (possibly empty) on success, or `Err(message)` if the
/// blob is non-empty and not valid JSON / not an object-of-string-arrays.
/// An absent or empty blob yields an empty map.
fn parse_config_filters(raw: Option<&str>) -> Result<HashMap<String, Vec<String>>, String> {
    let Some(s) = raw else {
        return Ok(HashMap::new());
    };
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(HashMap::new());
    }
    let value: Value =
        serde_json::from_str(trimmed).map_err(|e| format!("invalid filter '{trimmed}': {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| format!("invalid filter '{trimmed}': expected JSON object"))?;

    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for (k, v) in obj {
        let mut values: Vec<String> = Vec::new();
        if let Some(arr) = v.as_array() {
            for item in arr {
                let Some(s) = item.as_str() else {
                    return Err(format!("invalid filter '{k}': values must be strings"));
                };
                values.push(s.to_owned());
            }
        } else if let Some(map) = v.as_object() {
            // Tolerate the `{key: {value: true}}` form Docker also accepts.
            for (key, flag) in map {
                if flag.as_bool().unwrap_or(false) {
                    values.push(key.clone());
                }
            }
        } else {
            return Err(format!(
                "invalid filter '{k}': value must be array or object"
            ));
        }
        out.insert(k.clone(), values);
    }
    Ok(out)
}

/// Apply the supported subset of Docker config filters to a list of configs.
///
/// Honoured filter keys:
///
/// - `id`    — exact match against `Config.ID`.
/// - `name`  — exact match against `Config.Spec.Name`.
/// - `label` — `ZLayer` variables carry no labels, so any non-empty `label`
///   filter rejects every entry. An empty value list is a no-op.
///
/// Unknown filter keys are ignored (matching Docker's permissive semantics).
fn apply_config_filters(
    configs: Vec<Config>,
    filters: &HashMap<String, Vec<String>>,
) -> Vec<Config> {
    if filters.is_empty() {
        return configs;
    }
    configs
        .into_iter()
        .filter(|c| config_matches_filters(c, filters))
        .collect()
}

fn config_matches_filters(config: &Config, filters: &HashMap<String, Vec<String>>) -> bool {
    for (key, values) in filters {
        if values.is_empty() {
            continue;
        }
        let pass = match key.as_str() {
            "id" => values.iter().any(|v| v == &config.id),
            "name" => values.iter().any(|v| v == &config.spec.name),
            // Variables carry no labels; a non-empty label filter rejects
            // every entry.
            "label" => false,
            // Unknown filter keys are ignored (Docker semantics).
            _ => true,
        };
        if !pass {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Shape helpers
// ---------------------------------------------------------------------------

/// Build a Docker [`Config`] from a `ZLayer` [`StoredVariable`].
///
/// Unlike Secrets, Docker does include `Spec.Data` on inspect/list — so the
/// variable's plaintext value is base64-encoded into the wire shape. Labels
/// are always empty (variables have no labels) and templating is always
/// null (`ZLayer` does not template variable values).
fn config_from_variable(var: &StoredVariable) -> Config {
    Config {
        id: var.id.clone(),
        version: Version { index: 0 },
        created_at: var.created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        updated_at: var.updated_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        spec: ConfigSpec {
            name: var.name.clone(),
            labels: Value::Object(serde_json::Map::new()),
            data: BASE64_STANDARD.encode(var.value.as_bytes()),
            templating: Value::Null,
        },
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /configs` — List configs as Docker swarm configs.
async fn list_configs(State(state): State<SocketState>, Query(q): Query<ListQuery>) -> Response {
    let filters = match parse_config_filters(q.filters.as_deref()) {
        Ok(f) => f,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, msg),
    };

    let variables = match state.client.variables_list().await {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to list variables: {e}"),
            );
        }
    };

    let configs: Vec<Config> = variables.iter().map(config_from_variable).collect();
    let filtered = apply_config_filters(configs, &filters);
    (StatusCode::OK, Json(filtered)).into_response()
}

/// `GET /configs/{id}` — Inspect a single config.
///
/// `/api/v1/variables` does not expose a single-variable lookup that
/// matches the Docker shape, so we list and filter by id. Returns 404 when
/// no variable carries the requested id.
async fn inspect_config(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    let variables = match state.client.variables_list().await {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to list variables: {e}"),
            );
        }
    };

    match variables.iter().find(|v| v.id == id) {
        Some(var) => (StatusCode::OK, Json(config_from_variable(var))).into_response(),
        None => error_response(StatusCode::NOT_FOUND, format!("config {id} not found")),
    }
}

/// `POST /configs/create` — Create a config from a Docker `ConfigSpec`.
///
/// Decodes the base64 `Data` payload into plaintext and forwards to
/// `variables_create` with no scope (global variable). Returns
/// `200 OK` with `{"ID":"<id>"}` so Docker clients see the freshly-created
/// resource's id.
async fn create_config(State(state): State<SocketState>, Json(body): Json<CreateBody>) -> Response {
    if body.name.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "config name is required");
    }

    let raw_data = body.data.unwrap_or_default();
    let decoded = match BASE64_STANDARD.decode(raw_data.as_bytes()) {
        Ok(bytes) => bytes,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("Data is not valid base64: {e}"),
            );
        }
    };
    let value = match String::from_utf8(decoded) {
        Ok(s) => s,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("Data is not valid UTF-8: {e}"),
            );
        }
    };

    match state
        .client
        .variables_create(&body.name, &value, None)
        .await
    {
        Ok(var) => (StatusCode::OK, Json(serde_json::json!({ "ID": var.id }))).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to create variable: {e}"),
        ),
    }
}

/// `POST /configs/{id}/update` — Update a config's data.
///
/// Docker sends the full updated `ConfigSpec`. We honour `Data` (rotates
/// the variable's value via `variables_patch`) and silently no-op when only
/// `Labels` / `Templating` change — `ZLayer` doesn't track those fields.
async fn update_config(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Response {
    let Some(raw_data) = body.data else {
        // Labels / Templating-only update — accept silently.
        return (StatusCode::OK, Json(Value::Object(serde_json::Map::new()))).into_response();
    };

    let decoded = match BASE64_STANDARD.decode(raw_data.as_bytes()) {
        Ok(bytes) => bytes,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("Data is not valid base64: {e}"),
            );
        }
    };
    let value = match String::from_utf8(decoded) {
        Ok(s) => s,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("Data is not valid UTF-8: {e}"),
            );
        }
    };

    match state.client.variables_patch(&id, &value).await {
        Ok(_) => (StatusCode::OK, Json(Value::Object(serde_json::Map::new()))).into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("404") {
                error_response(StatusCode::NOT_FOUND, format!("config {id} not found"))
            } else {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to update variable: {msg}"),
                )
            }
        }
    }
}

/// `DELETE /configs/{id}` — Remove a config.
///
/// Maps onto `variables_delete`. The daemon's variables endpoint returns
/// 404 for unknown ids; we surface that as a Docker-shape 404. Successful
/// deletes return 204 to match Docker's `docker config rm` expectation.
async fn delete_config(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    match state.client.variables_delete(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("404") {
                error_response(StatusCode::NOT_FOUND, format!("config {id} not found"))
            } else {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to delete variable: {msg}"),
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn fixture_var(id: &str, name: &str, value: &str) -> StoredVariable {
        let ts = Utc.with_ymd_and_hms(2026, 4, 15, 12, 0, 0).unwrap();
        StoredVariable {
            id: id.to_string(),
            name: name.to_string(),
            value: value.to_string(),
            scope: None,
            created_at: ts,
            updated_at: ts,
        }
    }

    #[test]
    fn config_from_variable_encodes_data_base64() {
        let var = fixture_var("v-1", "my-config", "hello world");
        let cfg = config_from_variable(&var);
        assert_eq!(cfg.id, "v-1");
        assert_eq!(cfg.spec.name, "my-config");
        assert_eq!(cfg.version.index, 0);
        assert_eq!(cfg.spec.data, BASE64_STANDARD.encode(b"hello world"));
        assert!(cfg.spec.templating.is_null());
        assert!(cfg
            .spec
            .labels
            .as_object()
            .is_some_and(serde_json::Map::is_empty));
        assert_eq!(cfg.created_at, "2026-04-15T12:00:00Z");
        assert_eq!(cfg.updated_at, "2026-04-15T12:00:00Z");
    }

    #[test]
    fn parse_filters_handles_well_formed_json() {
        let out = parse_config_filters(Some(r#"{"name":["foo"]}"#)).unwrap();
        assert_eq!(
            out.get("name").map(Vec::as_slice),
            Some(&["foo".to_string()][..])
        );
    }

    #[test]
    fn parse_filters_empty_input_yields_empty_map() {
        assert!(parse_config_filters(None).unwrap().is_empty());
        assert!(parse_config_filters(Some("")).unwrap().is_empty());
        assert!(parse_config_filters(Some("   ")).unwrap().is_empty());
    }

    #[test]
    fn parse_filters_rejects_garbage() {
        assert!(parse_config_filters(Some("not-json")).is_err());
        assert!(parse_config_filters(Some(r#"["not","object"]"#)).is_err());
    }

    #[test]
    fn parse_filters_accepts_map_form() {
        let out = parse_config_filters(Some(r#"{"name":{"foo":true,"bar":false}}"#)).unwrap();
        let v = out.get("name").unwrap();
        assert!(v.contains(&"foo".to_string()));
        assert!(!v.contains(&"bar".to_string()));
    }

    #[test]
    fn apply_filters_id_match() {
        let configs = vec![
            config_from_variable(&fixture_var("a", "foo", "x")),
            config_from_variable(&fixture_var("b", "bar", "y")),
        ];
        let mut filters = HashMap::new();
        filters.insert("id".to_string(), vec!["a".to_string()]);
        let out = apply_config_filters(configs, &filters);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "a");
    }

    #[test]
    fn apply_filters_name_match() {
        let configs = vec![
            config_from_variable(&fixture_var("a", "foo", "x")),
            config_from_variable(&fixture_var("b", "bar", "y")),
        ];
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), vec!["bar".to_string()]);
        let out = apply_config_filters(configs, &filters);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].spec.name, "bar");
    }

    #[test]
    fn apply_filters_label_rejects_all() {
        let configs = vec![config_from_variable(&fixture_var("a", "foo", "x"))];
        let mut filters = HashMap::new();
        filters.insert("label".to_string(), vec!["env=prod".to_string()]);
        let out = apply_config_filters(configs, &filters);
        assert!(out.is_empty());
    }

    #[test]
    fn apply_filters_unknown_key_ignored() {
        let configs = vec![config_from_variable(&fixture_var("a", "foo", "x"))];
        let mut filters = HashMap::new();
        filters.insert("driver".to_string(), vec!["local".to_string()]);
        let out = apply_config_filters(configs, &filters);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn apply_filters_empty_value_list_is_noop() {
        let configs = vec![config_from_variable(&fixture_var("a", "foo", "x"))];
        let mut filters = HashMap::new();
        filters.insert("id".to_string(), Vec::new());
        let out = apply_config_filters(configs, &filters);
        assert_eq!(out.len(), 1);
    }
}
