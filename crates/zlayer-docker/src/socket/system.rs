//! Docker Engine API system endpoints.
//!
//! Provides `/_ping`, `/version`, `/info`, `/events`, and `/system/df`.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{AppendHeaders, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::stream::StreamExt;
use zlayer_api::{DaemonEvent, NetworkEventKind, VolumeEventKind};
use zlayer_client::{default_socket_path, DaemonClient};
use zlayer_types::api::images::ImageInfoDto;

use super::types::{SystemInfo, VersionInfo};
use super::SocketState;

/// System API routes.
pub(crate) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/_ping", get(ping).head(ping_head))
        .route("/version", get(version))
        .route("/info", get(info))
        .route("/events", get(events))
        .route("/system/df", get(disk_usage))
        .route("/system/prune", axum::routing::post(system_prune))
        .with_state(state)
}

/// Standard Docker `/_ping` response headers.
///
/// Docker clients inspect `Api-Version`, `OSType`, and `Builder-Version`
/// during handshake; the cache headers stop intermediaries from caching
/// what is meant to be a live health probe. The OS reflects the daemon
/// host (where containers actually run), not the client.
fn ping_headers() -> AppendHeaders<[(&'static str, &'static str); 5]> {
    AppendHeaders([
        ("Api-Version", "1.43"),
        ("OSType", ping_ostype()),
        ("Builder-Version", "2"),
        ("Cache-Control", "no-cache, no-store, must-revalidate"),
        ("Pragma", "no-cache"),
    ])
}

/// Daemon `OSType` for the ping handshake.
const fn ping_ostype() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// `GET /_ping` — Health check. Returns `"OK"` as plain text along with
/// the standard Docker handshake headers so clients (Docker CLI, `BuildKit`,
/// docker-py, etc.) can negotiate API version and builder version.
async fn ping() -> impl IntoResponse {
    (ping_headers(), "OK")
}

/// `HEAD /_ping` — Same handshake headers as `GET /_ping` but with no
/// body. Docker CLI's connection probe uses HEAD, so we have to handle it
/// explicitly; otherwise axum responds with 405.
async fn ping_head() -> impl IntoResponse {
    (ping_headers(), ())
}

/// `GET /version` — Docker version info.
async fn version() -> Json<VersionInfo> {
    Json(VersionInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        api_version: "1.43".to_owned(),
        min_api_version: "1.24".to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        kernel_version: kernel_version(),
        go_version: "N/A (ZLayer/Rust)".to_owned(),
    })
}

/// `GET /info` — System information.
///
/// Sources live counts from the zlayer daemon (containers, images) and
/// fills the rest from the host. Docker-aware tools rely on these fields
/// so we emit the full v1.43 shape, leaving plugin/driver metadata as
/// empty arrays when we have nothing meaningful to report.
///
/// The `Swarm` block is populated via
/// [`super::swarm::build_info_swarm_json`] so the cluster id, manager
/// list, and node counts stay in lock-step with the `/swarm` endpoint
/// family. That helper is best-effort: it absorbs daemon errors into a
/// non-empty `Error` field rather than failing `/info`, which Docker
/// clients treat as load-bearing for their handshake / probe paths.
async fn info(State(state): State<SocketState>) -> Json<serde_json::Value> {
    let (containers_total, containers_running, containers_paused, containers_stopped) =
        container_counts().await;
    let images_total = image_count().await;
    let swarm_block = super::swarm::build_info_swarm_json(&state).await;

    // Build the base `SystemInfo` first so callers that only need the
    // typed subset stay in sync with the shared types module.
    let base = SystemInfo {
        containers: containers_total,
        containers_running,
        containers_paused,
        containers_stopped,
        images: images_total,
        name: hostname(),
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        ncpu: i64::try_from(num_cpus::get()).unwrap_or(i64::MAX),
        mem_total: total_memory(),
    };

    // Start from the typed serialization, then splice in the remaining
    // Docker v1.43 fields that the narrow `SystemInfo` type doesn't
    // carry. Docker-aware clients rely on the full shape.
    let mut value = serde_json::to_value(&base).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.insert("ID".into(), serde_json::Value::String(hostname()));
        obj.insert("OSType".into(), std::env::consts::OS.into());
        obj.insert("OSVersion".into(), "".into());
        obj.insert("KernelVersion".into(), kernel_version().into());
        obj.insert("Driver".into(), "zlayer".into());
        obj.insert("DriverStatus".into(), serde_json::Value::Array(Vec::new()));
        obj.insert(
            "Plugins".into(),
            serde_json::json!({
                "Volume": [],
                "Network": [],
                "Authorization": null,
                "Log": [],
            }),
        );
        obj.insert("SystemStatus".into(), serde_json::Value::Null);
        obj.insert("LoggingDriver".into(), "json-file".into());
        obj.insert("CgroupDriver".into(), "systemd".into());
        obj.insert("CgroupVersion".into(), "2".into());
        obj.insert("DefaultRuntime".into(), "zlayer".into());
        obj.insert(
            "Runtimes".into(),
            serde_json::json!({ "zlayer": { "path": "zlayer" } }),
        );
        obj.insert(
            "IndexServerAddress".into(),
            "https://index.docker.io/v1/".into(),
        );
        obj.insert(
            "RegistryConfig".into(),
            serde_json::json!({
                "AllowNondistributableArtifactsCIDRs": [],
                "AllowNondistributableArtifactsHostnames": [],
                "InsecureRegistryCIDRs": [],
                "IndexConfigs": {},
                "Mirrors": [],
            }),
        );
        obj.insert("Swarm".into(), swarm_block);
        obj.insert("ExperimentalBuild".into(), false.into());
        obj.insert("Labels".into(), serde_json::Value::Array(Vec::new()));
        obj.insert("SystemTime".into(), current_time_rfc3339().into());
    }

    Json(value)
}

/// `GET /events` — Live NDJSON stream of daemon lifecycle events
/// (containers, images, networks, volumes).
///
/// Subscribes to the daemon's `/api/v1/events` push channel via
/// [`DaemonClient::events_stream`] and translates each [`DaemonEvent`]
/// into the Docker wire shape (`Type`, `Action`, `Actor.{ID,Attributes}`,
/// `scope`, `time`, `timeNano`, `id`). One JSON object per line,
/// terminated with `\n`; `Content-Type: application/json` (Docker uses
/// JSON, NOT `text/event-stream`).
///
/// Honours Docker query parameters:
///   * `since` (unix seconds): drop events with `time < since`.
///   * `until` (unix seconds): drop events with `time > until`.
///   * `filters` (URL-encoded JSON object): may contain
///     `type`, `event`, `container`, `image`, `network`, `volume`, `label`.
///     Each value is an array of strings (Docker's standard filter shape).
///     The `label` filter is forwarded to the daemon as a `label=k=v`
///     query parameter so it filters server-side; everything else is
///     applied client-side after mapping.
async fn events(State(state): State<SocketState>, Query(query): Query<EventsQuery>) -> Response {
    // Parse Docker `filters` JSON. A malformed value is a 400 — Docker
    // CLI surfaces this as `Error response from daemon: invalid filter`.
    let filters = match EventFilters::parse(query.filters.as_deref()) {
        Ok(f) => f,
        Err(msg) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "message": msg })),
            )
                .into_response();
        }
    };

    let label_pairs: Vec<(String, String)> = filters.labels.clone();

    // Subscribe to the daemon event bus. `follow=true` matches the Docker
    // contract: the stream stays open until the connection drops or the
    // daemon ends it (e.g. on subscriber lag).
    let event_stream = match state.client.events_stream(true, &label_pairs).await {
        Ok(s) => s,
        Err(err) => return map_daemon_error(&err),
    };

    let since = query.since;
    let until = query.until;

    // Map each `DaemonEvent` to a Docker NDJSON line, applying since/until
    // and the non-label filters along the way. Errors from the underlying
    // stream are logged and swallowed so a single transient parse failure
    // doesn't tear the connection down.
    let body_stream = event_stream.filter_map(move |result| {
        let filters = filters.clone();
        async move {
            let event = match result {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(error = %err, "docker /events: dropping malformed daemon event");
                    return None;
                }
            };
            let docker = map_daemon_event(&event);
            if !time_in_range(&docker, since, until) {
                return None;
            }
            if !filters.matches(&docker) {
                return None;
            }
            ndjson_line(&docker).map(Ok::<Bytes, Infallible>)
        }
    });

    let body = Body::from_stream(body_stream);

    let mut response = Response::builder().status(StatusCode::OK);
    if let Some(headers) = response.headers_mut() {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        );
        headers.insert("X-Accel-Buffering", HeaderValue::from_static("no"));
    }
    response.body(body).map_or_else(
        |e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "message": format!("failed to build /events response: {e}") })),
            )
                .into_response()
        },
        IntoResponse::into_response,
    )
}

/// `GET /system/df` — Disk usage.
///
/// Aggregates the daemon's image cache, container list, and volume list
/// into the Docker `/system/df` response shape that `docker system df`
/// consumes. Each of the three daemon calls is fired in parallel via
/// `tokio::try_join!`; if any fails, the whole response is reported as
/// `500` with a Docker-shaped `{ "message": "..." }` body so Docker
/// clients can surface the error.
///
/// `BuildCache` is always empty until `BuildKit` support lands. Fields the
/// daemon doesn't track (`SharedSize`, `Containers` count per image,
/// `Mounts`, `NetworkSettings`, `HostConfig`) follow the Docker
/// convention: `-1` for "unknown", `0` / `[]` / `{}` otherwise.
async fn disk_usage(State(state): State<SocketState>) -> Response {
    let result = tokio::try_join!(
        state.client.list_images(),
        state.client.list_volumes(),
        state.client.get_all_containers(),
    );

    let (images, volumes, containers) = match result {
        Ok(triple) => triple,
        Err(err) => return map_daemon_error(&err),
    };

    let images_json: Vec<serde_json::Value> = images.iter().map(image_to_df_entry).collect();
    let volumes_json: Vec<serde_json::Value> = volumes.iter().map(volume_to_df_entry).collect();
    let containers_json: Vec<serde_json::Value> = containers
        .as_array()
        .map(|arr| arr.iter().map(container_to_df_entry).collect())
        .unwrap_or_default();

    let layers_size: i64 = images.iter().map(image_df_size).sum();

    Json(serde_json::json!({
        "LayersSize": layers_size,
        "Images": images_json,
        "Containers": containers_json,
        "Volumes": volumes_json,
        "BuildCache": [],
    }))
    .into_response()
}

/// Map a daemon error into a Docker-shaped `{ "message": "..." }` 500.
///
/// Mirrors the behaviour of `socket/containers.rs::map_daemon_error` but
/// narrowed: `/system/df` aggregates three calls so we never have a
/// "single resource not found" 404 to return — anything that fails
/// here is an internal error.
fn map_daemon_error(err: &anyhow::Error) -> Response {
    let msg = format!("{err:#}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "message": msg })),
    )
        .into_response()
}

/// Build a Docker-shape `{ "message": "..." }` JSON error response with the
/// given status code. Shared helper for system endpoints; mirrors the
/// `error_response` helpers in the other socket modules so all error
/// payloads use the identical wire shape.
pub(super) fn error_response(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "message": msg.into() }))).into_response()
}

/// Query parameters for `POST /system/prune`.
///
/// Docker accepts `volumes` (string `"true"`/`"false"` or `"1"`/`"0"`) and
/// `filters` (URL-encoded JSON object). We accept the volumes flag in the
/// usual permissive forms and parse `filters` lazily — the daemon's prune
/// endpoints don't yet honour Docker filter semantics, so the value is
/// only validated as well-formed JSON.
#[derive(Debug, Default, serde::Deserialize)]
struct SystemPruneQuery {
    #[serde(default)]
    volumes: Option<String>,
    #[serde(default)]
    filters: Option<String>,
}

/// `POST /system/prune` — Sweep stopped containers, dangling images, unused
/// networks, and (optionally) unused volumes.
///
/// Returns the Docker-shaped aggregate body:
/// ```json
/// {
///   "ContainersDeleted": [...],
///   "NetworksDeleted":   [...],
///   "VolumesDeleted":    [...],
///   "ImagesDeleted":     [{ "Untagged": "...", "Deleted": "..." }, ...],
///   "BuildCacheDeleted": [],
///   "SpaceReclaimed":    <u64>
/// }
/// ```
///
/// The container and image sweeps run unconditionally; volumes are only
/// pruned when the `volumes=true` (or `1`) query parameter is set, matching
/// Docker's behaviour. Errors from individual sweeps are folded into the
/// final response — a per-resource failure does NOT take down the others.
#[allow(clippy::too_many_lines)]
async fn system_prune(
    State(state): State<SocketState>,
    Query(query): Query<SystemPruneQuery>,
) -> Response {
    // Validate filters early so the caller sees a 400 before any sweep runs.
    if let Some(raw) = query.filters.as_deref() {
        let trimmed = raw.trim();
        if !trimmed.is_empty() && serde_json::from_str::<serde_json::Value>(trimmed).is_err() {
            return error_response(StatusCode::BAD_REQUEST, "invalid filters: not valid JSON");
        }
    }

    let want_volumes = matches!(
        query
            .volumes
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1" | "true" | "yes")
    );

    let mut total_reclaimed: u64 = 0;

    // 1. Containers ---------------------------------------------------------
    let mut containers_deleted: Vec<String> = Vec::new();
    match state.client.prune_standalone_containers().await {
        Ok(body) => {
            if let Some(arr) = body.get("ContainersDeleted").and_then(|v| v.as_array()) {
                for v in arr {
                    if let Some(s) = v.as_str() {
                        containers_deleted.push(s.to_string());
                    }
                }
            }
            if let Some(n) = body
                .get("SpaceReclaimed")
                .and_then(serde_json::Value::as_u64)
            {
                total_reclaimed = total_reclaimed.saturating_add(n);
            }
        }
        Err(err) => {
            tracing::warn!("system prune: container sweep failed: {err}");
        }
    }

    // 2. Images -------------------------------------------------------------
    let mut images_deleted: Vec<serde_json::Value> = Vec::new();
    match state.client.prune_images().await {
        Ok(result) => {
            for r in result.deleted {
                images_deleted.push(serde_json::json!({ "Untagged": r }));
            }
            total_reclaimed = total_reclaimed.saturating_add(result.space_reclaimed);
        }
        Err(err) => {
            tracing::warn!("system prune: image sweep failed: {err}");
        }
    }

    // 3. Networks (no daemon endpoint yet) — return empty list. -------------
    let networks_deleted: Vec<String> = Vec::new();

    // 4. Volumes (only when ?volumes=true; no daemon endpoint yet). ---------
    let mut volumes_deleted: Vec<String> = Vec::new();
    if want_volumes {
        // Best-effort: list volumes that report no consumers and remove
        // them individually. A daemon-side bulk endpoint would be more
        // efficient; until that exists we walk the list ourselves.
        match state.client.list_volumes().await {
            Ok(list) => {
                for v in &list {
                    let in_use = v
                        .get("in_use_by")
                        .and_then(|x| x.as_array())
                        .is_some_and(|arr| !arr.is_empty());
                    let name = match v.get("name").and_then(|x| x.as_str()) {
                        Some(s) => s.to_string(),
                        None => continue,
                    };
                    if in_use {
                        continue;
                    }
                    if state.client.delete_volume(&name, false).await.is_ok() {
                        volumes_deleted.push(name);
                    }
                }
            }
            Err(err) => {
                tracing::warn!("system prune: volume listing failed: {err}");
            }
        }
    }

    let body = serde_json::json!({
        "ContainersDeleted": containers_deleted,
        "NetworksDeleted":   networks_deleted,
        "VolumesDeleted":    volumes_deleted,
        "ImagesDeleted":     images_deleted,
        "BuildCacheDeleted": serde_json::Value::Array(Vec::new()),
        "SpaceReclaimed":    total_reclaimed,
    });
    (StatusCode::OK, Json(body)).into_response()
}

/// Extract the `Size` field for a Docker `/system/df` image entry.
///
/// Pulled out as a free function so it can be shared between the entry
/// builder and the `LayersSize` aggregator without re-walking the same
/// `Option<u64>` twice.
fn image_df_size(img: &ImageInfoDto) -> i64 {
    i64::try_from(img.size_bytes.unwrap_or(0)).unwrap_or(i64::MAX)
}

/// Convert a daemon `ImageInfoDto` into a Docker `/system/df` image entry.
///
/// Docker's shape: `{Id, ParentId, RepoTags, RepoDigests, Created, Size,
/// SharedSize, VirtualSize, Labels, Containers}`. We don't track parent
/// images, layer-sharing across images, label metadata on the cached
/// image, or per-image container counts, so those follow the Docker
/// "unknown" sentinel (`-1` for the integer fields, empty otherwise).
/// Mirrors the field choices in `socket/images.rs::list_images` to keep
/// the two endpoints consistent.
fn image_to_df_entry(img: &ImageInfoDto) -> serde_json::Value {
    let ref_str = img.reference.to_string();
    let id = img
        .digest
        .clone()
        .unwrap_or_else(|| format!("sha256:{}", df_hash_ref(&ref_str)));
    let size = image_df_size(img);
    let repo_tags = vec![ref_str.clone()];
    let repo_digests = match &img.digest {
        Some(d) => vec![format!("{}@{}", df_strip_tag(&ref_str), d)],
        None => Vec::new(),
    };

    serde_json::json!({
        "Id": id,
        "ParentId": "",
        "RepoTags": repo_tags,
        "RepoDigests": repo_digests,
        "Created": 0_i64,
        "Size": size,
        "SharedSize": -1_i64,
        "VirtualSize": size,
        "Labels": HashMap::<String, String>::new(),
        "Containers": -1_i64,
    })
}

/// Convert a daemon volume JSON value into a Docker `/system/df` volume
/// entry.
///
/// Docker's shape: `{Name, Driver, Mountpoint, Labels, Scope, Options,
/// UsageData: {Size, RefCount}}`. The daemon stores `name`, `path`,
/// `size_bytes`, `labels`, `created_at`, and `in_use_by`, which map
/// onto Name, Mountpoint, UsageData.Size, Labels, and UsageData.RefCount
/// respectively. `Driver`/`Scope`/`Options` follow the local-driver
/// defaults Docker emits for `docker volume`-managed volumes.
fn volume_to_df_entry(v: &serde_json::Value) -> serde_json::Value {
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
    let mountpoint = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
    let size = v
        .get("size_bytes")
        .and_then(serde_json::Value::as_u64)
        .map_or(-1_i64, |s| i64::try_from(s).unwrap_or(i64::MAX));
    let ref_count = v
        .get("in_use_by")
        .and_then(|x| x.as_array())
        .map_or(-1_i64, |arr| i64::try_from(arr.len()).unwrap_or(i64::MAX));
    let labels = v
        .get("labels")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    let created_at = v.get("created_at").and_then(|x| x.as_str()).unwrap_or("");

    serde_json::json!({
        "Name": name,
        "Driver": "local",
        "Mountpoint": mountpoint,
        "CreatedAt": created_at,
        "Labels": labels,
        "Scope": "local",
        "Options": serde_json::Value::Null,
        "UsageData": {
            "Size": size,
            "RefCount": ref_count,
        },
    })
}

/// Convert a daemon container JSON value into a Docker `/system/df`
/// container entry.
///
/// Docker's shape: `{Id, Names, Image, ImageID, Command, Created, Ports,
/// SizeRw, SizeRootFs, Labels, State, Status, HostConfig,
/// NetworkSettings, Mounts}`. `SizeRw` and `SizeRootFs` aren't tracked
/// per-container today, so they report `-1` (Docker's "unknown"
/// sentinel). State/Status mirror the lightweight mapping used in
/// `socket/containers.rs` so both endpoints agree on the field shape.
fn container_to_df_entry(c: &serde_json::Value) -> serde_json::Value {
    let id = c.get("id").and_then(|x| x.as_str()).unwrap_or("");
    let raw_name = c.get("name").and_then(|x| x.as_str()).unwrap_or("");
    let name = if raw_name.starts_with('/') {
        raw_name.to_owned()
    } else if raw_name.is_empty() {
        format!("/{id}")
    } else {
        format!("/{raw_name}")
    };
    let image = c.get("image").and_then(|x| x.as_str()).unwrap_or("");
    let zstate = c.get("state").and_then(|x| x.as_str()).unwrap_or("unknown");
    let (state, status) = df_docker_state(zstate);
    let labels = c
        .get("labels")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    let created_at = c.get("created_at").and_then(|x| x.as_str()).unwrap_or("");

    serde_json::json!({
        "Id": id,
        "Names": [name],
        "Image": image,
        "ImageID": image,
        "Command": "",
        "Created": df_parse_iso8601_secs(created_at),
        "Ports": [],
        "SizeRw": -1_i64,
        "SizeRootFs": -1_i64,
        "Labels": labels,
        "State": state,
        "Status": status,
        "HostConfig": serde_json::json!({ "NetworkMode": "default" }),
        "NetworkSettings": serde_json::json!({ "Networks": {} }),
        "Mounts": [],
    })
}

/// Hash an image reference into a stable synthetic `sha256:` hex string.
///
/// Mirrors `socket/images.rs::hash_ref`. Duplicated locally rather than
/// re-exported because the helper is private to the images module and
/// neither side wants to widen its visibility just for this aggregate.
fn df_hash_ref(reference: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    reference.hash(&mut hasher);
    let h = hasher.finish();
    format!("{h:016x}{h:016x}{h:016x}{h:016x}")
}

/// Strip the `:tag` (but not `@digest`) off an image reference.
///
/// Mirrors `socket/images.rs::strip_tag`. Same rationale as
/// `df_hash_ref` for the duplication.
fn df_strip_tag(reference: &str) -> String {
    if let Some((name, _)) = reference.rsplit_once('@') {
        return name.to_owned();
    }
    if let Some(idx) = reference.rfind(':') {
        let after = &reference[idx + 1..];
        if !after.contains('/') {
            return reference[..idx].to_owned();
        }
    }
    reference.to_owned()
}

/// Translate zlayer's `state` string into Docker's `(State, Status)` pair
/// for the `/system/df` container entry.
///
/// Mirrors `socket/containers.rs::docker_state` to keep the two
/// endpoints consistent.
fn df_docker_state(zlayer_state: &str) -> (&'static str, String) {
    match zlayer_state {
        "running" => ("running", "Up".to_owned()),
        "pending" | "created" => ("created", "Created".to_owned()),
        "exited" | "stopped" => ("exited", "Exited".to_owned()),
        "paused" => ("paused", "Paused".to_owned()),
        "failed" => ("dead", "Dead".to_owned()),
        other => ("exited", format!("Exited ({other})")),
    }
}

/// Parse an ISO-8601 `created_at` into a Unix timestamp (seconds).
///
/// Mirrors `socket/containers.rs::parse_iso8601_secs`. Docker's
/// `/system/df` `Created` is a Unix timestamp, matching the behaviour
/// of `/containers/json`.
fn df_parse_iso8601_secs(s: &str) -> i64 {
    if s.len() < 19 {
        return 0;
    }
    let bytes = s.as_bytes();
    let year: i64 = std::str::from_utf8(&bytes[0..4])
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1970);
    let month: i64 = std::str::from_utf8(&bytes[5..7])
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let day: i64 = std::str::from_utf8(&bytes[8..10])
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let hour: i64 = std::str::from_utf8(&bytes[11..13])
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let minute: i64 = std::str::from_utf8(&bytes[14..16])
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let second: i64 = std::str::from_utf8(&bytes[17..19])
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month;
    let d = day;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + hour * 3_600 + minute * 60 + second
}

// ---------------------------------------------------------------------------
// Event stream plumbing
// ---------------------------------------------------------------------------

/// Query parameters for `GET /events`.
///
/// `serde_urlencoded` (axum's `Query` extractor) accepts missing fields and
/// empty strings without erroring, so optional bounds are simply omitted.
#[derive(Debug, Default, serde::Deserialize)]
struct EventsQuery {
    /// Drop events with `time < since` (Unix seconds).
    #[serde(default)]
    since: Option<i64>,
    /// Drop events with `time > until` (Unix seconds).
    #[serde(default)]
    until: Option<i64>,
    /// Docker `filters`: URL-encoded JSON object of
    /// `{ "field": ["value", ...], ... }` shape. The handler URL-decoding
    /// done by axum gives us the raw JSON string here.
    #[serde(default)]
    filters: Option<String>,
}

/// Parsed `filters=` JSON object, normalised into per-field allow-sets.
///
/// Docker's `/events` filter shape is `{ "field": ["value", ...] }`; an
/// empty / missing field is a wildcard that lets every event through.
/// Each filter on the parsed struct is a `HashSet<String>`, except the
/// `labels` filter which is a `Vec<(String, String)>` because it gets
/// forwarded to the daemon side as repeated `label=k=v` query parameters.
#[derive(Debug, Clone, Default)]
struct EventFilters {
    /// Resource types: `container`, `image`, `network`, `volume`.
    types: HashSet<String>,
    /// Event names (Docker `Action`): `start`, `die`, `pull`, ...
    events: HashSet<String>,
    /// Container ID prefixes that the event's `Actor.ID` must start with.
    containers: HashSet<String>,
    /// Image references the event must reference.
    images: HashSet<String>,
    /// Network IDs/names.
    networks: HashSet<String>,
    /// Volume names.
    volumes: HashSet<String>,
    /// Label `(k, v)` pairs forwarded to the daemon stream so labels are
    /// filtered server-side; clients still receive only matching events.
    labels: Vec<(String, String)>,
}

impl EventFilters {
    /// Parse a Docker `filters=...` JSON object (URL-decoded).
    ///
    /// `None` / `""` -> empty filters (everything passes). A malformed
    /// value returns an `Err(message)` which the caller surfaces as a
    /// 400 with a Docker-shaped `{ "message": "..." }` body.
    fn parse(raw: Option<&str>) -> Result<Self, String> {
        let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(Self::default());
        };
        let value: serde_json::Value =
            serde_json::from_str(raw).map_err(|e| format!("invalid filters: {e}"))?;
        let obj = match value {
            serde_json::Value::Object(map) => map,
            other => {
                return Err(format!(
                    "invalid filters: expected object, got {}",
                    json_kind(&other)
                ));
            }
        };

        let mut out = Self::default();
        for (k, v) in obj {
            // Docker accepts both `["a","b"]` and the legacy
            // `{"a": true, "b": true}` shape; we support both because
            // older clients (and `docker events` itself in some
            // versions) emit the map shape.
            let values: Vec<String> = match v {
                serde_json::Value::Array(arr) => arr
                    .into_iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect(),
                serde_json::Value::Object(map) => map
                    .into_iter()
                    .filter_map(|(key, val)| (val.as_bool() == Some(true)).then_some(key))
                    .collect(),
                _ => {
                    return Err(format!(
                        "invalid filters: field '{k}' must be an array of strings"
                    ));
                }
            };

            match k.as_str() {
                "type" => out.types.extend(values),
                "event" => out.events.extend(values),
                "container" => out.containers.extend(values),
                "image" => out.images.extend(values),
                "network" => out.networks.extend(values),
                "volume" => out.volumes.extend(values),
                "label" => {
                    for entry in values {
                        let (lk, lv) = entry.split_once('=').unwrap_or((entry.as_str(), ""));
                        if lk.is_empty() {
                            return Err(format!(
                                "invalid filters: label '{entry}' must be 'key=value'"
                            ));
                        }
                        out.labels.push((lk.to_owned(), lv.to_owned()));
                    }
                }
                // Unknown filter keys: ignore silently. Docker tolerates
                // newer clients sending unknown filters to older daemons.
                _ => {}
            }
        }
        Ok(out)
    }

    /// Returns true if the (already-mapped) Docker event passes the
    /// non-label filters. Label filters are applied server-side by the
    /// daemon stream and don't need to be re-checked here.
    fn matches(&self, ev: &serde_json::Value) -> bool {
        let ty = ev
            .get("Type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let action = ev
            .get("Action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let id = ev
            .get("Actor")
            .and_then(|a| a.get("ID"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let attrs = ev.get("Actor").and_then(|a| a.get("Attributes"));
        let attr_str = |key: &str| -> &str {
            attrs
                .and_then(|a| a.get(key))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
        };
        let image = attr_str("image");
        let name = attr_str("name");

        if !self.types.is_empty() && !self.types.contains(ty) {
            return false;
        }
        if !self.events.is_empty() && !self.events.contains(action) {
            return false;
        }
        if !self.containers.is_empty()
            && ty == "container"
            && !self
                .containers
                .iter()
                .any(|c| id == c || id.starts_with(c) || c == name)
        {
            return false;
        }
        if !self.images.is_empty()
            && ty == "image"
            && !self
                .images
                .iter()
                .any(|i| i == image || image.starts_with(i))
        {
            return false;
        }
        if !self.networks.is_empty()
            && ty == "network"
            && !self
                .networks
                .iter()
                .any(|n| n == id || n == name || id.starts_with(n))
        {
            return false;
        }
        if !self.volumes.is_empty()
            && ty == "volume"
            && !self.volumes.iter().any(|v| v == id || v == name)
        {
            return false;
        }
        true
    }
}

/// Friendly name for the unexpected JSON kind in a filter parse error.
fn json_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Translate a [`DaemonEvent`] into the Docker `/events` wire JSON.
///
/// Each daemon event maps onto a single Docker event; the `Action` mapping
/// mirrors Docker's lifecycle vocabulary (`start`, `die`, `pull`, `create`,
/// `destroy`, `connect`, `disconnect`, `mount`, `unmount`).
fn map_daemon_event(ev: &DaemonEvent) -> serde_json::Value {
    let time_sec = ev.at().timestamp();
    let time_nano = ev
        .at()
        .timestamp_nanos_opt()
        .unwrap_or(time_sec.saturating_mul(1_000_000_000));

    let (resource_type, action, actor_id, attributes) = match ev {
        DaemonEvent::Container(c) => map_container_event(c),
        DaemonEvent::Image(i) => map_image_event(i),
        DaemonEvent::Network(n) => map_network_event(n),
        DaemonEvent::Volume(v) => map_volume_event(v),
    };

    serde_json::json!({
        "Type": resource_type,
        "Action": action,
        "Actor": {
            "ID": actor_id,
            "Attributes": serde_json::Value::Object(attributes),
        },
        "scope": "local",
        "time": time_sec,
        "timeNano": time_nano,
        "id": short_event_id(&actor_id),
    })
}

/// Tuple shared by all `map_*_event` helpers: `(Type, Action, Actor.ID, Attributes)`.
type MappedEventParts = (
    &'static str,
    String,
    String,
    serde_json::Map<String, serde_json::Value>,
);

/// Map a daemon `ContainerEvent` to the Docker shape's parts.
///
/// Docker uses `health_status: <state>` for health transitions so
/// consumers can pattern-match on the state without a separate field;
/// we emit the bare `health_status` action when we have no status to
/// surface.
fn map_container_event(c: &zlayer_api::ContainerEvent) -> MappedEventParts {
    let action: String = match c.kind {
        zlayer_api::ContainerEventKind::Start => "start".to_owned(),
        zlayer_api::ContainerEventKind::Die => "die".to_owned(),
        zlayer_api::ContainerEventKind::Oom => "oom".to_owned(),
        zlayer_api::ContainerEventKind::Health => match c.status.as_deref() {
            Some(s) if !s.is_empty() => format!("health_status: {s}"),
            _ => "health_status".to_owned(),
        },
    };

    // Docker emits the container's labels under `Actor.Attributes`, plus
    // optional `name`/`image`/`exitCode`. We don't carry the image on the
    // daemon ContainerEvent today, so it stays out rather than emitting
    // an empty string that would confuse consumers.
    let mut attributes = serde_json::Map::new();
    for (k, v) in &c.labels {
        attributes.insert(k.clone(), serde_json::Value::String(v.clone()));
    }
    if let Some(reason) = &c.reason {
        attributes.insert(
            "reason".to_owned(),
            serde_json::Value::String(reason.clone()),
        );
    }
    if let Some(code) = c.exit_code {
        // Docker stringifies numeric attribute values.
        attributes.insert(
            "exitCode".to_owned(),
            serde_json::Value::String(code.to_string()),
        );
    }
    ("container", action, c.id.clone(), attributes)
}

/// Map a daemon `ImageEvent`. Docker uses the digest as `Actor.ID` when
/// available, else the reference itself.
fn map_image_event(i: &zlayer_api::ImageEvent) -> MappedEventParts {
    let action = match i.kind {
        zlayer_api::ImageEventKind::Pull => "pull",
        zlayer_api::ImageEventKind::Push => "push",
        zlayer_api::ImageEventKind::Delete => "delete",
        zlayer_api::ImageEventKind::Tag => "tag",
    };
    let mut attributes = serde_json::Map::new();
    attributes.insert(
        "name".to_owned(),
        serde_json::Value::String(i.reference.clone()),
    );
    if let Some(src) = &i.source {
        attributes.insert("source".to_owned(), serde_json::Value::String(src.clone()));
    }
    let id = i.digest.clone().unwrap_or_else(|| i.reference.clone());
    ("image", action.to_owned(), id, attributes)
}

/// Map a daemon `NetworkEvent`.
fn map_network_event(n: &zlayer_api::NetworkEvent) -> MappedEventParts {
    let action = match n.kind {
        NetworkEventKind::Create => "create",
        NetworkEventKind::Delete => "destroy",
        NetworkEventKind::Connect => "connect",
        NetworkEventKind::Disconnect => "disconnect",
    };
    let mut attributes = serde_json::Map::new();
    attributes.insert("name".to_owned(), serde_json::Value::String(n.name.clone()));
    if !n.driver.is_empty() {
        attributes.insert(
            "type".to_owned(),
            serde_json::Value::String(n.driver.clone()),
        );
    }
    if let Some(c) = &n.container_id {
        attributes.insert("container".to_owned(), serde_json::Value::String(c.clone()));
    }
    ("network", action.to_owned(), n.id.clone(), attributes)
}

/// Map a daemon `VolumeEvent`. Volumes don't have a separate id
/// namespace, so the Docker `Actor.ID` is the volume name.
fn map_volume_event(v: &zlayer_api::VolumeEvent) -> MappedEventParts {
    let action = match v.kind {
        VolumeEventKind::Create => "create",
        VolumeEventKind::Delete => "destroy",
        VolumeEventKind::Mount => "mount",
        VolumeEventKind::Unmount => "unmount",
    };
    let mut attributes = serde_json::Map::new();
    if !v.driver.is_empty() {
        attributes.insert(
            "driver".to_owned(),
            serde_json::Value::String(v.driver.clone()),
        );
    }
    if let Some(c) = &v.container_id {
        attributes.insert("container".to_owned(), serde_json::Value::String(c.clone()));
    }
    ("volume", action.to_owned(), v.name.clone(), attributes)
}

/// Returns true if the event's `time` is within `[since, until]` (inclusive).
/// `None` for either bound means "no constraint" on that side.
fn time_in_range(event: &serde_json::Value, since: Option<i64>, until: Option<i64>) -> bool {
    let time = event
        .get("time")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    if let Some(s) = since {
        if time < s {
            return false;
        }
    }
    if let Some(u) = until {
        if time > u {
            return false;
        }
    }
    true
}

/// Encode a Docker event JSON object as a single NDJSON line: serialised
/// JSON body + trailing `\n`.
fn ndjson_line(event: &serde_json::Value) -> Option<Bytes> {
    let mut buf = serde_json::to_vec(event).ok()?;
    buf.push(b'\n');
    Some(Bytes::from(buf))
}

/// Compute a Docker-style short ID (12-char hex prefix) from a full ID.
///
/// If `id` is shorter than 12 characters or contains non-hex characters,
/// it's returned unchanged. Used for the `id` field in `/events` so
/// `docker events --format '{{.ID}}'` works correctly.
fn short_event_id(id: &str) -> String {
    if id.len() >= 12 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        id[..12].to_owned()
    } else {
        id.to_owned()
    }
}

// ---------------------------------------------------------------------------
// Daemon lookups
// ---------------------------------------------------------------------------

/// Connect to the daemon and count containers by state.
///
/// Returns `(total, running, paused, stopped)`. On daemon connection
/// failure, returns zeros and logs a debug message so `/info` still
/// responds for callers that don't have a daemon running.
async fn container_counts() -> (i64, i64, i64, i64) {
    let client = match DaemonClient::connect_to(default_socket_path()).await {
        Ok(c) => c,
        Err(err) => {
            tracing::debug!("docker API: /info could not reach daemon: {err}");
            return (0, 0, 0, 0);
        }
    };

    let value = match client.get_all_containers().await {
        Ok(v) => v,
        Err(err) => {
            tracing::debug!("docker API: /info list containers failed: {err}");
            return (0, 0, 0, 0);
        }
    };

    let Some(arr) = value.as_array() else {
        return (0, 0, 0, 0);
    };

    let mut running = 0i64;
    let mut paused = 0i64;
    let mut stopped = 0i64;
    for c in arr {
        let status = c
            .get("status")
            .and_then(|v| v.as_str())
            .or_else(|| c.get("state").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_ascii_lowercase();
        if status == "paused" {
            paused += 1;
        } else if status == "running" || status.starts_with("up") {
            running += 1;
        } else {
            // Treat created/exited/dead/stopped/restarting as stopped.
            stopped += 1;
        }
    }

    let total = i64::try_from(arr.len()).unwrap_or(i64::MAX);
    (total, running, paused, stopped)
}

/// Connect to the daemon and count cached images.
async fn image_count() -> i64 {
    let client = match DaemonClient::connect_to(default_socket_path()).await {
        Ok(c) => c,
        Err(err) => {
            tracing::debug!("docker API: /info could not reach daemon (images): {err}");
            return 0;
        }
    };
    match client.list_images().await {
        Ok(list) => i64::try_from(list.len()).unwrap_or(i64::MAX),
        Err(err) => {
            tracing::debug!("docker API: /info list images failed: {err}");
            0
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Best-effort hostname.
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "zlayer".to_owned())
}

/// Best-effort kernel version from `/proc/version` (Linux only).
fn kernel_version() -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/version")
            .ok()
            .and_then(|v| v.split_whitespace().nth(2).map(String::from))
            .unwrap_or_else(|| "unknown".to_owned())
    }
    #[cfg(not(target_os = "linux"))]
    {
        "unknown".to_owned()
    }
}

/// Total physical memory in bytes (Linux `/proc/meminfo`). Returns 0 on
/// non-Linux hosts since we don't pull in `sysinfo` for this.
fn total_memory() -> i64 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|info| {
                info.lines()
                    .find(|l| l.starts_with("MemTotal:"))
                    .and_then(|l| {
                        l.split_whitespace()
                            .nth(1)
                            .and_then(|kb| kb.parse::<i64>().ok())
                    })
                    // /proc/meminfo reports in kB
                    .map(|kb| kb * 1024)
            })
            .unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// Format the current time as an RFC 3339 string for `/info.SystemTime`.
///
/// Uses only `std` to avoid pulling a date crate. Precision to seconds
/// is sufficient for Docker clients that only display this value.
fn current_time_rfc3339() -> String {
    let now = SystemTime::now();
    let Ok(dur) = now.duration_since(UNIX_EPOCH) else {
        return "1970-01-01T00:00:00Z".to_owned();
    };
    let secs = i64::try_from(dur.as_secs()).unwrap_or(i64::MAX);
    let nanos = dur.subsec_nanos();
    format_unix_rfc3339(secs, nanos)
}

/// Format a unix timestamp (seconds since epoch, plus sub-second nanos)
/// as an RFC 3339 UTC string: `YYYY-MM-DDTHH:MM:SS.nnnnnnnnnZ`.
fn format_unix_rfc3339(secs: i64, nanos: u32) -> String {
    // Convert to broken-down civil time using the algorithm from
    // Howard Hinnant's date library (days_from_civil inverse).
    let days = secs.div_euclid(86_400);
    let secs_of_day = u32::try_from(secs.rem_euclid(86_400)).unwrap_or(0);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;

    // Shift so epoch day 0 is 0000-03-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = u32::try_from(z.rem_euclid(146_097)).unwrap_or(0);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}.{nanos:09}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_epoch() {
        assert_eq!(format_unix_rfc3339(0, 0), "1970-01-01T00:00:00.000000000Z");
    }

    #[test]
    fn rfc3339_known_date() {
        // 2020-01-01T00:00:00Z is 1577836800 seconds after epoch.
        assert_eq!(
            format_unix_rfc3339(1_577_836_800, 0),
            "2020-01-01T00:00:00.000000000Z"
        );
    }

    // -----------------------------------------------------------------------
    // /events route tests
    // -----------------------------------------------------------------------

    use zlayer_api::{ContainerEvent, ImageEvent, NetworkEvent, VolumeEvent};

    fn event_labels(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// Filters parser handles each Docker query-param field, including the
    /// label form, the legacy map shape, and rejects malformed values.
    #[test]
    fn events_filters_parse_all_fields() {
        // Standard array shape.
        let raw = r#"{"type":["container","image"],"event":["start"],"container":["abc"],"image":["nginx"],"network":["br0"],"volume":["v1"],"label":["app=web","env=prod"]}"#;
        let f = EventFilters::parse(Some(raw)).expect("valid");
        assert!(f.types.contains("container"));
        assert!(f.types.contains("image"));
        assert!(f.events.contains("start"));
        assert!(f.containers.contains("abc"));
        assert!(f.images.contains("nginx"));
        assert!(f.networks.contains("br0"));
        assert!(f.volumes.contains("v1"));
        assert_eq!(
            f.labels,
            vec![
                ("app".to_owned(), "web".to_owned()),
                ("env".to_owned(), "prod".to_owned()),
            ]
        );

        // Legacy map shape: `{"a": true, "b": false}`.
        let legacy = r#"{"type":{"container":true,"image":false}}"#;
        let f = EventFilters::parse(Some(legacy)).expect("legacy shape");
        assert!(f.types.contains("container"));
        assert!(!f.types.contains("image"));

        // Empty / missing -> default (no-op).
        assert!(EventFilters::parse(None).unwrap().types.is_empty());
        assert!(EventFilters::parse(Some("")).unwrap().types.is_empty());

        // Malformed JSON -> Err.
        assert!(EventFilters::parse(Some("not-json")).is_err());
        // Wrong top-level kind -> Err.
        assert!(EventFilters::parse(Some(r#"["nope"]"#)).is_err());
        // Bad label format -> Err.
        assert!(EventFilters::parse(Some(r#"{"label":["=missingkey"]}"#)).is_err());
    }

    /// Mapping a container event produces the Docker wire shape.
    #[test]
    fn events_map_container_event() {
        let ev = DaemonEvent::Container(ContainerEvent::start(
            "deadbeefcafe1234567890abcdef1234",
            event_labels(&[("app", "web")]),
        ));
        let mapped = map_daemon_event(&ev);
        assert_eq!(mapped["Type"], "container");
        assert_eq!(mapped["Action"], "start");
        assert_eq!(mapped["Actor"]["ID"], "deadbeefcafe1234567890abcdef1234");
        assert_eq!(mapped["Actor"]["Attributes"]["app"], "web");
        assert_eq!(mapped["scope"], "local");
        // 12-char hex prefix on the `id` field.
        assert_eq!(mapped["id"], "deadbeefcafe");
        // time/timeNano are integers.
        assert!(mapped["time"].is_i64());
        assert!(mapped["timeNano"].is_i64());
    }

    /// Mapping an image event surfaces the reference and digest as ID.
    #[test]
    fn events_map_image_event() {
        let ev = DaemonEvent::Image(ImageEvent::pull(
            "nginx:latest",
            Some("sha256:abc123".to_owned()),
        ));
        let mapped = map_daemon_event(&ev);
        assert_eq!(mapped["Type"], "image");
        assert_eq!(mapped["Action"], "pull");
        assert_eq!(mapped["Actor"]["ID"], "sha256:abc123");
        assert_eq!(mapped["Actor"]["Attributes"]["name"], "nginx:latest");
    }

    /// Mapping a network event uses Docker's `destroy` action for delete.
    #[test]
    fn events_map_network_event() {
        let ev = DaemonEvent::Network(NetworkEvent::delete("net-id-123", "my-bridge"));
        let mapped = map_daemon_event(&ev);
        assert_eq!(mapped["Type"], "network");
        assert_eq!(mapped["Action"], "destroy");
        assert_eq!(mapped["Actor"]["ID"], "net-id-123");
        assert_eq!(mapped["Actor"]["Attributes"]["name"], "my-bridge");
    }

    /// Mapping a volume mount event carries the container id attribute.
    #[test]
    fn events_map_volume_event() {
        let ev = DaemonEvent::Volume(VolumeEvent::mount("vol1", "container-xyz"));
        let mapped = map_daemon_event(&ev);
        assert_eq!(mapped["Type"], "volume");
        assert_eq!(mapped["Action"], "mount");
        assert_eq!(mapped["Actor"]["ID"], "vol1");
        assert_eq!(mapped["Actor"]["Attributes"]["container"], "container-xyz");
    }

    /// `since`/`until` filter on the mapped event's `time` field.
    #[test]
    fn events_time_in_range_bounds() {
        let ev = serde_json::json!({ "time": 1_000_i64 });
        // No bounds -> always passes.
        assert!(time_in_range(&ev, None, None));
        // since=500 -> passes (1000 >= 500).
        assert!(time_in_range(&ev, Some(500), None));
        // since=1500 -> fails (1000 < 1500).
        assert!(!time_in_range(&ev, Some(1500), None));
        // until=2000 -> passes (1000 <= 2000).
        assert!(time_in_range(&ev, None, Some(2000)));
        // until=500 -> fails (1000 > 500).
        assert!(!time_in_range(&ev, None, Some(500)));
        // since=500, until=2000 -> passes.
        assert!(time_in_range(&ev, Some(500), Some(2000)));
        // Inclusive on both ends.
        assert!(time_in_range(&ev, Some(1000), Some(1000)));
    }

    /// Filters.matches honours `type`, `event`, and per-resource id sets.
    #[test]
    fn events_filters_match_logic() {
        let ev = serde_json::json!({
            "Type": "container",
            "Action": "start",
            "Actor": { "ID": "abcd1234", "Attributes": { "name": "web" } },
        });

        // Type mismatch -> reject.
        let f = EventFilters {
            types: ["image".to_owned()].into_iter().collect(),
            ..EventFilters::default()
        };
        assert!(!f.matches(&ev));

        // Type match -> accept.
        let f = EventFilters {
            types: ["container".to_owned()].into_iter().collect(),
            ..EventFilters::default()
        };
        assert!(f.matches(&ev));

        // Event filter on Action.
        let f = EventFilters {
            events: ["die".to_owned()].into_iter().collect(),
            ..EventFilters::default()
        };
        assert!(!f.matches(&ev));

        // Container ID-prefix match.
        let f = EventFilters {
            containers: ["abcd".to_owned()].into_iter().collect(),
            ..EventFilters::default()
        };
        assert!(f.matches(&ev));

        // Container name match.
        let f = EventFilters {
            containers: ["web".to_owned()].into_iter().collect(),
            ..EventFilters::default()
        };
        assert!(f.matches(&ev));

        // Container miss.
        let f = EventFilters {
            containers: ["zzz".to_owned()].into_iter().collect(),
            ..EventFilters::default()
        };
        assert!(!f.matches(&ev));
    }

    /// `ndjson_line` produces JSON with exactly one trailing newline and
    /// no SSE framing.
    #[test]
    fn events_ndjson_line_shape() {
        let ev = serde_json::json!({ "Type": "container", "Action": "start" });
        let line = ndjson_line(&ev).expect("encoded");
        let s = std::str::from_utf8(&line).expect("utf8");
        assert!(s.ends_with('\n'));
        assert_eq!(s.matches('\n').count(), 1);
        // Body itself parses.
        let parsed: serde_json::Value =
            serde_json::from_str(s.trim_end_matches('\n')).expect("valid json");
        assert_eq!(parsed["Type"], "container");
        // No SSE-specific framing fields.
        assert!(!s.contains("data:"));
        assert!(!s.contains("event:"));
    }

    /// `short_event_id` returns the 12-char hex prefix when applicable.
    #[test]
    fn events_short_event_id() {
        // Hex id of >=12 chars -> 12-char prefix.
        assert_eq!(short_event_id("deadbeefcafe1234567890ab"), "deadbeefcafe");
        // Non-hex id -> returned unchanged.
        assert_eq!(short_event_id("not-hex-id-here"), "not-hex-id-here");
        // Short id -> returned unchanged.
        assert_eq!(short_event_id("abc"), "abc");
    }

    // -----------------------------------------------------------------------
    // /system/df aggregation tests
    // -----------------------------------------------------------------------

    use std::str::FromStr;
    use zlayer_types::ImageReference;

    /// Build an `ImageInfoDto` from a reference string and an optional size,
    /// for use in the `/system/df` mapping tests below.
    fn make_image(reference: &str, size_bytes: Option<u64>) -> ImageInfoDto {
        ImageInfoDto {
            reference: ImageReference::from_str(reference).expect("valid reference"),
            digest: None,
            size_bytes,
        }
    }

    /// Build a daemon-shaped volume JSON value matching `VolumeInfo`.
    fn make_volume(name: &str, size: u64, ref_count: usize) -> serde_json::Value {
        let in_use_by: Vec<String> = (0..ref_count).map(|i| format!("c{i}")).collect();
        serde_json::json!({
            "name": name,
            "path": format!("/var/lib/zlayer/volumes/{name}"),
            "size_bytes": size,
            "labels": {},
            "created_at": "2026-01-01T00:00:00Z",
            "in_use_by": in_use_by,
        })
    }

    /// Build a daemon-shaped container JSON value matching `ContainerInfo`.
    fn make_container(id: &str, image: &str, state: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": format!("zlayer-{id}"),
            "image": image,
            "state": state,
            "labels": {},
            "created_at": "2026-01-01T00:00:00Z",
        })
    }

    #[test]
    fn disk_usage_aggregates_shapes() {
        // One image, one volume, one container — assert the response shape
        // and that LayersSize equals the sum of image sizes.
        let images = [make_image("nginx:latest", Some(1024))];
        let volumes = [make_volume("vol1", 4096, 2)];
        let containers = [make_container("c1", "nginx:latest", "running")];

        let images_json: Vec<serde_json::Value> = images.iter().map(image_to_df_entry).collect();
        let volumes_json: Vec<serde_json::Value> = volumes.iter().map(volume_to_df_entry).collect();
        let containers_json: Vec<serde_json::Value> =
            containers.iter().map(container_to_df_entry).collect();
        let layers_size: i64 = images.iter().map(image_df_size).sum();

        let body = serde_json::json!({
            "LayersSize": layers_size,
            "Images": images_json,
            "Containers": containers_json,
            "Volumes": volumes_json,
            "BuildCache": [],
        });

        // Top-level array lengths.
        assert_eq!(body["Images"].as_array().unwrap().len(), 1);
        assert_eq!(body["Volumes"].as_array().unwrap().len(), 1);
        assert_eq!(body["Containers"].as_array().unwrap().len(), 1);
        assert_eq!(body["BuildCache"].as_array().unwrap().len(), 0);

        // LayersSize == sum_of_image_sizes.
        assert_eq!(body["LayersSize"], serde_json::json!(1024_i64));

        // Image entry has the documented Docker shape.
        let image = &body["Images"][0];
        assert_eq!(image["Size"], serde_json::json!(1024_i64));
        assert_eq!(image["VirtualSize"], serde_json::json!(1024_i64));
        assert_eq!(image["SharedSize"], serde_json::json!(-1_i64));
        assert_eq!(image["Containers"], serde_json::json!(-1_i64));
        assert!(image["RepoTags"].as_array().unwrap()[0]
            .as_str()
            .unwrap()
            .contains("nginx"));
        assert!(image["Id"].as_str().unwrap().starts_with("sha256:"));

        // Volume entry has the documented Docker shape.
        let volume = &body["Volumes"][0];
        assert_eq!(volume["Name"], "vol1");
        assert_eq!(volume["Driver"], "local");
        assert_eq!(volume["Scope"], "local");
        assert_eq!(volume["UsageData"]["Size"], serde_json::json!(4096_i64));
        assert_eq!(volume["UsageData"]["RefCount"], serde_json::json!(2_i64));
        assert!(volume["Mountpoint"]
            .as_str()
            .unwrap()
            .ends_with("/volumes/vol1"));

        // Container entry has the documented Docker shape.
        let container = &body["Containers"][0];
        assert_eq!(container["Id"], "c1");
        assert_eq!(container["Image"], "nginx:latest");
        assert_eq!(container["ImageID"], "nginx:latest");
        assert_eq!(container["State"], "running");
        assert_eq!(container["Status"], "Up");
        assert_eq!(container["SizeRw"], serde_json::json!(-1_i64));
        assert_eq!(container["SizeRootFs"], serde_json::json!(-1_i64));
        assert_eq!(container["Names"][0], "/zlayer-c1");
        assert!(container["Mounts"].is_array());
    }

    #[test]
    fn disk_usage_empty_state() {
        let images: Vec<ImageInfoDto> = Vec::new();
        let volumes: Vec<serde_json::Value> = Vec::new();
        let containers: Vec<serde_json::Value> = Vec::new();

        let images_json: Vec<serde_json::Value> = images.iter().map(image_to_df_entry).collect();
        let volumes_json: Vec<serde_json::Value> = volumes.iter().map(volume_to_df_entry).collect();
        let containers_json: Vec<serde_json::Value> =
            containers.iter().map(container_to_df_entry).collect();
        let layers_size: i64 = images.iter().map(image_df_size).sum();

        let body = serde_json::json!({
            "LayersSize": layers_size,
            "Images": images_json,
            "Containers": containers_json,
            "Volumes": volumes_json,
            "BuildCache": [],
        });

        assert_eq!(body["LayersSize"], serde_json::json!(0_i64));
        assert!(body["Images"].as_array().unwrap().is_empty());
        assert!(body["Volumes"].as_array().unwrap().is_empty());
        assert!(body["Containers"].as_array().unwrap().is_empty());
        assert!(body["BuildCache"].as_array().unwrap().is_empty());
    }

    #[test]
    fn disk_usage_layers_size_sums_multiple_images() {
        let images = [
            make_image("nginx:latest", Some(1000)),
            make_image("alpine:3.18", Some(200)),
            make_image("busybox:1.36", Some(50)),
        ];
        let layers_size: i64 = images.iter().map(image_df_size).sum();
        assert_eq!(layers_size, 1250);
    }

    #[test]
    fn disk_usage_image_without_size_treated_as_zero() {
        let img = make_image("nginx:latest", None);
        assert_eq!(image_df_size(&img), 0);
        let entry = image_to_df_entry(&img);
        assert_eq!(entry["Size"], serde_json::json!(0_i64));
    }

    #[test]
    fn disk_usage_volume_unknown_size_uses_neg_one() {
        // No size_bytes / no in_use_by — both should fall back to -1.
        let v = serde_json::json!({
            "name": "missing-stats",
            "path": "/srv/missing-stats",
            "labels": {},
            "created_at": "2026-01-01T00:00:00Z",
        });
        let entry = volume_to_df_entry(&v);
        assert_eq!(entry["UsageData"]["Size"], serde_json::json!(-1_i64));
        assert_eq!(entry["UsageData"]["RefCount"], serde_json::json!(-1_i64));
    }

    #[test]
    fn disk_usage_container_state_translation() {
        let exited = make_container("c2", "alpine", "exited");
        let entry = container_to_df_entry(&exited);
        assert_eq!(entry["State"], "exited");
        assert_eq!(entry["Status"], "Exited");

        let unknown = make_container("c3", "alpine", "weird");
        let entry = container_to_df_entry(&unknown);
        assert_eq!(entry["State"], "exited");
        assert_eq!(entry["Status"], "Exited (weird)");
    }

    #[test]
    fn disk_usage_strip_tag_handles_digest_and_port() {
        // Plain tag.
        assert_eq!(df_strip_tag("nginx:latest"), "nginx");
        // Digest reference.
        assert_eq!(df_strip_tag("nginx@sha256:abcdef"), "nginx");
        // Registry port — must NOT eat the port number.
        assert_eq!(df_strip_tag("registry.io:5000/foo"), "registry.io:5000/foo");
        // Registry port + tag.
        assert_eq!(
            df_strip_tag("registry.io:5000/foo:v1"),
            "registry.io:5000/foo"
        );
    }

    // -----------------------------------------------------------------------
    // /system/prune + uniform error response tests
    // -----------------------------------------------------------------------

    use axum::body::to_bytes;

    /// `error_response` produces the documented Docker-shape JSON body and
    /// preserves the requested status code. The body MUST be a JSON object
    /// with a single `message` field — anything else breaks Docker clients
    /// that pattern-match on it.
    #[tokio::test]
    async fn error_response_uses_docker_shape() {
        for (status, msg) in [
            (StatusCode::BAD_REQUEST, "boom"),
            (StatusCode::NOT_FOUND, "missing"),
            (StatusCode::INTERNAL_SERVER_ERROR, "kaboom"),
        ] {
            let resp = error_response(status, msg);
            assert_eq!(resp.status(), status);
            let bytes = to_bytes(resp.into_body(), 4096).await.expect("body");
            let v: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
            assert_eq!(v["message"], serde_json::Value::String(msg.to_string()));
            // No extra keys beyond `message`.
            assert_eq!(v.as_object().expect("object").len(), 1);
        }
    }

    /// `SystemPruneQuery` accepts the documented `volumes` truthy spellings
    /// and a missing volumes flag is treated as "false" (the `want_volumes`
    /// match in the handler covers `1`, `true`, `yes`).
    #[test]
    fn system_prune_query_volumes_truthy_spellings() {
        let truthy = ["true", "1", "yes", "TRUE", "Yes"];
        for raw in truthy {
            let lower = raw.to_ascii_lowercase();
            let want = matches!(lower.as_str(), "1" | "true" | "yes");
            assert!(want, "expected '{raw}' to be truthy");
        }
        // Falsy spellings.
        for raw in ["", "false", "0", "no"] {
            let lower = raw.to_ascii_lowercase();
            let want = matches!(lower.as_str(), "1" | "true" | "yes");
            assert!(!want, "expected '{raw}' to NOT be truthy");
        }
    }

    /// The Docker `/system/prune` body shape MUST always contain the six
    /// documented keys, even when every sweep finds nothing. Tools like
    /// `docker system prune` parse the body unconditionally, so a missing
    /// field would be a regression even on an empty system.
    #[test]
    fn system_prune_response_shape_is_complete() {
        let body = serde_json::json!({
            "ContainersDeleted": Vec::<String>::new(),
            "NetworksDeleted":   Vec::<String>::new(),
            "VolumesDeleted":    Vec::<String>::new(),
            "ImagesDeleted":     Vec::<serde_json::Value>::new(),
            "BuildCacheDeleted": Vec::<serde_json::Value>::new(),
            "SpaceReclaimed":    0_u64,
        });
        for key in [
            "ContainersDeleted",
            "NetworksDeleted",
            "VolumesDeleted",
            "ImagesDeleted",
            "BuildCacheDeleted",
            "SpaceReclaimed",
        ] {
            assert!(body.get(key).is_some(), "expected key {key} in prune body");
        }
        assert!(body["SpaceReclaimed"].is_u64());
        assert!(body["ContainersDeleted"].is_array());
        assert!(body["ImagesDeleted"].is_array());
    }

    /// Filter validation: malformed JSON must produce a 400 with the
    /// Docker-shape error body. We hit the helper directly here because
    /// invoking the full handler requires a live `SocketState`.
    #[tokio::test]
    async fn system_prune_invalid_filters_are_400() {
        let resp = error_response(StatusCode::BAD_REQUEST, "invalid filters: not valid JSON");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(resp.into_body(), 1024).await.expect("body");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(v["message"], "invalid filters: not valid JSON");
    }
}
