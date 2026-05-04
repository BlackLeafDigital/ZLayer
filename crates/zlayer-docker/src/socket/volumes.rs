//! Docker Engine API volume endpoints.
//!
//! Bridges the `/volumes*` surface of the Docker Engine API (v1.43) to the
//! running zlayer daemon via [`zlayer_client::DaemonClient`]. Tools that
//! speak Docker — `docker volume ls`, `docker volume create`, `docker
//! volume inspect`, `docker volume rm`, `docker volume prune` — can drive
//! zlayer through this router.
//!
//! The handlers translate between Docker's `Volume` JSON shape (`Name`,
//! `Driver`, `Mountpoint`, `CreatedAt`, `Labels`, `Scope`, `Status`,
//! `Options`, `UsageData { Size, RefCount }`) and zlayer's native
//! [`zlayer_types::api::volumes::VolumeInfo`].
//!
//! Endpoints handled here:
//!
//! - `GET    /volumes`              — list volumes (with `filters=`)
//! - `POST   /volumes/create`       — create a named volume
//! - `GET    /volumes/{name}`       — inspect a single volume
//! - `DELETE /volumes/{name}`       — remove a volume (with `force=`)
//! - `POST   /volumes/prune`        — prune unused volumes

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use zlayer_types::api::volumes::{CreateVolumeRequest, VolumeInfo};

use super::SocketState;

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/// Volume API routes.
pub(crate) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/volumes", get(list_volumes))
        .route("/volumes/create", post(create_volume))
        .route("/volumes/prune", post(prune_volumes))
        .route("/volumes/{name}", get(inspect_volume).delete(remove_volume))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Error helpers
// ---------------------------------------------------------------------------

/// Build a Docker-style JSON error response: `{ "message": "..." }`.
fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "message": message.into() }))).into_response()
}

/// Map a [`zlayer_client::DaemonClient`] error into a Docker-style
/// HTTP response.
///
/// The daemon client formats its errors as strings; we pattern-match on
/// the well-known prefixes used by `DaemonClient::check_status`:
///
/// - `"404 Not Found: ..."`       -> 404
/// - `"Daemon returned 409 ..."` -> 409 (volume in use without force)
/// - everything else              -> 500
fn map_daemon_error(err: &anyhow::Error) -> Response {
    let msg = format!("{err:#}");
    let status = classify_daemon_error(&msg);
    error_response(status, msg)
}

/// Classify a daemon-error message into a Docker-compatible HTTP status.
///
/// Pulled out so it can be unit-tested without hitting a live daemon.
fn classify_daemon_error(msg: &str) -> StatusCode {
    let lower = msg.to_ascii_lowercase();
    if lower.starts_with("404 ") || lower.contains("404 not found") {
        StatusCode::NOT_FOUND
    } else if lower.contains("409 conflict") || lower.contains(" 409 ") || lower.contains("in use")
    {
        StatusCode::CONFLICT
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

// ---------------------------------------------------------------------------
// VolumeInfo -> Docker `Volume`
// ---------------------------------------------------------------------------

/// Translate a daemon-side [`VolumeInfo`] into Docker's `Volume`
/// JSON shape.
///
/// Docker's `Volume` object shape (v1.43):
///
/// ```json
/// {
///   "Name": "foo",
///   "Driver": "local",
///   "Mountpoint": "/var/lib/docker/volumes/foo/_data",
///   "CreatedAt": "2024-01-01T00:00:00Z",
///   "Status": {},
///   "Labels": {},
///   "Scope": "local",
///   "Options": {},
///   "UsageData": { "Size": -1, "RefCount": -1 }
/// }
/// ```
///
/// We populate `UsageData.Size` from `size_bytes` when known (else `-1`)
/// and `UsageData.RefCount` from `in_use_by.len()` when populated (else
/// `-1`, matching Docker's "not yet computed" sentinel).
fn volume_info_to_docker(info: &VolumeInfo) -> Value {
    let size: i64 = info
        .size_bytes
        .and_then(|n| i64::try_from(n).ok())
        .unwrap_or(-1);

    let ref_count: i64 = if info.in_use_by.is_empty() {
        // Docker uses `-1` to signal "not computed" rather than "zero".
        // The daemon currently always reports `in_use_by` (possibly empty),
        // so we report `0` when we have a real, populated answer and `-1`
        // only when the daemon would have returned `Vec::new()` without
        // ever consulting a usage source. Without a way to distinguish,
        // we mirror Docker's stance: when the list is empty, report
        // `RefCount: 0`. Tools render this as "no containers using it",
        // which is accurate.
        0
    } else {
        i64::try_from(info.in_use_by.len()).unwrap_or(i64::MAX)
    };

    json!({
        "Name": info.name,
        "Driver": "local",
        "Mountpoint": info.path,
        "CreatedAt": info.created_at,
        "Status": json!({}),
        "Labels": info.labels,
        "Scope": "local",
        "Options": json!({}),
        "UsageData": {
            "Size": size,
            "RefCount": ref_count,
        }
    })
}

// ---------------------------------------------------------------------------
// Filter parsing
// ---------------------------------------------------------------------------

/// Parse Docker's `filters` query parameter into a map of filter-key ->
/// list of values.
///
/// Docker passes filters as a URL-encoded JSON object whose values are
/// arrays of strings, e.g. `{"label":["foo=bar","app=web"]}`. We parse
/// it permissively: an unparseable filter string yields an empty map,
/// matching Docker's behaviour of silently ignoring malformed filters.
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
            // Docker also tolerates `{key: {value: true}}` form.
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

/// Apply Docker's `name`/`label`/`driver` filters to a list of volumes.
///
/// Honoured filters:
///
/// - `name=<substring>`: keeps volumes whose name contains the substring.
/// - `label=<key>` / `label=<key>=<value>`: keeps volumes that carry the
///   label (and value, when supplied).
/// - `driver=<name>`: only `local` is supported by zlayer; volumes are
///   kept when the filter value is `local` and dropped otherwise.
///
/// Unknown filter keys are ignored (Docker's behaviour).
fn apply_volume_filters(
    volumes: Vec<VolumeInfo>,
    filters: &HashMap<String, Vec<String>>,
) -> Vec<VolumeInfo> {
    if filters.is_empty() {
        return volumes;
    }

    volumes
        .into_iter()
        .filter(|v| {
            if let Some(names) = filters.get("name") {
                if !names.is_empty() && !names.iter().any(|n| v.name.contains(n)) {
                    return false;
                }
            }
            if let Some(labels) = filters.get("label") {
                for spec in labels {
                    if let Some((k, val)) = spec.split_once('=') {
                        match v.labels.get(k) {
                            Some(actual) if actual == val => {}
                            _ => return false,
                        }
                    } else if !v.labels.contains_key(spec) {
                        return false;
                    }
                }
            }
            if let Some(drivers) = filters.get("driver") {
                if !drivers.is_empty() && !drivers.iter().any(|d| d == "local") {
                    return false;
                }
            }
            true
        })
        .collect()
}

// ---------------------------------------------------------------------------
// `GET /volumes`
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ListVolumesQuery {
    /// Raw JSON-encoded filter map; see [`parse_filters`].
    filters: Option<String>,
}

/// `GET /volumes` — List volumes.
///
/// Calls [`zlayer_client::DaemonClient::list_volumes`], maps each entry
/// to Docker's `Volume` shape and returns
/// `{ "Volumes": [...], "Warnings": [] }`.
async fn list_volumes(
    State(state): State<SocketState>,
    Query(query): Query<ListVolumesQuery>,
) -> Response {
    let raw = match state.client.list_volumes().await {
        Ok(v) => v,
        Err(e) => return map_daemon_error(&e),
    };

    // The daemon's typed `list_volumes` returns `Vec<serde_json::Value>`,
    // each element shaped like `VolumeInfo`. Decode them into `VolumeInfo`
    // so we can apply structured filters.
    let infos: Vec<VolumeInfo> = raw
        .into_iter()
        .filter_map(|v| serde_json::from_value::<VolumeInfo>(v).ok())
        .collect();

    let filters = parse_filters(query.filters.as_deref());
    let filtered = apply_volume_filters(infos, &filters);

    let docker_volumes: Vec<Value> = filtered.iter().map(volume_info_to_docker).collect();

    Json(json!({
        "Volumes": docker_volumes,
        "Warnings": Vec::<String>::new(),
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// `POST /volumes/create`
// ---------------------------------------------------------------------------

/// Docker's create-volume request body.
///
/// Docker fields:
///
/// - `Name`: required volume name.
/// - `Driver`: driver name (zlayer only supports `local`; other drivers
///   are ignored — we still create the volume).
/// - `DriverOpts`: arbitrary driver options. We forward any `size` /
///   `tier` keys onto the daemon's [`CreateVolumeRequest`].
/// - `Labels`: labels to attach to the volume.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct DockerCreateVolume {
    name: String,
    #[allow(dead_code)]
    driver: Option<String>,
    driver_opts: Option<HashMap<String, String>>,
    labels: Option<HashMap<String, String>>,
}

/// Translate Docker's create body into the daemon's request DTO.
///
/// Pulled out for unit tests so we can exercise the translation
/// without spinning up a daemon.
fn docker_create_to_daemon(body: DockerCreateVolume) -> Result<CreateVolumeRequest, String> {
    if body.name.trim().is_empty() {
        return Err("volume name is required".to_owned());
    }

    // Docker's `DriverOpts` is a free-form string map. zlayer's volume
    // create takes a structured `size` / `tier` pair; we honour the two
    // most-likely option keys (`size`, `tier`) so callers can pass
    // `--opt size=10Gi --opt tier=cached` through the Docker CLI.
    let (size, tier) = body.driver_opts.as_ref().map_or((None, None), |opts| {
        (opts.get("size").cloned(), opts.get("tier").cloned())
    });

    Ok(CreateVolumeRequest {
        name: body.name,
        size,
        tier,
        labels: body.labels,
    })
}

/// `POST /volumes/create` — Create a volume.
///
/// Body shape: `{ "Name": "foo", "Driver": "local", "DriverOpts":
/// {...}, "Labels": {...} }`. Returns `201 Created` with the freshly
/// created volume in Docker's `Volume` shape.
async fn create_volume(
    State(state): State<SocketState>,
    Json(body): Json<DockerCreateVolume>,
) -> Response {
    let req = match docker_create_to_daemon(body) {
        Ok(r) => r,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, msg),
    };

    match state.client.create_volume(req).await {
        Ok(info) => (StatusCode::CREATED, Json(volume_info_to_docker(&info))).into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

// ---------------------------------------------------------------------------
// `GET /volumes/{name}`
// ---------------------------------------------------------------------------

/// `GET /volumes/{name}` — Inspect a single volume.
///
/// Returns Docker's `Volume` JSON shape. A missing volume produces
/// `404 Not Found` with `{"message": "..."}`; the daemon client
/// already collapses 404s into `Ok(None)` so we don't have to inspect
/// error strings here.
async fn inspect_volume(State(state): State<SocketState>, Path(name): Path<String>) -> Response {
    match state.client.inspect_volume(&name).await {
        Ok(Some(info)) => Json(volume_info_to_docker(&info)).into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, format!("no such volume: {name}")),
        Err(e) => map_daemon_error(&e),
    }
}

// ---------------------------------------------------------------------------
// `DELETE /volumes/{name}`
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RemoveVolumeQuery {
    force: Option<String>,
}

/// Parse Docker's `force=` query parameter.
///
/// Docker accepts `1`, `true`, `yes`, `on` (case-insensitive) as truthy.
/// Anything else (including absence) is `false`.
fn parse_force(raw: Option<&str>) -> bool {
    matches!(
        raw.map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// `DELETE /volumes/{name}` — Remove a volume.
///
/// `force=1` lets the daemon remove a non-empty or in-use volume.
/// Returns `204 No Content` on success, `404` when the volume does not
/// exist, `409` when the volume is in use without `force=1`, `500`
/// otherwise.
async fn remove_volume(
    State(state): State<SocketState>,
    Path(name): Path<String>,
    Query(q): Query<RemoveVolumeQuery>,
) -> Response {
    let force = parse_force(q.force.as_deref());
    match state.client.delete_volume(&name, force).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

// ---------------------------------------------------------------------------
// `POST /volumes/prune`
// ---------------------------------------------------------------------------

/// Build the `{VolumesDeleted, SpaceReclaimed}` Docker response.
///
/// Pure helper for unit-testing the prune response shape without
/// requiring a live daemon.
fn build_prune_response(deleted: &[String], space_reclaimed: i64) -> Value {
    json!({
        "VolumesDeleted": deleted,
        "SpaceReclaimed": space_reclaimed,
    })
}

/// `POST /volumes/prune` — Prune unused volumes.
///
/// Walks the daemon's volume list, inspects each one to obtain
/// `in_use_by` and `size_bytes`, deletes those with zero in-use
/// containers, and returns the canonical Docker prune response.
///
/// Body filters (`label=...`) narrow the candidate set the same way
/// `GET /volumes` does. Errors deleting an individual volume are
/// recorded but do not abort the prune — Docker's prune is best-effort.
async fn prune_volumes(
    State(state): State<SocketState>,
    Query(query): Query<ListVolumesQuery>,
) -> Response {
    let raw = match state.client.list_volumes().await {
        Ok(v) => v,
        Err(e) => return map_daemon_error(&e),
    };

    let infos: Vec<VolumeInfo> = raw
        .into_iter()
        .filter_map(|v| serde_json::from_value::<VolumeInfo>(v).ok())
        .collect();

    let filters = parse_filters(query.filters.as_deref());
    let candidates = apply_volume_filters(infos, &filters);

    let mut deleted: Vec<String> = Vec::new();
    let mut space_reclaimed: i64 = 0;

    for vol in candidates {
        // Re-fetch the volume so we see the freshest `in_use_by` and
        // `size_bytes`. Skip on lookup failure rather than aborting.
        let Ok(Some(detail)) = state.client.inspect_volume(&vol.name).await else {
            continue;
        };
        if !detail.in_use_by.is_empty() {
            continue;
        }
        if state
            .client
            .delete_volume(&detail.name, false)
            .await
            .is_ok()
        {
            if let Some(bytes) = detail.size_bytes {
                space_reclaimed =
                    space_reclaimed.saturating_add(i64::try_from(bytes).unwrap_or(i64::MAX));
            }
            deleted.push(detail.name);
        }
    }

    Json(build_prune_response(&deleted, space_reclaimed)).into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_info() -> VolumeInfo {
        VolumeInfo {
            name: "data".to_owned(),
            path: "/var/lib/zlayer/volumes/data".to_owned(),
            size_bytes: Some(2048),
            labels: HashMap::from([("app".to_owned(), "web".to_owned())]),
            created_at: "2024-01-01T00:00:00Z".to_owned(),
            in_use_by: Vec::new(),
        }
    }

    #[test]
    fn list_volumes_maps_daemon_to_docker_shape() {
        let info = sample_info();
        let mapped = volume_info_to_docker(&info);

        assert_eq!(mapped["Name"], "data");
        assert_eq!(mapped["Driver"], "local");
        assert_eq!(mapped["Mountpoint"], "/var/lib/zlayer/volumes/data");
        assert_eq!(mapped["CreatedAt"], "2024-01-01T00:00:00Z");
        assert_eq!(mapped["Scope"], "local");
        assert!(mapped["Status"].is_object());
        assert!(mapped["Options"].is_object());
        assert_eq!(mapped["Labels"]["app"], "web");
        assert_eq!(mapped["UsageData"]["Size"], 2048);
        assert_eq!(mapped["UsageData"]["RefCount"], 0);

        // Wrap into the `GET /volumes` response and verify the outer shape.
        let response = json!({
            "Volumes": vec![volume_info_to_docker(&info)],
            "Warnings": Vec::<String>::new(),
        });
        assert!(response["Volumes"].is_array());
        assert_eq!(response["Volumes"].as_array().unwrap().len(), 1);
        assert_eq!(response["Warnings"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn list_volumes_filters_by_label() {
        let mut tagged = sample_info();
        tagged.name = "tagged".to_owned();
        tagged.labels = HashMap::from([("app".to_owned(), "web".to_owned())]);

        let mut other = sample_info();
        other.name = "other".to_owned();
        other.labels = HashMap::from([("app".to_owned(), "db".to_owned())]);

        let mut filters: HashMap<String, Vec<String>> = HashMap::new();
        filters.insert("label".to_owned(), vec!["app=web".to_owned()]);

        let kept = apply_volume_filters(vec![tagged, other], &filters);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "tagged");
    }

    #[test]
    fn parse_filters_handles_well_formed_json() {
        let raw = r#"{"label":["app=web"],"name":["data"]}"#;
        let parsed = parse_filters(Some(raw));
        assert_eq!(parsed.get("label").map(Vec::len), Some(1));
        assert_eq!(parsed.get("name").map(Vec::len), Some(1));
    }

    #[test]
    fn parse_filters_tolerates_garbage() {
        assert!(parse_filters(Some("not json")).is_empty());
        assert!(parse_filters(Some("")).is_empty());
        assert!(parse_filters(None).is_empty());
    }

    #[test]
    fn create_volume_translates_request_and_returns_docker_volume() {
        let body = DockerCreateVolume {
            name: "build-cache".to_owned(),
            driver: Some("local".to_owned()),
            driver_opts: Some(HashMap::from([
                ("size".to_owned(), "10Gi".to_owned()),
                ("tier".to_owned(), "cached".to_owned()),
            ])),
            labels: Some(HashMap::from([("owner".to_owned(), "ci".to_owned())])),
        };

        let req = docker_create_to_daemon(body).expect("translation succeeds");
        assert_eq!(req.name, "build-cache");
        assert_eq!(req.size.as_deref(), Some("10Gi"));
        assert_eq!(req.tier.as_deref(), Some("cached"));
        assert_eq!(
            req.labels
                .as_ref()
                .and_then(|m| m.get("owner"))
                .map(String::as_str),
            Some("ci")
        );

        // The success-path also returns Docker's `Volume` JSON shape.
        let info = VolumeInfo {
            name: req.name.clone(),
            path: "/var/lib/zlayer/volumes/build-cache".to_owned(),
            size_bytes: Some(0),
            labels: req.labels.clone().unwrap_or_default(),
            created_at: "2024-01-01T00:00:00Z".to_owned(),
            in_use_by: Vec::new(),
        };
        let docker = volume_info_to_docker(&info);
        assert_eq!(docker["Name"], "build-cache");
        assert_eq!(docker["Driver"], "local");
        assert_eq!(docker["Labels"]["owner"], "ci");
    }

    #[test]
    fn create_volume_rejects_blank_name() {
        let body = DockerCreateVolume {
            name: "  ".to_owned(),
            ..DockerCreateVolume::default()
        };
        assert!(docker_create_to_daemon(body).is_err());
    }

    #[test]
    fn inspect_volume_404_on_missing() {
        // The handler maps `Ok(None)` -> 404 with a JSON message body.
        // We validate the helper used for that response shape here.
        let resp = error_response(StatusCode::NOT_FOUND, "no such volume: ghost");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // The daemon-error classifier also maps the daemon's "404 Not
        // Found: ..." string into a 404, which is what
        // `inspect_volume` falls back on for Err paths.
        let err = anyhow::anyhow!("404 Not Found: volume 'ghost' missing");
        assert_eq!(
            classify_daemon_error(&format!("{err:#}")),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn delete_volume_handles_force_query() {
        // Default (no query) -> not forced.
        assert!(!parse_force(None));

        // Common truthy values from the Docker CLI / SDKs.
        assert!(parse_force(Some("1")));
        assert!(parse_force(Some("true")));
        assert!(parse_force(Some("TRUE")));
        assert!(parse_force(Some("yes")));
        assert!(parse_force(Some("on")));

        // Anything else is false.
        assert!(!parse_force(Some("0")));
        assert!(!parse_force(Some("false")));
        assert!(!parse_force(Some("nope")));

        // An "in use" daemon error maps to 409.
        let msg = "Daemon returned 409 Conflict -- volume 'data' is in use by 1 container(s)";
        assert_eq!(classify_daemon_error(msg), StatusCode::CONFLICT);
    }

    #[test]
    fn prune_volumes_returns_docker_shape() {
        let deleted = vec!["a".to_owned(), "b".to_owned()];
        let resp = build_prune_response(&deleted, 4096);
        assert_eq!(resp["VolumesDeleted"].as_array().unwrap().len(), 2);
        assert_eq!(resp["VolumesDeleted"][0], "a");
        assert_eq!(resp["VolumesDeleted"][1], "b");
        assert_eq!(resp["SpaceReclaimed"], 4096);

        // Empty prune is also valid.
        let empty_list: Vec<String> = Vec::new();
        let empty = build_prune_response(&empty_list, 0);
        assert_eq!(empty["VolumesDeleted"].as_array().unwrap().len(), 0);
        assert_eq!(empty["SpaceReclaimed"], 0);
    }
}
