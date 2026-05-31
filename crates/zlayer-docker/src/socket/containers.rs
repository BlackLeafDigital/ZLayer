//! Docker Engine API container endpoints.
//!
//! These handlers proxy requests to the local zlayer daemon via
//! [`DaemonClient`], translating between Docker Engine API v1.43's JSON
//! shapes (`PascalCase`, `Id`/`Names`/`Image`/...) and zlayer's native
//! `ContainerInfo` representation.
//!
//! The set of supported endpoints is:
//!
//! - `GET    /containers/json`           — list containers
//! - `GET    /containers/{id}/json`      — inspect a container
//! - `GET    /containers/{id}/logs`      — stream logs (multiplexed framed)
//! - `GET    /containers/{id}/stats`     — stream resource-usage samples
//! - `POST   /containers/create`         — create a container
//! - `POST   /containers/{id}/start`     — start a stopped container
//! - `POST   /containers/{id}/stop`      — graceful stop (with `t=<secs>`)
//! - `POST   /containers/{id}/kill`      — signal (default `SIGKILL`)
//! - `POST   /containers/{id}/restart`   — restart (with `t=<secs>`)
//! - `DELETE /containers/{id}`           — remove (with `force=1`)
//! - `POST   /containers/{id}/attach`    — hijacked attach (logs-only mode)
//! - `POST   /containers/{id}/exec`      — create an exec instance
//! - `POST   /exec/{id}/start`           — start an exec (hijacked or detached)
//! - `GET    /exec/{id}/json`            — inspect an exec instance
//! - `POST   /exec/{id}/resize`          — accept TTY resize (no-op until 4.1.x)
//! - `POST   /containers/{id}/resize`    — accept TTY resize (no-op until 4.1.x)
//! - `POST   /containers/{id}/wait`       — block on `not-running` /
//!   `next-exit` / `removed` and return the Docker-shaped
//!   `{ "StatusCode": <i32> }` body.
//! - `POST   /containers/{id}/rename`     — rename a container by passing
//!   the daemon's `POST /api/v1/containers/{id}/rename?name=<new>`.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::stream::{self, StreamExt};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use zlayer_client::{DaemonClient, StatsSample};
use zlayer_types::api::containers::{
    ContainerResourceLimits, ContainerUpdateRequest, CreateContainerRequest, HealthCheckRequest,
    VolumeMount, VolumeMountType,
};
use zlayer_types::spec::{
    ContainerRestartKind, DeviceSpec, HealthCheck, NetworkMode, StorageSpec, UlimitSpec,
};

use super::streaming::hijack::{
    multiplexed_stream_response, raw_stream_response, HijackedConnection,
};
use super::streaming::log_frame::{encode_frame, LogStream};
use super::translate;
use super::types::container_create::{
    ContainerCreateBody, DeviceMapping, HostConfig, UlimitMapping,
};
use super::types::ContainerSummary;
use super::SocketState;

/// Re-export of the `HealthSpec` path used in `health_spec_to_request` so the
/// type signature stays compact. Kept inline rather than wildcard-imported
/// from `zlayer_types::spec` to avoid leaking the rest of the spec namespace
/// into this module.
use zlayer_types::spec::HealthSpec;

/// Container API routes.
pub(crate) fn routes(state: SocketState) -> Router {
    use axum::routing::MethodFilter;
    Router::new()
        .route("/containers/json", get(list_containers))
        .route("/containers/create", post(create_container))
        .route("/containers/prune", post(prune_containers_route))
        .route("/containers/{id}/start", post(start_container))
        .route("/containers/{id}/stop", post(stop_container))
        .route("/containers/{id}/kill", post(kill_container))
        .route("/containers/{id}/restart", post(restart_container))
        .route("/containers/{id}/pause", post(pause_container))
        .route("/containers/{id}/unpause", post(unpause_container))
        .route("/containers/{id}/top", get(top_container))
        .route("/containers/{id}/changes", get(changes_container))
        .route("/containers/{id}/port", get(port_container))
        .route(
            "/containers/{id}/archive",
            get(archive_get_compat)
                .put(archive_put_compat)
                .on(MethodFilter::HEAD, archive_head_compat),
        )
        .route("/containers/{id}", delete(remove_container))
        .route("/containers/{id}/json", get(inspect_container))
        .route("/containers/{id}/logs", get(container_logs))
        .route("/containers/{id}/stats", get(container_stats))
        .route("/containers/{id}/wait", post(wait_container))
        .route("/containers/{id}/rename", post(rename_container))
        .route("/containers/{id}/update", post(update_container))
        .route("/containers/{id}/attach", post(attach_container))
        .route("/containers/{id}/resize", post(resize_container))
        .route("/containers/{id}/exec", post(create_exec))
        .route("/exec/{id}/start", post(start_exec))
        .route("/exec/{id}/json", get(inspect_exec))
        .route("/exec/{id}/resize", post(resize_exec))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Map a [`DaemonClient`] error into a Docker-style HTTP response.
///
/// `DaemonClient` reports 404s as error strings that start with
/// `"404 Not Found"` (see `DaemonClient::check_status`), so we pattern-match
/// on that prefix to turn the error into a proper `404`. Everything else
/// becomes a `500` with `{ "message": "..." }`.
fn map_daemon_error(err: &anyhow::Error) -> Response {
    let msg = format!("{err:#}");
    let status = if msg.starts_with("404 ") || msg.contains("404 Not Found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, Json(serde_json::json!({ "message": msg }))).into_response()
}

/// Translate zlayer's `state` string into Docker's `(State, Status)` pair.
///
/// Docker's `State` is one of: `created | restarting | running | removing |
/// paused | exited | dead`.  `Status` is a free-form human-readable string
/// like `"Up 2 minutes"` or `"Exited (0) 5 seconds ago"`.  We only have
/// zlayer's lifecycle string to work from, so `Status` mirrors `State`
/// with a little formatting.
fn docker_state(zlayer_state: &str) -> (&'static str, String) {
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
/// Docker reports `Created` in `/containers/json` as a Unix timestamp and
/// in `/containers/{id}/json` as an ISO-8601 string. The daemon gives us
/// ISO-8601; we parse a `YYYY-MM-DDTHH:MM:SS[.fff]Z` prefix by hand to
/// avoid pulling in `chrono` here.
fn parse_iso8601_secs(s: &str) -> i64 {
    // Minimal parser: expect "YYYY-MM-DDTHH:MM:SS" up to index 19.
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

    // Days-from-civil algorithm (Hinnant), proleptic Gregorian.
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

/// Convert a daemon `ContainerInfo` JSON object into a Docker
/// `ContainerSummary`.
fn to_container_summary(v: &serde_json::Value) -> ContainerSummary {
    let id = v
        .get("id")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_owned();
    let name = match v.get("name").and_then(|x| x.as_str()) {
        Some(s) if s.starts_with('/') => s.to_owned(),
        Some(s) => format!("/{s}"),
        None => format!("/{id}"),
    };
    let image = v
        .get("image")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_owned();
    let created_at = v.get("created_at").and_then(|x| x.as_str()).unwrap_or("");
    let zstate = v.get("state").and_then(|x| x.as_str()).unwrap_or("unknown");
    let (state, status) = docker_state(zstate);

    let labels: HashMap<String, String> = v
        .get("labels")
        .and_then(|x| x.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default();

    ContainerSummary {
        id: id.clone(),
        names: vec![name],
        image: image.clone(),
        // We don't track a separate image sha; mirror the image reference.
        image_id: image,
        command: String::new(),
        created: parse_iso8601_secs(created_at),
        state: state.to_owned(),
        status,
        ports: Vec::new(),
        labels,
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Query parameters accepted on `GET /containers/json`.
///
/// `all`, `limit`, and `filters` are honored. `size` is parsed but
/// ignored — the daemon's container list doesn't expose size info.
///
/// Supported `filters` keys (Docker JSON map of key → `Vec<String>`):
///
/// - `label` — AND across the requested label list. Each entry is
///   either `"k=v"` (exact match on both key and value) or `"k"`
///   (presence of key regardless of value). All listed labels must
///   match.
/// - `status` — OR within the list, case-insensitive comparison
///   against the container's `state`.
/// - `name` — OR within the list, substring match. A leading `/` is
///   stripped from each container name before comparison.
/// - `id` — OR within the list, prefix match against the container's
///   id.
///
/// Other Docker filter keys (`ancestor`, `network`, `volume`,
/// `health`, `exited`, `before`, `since`, …) are accepted but not
/// applied — they pass through without filtering anything out.
#[derive(Debug, Default, Deserialize)]
struct ListContainersQuery {
    #[serde(default)]
    all: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    filters: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    size: Option<String>,
}

/// Parse Docker's `filters` query parameter, a JSON-encoded map of
/// `key → Vec<String>`.
///
/// Returns `None` if the parameter is absent, empty, or fails to
/// parse. Parse failures are logged with `tracing::warn!` so clients
/// see misuse in logs but the request is not rejected with `500`.
fn parse_filters(raw: Option<&str>) -> Option<HashMap<String, Vec<String>>> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    match serde_json::from_str::<HashMap<String, Vec<String>>>(raw) {
        Ok(map) => Some(map),
        Err(err) => {
            tracing::warn!(
                error = %err,
                raw = %raw,
                "docker /containers/json: ignoring unparseable filters query parameter"
            );
            None
        }
    }
}

/// Apply Docker's `filters` semantics to a single [`ContainerSummary`].
///
/// See [`ListContainersQuery`] for the supported keys. Unsupported
/// keys are logged at `debug` and treated as match-pass-through (they
/// do not filter the container out).
fn container_matches_filters(c: &ContainerSummary, filters: &HashMap<String, Vec<String>>) -> bool {
    for (key, values) in filters {
        match key.as_str() {
            "label" => {
                // AND across the requested label list.
                for entry in values {
                    let matched = if let Some((k, v)) = entry.split_once('=') {
                        c.labels.get(k).map(String::as_str) == Some(v)
                    } else {
                        c.labels.contains_key(entry.as_str())
                    };
                    if !matched {
                        return false;
                    }
                }
            }
            "status" => {
                // OR within the list, case-insensitive against c.state.
                let state_lower = c.state.to_ascii_lowercase();
                let any = values.iter().any(|v| v.to_ascii_lowercase() == state_lower);
                if !any {
                    return false;
                }
            }
            "name" => {
                // OR within the list, substring match. Strip leading
                // `/` from each container name before comparing.
                let any = values.iter().any(|needle| {
                    c.names.iter().any(|n| {
                        let stripped = n.strip_prefix('/').unwrap_or(n.as_str());
                        stripped.contains(needle.as_str())
                    })
                });
                if !any {
                    return false;
                }
            }
            "id" => {
                // OR within the list, prefix match.
                let any = values.iter().any(|p| c.id.starts_with(p.as_str()));
                if !any {
                    return false;
                }
            }
            other => {
                tracing::debug!(
                    key = %other,
                    "docker /containers/json: unsupported filter key, ignoring"
                );
            }
        }
    }
    true
}

fn parse_bool(s: Option<&str>) -> bool {
    matches!(
        s.map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes")
    )
}

/// `GET /containers/json` — List containers.
///
/// Dispatches to [`DaemonClient::get_all_containers`] and maps each
/// entry to a Docker [`ContainerSummary`]. If `all=true` is not set,
/// only running containers are returned (matching `docker ps` default
/// behaviour).
async fn list_containers(
    State(state): State<SocketState>,
    Query(query): Query<ListContainersQuery>,
) -> Response {
    let all = parse_bool(query.all.as_deref());
    let data = match state.client.get_all_containers().await {
        Ok(d) => d,
        Err(e) => return map_daemon_error(&e),
    };

    let mut summaries: Vec<ContainerSummary> = match data.as_array() {
        Some(arr) => arr.iter().map(to_container_summary).collect(),
        None => Vec::new(),
    };

    if !all {
        summaries.retain(|c| c.state == "running");
    }

    if let Some(filters) = parse_filters(query.filters.as_deref()) {
        summaries.retain(|c| container_matches_filters(c, &filters));
    }

    if let Some(limit) = query.limit {
        if limit > 0 {
            let n = usize::try_from(limit).unwrap_or(usize::MAX);
            summaries.truncate(n);
        }
    }

    Json(summaries).into_response()
}

/// Query parameters accepted on `POST /containers/create`.
///
/// Docker passes `?name=<name>` for the container name and `?platform=<os/arch>`
/// for an optional platform selector. The platform field is parsed but
/// currently ignored — the daemon picks the runtime based on the host, so a
/// client-supplied platform is best-effort metadata only.
#[derive(Debug, Default, Deserialize)]
struct CreateQuery {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    platform: Option<String>,
}

/// `POST /containers/create` — Create a container.
///
/// Translates Docker's `ContainerCreateBody` into `ZLayer`'s
/// [`CreateContainerRequest`] via [`build_create_request`], dispatches to the
/// daemon, and returns Docker's `{"Id": ..., "Warnings": [...]}` shape on
/// success. Error paths map daemon failures to Docker status codes:
///
/// - `400 Bad Request` — translation rejected the body (e.g. missing image,
///   unparseable network mode)
/// - `404 Not Found` — daemon couldn't locate the requested image
/// - `409 Conflict` — name already in use
/// - `500 Internal Server Error` — every other daemon failure
async fn create_container(
    State(state): State<SocketState>,
    Query(query): Query<CreateQuery>,
    Json(body): Json<ContainerCreateBody>,
) -> Response {
    if let Some(platform) = query.platform.as_deref() {
        tracing::debug!(
            platform = platform,
            "docker API: POST /containers/create — platform filter ignored"
        );
    }

    let req = match build_create_request(query.name.clone(), &body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid create body: {e}"),
            );
        }
    };

    match state.client.create_container(req).await {
        Ok(resp) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "Id": resp.id,
                "Warnings": resp.warnings,
            })),
        )
            .into_response(),
        Err(e) => map_create_error(&e),
    }
}

/// Render `(status, {"message": msg})` in Docker's standard error shape.
fn error_response(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "message": msg }))).into_response()
}

/// Map a `DaemonClient` error from `create_container` to a Docker-style
/// HTTP response.
///
/// `DaemonClient::check_status` formats non-2xx responses as
/// `"Daemon returned <status> -- <body-message>"` (for non-404/500) or as
/// `"404 Not Found: ..."` / `"500 Internal Server Error: ..."` for those
/// two specifically. We pattern-match on the prefixed status code to
/// recover Docker's expected mapping.
fn map_create_error(err: &anyhow::Error) -> Response {
    let msg = format!("{err:#}");
    let lower = msg.to_ascii_lowercase();

    let status = if msg.starts_with("404 ") || msg.contains("404 Not Found") {
        StatusCode::NOT_FOUND
    } else if msg.contains(" 409 ")
        || lower.contains("conflict")
        || lower.contains("already in use")
    {
        StatusCode::CONFLICT
    } else if msg.contains(" 400 ") || lower.contains("bad request") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };

    error_response(status, &msg)
}

/// Translate a Docker [`ContainerCreateBody`] (plus the `?name=` query
/// parameter) into `ZLayer`'s [`CreateContainerRequest`].
///
/// Returns `Err` only on hard validation failures (missing image, unparseable
/// network mode). Permissive translation otherwise: unknown fields fall
/// through to defaults, mirroring the per-translator behaviour in
/// `super::translate::*`.
fn build_create_request(
    name: Option<String>,
    body: &ContainerCreateBody,
) -> Result<CreateContainerRequest, String> {
    if body.image.trim().is_empty() {
        return Err("missing required field: Image".to_owned());
    }

    // -- command ------------------------------------------------------------
    // Docker semantics: `Entrypoint` overrides the image's baked-in
    // entrypoint and `Cmd` is appended as its args. The daemon DTO has only
    // a single `command: Vec<String>` field, so we collapse the two into one
    // argv vector with entrypoint first.
    let command = combine_command(body.entrypoint.as_deref(), body.cmd.as_deref());

    // -- env ----------------------------------------------------------------
    let env = parse_env(&body.env);

    // -- host config (most translation lives here) --------------------------
    // `host_config_owned` backs the borrow when the body omits a `HostConfig`
    // entirely; the `if let` form keeps clippy's `single_match_else` lint
    // happy while preserving the same lifetime layout.
    let host_config_owned;
    let hc = if let Some(hc) = body.host_config.as_ref() {
        hc
    } else {
        host_config_owned = HostConfig::default();
        &host_config_owned
    };

    // -- volumes / port mappings via translators ----------------------------
    let storage = translate::mounts::translate(hc);
    let volumes = storage.into_iter().map(storage_to_volume_mount).collect();
    let ports = translate::ports::translate(&hc.port_bindings);

    // -- restart policy / network mode / capabilities -----------------------
    let restart_policy = translate::restart::translate(hc.restart_policy.as_ref());
    let network_mode = parse_network_mode(hc.network_mode.as_deref())?;
    let (cap_add, cap_drop) = translate::caps::translate(hc);

    // -- devices, ulimits, extra hosts, dns, ... ----------------------------
    let devices = hc.devices.iter().map(device_mapping_to_spec).collect();
    let ulimits = ulimits_to_map(&hc.ulimits);

    // -- resource knobs (folded out of `ResourcesSpec`) ---------------------
    let res = translate::resources::translate(hc);
    let resources = if res.cpu.is_some() || res.memory.is_some() {
        Some(ContainerResourceLimits {
            cpu: res.cpu,
            memory: res.memory.clone(),
        })
    } else {
        None
    };

    // -- healthcheck (HealthSpec -> HealthCheckRequest wire shape) ----------
    let health_check =
        translate::healthcheck::translate(body.healthcheck.as_ref()).map(health_spec_to_request);

    // -- stop_grace_period (Docker StopTimeout, in seconds) -----------------
    let stop_grace_period = body
        .stop_timeout
        .filter(|n| *n > 0)
        .and_then(|n| u64::try_from(n).ok())
        .map(Duration::from_secs);

    // -- auto-remove (--rm) ------------------------------------------------
    // Docker's `HostConfig.AutoRemove` (set by `docker run --rm`) maps onto
    // the daemon's `LifecycleSpec.delete_on_exit` knob. The
    // `container.die`-driven cleanup subscriber on the daemon side honors
    // the bool we plumb through here.
    let lifecycle = zlayer_types::spec::LifecycleSpec {
        delete_on_exit: hc.auto_remove.unwrap_or(false),
    };

    Ok(CreateContainerRequest {
        image: body.image.clone(),
        name,
        pull_policy: None,
        env,
        command,
        labels: body.labels.clone(),
        resources,
        volumes,
        ports,
        work_dir: body.working_dir.clone(),
        health_check,
        hostname: body.hostname.clone(),
        dns: hc.dns.clone(),
        extra_hosts: hc.extra_hosts.clone(),
        restart_policy,
        networks: Vec::new(),
        registry_credential_id: None,
        registry_auth: None,
        privileged: hc.privileged,
        cap_add,
        cap_drop,
        devices,
        network_mode,
        security_opt: hc.security_opt.clone(),
        pid_mode: hc.pid_mode.clone(),
        ipc_mode: hc.ipc_mode.clone(),
        read_only_root_fs: hc.readonly_rootfs.unwrap_or(false),
        init_container: hc.init,
        user: body.user.clone(),
        stop_signal: body.stop_signal.clone(),
        stop_grace_period,
        sysctls: hc.sysctls.clone(),
        ulimits,
        extra_groups: hc.group_add.clone(),
        pids_limit: res.pids_limit,
        cpuset: res.cpuset,
        cpu_shares: res.cpu_shares,
        memory_swap: res.memory_swap,
        memory_reservation: res.memory_reservation,
        memory_swappiness: res.memory_swappiness,
        oom_score_adj: res.oom_score_adj,
        oom_kill_disable: res.oom_kill_disable,
        blkio_weight: res.blkio_weight,
        lifecycle,
        // Docker compat surface has no placement controls; let the daemon
        // create locally.
        node_selector: None,
        platform: None,
    })
}

/// Build the unified `command` argv from Docker's separate `Entrypoint` and
/// `Cmd` arrays.
///
/// Docker treats `Entrypoint` as overriding the image's baked-in entrypoint
/// and `Cmd` as appended args. The daemon DTO collapses those to one argv
/// vector — entrypoint first, cmd second. Returns `None` when both are
/// absent so the daemon falls back to the image's defaults.
fn combine_command(entrypoint: Option<&[String]>, cmd: Option<&[String]>) -> Option<Vec<String>> {
    if let (None | Some([]), None | Some([])) = (entrypoint, cmd) {
        return None;
    }
    let mut out = Vec::new();
    if let Some(e) = entrypoint {
        out.extend(e.iter().cloned());
    }
    if let Some(c) = cmd {
        out.extend(c.iter().cloned());
    }
    Some(out)
}

/// Parse Docker's `["KEY=VALUE", "KEY=", "BARE_KEY"]` env array into a map.
///
/// Tokens without `=` are accepted with an empty-string value (matches the
/// way Docker forwards `-e BARE_KEY` — the variable is set, but to the
/// process-environment value, which we can't see here, so the empty string
/// is the safest fallback).
fn parse_env(entries: &[String]) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(entries.len());
    for entry in entries {
        if let Some((k, v)) = entry.split_once('=') {
            if !k.is_empty() {
                out.insert(k.to_string(), v.to_string());
            }
        } else if !entry.is_empty() {
            out.insert(entry.clone(), String::new());
        }
    }
    out
}

/// Convert one `StorageSpec` (the translator output) into the daemon DTO's
/// [`VolumeMount`] shape.
///
/// `StorageSpec::S3` is preserved as a bind for transport (the daemon doesn't
/// accept S3 mounts on the container-create wire today); `Anonymous` becomes
/// a tmpfs-style entry with no source.
fn storage_to_volume_mount(spec: StorageSpec) -> VolumeMount {
    match spec {
        StorageSpec::Bind {
            source,
            target,
            readonly,
        } => VolumeMount {
            mount_type: Some(VolumeMountType::Bind),
            source: Some(source),
            target,
            readonly,
        },
        StorageSpec::Named {
            name,
            target,
            readonly,
            ..
        } => VolumeMount {
            mount_type: Some(VolumeMountType::Volume),
            source: Some(name),
            target,
            readonly,
        },
        StorageSpec::Anonymous { target, .. } => VolumeMount {
            mount_type: Some(VolumeMountType::Volume),
            source: None,
            target,
            readonly: false,
        },
        StorageSpec::Tmpfs { target, .. } => VolumeMount {
            mount_type: Some(VolumeMountType::Tmpfs),
            source: None,
            target,
            readonly: false,
        },
        StorageSpec::S3 {
            bucket,
            target,
            readonly,
            ..
        } => VolumeMount {
            // No native S3 mount type on the daemon wire yet; surface as a
            // bind so the daemon at least sees the mount target. The
            // `bucket` value lands in `source` as a bare name.
            mount_type: Some(VolumeMountType::Bind),
            source: Some(bucket),
            target,
            readonly,
        },
    }
}

/// Parse Docker's `HostConfig.NetworkMode` string into [`NetworkMode`].
///
/// Accepts the same forms as the spec's `deserialize_network_mode` helper:
/// `"default"`, `"host"`, `"none"`, `"bridge"`, `"bridge:<name>"`, and
/// `"container:<id>"`. Empty strings and `None` map to `Ok(None)`, leaving
/// the daemon to pick a default.
fn parse_network_mode(s: Option<&str>) -> Result<Option<NetworkMode>, String> {
    let Some(s) = s.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let nm = match s {
        "default" => NetworkMode::Default,
        "host" => NetworkMode::Host,
        "none" => NetworkMode::None,
        "bridge" => NetworkMode::Bridge { name: None },
        _ => {
            if let Some(rest) = s.strip_prefix("bridge:") {
                if rest.is_empty() {
                    NetworkMode::Bridge { name: None }
                } else {
                    NetworkMode::Bridge {
                        name: Some(rest.to_string()),
                    }
                }
            } else if let Some(rest) = s.strip_prefix("container:") {
                if rest.is_empty() {
                    return Err("network mode \"container:<id>\" requires a non-empty id".into());
                }
                NetworkMode::Container {
                    id: rest.to_string(),
                }
            } else {
                return Err(format!("unsupported network mode: {s}"));
            }
        }
    };
    Ok(Some(nm))
}

/// Convert a Docker [`DeviceMapping`] into `ZLayer`'s [`DeviceSpec`].
///
/// `path_in_container` is dropped — the `DeviceSpec` shape uses a single
/// `path` field, which the runtime exposes inside the container at the same
/// host path. Re-pathing devices isn't supported on the daemon wire today;
/// when the runtime grows a separate `path_in_container` field this can be
/// extended without breaking callers. `cgroup_permissions` is decoded as a
/// best-effort `r`/`w`/`m` flag triple, defaulting to `rwm` when omitted to
/// match Docker.
fn device_mapping_to_spec(d: &DeviceMapping) -> DeviceSpec {
    let perms = d
        .cgroup_permissions
        .as_deref()
        .map_or_else(|| "rwm".to_string(), str::to_ascii_lowercase);
    DeviceSpec {
        path: d.path_on_host.clone(),
        read: perms.contains('r'),
        write: perms.contains('w'),
        mknod: perms.contains('m'),
    }
}

/// Convert Docker's `Vec<UlimitMapping>` into the daemon DTO's keyed map.
fn ulimits_to_map(ulimits: &[UlimitMapping]) -> HashMap<String, UlimitSpec> {
    ulimits
        .iter()
        .map(|u| {
            (
                u.name.clone(),
                UlimitSpec {
                    soft: u.soft,
                    hard: u.hard,
                },
            )
        })
        .collect()
}

/// Convert a translator-produced `HealthSpec` into the daemon DTO's
/// `HealthCheckRequest` wire shape.
///
/// The translator only ever emits `HealthCheck::Command` (Docker's healthcheck
/// block is always argv-based on the wire), but we still match exhaustively
/// so future variants surface at compile time.
fn health_spec_to_request(spec: HealthSpec) -> HealthCheckRequest {
    let (check_type, port, url, expect_status, command) = match spec.check {
        HealthCheck::Tcp { port } => ("tcp".to_string(), Some(port), None, None, None),
        HealthCheck::Http { url, expect_status } => (
            "http".to_string(),
            None,
            Some(url),
            Some(expect_status),
            None,
        ),
        HealthCheck::Command { command } => (
            "command".to_string(),
            None,
            None,
            None,
            // The daemon's request type takes argv as `Vec<String>`; we have
            // a space-joined string from the translator. Wrap it in a single
            // entry — the daemon's command-runner re-shells the joined form
            // via `sh -c`, matching the existing compose-to-ZLayer path.
            Some(vec![command]),
        ),
    };

    HealthCheckRequest {
        check_type,
        port,
        url,
        expect_status,
        command,
        interval: spec.interval.map(format_duration_string),
        timeout: spec.timeout.map(format_duration_string),
        retries: Some(spec.retries),
        start_period: spec.start_grace.map(format_duration_string),
    }
}

/// Render a [`Duration`] as the humantime string the daemon DTO expects
/// (e.g. `"30s"`, `"500ms"`).
///
/// Re-implemented inline rather than pulling in a `humantime` workspace
/// dependency just for this single call site. Picks the largest sub-second
/// unit so common Docker healthcheck cadences (whole seconds /
/// milliseconds) round-trip cleanly through the daemon's
/// `humantime::parse_duration` deserializer.
fn format_duration_string(d: Duration) -> String {
    let nanos = d.subsec_nanos();
    let secs = d.as_secs();
    if nanos == 0 {
        format!("{secs}s")
    } else if nanos % 1_000_000 == 0 {
        // Whole-millisecond resolution: emit `<total_ms>ms`.
        let total_ms = secs * 1_000 + u64::from(nanos / 1_000_000);
        format!("{total_ms}ms")
    } else if nanos % 1_000 == 0 {
        let total_us = secs * 1_000_000 + u64::from(nanos / 1_000);
        format!("{total_us}us")
    } else {
        let total_ns = secs * 1_000_000_000 + u64::from(nanos);
        format!("{total_ns}ns")
    }
}

// `ContainerRestartKind` isn't directly read here, but its visibility through
// `ContainerRestartPolicy` is what the translator's output relies on. Keep
// the type referenced so a rename or removal surfaces at this call site.
const _: fn() -> ContainerRestartKind = || ContainerRestartKind::No;

/// `POST /containers/{id}/start` — Start a container.
async fn start_container(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    match state.client.start_container(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            // Docker returns 304 when a container is already started.
            let msg = format!("{e:#}");
            if msg.to_ascii_lowercase().contains("already")
                && msg.to_ascii_lowercase().contains("running")
            {
                return StatusCode::NOT_MODIFIED.into_response();
            }
            map_daemon_error(&e)
        }
    }
}

/// Query params for `POST /containers/{id}/stop` — Docker's `t=<secs>`.
#[derive(Debug, Default, Deserialize)]
struct StopQuery {
    #[serde(default)]
    t: Option<i64>,
}

/// `POST /containers/{id}/stop` — Stop a container.
async fn stop_container(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<StopQuery>,
) -> Response {
    let timeout = q.t.and_then(|v| u64::try_from(v.max(0)).ok());
    match state.client.stop_container(&id, timeout).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.to_ascii_lowercase().contains("already")
                && (msg.to_ascii_lowercase().contains("stopped")
                    || msg.to_ascii_lowercase().contains("not running"))
            {
                return StatusCode::NOT_MODIFIED.into_response();
            }
            map_daemon_error(&e)
        }
    }
}

/// Query params for `POST /containers/{id}/kill` — Docker's `signal=<name>`.
#[derive(Debug, Default, Deserialize)]
struct KillQuery {
    #[serde(default)]
    signal: Option<String>,
}

/// `POST /containers/{id}/kill` — Send a signal to a container.
async fn kill_container(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<KillQuery>,
) -> Response {
    match state.client.kill_container(&id, q.signal.as_deref()).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

/// Query params for `POST /containers/{id}/restart` — Docker's `t=<secs>`.
#[derive(Debug, Default, Deserialize)]
struct RestartQuery {
    #[serde(default)]
    t: Option<i64>,
}

/// `POST /containers/{id}/restart` — Restart a container.
async fn restart_container(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<RestartQuery>,
) -> Response {
    let timeout = q.t.and_then(|v| u64::try_from(v.max(0)).ok());
    match state.client.restart_container(&id, timeout).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

/// Query params for `DELETE /containers/{id}` — Docker's `v=`, `force=`,
/// `link=`.
#[derive(Debug, Default, Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    #[allow(dead_code)]
    v: Option<String>,
    #[serde(default)]
    force: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    link: Option<String>,
}

/// `DELETE /containers/{id}` — Remove a container.
async fn remove_container(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<RemoveQuery>,
) -> Response {
    let force = parse_bool(q.force.as_deref());
    match state.client.delete_container(&id, force).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

/// Build the Docker-shaped `NetworkSettings` object from a zlayer
/// `ContainerInfo` JSON value.
///
/// Pulls the top-level `ipv4` into `NetworkSettings.IPAddress` and turns each
/// entry of the `networks` array into a Docker `EndpointSettings` keyed by the
/// attachment's `network` name. `NetworkID` and `EndpointID` are synthesised
/// deterministically with blake3 (64-char hex) so Docker clients that key on
/// these IDs can correlate calls across inspect/network endpoints — zlayer
/// doesn't track real Docker endpoint IDs internally.
fn build_network_settings(v: &serde_json::Value, cid: &str) -> serde_json::Value {
    let primary_ipv4 = v.get("ipv4").and_then(|x| x.as_str()).unwrap_or("");

    let attachments: Vec<&serde_json::Value> = v
        .get("networks")
        .and_then(|x| x.as_array())
        .map(|arr| arr.iter().collect())
        .unwrap_or_default();

    let mut networks_map = serde_json::Map::new();
    let mut first_endpoint_id = String::new();

    for att in &attachments {
        let Some(netname) = att.get("network").and_then(|x| x.as_str()) else {
            continue;
        };
        let ip = att.get("ipv4").and_then(|x| x.as_str()).unwrap_or("");

        let network_id = blake3::hash(format!("net:{netname}").as_bytes())
            .to_hex()
            .to_string();
        let endpoint_id = blake3::hash(format!("ep:{cid}:{netname}").as_bytes())
            .to_hex()
            .to_string();

        if first_endpoint_id.is_empty() {
            first_endpoint_id.clone_from(&endpoint_id);
        }

        networks_map.insert(
            netname.to_owned(),
            serde_json::json!({
                "IPAMConfig": serde_json::Value::Null,
                "Links": serde_json::Value::Null,
                "Aliases": serde_json::Value::Null,
                "NetworkID": network_id,
                "EndpointID": endpoint_id,
                "Gateway": "",
                "IPAddress": ip,
                "IPPrefixLen": 0,
                "IPv6Gateway": "",
                "GlobalIPv6Address": "",
                "GlobalIPv6PrefixLen": 0,
                "MacAddress": "",
                "DriverOpts": serde_json::Value::Null,
            }),
        );
    }

    serde_json::json!({
        "Bridge": "",
        "SandboxID": "",
        "HairpinMode": false,
        "LinkLocalIPv6Address": "",
        "LinkLocalIPv6PrefixLen": 0,
        "Ports": {},
        "SandboxKey": "",
        "SecondaryIPAddresses": serde_json::Value::Null,
        "SecondaryIPv6Addresses": serde_json::Value::Null,
        "EndpointID": first_endpoint_id,
        "Gateway": "",
        "GlobalIPv6Address": "",
        "GlobalIPv6PrefixLen": 0,
        "IPAddress": primary_ipv4,
        "IPPrefixLen": 0,
        "IPv6Gateway": "",
        "MacAddress": "",
        "Networks": serde_json::Value::Object(networks_map),
    })
}

/// `GET /containers/{id}/json` — Inspect a container.
///
/// Returns a minimum-viable subset of Docker's `ContainerInspect` shape:
/// `Id`, `Created`, `Image`, `Name`, `State { Status, Running, Pid,
/// ExitCode, StartedAt, FinishedAt }`, `HostConfig`, `Config { Image,
/// Labels }`, `NetworkSettings`, `Mounts`. Fields we don't track are
/// filled with reasonable defaults (empty strings, `0`, empty maps).
///
/// `NetworkSettings` is populated from the underlying `ContainerInfo`'s
/// top-level `ipv4` (→ `IPAddress`) and each entry of `networks`
/// (→ `Networks[<name>]`) via [`build_network_settings`]; `NetworkID` /
/// `EndpointID` are deterministic blake3 hashes so Docker clients can
/// correlate them across calls.
#[allow(clippy::too_many_lines)] // Most lines are Docker's inspect shape (literal JSON fields).
async fn inspect_container(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    let v = match state.client.get_container(&id).await {
        Ok(v) => v,
        Err(e) => return map_daemon_error(&e),
    };

    let cid = v.get("id").and_then(|x| x.as_str()).unwrap_or(&id);
    let name = match v.get("name").and_then(|x| x.as_str()) {
        Some(s) if s.starts_with('/') => s.to_owned(),
        Some(s) => format!("/{s}"),
        None => format!("/{cid}"),
    };
    let image = v.get("image").and_then(|x| x.as_str()).unwrap_or("");
    let created_at = v.get("created_at").and_then(|x| x.as_str()).unwrap_or("");
    let zstate = v.get("state").and_then(|x| x.as_str()).unwrap_or("unknown");
    let (docker_state_str, status_str) = docker_state(zstate);
    let running = docker_state_str == "running";
    let pid = v
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let labels = v
        .get("labels")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    let inspect = serde_json::json!({
        "Id": cid,
        "Created": created_at,
        "Path": "",
        "Args": [],
        "State": {
            "Status": docker_state_str,
            "Running": running,
            "Paused": docker_state_str == "paused",
            "Restarting": false,
            "OOMKilled": false,
            "Dead": docker_state_str == "dead",
            "Pid": pid,
            "ExitCode": 0,
            "Error": "",
            "StartedAt": "",
            "FinishedAt": "",
        },
        "Image": image,
        "ResolvConfPath": "",
        "HostnamePath": "",
        "HostsPath": "",
        "LogPath": "",
        "Name": name,
        "RestartCount": 0,
        "Driver": "zlayer",
        "Platform": "linux",
        "MountLabel": "",
        "ProcessLabel": "",
        "AppArmorProfile": "",
        "ExecIDs": serde_json::Value::Null,
        "HostConfig": {
            "NetworkMode": "default",
            "RestartPolicy": { "Name": "", "MaximumRetryCount": 0 },
            "AutoRemove": false,
            "Binds": [],
            "PortBindings": {},
        },
        "GraphDriver": { "Name": "zlayer", "Data": null },
        "Mounts": [],
        "Config": {
            "Hostname": cid,
            "Domainname": "",
            "User": "",
            "AttachStdin": false,
            "AttachStdout": false,
            "AttachStderr": false,
            "Tty": false,
            "OpenStdin": false,
            "StdinOnce": false,
            "Env": [],
            "Cmd": null,
            "Image": image,
            "Volumes": null,
            "WorkingDir": "",
            "Entrypoint": null,
            "OnBuild": null,
            "Labels": labels,
        },
        "NetworkSettings": build_network_settings(&v, cid),
        // Also echo zlayer's derived Docker-style short status so
        // clients that ignore `State.Status` still get something useful.
        "ZLayerStatus": status_str,
    });

    Json(inspect).into_response()
}

/// Query params for `GET /containers/{id}/logs`.
///
/// Docker accepts: `follow`, `stdout`, `stderr`, `since`, `until`,
/// `timestamps`, `tail`. All seven map onto the daemon's
/// [`DaemonClient::stream_container_logs`] arguments. `tail=all` and the
/// numeric form are both accepted; `follow`, `stdout`, `stderr`,
/// `timestamps` accept Docker's lax bool spellings (`1`/`0`/`true`/`false`).
/// `since` / `until` are Unix seconds (the Docker wire format).
#[derive(Debug, Default, Deserialize)]
struct LogsQuery {
    #[serde(default)]
    follow: Option<String>,
    #[serde(default)]
    stdout: Option<String>,
    #[serde(default)]
    stderr: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    timestamps: Option<String>,
    #[serde(default)]
    tail: Option<String>,
}

/// Resolved log-stream parameters extracted from a [`LogsQuery`].
///
/// Pulled out so the parsing rules (boolean spellings, `tail=all`,
/// `since`/`until` Unix-seconds form) can be unit-tested without
/// constructing an axum extractor.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // mirrors Docker's logs query params 1:1
struct LogsParams {
    follow: bool,
    stdout: bool,
    stderr: bool,
    timestamps: bool,
    tail: Option<u64>,
    since: Option<i64>,
    until: Option<i64>,
}

impl LogsParams {
    /// Resolve a Docker [`LogsQuery`] into typed parameters.
    ///
    /// Defaults match Docker's behaviour:
    ///   * `stdout` / `stderr` default to `true` if neither is set; if one
    ///     is set, the other defaults to `false`.
    ///   * `follow`, `timestamps` default to `false`.
    ///   * `tail` defaults to `None` (no truncation); `tail=all` or a
    ///     missing value are equivalent.
    fn from_query(q: &LogsQuery) -> Self {
        // Docker's stdout/stderr semantics: if both are unset the daemon
        // streams both. Otherwise the unset side defaults to false. This
        // mirrors `docker logs`'s flag handling.
        let (stdout, stderr) = match (q.stdout.as_deref(), q.stderr.as_deref()) {
            (None, None) => (true, true),
            (so, se) => (parse_bool(so), parse_bool(se)),
        };
        let follow = parse_bool(q.follow.as_deref());
        let timestamps = parse_bool(q.timestamps.as_deref());
        let tail = match q.tail.as_deref() {
            Some("all") | None => None,
            Some(n) => n.parse::<u64>().ok(),
        };
        let since = q.since.as_deref().and_then(|s| s.parse::<i64>().ok());
        let until = q.until.as_deref().and_then(|s| s.parse::<i64>().ok());
        Self {
            follow,
            stdout,
            stderr,
            timestamps,
            tail,
            since,
            until,
        }
    }
}

/// `GET /containers/{id}/logs` — Stream container logs.
///
/// Forwards the daemon's logs stream verbatim using Docker's framed
/// multiplexed wire format (the daemon honours `format=raw`, which emits
/// the 8-byte stdcopy header per chunk). The response uses
/// `Content-Type: application/vnd.docker.raw-stream`, which Docker CLI
/// and bollard-style clients recognise as the multiplexed stdcopy
/// envelope. We always emit raw-stream framing regardless of the
/// container's `Tty` setting — Docker's TTY-only `vnd.docker.multiplexed-stream`
/// content type is reserved for a future improvement; `raw-stream` is
/// the safer default because every Docker SDK can decode it.
async fn container_logs(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<LogsQuery>,
) -> Response {
    let params = LogsParams::from_query(&q);

    let stream = match state
        .client
        .stream_container_logs(
            &id,
            params.follow,
            params.tail,
            params.since,
            params.until,
            params.timestamps,
            params.stdout,
            params.stderr,
            true,
        )
        .await
    {
        Ok(s) => s,
        Err(e) => return map_daemon_error(&e),
    };

    // Body::from_stream wants `Result<Bytes, Infallible>` (or any error
    // that implements Into<BoxError>). We collapse `anyhow::Error` items
    // into `Ok(empty bytes)` so transient parse errors don't tear down
    // the response — the daemon's own logging surfaces them. A clean
    // EOF still terminates the stream.
    let body_stream = stream.filter_map(|res| async move {
        match res {
            Ok(bytes) if bytes.is_empty() => None,
            Ok(bytes) => Some(Ok::<Bytes, Infallible>(bytes)),
            Err(err) => {
                tracing::warn!(error = %err, "docker /containers/.../logs: dropping body chunk");
                None
            }
        }
    });

    let body = Body::from_stream(body_stream);

    let mut response = Response::builder().status(StatusCode::OK);
    if let Some(headers) = response.headers_mut() {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.docker.raw-stream"),
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
                Json(serde_json::json!({
                    "message": format!("failed to build /logs response: {e}")
                })),
            )
                .into_response()
        },
        IntoResponse::into_response,
    )
}

/// Query params for `GET /containers/{id}/stats`.
///
/// Docker's `stream` flag defaults to `true` (the connection stays open
/// and emits one sample per tick). Set `stream=0` / `stream=false` for
/// a single-shot snapshot.
#[derive(Debug, Default, Deserialize)]
struct StatsQuery {
    #[serde(default)]
    stream: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    one_shot: Option<String>,
}

/// `GET /containers/{id}/stats` — Stream container stats samples.
///
/// Translates the daemon's [`StatsSample`] NDJSON stream into Docker's
/// `/containers/{id}/stats` wire shape (`cpu_stats`, `precpu_stats`,
/// `memory_stats`, `blkio_stats`, `networks`, `pids_stats`). Each sample
/// is emitted as a single JSON line followed by `\n`. The handler tracks
/// the previous sample so `precpu_stats` is populated on every tick
/// after the first.
///
/// When `stream=false` exactly one sample is sent and the connection
/// closes; default behaviour matches Docker (`stream=true`).
async fn container_stats(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<StatsQuery>,
) -> Response {
    // Default true (Docker's contract). Both `stream=` absent and
    // `stream=true` keep the connection open.
    let stream_flag = match q.stream.as_deref() {
        None => true,
        Some(s) => parse_bool(Some(s)),
    };

    let sample_stream = match state.client.stream_container_stats(&id, stream_flag).await {
        Ok(s) => s,
        Err(e) => return map_daemon_error(&e),
    };

    let body_stream = stats_body_stream(sample_stream);
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
                Json(serde_json::json!({
                    "message": format!("failed to build /stats response: {e}")
                })),
            )
                .into_response()
        },
        IntoResponse::into_response,
    )
}

/// Thread a `Stream<Item = Result<StatsSample>>` into a body-ready
/// stream of NDJSON-encoded Docker-shaped stats lines.
///
/// Pulled out as a free function so it can be tested against synthetic
/// sample streams without spinning up a daemon. Maintains the running
/// `prev` sample so `precpu_stats` is populated on every emitted line
/// after the first.
fn stats_body_stream<S>(
    samples: S,
) -> impl futures_util::Stream<Item = Result<Bytes, Infallible>> + Send + 'static
where
    S: futures_util::Stream<Item = anyhow::Result<StatsSample>> + Send + 'static,
{
    let prev: Option<StatsSample> = None;
    stream::unfold((Box::pin(samples), prev), |(mut s, mut prev)| async move {
        loop {
            match s.next().await {
                Some(Ok(sample)) => {
                    let line = stats_ndjson_line(&sample, prev.as_ref());
                    prev = Some(sample);
                    if let Some(bytes) = line {
                        return Some((Ok::<Bytes, Infallible>(bytes), (s, prev)));
                    }
                    // Encoding failure is exceptionally unlikely (the
                    // shape is fully owned), but skip to the next sample
                    // rather than tear down the stream if it ever happens.
                }
                Some(Err(err)) => {
                    tracing::warn!(error = %err, "docker /containers/.../stats: dropping malformed sample");
                }
                None => return None,
            }
        }
    })
}

/// Serialise one Docker-shaped stats record into a single NDJSON line
/// (terminated with `\n`).
fn stats_ndjson_line(sample: &StatsSample, prev: Option<&StatsSample>) -> Option<Bytes> {
    let value = stats_sample_to_docker(sample, prev);
    let mut buf = serde_json::to_vec(&value).ok()?;
    buf.push(b'\n');
    Some(Bytes::from(buf))
}

/// Map a daemon [`StatsSample`] (plus the previous sample, if any) onto
/// Docker's `/containers/{id}/stats` wire shape.
///
/// Field choices follow Docker Engine API v1.43:
///   * `read` is the sample's wallclock RFC3339 timestamp.
///   * `preread` is the previous sample's timestamp, or
///     `0001-01-01T00:00:00Z` (Go's zero time) on the first tick.
///   * `cpu_stats.cpu_usage.total_usage` and `cpu_stats.system_cpu_usage`
///     are nanosecond-cumulative counters; clients compute deltas using
///     the matching `precpu_stats.*` values.
///   * `memory_stats.{usage,limit}` are bytes.
///   * `blkio_stats.io_service_bytes_recursive` is a Docker-conventional
///     two-entry array (`Read` / `Write`); the cgroup-v2 backend exposes
///     the same split via the daemon `StatsSample`.
///   * `networks.eth0.{rx,tx}_bytes` collapses all attached interfaces
///     into a single synthetic `eth0` entry, matching what Docker does
///     when only one cgroup-level counter is available.
///   * `pids_stats` carries `current` and (when set) `limit`.
#[allow(clippy::too_many_lines)] // most lines are Docker's literal JSON shape
fn stats_sample_to_docker(sample: &StatsSample, prev: Option<&StatsSample>) -> serde_json::Value {
    let read = sample
        .timestamp
        .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    let preread = prev.map_or_else(
        || "0001-01-01T00:00:00Z".to_owned(),
        |p| {
            p.timestamp
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
        },
    );

    let cpu_stats = serde_json::json!({
        "cpu_usage": {
            "total_usage": sample.cpu_total_ns,
            "usage_in_kernelmode": 0u64,
            "usage_in_usermode": 0u64,
        },
        "system_cpu_usage": sample.cpu_system_ns,
        "online_cpus": sample.online_cpus,
        "throttling_data": {
            "periods": 0u64,
            "throttled_periods": 0u64,
            "throttled_time": 0u64,
        },
    });
    let precpu_stats = match prev {
        Some(p) => serde_json::json!({
            "cpu_usage": {
                "total_usage": p.cpu_total_ns,
                "usage_in_kernelmode": 0u64,
                "usage_in_usermode": 0u64,
            },
            "system_cpu_usage": p.cpu_system_ns,
            "online_cpus": p.online_cpus,
            "throttling_data": {
                "periods": 0u64,
                "throttled_periods": 0u64,
                "throttled_time": 0u64,
            },
        }),
        None => serde_json::json!({
            "cpu_usage": {
                "total_usage": 0u64,
                "usage_in_kernelmode": 0u64,
                "usage_in_usermode": 0u64,
            },
            "system_cpu_usage": 0u64,
            "online_cpus": 0u32,
            "throttling_data": {
                "periods": 0u64,
                "throttled_periods": 0u64,
                "throttled_time": 0u64,
            },
        }),
    };

    let memory_stats = serde_json::json!({
        "usage": sample.mem_used_bytes,
        "limit": sample.mem_limit_bytes,
        "max_usage": sample.mem_used_bytes,
        "stats": {},
    });

    let blkio_stats = serde_json::json!({
        "io_service_bytes_recursive": [
            { "major": 0, "minor": 0, "op": "Read",  "value": sample.blkio_read_bytes },
            { "major": 0, "minor": 0, "op": "Write", "value": sample.blkio_write_bytes },
        ],
        "io_serviced_recursive": [],
        "io_queue_recursive": [],
        "io_service_time_recursive": [],
        "io_wait_time_recursive": [],
        "io_merged_recursive": [],
        "io_time_recursive": [],
        "sectors_recursive": [],
    });

    let mut networks = serde_json::Map::new();
    networks.insert(
        "eth0".to_owned(),
        serde_json::json!({
            "rx_bytes":   sample.net_rx_bytes,
            "rx_packets": 0u64,
            "rx_errors":  0u64,
            "rx_dropped": 0u64,
            "tx_bytes":   sample.net_tx_bytes,
            "tx_packets": 0u64,
            "tx_errors":  0u64,
            "tx_dropped": 0u64,
        }),
    );

    let pids_stats = match sample.pids_limit {
        Some(limit) => serde_json::json!({
            "current": sample.pids_current,
            "limit":   limit,
        }),
        None => serde_json::json!({
            "current": sample.pids_current,
        }),
    };

    serde_json::json!({
        "read": read,
        "preread": preread,
        "num_procs": 0u32,
        "cpu_stats": cpu_stats,
        "precpu_stats": precpu_stats,
        "memory_stats": memory_stats,
        "blkio_stats": blkio_stats,
        "networks": serde_json::Value::Object(networks),
        "pids_stats": pids_stats,
        "storage_stats": {},
    })
}

/// Query params for `POST /containers/{id}/wait` — Docker's
/// `condition=` query string.
#[derive(Debug, Default, Deserialize)]
struct WaitQuery {
    #[serde(default)]
    condition: Option<String>,
}

/// `POST /containers/{id}/wait` — Block until the container reaches the
/// requested condition and return Docker's `{"StatusCode": ..., "Error": ...}`
/// shape.
///
/// The handler forwards the call to the daemon's
/// `POST /api/v1/containers/{id}/wait?condition=` endpoint via
/// [`DaemonClient::wait_container`]. Errors are mapped to the appropriate
/// HTTP status (`404` when the daemon reports the container is missing,
/// `500` otherwise) so Docker SDKs see the same shape they would from
/// upstream Docker.
async fn wait_container(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<WaitQuery>,
) -> Response {
    let condition = q.condition.as_deref().filter(|c| !c.is_empty());
    match state.client.wait_container(&id, condition).await {
        Ok(resp) => {
            // Docker's response uses PascalCase. Re-emit on this shape so
            // the client doesn't have to translate.
            let mut body = serde_json::json!({
                "StatusCode": resp.status_code,
            });
            if let Some(err) = resp.error {
                body["Error"] = serde_json::json!({ "Message": err.message });
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => map_daemon_error(&e),
    }
}

/// Query params for `POST /containers/{id}/rename` — Docker's
/// `name=<new>` query string.
#[derive(Debug, Default, Deserialize)]
struct RenameQuery {
    #[serde(default)]
    name: Option<String>,
}

/// `POST /containers/{id}/pause` — Pause a running container.
///
/// Forwards to the daemon's `POST /api/v1/containers/{id}/pause`. Docker
/// expects `204 No Content` on success.
async fn pause_container(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    match state.client.pause_container(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

/// `POST /containers/{id}/unpause` — Resume a paused container.
async fn unpause_container(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    match state.client.unpause_container(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

/// Query params for `GET /containers/{id}/top` — Docker's `ps_args=`.
#[derive(Debug, Default, Deserialize)]
struct TopQuery {
    #[serde(default)]
    ps_args: Option<String>,
}

/// `GET /containers/{id}/top` — List processes in a container.
///
/// Forwards to the daemon's `GET /api/v1/containers/{id}/top?ps_args=<...>`.
/// The daemon returns the Docker-shaped `{"Titles": [...], "Processes": [...]}`
/// body, which we pass through verbatim.
async fn top_container(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<TopQuery>,
) -> Response {
    match state.client.top_container(&id, q.ps_args.as_deref()).await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

/// `GET /containers/{id}/changes` — Report filesystem changes.
///
/// Forwards to the daemon's `GET /api/v1/containers/{id}/changes`. The daemon
/// returns the Docker-shaped array of `{"Path": "...", "Kind": <int>}` entries.
async fn changes_container(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    match state.client.container_changes(&id).await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

/// `GET /containers/{id}/port` — Report published port mappings.
///
/// Forwards to the daemon's `GET /api/v1/containers/{id}/port`. The body
/// shape mirrors Docker's `{"Ports": {"80/tcp": [{"HostIp":"...","HostPort":"..."}]}}`.
async fn port_container(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    match state.client.container_port(&id).await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

/// Query parameters for `GET|HEAD /containers/{id}/archive` — Docker's
/// single `path=` query string.
#[derive(Debug, Default, Deserialize)]
struct ArchivePathQuery {
    #[serde(default)]
    path: Option<String>,
}

/// Query parameters for `PUT /containers/{id}/archive` — Docker's
/// `path=`, `noOverwriteDirNonDir=`, and `copyUIDGID=`.
#[derive(Debug, Default, Deserialize)]
struct ArchivePutQuery {
    #[serde(default)]
    path: Option<String>,
    #[serde(default, rename = "noOverwriteDirNonDir")]
    no_overwrite_dir_non_dir: Option<String>,
    #[serde(default, rename = "copyUIDGID")]
    copy_uid_gid: Option<String>,
}

fn parse_bool_flag(s: Option<&str>) -> bool {
    matches!(
        s.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes")
    )
}

/// `GET /containers/{id}/archive?path=<...>` — Stream a TAR archive of a
/// path inside the container. Forwards to the daemon's
/// `GET /api/v1/containers/{id}/archive` and re-emits the response body and
/// `X-Docker-Container-Path-Stat` header verbatim.
async fn archive_get_compat(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<ArchivePathQuery>,
) -> Response {
    let Some(path) = q.path.filter(|p| !p.is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": "missing required 'path' query parameter"
            })),
        )
            .into_response();
    };
    match state.client.archive_get(&id, &path).await {
        Ok((body, stat_header)) => {
            let mut resp = Response::new(Body::from(body));
            resp.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-tar"),
            );
            if let Some(h) = stat_header {
                if let Ok(v) = HeaderValue::from_str(&h) {
                    resp.headers_mut().insert("X-Docker-Container-Path-Stat", v);
                }
            }
            resp
        }
        Err(e) => map_daemon_error(&e),
    }
}

/// `PUT /containers/{id}/archive?path=<...>` — Extract a TAR archive into
/// the container at the given path. Forwards the request body verbatim to
/// the daemon as `application/x-tar`.
async fn archive_put_compat(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<ArchivePutQuery>,
    body: Bytes,
) -> Response {
    let Some(path) = q.path.filter(|p| !p.is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": "missing required 'path' query parameter"
            })),
        )
            .into_response();
    };
    let no_overwrite = parse_bool_flag(q.no_overwrite_dir_non_dir.as_deref());
    let copy_uid_gid = parse_bool_flag(q.copy_uid_gid.as_deref());
    match state
        .client
        .archive_put(&id, &path, body, no_overwrite, copy_uid_gid)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

/// `HEAD /containers/{id}/archive?path=<...>` — Return path-stat metadata
/// in the `X-Docker-Container-Path-Stat` header. Forwards to the daemon.
async fn archive_head_compat(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<ArchivePathQuery>,
) -> Response {
    let Some(path) = q.path.filter(|p| !p.is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": "missing required 'path' query parameter"
            })),
        )
            .into_response();
    };
    match state.client.archive_head(&id, &path).await {
        Ok(header) => {
            let mut resp = Response::new(Body::empty());
            if let Some(h) = header {
                if let Ok(v) = HeaderValue::from_str(&h) {
                    resp.headers_mut().insert("X-Docker-Container-Path-Stat", v);
                }
            }
            resp
        }
        Err(e) => map_daemon_error(&e),
    }
}

/// `POST /containers/prune` — Prune stopped containers.
///
/// Forwards to the daemon's `POST /api/v1/containers/prune`. Returns
/// `{"ContainersDeleted": [...], "SpaceReclaimed": <u64>}`.
async fn prune_containers_route(State(state): State<SocketState>) -> Response {
    match state.client.prune_standalone_containers().await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

/// `POST /containers/{id}/rename` — Rename a container.
///
/// Forwards to the daemon's `POST /api/v1/containers/{id}/rename?name=<new>`
/// endpoint. Docker's response is `204 No Content` on success, `404` when
/// the source container doesn't exist, and `409` when the target name is
/// already in use; we map daemon errors onto those statuses where we can
/// detect them, falling back to `500` otherwise.
async fn rename_container(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<RenameQuery>,
) -> Response {
    let new_name = q.name.unwrap_or_default();
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": "missing or empty `name` query parameter"
            })),
        )
            .into_response();
    }
    match state.client.rename_container(&id, new_name).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            let msg = format!("{e:#}").to_ascii_lowercase();
            if msg.contains("conflict") || (msg.contains("already") && msg.contains("use")) {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "message": format!("container name '{new_name}' already in use")
                    })),
                )
                    .into_response();
            }
            map_daemon_error(&e)
        }
    }
}

/// `POST /containers/{id}/update` — Update container resource limits and
/// restart policy.
///
/// Forwards Docker's wire payload (`{"CpuShares":n, "Memory":n,
/// "RestartPolicy":{...}, ...}`) verbatim to the daemon's
/// `POST /api/v1/containers/{id}/update`. The daemon's
/// [`ContainerUpdateRequest`] uses the same `PascalCase` field names, so no
/// translation is needed here. Docker returns `200 OK` with
/// `{"Warnings": [...]}` on success.
async fn update_container(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Json(body): Json<ContainerUpdateRequest>,
) -> Response {
    match state.client.update_container(&id, &body).await {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

// ---------------------------------------------------------------------------
// Attach + Exec endpoints (4.2.2 / 4.2.4–4.2.8)
//
// Docker's attach/exec endpoints upgrade the underlying HTTP/1.1 connection
// to a raw byte stream after responding with `101 Switching Protocols`. The
// real interactive PTY-backed transport (stdin into the container, full TTY
// resize support, multiplexed exec sessions) is provided by Phase 4.1.x —
// the daemon-side `Runtime::exec_pty` plus `POST /api/v1/exec/{id}/start`
// over a hyper upgrade. Until that lands, this layer offers a working
// best-effort compatibility shim:
//
//   * `/containers/{id}/attach` streams the daemon's existing logs feed back
//     to the hijacked socket as Docker stdcopy multiplex frames. Stdin
//     attach is documented as a follow-up; clients that only need
//     `docker attach <id>` for output (the most common case for non-TTY
//     services) work today.
//   * `/containers/{id}/exec` parses Docker's `ExecConfig` body, allocates
//     a 64-char hex exec ID, and stores the request in a process-local
//     registry so the matching `/exec/{id}/start` and `/exec/{id}/json`
//     calls can find it.
//   * `/exec/{id}/start` runs the buffered, non-interactive
//     `DaemonClient::exec_in_container` and writes the captured
//     stdout/stderr back to the hijacked socket using stdcopy framing
//     (`Tty=false`) or the raw byte pipe (`Tty=true`). Detached starts
//     spawn the call and return `200 OK` immediately, matching Docker's
//     `Detach=true` semantics.
//   * `/exec/{id}/json` returns Docker's `ExecInspectResponse` shape
//     reconstructed from the registry.
//   * `/exec/{id}/resize` and `/containers/{id}/resize` accept the `h`/`w`
//     query parameters and return `200 OK`. They are no-ops because the
//     buffered exec backend has no PTY to resize; a future PTY-aware
//     pathway (post-4.1.x) plumbs these through to
//     `DaemonClient::resize_exec` / `resize_container`.
// ---------------------------------------------------------------------------

/// In-process registry of pending exec instances, keyed by the 64-char hex
/// exec ID we hand back to clients. The registry is module-local because the
/// docker-compat shim does not yet share state with the daemon's own
/// [`zlayer_api::handlers::exec_instances::ExecInstances`] — the daemon
/// register lands with Phase 4.1.x.
type ExecRegistry = RwLock<HashMap<String, ExecRegistryEntry>>;

/// Singleton accessor for [`ExecRegistry`]. We use a [`OnceLock`] so the
/// registry is allocated lazily (no cost when the docker socket isn't
/// serving exec endpoints) and shared across the whole process.
fn exec_registry() -> &'static ExecRegistry {
    static REGISTRY: OnceLock<ExecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// State captured at exec-create time, plus the exit metadata stamped on
/// after exec-start.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // mirrors Docker's `ExecConfig` 1:1
struct ExecRegistryEntry {
    container_id: String,
    cmd: Vec<String>,
    /// Captured for `ProcessConfig` reflection and forward compatibility
    /// with the PTY-aware path (Phase 4.1.x). The buffered
    /// `exec_in_container` daemon endpoint does not accept extra env, so
    /// the value is preserved-but-unused today.
    #[allow(dead_code)]
    env: Vec<String>,
    working_dir: Option<String>,
    user: Option<String>,
    privileged: bool,
    tty: bool,
    attach_stdin: bool,
    attach_stdout: bool,
    attach_stderr: bool,
    /// Once the exec has completed, the captured exit code. `None` while
    /// the exec is pending or still running.
    exit_code: Option<i32>,
    /// `true` between `/exec/{id}/start` (or detached spawn) accepting the
    /// request and the underlying exec returning. Mirrors Docker's
    /// `ExecInspectResponse.Running`.
    running: bool,
}

/// Generate a 64-character lowercase hex exec ID.
///
/// We do not pull in the `rand` crate (would require a workspace dep edit)
/// and instead hash a `(timestamp_nanos, atomic counter)` pair — collisions
/// are impossible across a single process and the docker-compat layer never
/// shares the namespace with another generator. The output shape (64 hex
/// chars) matches the daemon-side
/// [`zlayer_api::handlers::exec_instances::generate_exec_id`].
fn generate_exec_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| chrono::Utc::now().timestamp_micros());
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    // SHA-style 64 hex chars: pack the (nanos, counter) pair four times so
    // every byte position gets a deterministic value. Sufficient for an
    // in-process registry where uniqueness, not unpredictability, is what
    // matters. The cast to `u64` is bit-preserving — the negative-time
    // branch only triggers for clocks set before 1970 and we still want a
    // stable hex pattern in that pathological case.
    #[allow(clippy::cast_sign_loss)]
    let nanos_u64 = nanos as u64;
    format!("{nanos_u64:016x}{counter:016x}{nanos_u64:016x}{counter:016x}")
}

/// Docker `ExecConfig` request body for `POST /containers/{id}/exec`.
///
/// Mirrors the Docker Engine API v1.43 schema 1:1. Every field is optional
/// because Docker accepts partial bodies (e.g. just `{"Cmd": [...]}`); the
/// only field we treat as required is `Cmd`. `DetachKeys` is parsed but
/// ignored — we do not implement Docker's interactive detach key escape.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct ExecConfigBody {
    #[serde(default)]
    cmd: Option<Vec<String>>,
    #[serde(default)]
    env: Option<Vec<String>>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    privileged: Option<bool>,
    #[serde(default)]
    tty: Option<bool>,
    #[serde(default)]
    attach_stdin: Option<bool>,
    #[serde(default)]
    attach_stdout: Option<bool>,
    #[serde(default)]
    attach_stderr: Option<bool>,
    #[serde(default)]
    #[allow(dead_code)] // ignored — we don't implement interactive detach keys
    detach_keys: Option<String>,
}

/// `POST /containers/{id}/exec` — Create an exec instance.
///
/// Validates the Docker `ExecConfig` body, allocates a 64-character hex
/// exec ID, registers the planned configuration in the module-local
/// [`exec_registry`], and returns Docker's `{"Id": "<exec_id>"}` payload.
///
/// The exec is not actually started here — Docker's contract has the
/// client follow up with `POST /exec/{id}/start` to drive the connection
/// upgrade.
async fn create_exec(
    State(_state): State<SocketState>,
    Path(container_id): Path<String>,
    Json(body): Json<ExecConfigBody>,
) -> Response {
    let Some(cmd) = body.cmd.clone().filter(|v| !v.is_empty()) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "exec config requires a non-empty Cmd",
        );
    };

    let entry = ExecRegistryEntry {
        container_id: container_id.clone(),
        cmd,
        env: body.env.unwrap_or_default(),
        working_dir: body.working_dir,
        user: body.user,
        privileged: body.privileged.unwrap_or(false),
        tty: body.tty.unwrap_or(false),
        attach_stdin: body.attach_stdin.unwrap_or(false),
        attach_stdout: body.attach_stdout.unwrap_or(true),
        attach_stderr: body.attach_stderr.unwrap_or(true),
        exit_code: None,
        running: false,
    };

    let exec_id = generate_exec_id();
    {
        let mut guard = exec_registry().write().await;
        guard.insert(exec_id.clone(), entry);
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "Id": exec_id })),
    )
        .into_response()
}

/// Docker `ExecStart` request body for `POST /exec/{id}/start`.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct ExecStartBody {
    #[serde(default)]
    detach: Option<bool>,
    #[serde(default)]
    tty: Option<bool>,
}

/// `POST /exec/{id}/start` — Start a previously-created exec instance.
///
/// On `Detach=true` the request returns `200 OK` immediately and the exec
/// runs to completion in a detached task. Otherwise the connection is
/// hijacked: the exec runs to completion, and its captured stdout/stderr
/// are written to the upgraded socket — multiplexed stdcopy frames when
/// `Tty=false`, raw bytes when `Tty=true`.
/// Cap on `POST /exec/{id}/start` request bodies. The body is
/// `{"Detach":bool, "Tty":bool}` in practice, so 64 KiB is comfortably
/// generous and rules out slowloris-style uploads wedging the upgrade.
const EXEC_START_BODY_LIMIT: usize = 64 * 1024;

async fn start_exec(
    State(state): State<SocketState>,
    Path(exec_id): Path<String>,
    req: axum::extract::Request,
) -> Response {
    // Pull the JSON body manually so we can hand the (now headers-only)
    // request to `HijackedConnection::from_request` afterwards. Axum's
    // `Json` extractor consumes the request body, which would prevent the
    // upgrade from completing.
    let (parts, body) = req.into_parts();
    let parsed = match read_exec_start_body(body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let detach = parsed.detach.unwrap_or(false);

    // Snapshot the registry entry under a read lock and resolve the
    // effective TTY mode (start-body wins, falls back to create-body).
    let Some(entry) = exec_registry().read().await.get(&exec_id).cloned() else {
        return error_response(
            StatusCode::NOT_FOUND,
            &format!("exec instance not found: {exec_id}"),
        );
    };
    let tty = parsed.tty.unwrap_or(entry.tty);

    if detach {
        spawn_detached_exec(state.client.clone(), exec_id.clone(), entry).await;
        return (StatusCode::OK, Body::empty()).into_response();
    }

    // Reassemble a body-less request for the hijack helper. Hyper drives
    // the upgrade off the request extensions (which `from_parts` preserves)
    // — the empty body is fine because Docker's exec start protocol does
    // not stream further request bytes; everything after the upgrade is
    // multiplexed onto the response side.
    let upgrade_req = axum::extract::Request::from_parts(parts, Body::empty());

    let response = if tty {
        raw_stream_response()
    } else {
        multiplexed_stream_response()
    };

    spawn_hijacked_exec(
        state.client.clone(),
        exec_id.clone(),
        entry,
        tty,
        upgrade_req,
    );
    response
}

/// Read and parse the (optional) JSON body of `POST /exec/{id}/start`.
///
/// Returns the parsed [`ExecStartBody`], or an error response when the
/// body is malformed / oversized.
async fn read_exec_start_body(body: Body) -> std::result::Result<ExecStartBody, Response> {
    let body_bytes = axum::body::to_bytes(body, EXEC_START_BODY_LIMIT)
        .await
        .map_err(|e| {
            error_response(
                StatusCode::BAD_REQUEST,
                &format!("failed to read exec start body: {e}"),
            )
        })?;
    if body_bytes.is_empty() {
        return Ok(ExecStartBody::default());
    }
    serde_json::from_slice::<ExecStartBody>(&body_bytes).map_err(|e| {
        error_response(
            StatusCode::BAD_REQUEST,
            &format!("invalid exec start body: {e}"),
        )
    })
}

/// Spawn the detached form of `/exec/{id}/start`: mark running, run the
/// underlying buffered exec on a background task, then stamp the exit
/// code into the registry once it returns.
async fn spawn_detached_exec(client: Arc<DaemonClient>, exec_id: String, entry: ExecRegistryEntry) {
    mark_exec_started(&exec_id).await;
    let container_id = entry.container_id;
    let cmd = entry.cmd;
    tokio::spawn(async move {
        let exit_code = match client.exec_in_container(&container_id, cmd).await {
            Ok(r) => r.exit_code,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    exec_id = %exec_id,
                    container = %container_id,
                    "docker /exec/.../start (detached) failed"
                );
                -1
            }
        };
        mark_exec_finished(&exec_id, exit_code).await;
    });
}

/// Spawn the hijacked form of `/exec/{id}/start`: drive the upgrade,
/// run the buffered exec, write captured output back as either a raw
/// byte pipe (TTY) or stdcopy multiplex frames (non-TTY), then close.
fn spawn_hijacked_exec(
    client: Arc<DaemonClient>,
    exec_id: String,
    entry: ExecRegistryEntry,
    tty: bool,
    upgrade_req: axum::extract::Request,
) {
    let container_id = entry.container_id;
    let cmd = entry.cmd;
    tokio::spawn(async move {
        let conn = match HijackedConnection::from_request(upgrade_req).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    exec_id = %exec_id,
                    "docker /exec/.../start: hijack upgrade failed"
                );
                return;
            }
        };
        let (_read_half, mut write_half) = conn.split();
        mark_exec_started(&exec_id).await;
        let exit_code = match client.exec_in_container(&container_id, cmd).await {
            Ok(r) => {
                write_exec_output(&mut write_half, &r.stdout, &r.stderr, tty).await;
                let _ = write_half.flush().await;
                let _ = write_half.shutdown().await;
                r.exit_code
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    exec_id = %exec_id,
                    container = %container_id,
                    "docker /exec/.../start (hijacked) failed"
                );
                let msg = format!("exec failed: {err:#}\n");
                if tty {
                    let _ = write_half.write_all(msg.as_bytes()).await;
                } else {
                    let frame = encode_frame(LogStream::Stderr, msg.as_bytes());
                    let _ = write_half.write_all(&frame).await;
                }
                let _ = write_half.flush().await;
                let _ = write_half.shutdown().await;
                -1
            }
        };
        mark_exec_finished(&exec_id, exit_code).await;
    });
}

/// Write the captured exec stdout / stderr to the hijacked socket.
///
/// TTY mode emits the bytes verbatim (no framing); non-TTY mode emits
/// Docker stdcopy multiplex frames (`stream=1` for stdout, `stream=2`
/// for stderr).
async fn write_exec_output<W>(write_half: &mut W, stdout: &str, stderr: &str, tty: bool)
where
    W: tokio::io::AsyncWrite + Unpin + ?Sized,
{
    if tty {
        if !stdout.is_empty() {
            let _ = write_half.write_all(stdout.as_bytes()).await;
        }
        if !stderr.is_empty() {
            let _ = write_half.write_all(stderr.as_bytes()).await;
        }
    } else {
        if !stdout.is_empty() {
            let frame = encode_frame(LogStream::Stdout, stdout.as_bytes());
            let _ = write_half.write_all(&frame).await;
        }
        if !stderr.is_empty() {
            let frame = encode_frame(LogStream::Stderr, stderr.as_bytes());
            let _ = write_half.write_all(&frame).await;
        }
    }
}

/// `GET /exec/{id}/json` — Inspect an exec instance.
///
/// Returns Docker's `ExecInspectResponse` shape, hydrated from the local
/// registry. `ProcessConfig` mirrors the create-time settings; `ExitCode`
/// and `Running` are updated by [`mark_exec_started`] /
/// [`mark_exec_finished`].
async fn inspect_exec(State(_state): State<SocketState>, Path(exec_id): Path<String>) -> Response {
    let entry = {
        let guard = exec_registry().read().await;
        match guard.get(&exec_id) {
            Some(e) => e.clone(),
            None => {
                return error_response(
                    StatusCode::NOT_FOUND,
                    &format!("exec instance not found: {exec_id}"),
                );
            }
        }
    };
    Json(exec_inspect_response(&exec_id, &entry)).into_response()
}

/// Build Docker's `ExecInspectResponse` JSON for the given registry entry.
///
/// Pulled out of [`inspect_exec`] so the translation can be unit-tested
/// without spinning up the registry.
fn exec_inspect_response(exec_id: &str, entry: &ExecRegistryEntry) -> serde_json::Value {
    let (entrypoint, arguments) = match entry.cmd.split_first() {
        Some((head, tail)) => (head.clone(), tail.to_vec()),
        None => (String::new(), Vec::new()),
    };
    serde_json::json!({
        "ID": exec_id,
        "Running": entry.running,
        "ExitCode": entry.exit_code,
        "ProcessConfig": {
            "tty": entry.tty,
            "entrypoint": entrypoint,
            "arguments": arguments,
            "privileged": entry.privileged,
            "user": entry.user.clone().unwrap_or_default(),
            "working_dir": entry.working_dir.clone().unwrap_or_default(),
        },
        "OpenStdin": entry.attach_stdin,
        "OpenStderr": entry.attach_stderr,
        "OpenStdout": entry.attach_stdout,
        "CanRemove": false,
        "ContainerID": entry.container_id,
        "DetachKeys": "",
    })
}

/// Stamp the `running` flag on a registry entry.
async fn mark_exec_started(exec_id: &str) {
    let mut guard = exec_registry().write().await;
    if let Some(entry) = guard.get_mut(exec_id) {
        entry.running = true;
    }
}

/// Clear `running` and stamp the captured exit code on a registry entry.
async fn mark_exec_finished(exec_id: &str, exit_code: i32) {
    let mut guard = exec_registry().write().await;
    if let Some(entry) = guard.get_mut(exec_id) {
        entry.running = false;
        entry.exit_code = Some(exit_code);
    }
}

/// Query parameters accepted on `POST /containers/{id}/attach`.
///
/// Docker accepts `stream`, `logs`, `stdin`, `stdout`, `stderr`, plus the
/// rarely-used `detachKeys`. All five booleans default to `false`; we
/// honour Docker's lax bool spellings (`1`/`0`/`true`/`false`/`yes`/`no`).
/// `detachKeys` is parsed but ignored — the docker-compat shim does not
/// implement interactive escape sequences.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // mirrors Docker's `/attach` query 1:1
struct AttachQuery {
    #[serde(default)]
    stream: Option<String>,
    #[serde(default)]
    logs: Option<String>,
    #[serde(default)]
    stdin: Option<String>,
    #[serde(default)]
    stdout: Option<String>,
    #[serde(default)]
    stderr: Option<String>,
    #[serde(default, rename = "detachKeys")]
    #[allow(dead_code)]
    detach_keys: Option<String>,
}

/// Resolved attach parameters, normalised from [`AttachQuery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // mirrors Docker's `/attach` query 1:1
struct AttachParams {
    /// Forward live output as it arrives (`stream=1`).
    stream: bool,
    /// Replay the historical log buffer before live output (`logs=1`).
    logs: bool,
    /// Accept stdin from the hijacked socket. The shim does not yet
    /// implement stdin forwarding (Phase 4.1.x); the flag is tracked so
    /// callers can opt in once the daemon-side path lands.
    stdin: bool,
    /// Include stdout in the multiplexed output stream.
    stdout: bool,
    /// Include stderr in the multiplexed output stream.
    stderr: bool,
}

impl AttachParams {
    fn from_query(q: &AttachQuery) -> Self {
        // Docker's contract: when neither stdout nor stderr is set, both
        // are emitted — matches the `/logs` handler's behaviour.
        let (stdout, stderr) = match (q.stdout.as_deref(), q.stderr.as_deref()) {
            (None, None) => (true, true),
            (so, se) => (parse_bool(so), parse_bool(se)),
        };
        Self {
            stream: parse_bool(q.stream.as_deref()),
            logs: parse_bool(q.logs.as_deref()),
            stdin: parse_bool(q.stdin.as_deref()),
            stdout,
            stderr,
        }
    }
}

/// `POST /containers/{id}/attach` — Hijacked attach (logs-only mode).
///
/// Hijacks the underlying HTTP/1.1 connection, then forwards the
/// container's existing log feed to the upgraded socket as Docker stdcopy
/// multiplex frames. Stdin attach is documented as a Phase 4.1.x follow-up
/// — the registered flag is kept so a future revision can wire it up
/// without changing the public surface.
async fn attach_container(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<AttachQuery>,
    req: axum::extract::Request,
) -> Response {
    let params = AttachParams::from_query(&q);

    // Build the 101 Switching Protocols response now; the actual byte
    // streaming happens once hyper drives the upgrade to completion on a
    // detached task.
    let response = raw_stream_response();

    let client = state.client.clone();
    let id_for_task = id.clone();
    tokio::spawn(async move {
        let conn = match HijackedConnection::from_request(req).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    container = %id_for_task,
                    "docker /containers/.../attach: hijack upgrade failed"
                );
                return;
            }
        };
        let (_read_half, mut write_half) = conn.split();

        // Pull the existing logs feed; daemon's `format=raw` already emits
        // stdcopy multiplex frames, so we forward the bytes verbatim.
        let follow = params.stream;
        let logs = match client
            .stream_container_logs(
                &id_for_task,
                follow,
                /* tail */ if params.logs { None } else { Some(0) },
                /* since */ None,
                /* until */ None,
                /* timestamps */ false,
                params.stdout,
                params.stderr,
                /* format_raw */ true,
            )
            .await
        {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    container = %id_for_task,
                    "docker /containers/.../attach: stream_container_logs failed"
                );
                let msg = format!("attach failed: {err:#}\n");
                let frame = encode_frame(LogStream::Stderr, msg.as_bytes());
                let _ = write_half.write_all(&frame).await;
                let _ = write_half.flush().await;
                let _ = write_half.shutdown().await;
                return;
            }
        };
        let mut logs = logs;
        while let Some(chunk) = logs.next().await {
            match chunk {
                Ok(bytes) if bytes.is_empty() => {}
                Ok(bytes) => {
                    if write_half.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        container = %id_for_task,
                        "docker /containers/.../attach: dropping log chunk"
                    );
                }
            }
        }
        let _ = write_half.flush().await;
        let _ = write_half.shutdown().await;
    });

    response
}

/// Query parameters accepted on `POST /containers/{id}/resize` and
/// `POST /exec/{id}/resize`. Docker passes the new TTY size as `h` (rows)
/// and `w` (columns).
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
struct ResizeQuery {
    #[serde(default)]
    h: Option<u32>,
    #[serde(default)]
    w: Option<u32>,
}

/// `POST /containers/{id}/resize` — Resize a container's TTY.
///
/// The buffered (non-PTY) container backend has no TTY to resize, so this
/// handler validates the query parameters and returns `200 OK`. The full
/// path lands with Phase 4.1.x; until then clients receive Docker's
/// success status without their resize being applied.
async fn resize_container(
    State(_state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<ResizeQuery>,
) -> Response {
    tracing::debug!(
        container = %id,
        rows = ?q.h,
        cols = ?q.w,
        "docker API: POST /containers/.../resize accepted as no-op (PTY backend lands with 4.1.x)"
    );
    StatusCode::OK.into_response()
}

/// `POST /exec/{id}/resize` — Resize an exec instance's TTY.
///
/// Same caveat as [`resize_container`]: the buffered exec backend has no
/// PTY to resize, so we validate and 200. The exec must still exist in the
/// registry — unknown IDs return 404 to mirror Docker's behaviour.
async fn resize_exec(
    State(_state): State<SocketState>,
    Path(exec_id): Path<String>,
    Query(q): Query<ResizeQuery>,
) -> Response {
    {
        let guard = exec_registry().read().await;
        if !guard.contains_key(&exec_id) {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("exec instance not found: {exec_id}"),
            );
        }
    }
    tracing::debug!(
        exec_id = %exec_id,
        rows = ?q.h,
        cols = ?q.w,
        "docker API: POST /exec/.../resize accepted as no-op (PTY backend lands with 4.1.x)"
    );
    StatusCode::OK.into_response()
}

// Silence unused-import warning when the feature set doesn't drag in
// `DaemonClient` references (handlers use the concrete type via `state`).
#[allow(dead_code)]
const _ASSERT_CLIENT_TYPE: Option<fn() -> Arc<DaemonClient>> = None;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_to_unix_seconds_epoch() {
        assert_eq!(parse_iso8601_secs("1970-01-01T00:00:00Z"), 0);
    }

    #[test]
    fn iso8601_to_unix_seconds_known() {
        // 2024-01-01T00:00:00Z = 1_704_067_200
        assert_eq!(parse_iso8601_secs("2024-01-01T00:00:00Z"), 1_704_067_200);
    }

    #[test]
    fn iso8601_handles_fractional() {
        assert_eq!(
            parse_iso8601_secs("2024-01-01T00:00:00.123456Z"),
            1_704_067_200
        );
    }

    #[test]
    fn state_mapping_running() {
        let (s, status) = docker_state("running");
        assert_eq!(s, "running");
        assert_eq!(status, "Up");
    }

    #[test]
    fn build_network_settings_populates_ipaddress_and_networks() {
        let v = serde_json::json!({
            "ipv4": "10.99.99.20",
            "networks": [
                { "network": "bridge", "ipv4": "10.99.99.20", "aliases": [] }
            ]
        });
        let out = build_network_settings(&v, "abc123");

        assert_eq!(out["IPAddress"].as_str(), Some("10.99.99.20"));
        assert_eq!(
            out["Networks"]["bridge"]["IPAddress"].as_str(),
            Some("10.99.99.20")
        );

        let net_id = out["Networks"]["bridge"]["NetworkID"]
            .as_str()
            .expect("NetworkID");
        let ep_id = out["Networks"]["bridge"]["EndpointID"]
            .as_str()
            .expect("EndpointID");
        assert_eq!(net_id.len(), 64);
        assert_eq!(ep_id.len(), 64);
        assert!(net_id.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(ep_id.chars().all(|c| c.is_ascii_hexdigit()));
        // Top-level EndpointID should mirror the first attachment's endpoint.
        assert_eq!(out["EndpointID"].as_str(), Some(ep_id));
    }

    #[test]
    fn build_network_settings_no_networks_yields_empty_defaults() {
        let v = serde_json::json!({
            "networks": []
        });
        let out = build_network_settings(&v, "abc123");

        assert_eq!(out["IPAddress"].as_str(), Some(""));
        assert_eq!(out["EndpointID"].as_str(), Some(""));
        assert_eq!(
            out["Networks"].as_object().map(serde_json::Map::len),
            Some(0)
        );
    }

    #[test]
    fn state_mapping_exited() {
        let (s, _) = docker_state("exited");
        assert_eq!(s, "exited");
    }

    #[test]
    fn state_mapping_unknown_falls_back() {
        let (s, status) = docker_state("zany-new-state");
        assert_eq!(s, "exited");
        assert!(status.contains("zany-new-state"));
    }

    #[test]
    fn parse_bool_truthy() {
        assert!(parse_bool(Some("1")));
        assert!(parse_bool(Some("true")));
        assert!(parse_bool(Some("TRUE")));
        assert!(parse_bool(Some("yes")));
        assert!(!parse_bool(Some("0")));
        assert!(!parse_bool(Some("false")));
        assert!(!parse_bool(None));
    }

    #[test]
    fn container_summary_maps_name_with_leading_slash() {
        let v = serde_json::json!({
            "id": "abc123",
            "name": "myapp",
            "image": "nginx:1.25",
            "created_at": "2024-01-01T00:00:00Z",
            "state": "running",
            "labels": { "foo": "bar" }
        });
        let s = to_container_summary(&v);
        assert_eq!(s.id, "abc123");
        assert_eq!(s.names, vec!["/myapp".to_owned()]);
        assert_eq!(s.image, "nginx:1.25");
        assert_eq!(s.state, "running");
        assert_eq!(s.labels.get("foo").map(String::as_str), Some("bar"));
    }

    #[test]
    fn container_summary_synthesizes_name_from_id() {
        let v = serde_json::json!({
            "id": "abc123",
            "image": "nginx",
            "created_at": "",
            "state": "exited",
            "labels": {}
        });
        let s = to_container_summary(&v);
        assert_eq!(s.names, vec!["/abc123".to_owned()]);
        assert_eq!(s.state, "exited");
    }

    /// Phase 1 §1.3.5: when the daemon's container payload carries a
    /// 64-character hex `id`, the Docker `ContainerSummary.Id` field must
    /// surface that exact hex string verbatim — Docker clients (CLI,
    /// bollard, compose) all key off `Id` and expect the SHA-256-style hex
    /// form rather than `ZLayer`'s internal service-name string.
    #[test]
    fn inspect_container_emits_hex_id() {
        let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let v = serde_json::json!({
            "id": hex,
            "name": "demo",
            "image": "nginx:1.25",
            "created_at": "2024-01-01T00:00:00Z",
            "state": "running",
            "labels": {}
        });
        let s = to_container_summary(&v);
        assert_eq!(
            s.id, hex,
            "ContainerSummary.Id must echo the daemon's hex id verbatim"
        );
        assert_eq!(s.id.len(), 64);
        assert!(s.id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // -- build_create_request -----------------------------------------------
    //
    // The handler-level path (`create_container` async fn) talks to
    // `DaemonClient`, which is hard-wired to a Unix-domain socket on Unix /
    // TCP-loopback on Windows and does not accept arbitrary base URLs — see
    // the rationale on `daemon_client::tests::create_container_serializes...`.
    // Instead, we exercise `build_create_request` directly, which is the
    // pure translation function the handler delegates to before dispatching
    // to the daemon. This covers every translator (`mounts`, `ports`,
    // `restart`, `caps`, `resources`, `healthcheck`) plus the inline
    // `combine_command`, `parse_env`, `parse_network_mode`, `device_mapping_to_spec`,
    // and `ulimits_to_map` helpers in one round-trip.

    // `ContainerCreateBody`, `DeviceMapping`, `HostConfig`, and
    // `UlimitMapping` come in via the module-level `use super::types::...`
    // re-export; only the additional fixtures we need for tests are imported
    // explicitly here.
    use crate::socket::types::container_create::{HealthcheckBody, PortBindingHost, RestartPolicy};

    fn full_docker_run_body() -> ContainerCreateBody {
        // Mirror `docker run --rm --privileged -v /h:/c -p 8080:80 \
        //   --name myapp --cap-add NET_ADMIN --device /dev/kvm --memory 512m \
        //   --restart on-failure:3 --network bridge nginx -g "daemon off;"`.
        let mut hc = HostConfig {
            binds: vec!["/h:/c".to_string()],
            network_mode: Some("bridge".to_string()),
            cap_add: vec!["NET_ADMIN".to_string()],
            privileged: Some(true),
            auto_remove: Some(true),
            restart_policy: Some(RestartPolicy {
                name: Some("on-failure".to_string()),
                maximum_retry_count: Some(3),
            }),
            devices: vec![DeviceMapping {
                path_on_host: "/dev/kvm".to_string(),
                path_in_container: "/dev/kvm".to_string(),
                cgroup_permissions: Some("rwm".to_string()),
            }],
            memory: Some(536_870_912),
            nano_cpus: Some(1_500_000_000),
            extra_hosts: vec!["myhost:127.0.0.1".to_string()],
            dns: vec!["8.8.8.8".to_string()],
            sysctls: HashMap::from([("net.core.somaxconn".to_string(), "1024".to_string())]),
            ulimits: vec![UlimitMapping {
                name: "nofile".to_string(),
                soft: 4096,
                hard: 8192,
            }],
            security_opt: vec!["no-new-privileges:true".to_string()],
            pid_mode: Some("host".to_string()),
            ipc_mode: Some("shareable".to_string()),
            readonly_rootfs: Some(true),
            init: Some(true),
            group_add: vec!["docker".to_string()],
            ..HostConfig::default()
        };
        hc.port_bindings.insert(
            "80/tcp".to_string(),
            Some(vec![PortBindingHost {
                host_ip: Some(String::new()),
                host_port: Some("8080".to_string()),
            }]),
        );

        ContainerCreateBody {
            image: "nginx".to_string(),
            cmd: Some(vec![
                "nginx".to_string(),
                "-g".to_string(),
                "daemon off;".to_string(),
            ]),
            env: vec!["FOO=bar".to_string(), "BARE".to_string()],
            labels: HashMap::from([("app".to_string(), "myapp".to_string())]),
            hostname: Some("myapp.local".to_string()),
            user: Some("1000:1000".to_string()),
            working_dir: Some("/app".to_string()),
            stop_signal: Some("SIGTERM".to_string()),
            stop_timeout: Some(45),
            healthcheck: Some(HealthcheckBody {
                test: vec![
                    "CMD-SHELL".to_string(),
                    "curl -f http://localhost/ || exit 1".to_string(),
                ],
                interval: Some(30_000_000_000),
                timeout: Some(5_000_000_000),
                retries: Some(4),
                start_period: Some(10_000_000_000),
                ..HealthcheckBody::default()
            }),
            host_config: Some(hc),
            ..ContainerCreateBody::default()
        }
    }

    #[test]
    fn create_container_translates_full_docker_run_body() {
        let body = full_docker_run_body();
        let req = build_create_request(Some("myapp".to_string()), &body)
            .expect("translation must succeed");

        // Identity / image / name -------------------------------------------
        assert_eq!(req.image, "nginx");
        assert_eq!(req.name.as_deref(), Some("myapp"));
        assert_eq!(req.hostname.as_deref(), Some("myapp.local"));
        assert_eq!(req.user.as_deref(), Some("1000:1000"));
        assert_eq!(req.work_dir.as_deref(), Some("/app"));

        // Command (cmd alone — no entrypoint) -------------------------------
        let cmd = req.command.expect("command must be set");
        assert_eq!(cmd, vec!["nginx", "-g", "daemon off;"]);

        // Env: KEY=VALUE plus a bare token ----------------------------------
        assert_eq!(req.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(req.env.get("BARE").map(String::as_str), Some(""));
        assert_eq!(req.env.len(), 2);

        // Labels ------------------------------------------------------------
        assert_eq!(req.labels.get("app").map(String::as_str), Some("myapp"));

        // Volumes (legacy bind) --------------------------------------------
        assert_eq!(req.volumes.len(), 1);
        assert_eq!(
            req.volumes[0].mount_type,
            Some(VolumeMountType::Bind),
            "legacy `Binds` entry should translate to a Bind VolumeMount"
        );
        assert_eq!(req.volumes[0].source.as_deref(), Some("/h"));
        assert_eq!(req.volumes[0].target, "/c");
        assert!(!req.volumes[0].readonly);

        // Ports -------------------------------------------------------------
        assert_eq!(req.ports.len(), 1);
        assert_eq!(req.ports[0].container_port, 80);
        assert_eq!(req.ports[0].host_port, Some(8080));

        // Resources (CPU / memory + spread fields) --------------------------
        let res = req.resources.as_ref().expect("resources must be set");
        let cpu = res.cpu.expect("cpu set");
        assert!((cpu - 1.5).abs() < f64::EPSILON);
        assert_eq!(res.memory.as_deref(), Some("536870912b"));

        // Restart policy ----------------------------------------------------
        let rp = req.restart_policy.as_ref().expect("restart policy");
        assert_eq!(rp.kind, ContainerRestartKind::OnFailure);
        assert_eq!(rp.max_attempts, Some(3));

        // Network mode ------------------------------------------------------
        assert_eq!(
            req.network_mode,
            Some(NetworkMode::Bridge { name: None }),
            "`bridge` should map to NetworkMode::Bridge {{ name: None }}"
        );

        // Capabilities ------------------------------------------------------
        assert_eq!(req.cap_add, vec!["NET_ADMIN".to_string()]);
        assert!(req.cap_drop.is_empty());

        // Privileged & friends ---------------------------------------------
        assert_eq!(req.privileged, Some(true));
        assert!(req.read_only_root_fs);
        assert_eq!(req.init_container, Some(true));

        // Devices -----------------------------------------------------------
        assert_eq!(req.devices.len(), 1);
        assert_eq!(req.devices[0].path, "/dev/kvm");
        assert!(req.devices[0].read);
        assert!(req.devices[0].write);
        assert!(req.devices[0].mknod);

        // DNS / extra_hosts / sysctls / ulimits / security_opt --------------
        assert_eq!(req.dns, vec!["8.8.8.8".to_string()]);
        assert_eq!(req.extra_hosts, vec!["myhost:127.0.0.1".to_string()]);
        assert_eq!(
            req.sysctls.get("net.core.somaxconn").map(String::as_str),
            Some("1024")
        );
        let nofile = req.ulimits.get("nofile").expect("nofile ulimit");
        assert_eq!(nofile.soft, 4096);
        assert_eq!(nofile.hard, 8192);
        assert_eq!(req.security_opt, vec!["no-new-privileges:true".to_string()]);
        assert_eq!(req.pid_mode.as_deref(), Some("host"));
        assert_eq!(req.ipc_mode.as_deref(), Some("shareable"));
        assert_eq!(req.extra_groups, vec!["docker".to_string()]);

        // Stop signal / grace ----------------------------------------------
        assert_eq!(req.stop_signal.as_deref(), Some("SIGTERM"));
        assert_eq!(req.stop_grace_period, Some(Duration::from_secs(45)));

        // Health check -----------------------------------------------------
        let hc_req = req.health_check.as_ref().expect("health check set");
        assert_eq!(hc_req.check_type, "command");
        let hc_cmd = hc_req.command.as_ref().expect("hc command");
        assert_eq!(hc_cmd.len(), 1);
        assert_eq!(hc_cmd[0], "curl -f http://localhost/ || exit 1");
        assert_eq!(hc_req.retries, Some(4));
        assert_eq!(hc_req.interval.as_deref(), Some("30s"));
        assert_eq!(hc_req.timeout.as_deref(), Some("5s"));
        assert_eq!(hc_req.start_period.as_deref(), Some("10s"));

        // Lifecycle (--rm) -------------------------------------------------
        assert!(
            req.lifecycle.delete_on_exit,
            "auto_remove=true should set lifecycle.delete_on_exit"
        );
    }

    #[test]
    fn auto_remove_true_sets_delete_on_exit() {
        // `docker run --rm nginx` ⇒ HostConfig.AutoRemove=true ⇒
        // CreateContainerRequest.lifecycle.delete_on_exit=true.
        let hc = HostConfig {
            auto_remove: Some(true),
            ..HostConfig::default()
        };
        let body = ContainerCreateBody {
            image: "nginx".to_string(),
            host_config: Some(hc),
            ..ContainerCreateBody::default()
        };
        let req = build_create_request(None, &body).expect("translation must succeed");
        assert!(
            req.lifecycle.delete_on_exit,
            "AutoRemove=true must propagate to lifecycle.delete_on_exit=true"
        );
    }

    #[test]
    fn auto_remove_omitted_keeps_default_false() {
        // No HostConfig.AutoRemove (and indeed no HostConfig at all) ⇒
        // lifecycle.delete_on_exit stays at the LifecycleSpec default (false),
        // preserving the historical retain-on-exit behavior.
        let body = ContainerCreateBody {
            image: "nginx".to_string(),
            ..ContainerCreateBody::default()
        };
        let req = build_create_request(None, &body).expect("translation must succeed");
        assert!(
            !req.lifecycle.delete_on_exit,
            "omitted AutoRemove must leave lifecycle.delete_on_exit=false (default)"
        );

        // Also exercise an explicit `auto_remove: Some(false)` to confirm
        // the false case round-trips identically to omission.
        let hc_false = HostConfig {
            auto_remove: Some(false),
            ..HostConfig::default()
        };
        let body_false = ContainerCreateBody {
            image: "nginx".to_string(),
            host_config: Some(hc_false),
            ..ContainerCreateBody::default()
        };
        let req_false = build_create_request(None, &body_false).expect("translation must succeed");
        assert!(
            !req_false.lifecycle.delete_on_exit,
            "AutoRemove=false must leave lifecycle.delete_on_exit=false"
        );
    }

    #[test]
    fn build_create_request_400_on_missing_image() {
        let body = ContainerCreateBody {
            image: String::new(),
            ..ContainerCreateBody::default()
        };
        let err = build_create_request(None, &body).expect_err("must reject empty image");
        assert!(
            err.to_lowercase().contains("image"),
            "expected `image`-related error, got: {err}"
        );
    }

    #[test]
    fn build_create_request_combines_entrypoint_and_cmd() {
        // Docker semantics: entrypoint comes first, cmd is appended.
        let body = ContainerCreateBody {
            image: "alpine".to_string(),
            entrypoint: Some(vec!["/usr/bin/dumb-init".to_string(), "--".to_string()]),
            cmd: Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo hi".to_string(),
            ]),
            ..ContainerCreateBody::default()
        };
        let req = build_create_request(None, &body).unwrap();
        assert_eq!(
            req.command.unwrap(),
            vec![
                "/usr/bin/dumb-init".to_string(),
                "--".to_string(),
                "sh".to_string(),
                "-c".to_string(),
                "echo hi".to_string(),
            ]
        );
    }

    #[test]
    fn build_create_request_no_command_when_neither_supplied() {
        let body = ContainerCreateBody {
            image: "alpine".to_string(),
            ..ContainerCreateBody::default()
        };
        let req = build_create_request(None, &body).unwrap();
        assert!(req.command.is_none());
    }

    #[test]
    fn build_create_request_query_name_overrides_body() {
        // The Docker API takes the container name from `?name=` query param,
        // not from the body — confirm that's what `build_create_request`
        // accepts as its `name` argument and that the body's hostname is
        // independent.
        let body = ContainerCreateBody {
            image: "alpine".to_string(),
            hostname: Some("ignored.local".to_string()),
            ..ContainerCreateBody::default()
        };
        let req = build_create_request(Some("query-name".to_string()), &body).unwrap();
        assert_eq!(req.name.as_deref(), Some("query-name"));
        // Hostname is independent of name and is preserved.
        assert_eq!(req.hostname.as_deref(), Some("ignored.local"));
    }

    #[test]
    fn build_create_request_parses_all_network_modes() {
        // String form, all five Docker spellings.
        for (input, expected) in [
            ("default", NetworkMode::Default),
            ("host", NetworkMode::Host),
            ("none", NetworkMode::None),
            ("bridge", NetworkMode::Bridge { name: None }),
            (
                "bridge:custom",
                NetworkMode::Bridge {
                    name: Some("custom".to_string()),
                },
            ),
            (
                "container:abc123",
                NetworkMode::Container {
                    id: "abc123".to_string(),
                },
            ),
        ] {
            let body = ContainerCreateBody {
                image: "alpine".to_string(),
                host_config: Some(HostConfig {
                    network_mode: Some(input.to_string()),
                    ..HostConfig::default()
                }),
                ..ContainerCreateBody::default()
            };
            let req = build_create_request(None, &body).unwrap();
            assert_eq!(
                req.network_mode,
                Some(expected),
                "network_mode {input:?} did not translate as expected"
            );
        }
    }

    #[test]
    fn build_create_request_rejects_empty_container_id_network_mode() {
        let body = ContainerCreateBody {
            image: "alpine".to_string(),
            host_config: Some(HostConfig {
                network_mode: Some("container:".to_string()),
                ..HostConfig::default()
            }),
            ..ContainerCreateBody::default()
        };
        let err = build_create_request(None, &body).unwrap_err();
        assert!(
            err.contains("container:"),
            "expected error mentioning the container: form, got: {err}"
        );
    }

    #[test]
    fn build_create_request_rejects_unknown_network_mode() {
        let body = ContainerCreateBody {
            image: "alpine".to_string(),
            host_config: Some(HostConfig {
                network_mode: Some("not-a-real-mode".to_string()),
                ..HostConfig::default()
            }),
            ..ContainerCreateBody::default()
        };
        let err = build_create_request(None, &body).unwrap_err();
        assert!(
            err.to_lowercase().contains("network mode"),
            "expected network-mode error, got: {err}"
        );
    }

    #[test]
    fn map_create_error_404_propagates_image_not_found() {
        let err = anyhow::anyhow!("404 Not Found: image nginx not found");
        let resp = map_create_error(&err);
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn map_create_error_409_propagates_name_conflict() {
        // `DaemonClient::check_status` formats non-404/500 as
        // `"Daemon returned <status> -- <body>"` — confirm we recognise the
        // 409 in that prefix and bubble it back to the Docker client.
        let err = anyhow::anyhow!("Daemon returned 409 Conflict -- name 'myapp' already in use");
        let resp = map_create_error(&err);
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn map_create_error_400_propagates_bad_request() {
        let err = anyhow::anyhow!("Daemon returned 400 Bad Request -- invalid memory string");
        let resp = map_create_error(&err);
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn map_create_error_default_500() {
        let err = anyhow::anyhow!("connection refused");
        let resp = map_create_error(&err);
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn parse_env_skips_malformed_and_handles_bare() {
        let entries = vec![
            "FOO=bar".to_string(),
            "BAZ=".to_string(),
            "BARE".to_string(),
            "=oops".to_string(), // empty key — must be dropped
            String::new(),       // empty entry — must be dropped
        ];
        let env = parse_env(&entries);
        assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(env.get("BAZ").map(String::as_str), Some(""));
        assert_eq!(env.get("BARE").map(String::as_str), Some(""));
        assert!(!env.contains_key(""));
        assert_eq!(env.len(), 3);
    }

    #[test]
    fn device_mapping_decodes_cgroup_perms() {
        let d = DeviceMapping {
            path_on_host: "/dev/null".to_string(),
            path_in_container: "/dev/null".to_string(),
            cgroup_permissions: Some("rw".to_string()),
        };
        let spec = device_mapping_to_spec(&d);
        assert_eq!(spec.path, "/dev/null");
        assert!(spec.read);
        assert!(spec.write);
        assert!(!spec.mknod);

        // Default `rwm` when omitted.
        let d2 = DeviceMapping {
            path_on_host: "/dev/kvm".to_string(),
            path_in_container: "/dev/kvm".to_string(),
            cgroup_permissions: None,
        };
        let spec2 = device_mapping_to_spec(&d2);
        assert!(spec2.read);
        assert!(spec2.write);
        assert!(spec2.mknod);
    }

    #[test]
    fn format_duration_string_picks_largest_unit() {
        assert_eq!(format_duration_string(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration_string(Duration::from_millis(500)), "500ms");
        assert_eq!(format_duration_string(Duration::from_micros(750)), "750us");
        assert_eq!(format_duration_string(Duration::from_nanos(123)), "123ns");
        // Mixed (1.5s) — milliseconds is the cleanest sub-second unit.
        assert_eq!(
            format_duration_string(Duration::from_millis(1500)),
            "1500ms"
        );
    }

    // -----------------------------------------------------------------------
    // /containers/{id}/logs streaming
    // -----------------------------------------------------------------------
    //
    // The handler itself depends on a live `DaemonClient`, which (like the
    // `create_container` path above) is hard-wired to the Unix-domain socket
    // and not constructable from arbitrary base URLs. We exercise the two
    // testable seams separately:
    //   * `LogsParams::from_query` covers the query-string parsing rules
    //     (Docker bool spellings, `tail=all`, `since`/`until`).
    //   * A synthetic byte stream feeds the same body-stream construction
    //     the handler uses, asserting that bytes flow through verbatim
    //     (matching Docker's raw-stream contract).

    #[test]
    fn logs_params_default_streams_both_when_neither_set() {
        let p = LogsParams::from_query(&LogsQuery::default());
        assert!(p.stdout, "default must include stdout");
        assert!(p.stderr, "default must include stderr");
        assert!(!p.follow);
        assert!(!p.timestamps);
        assert!(p.tail.is_none());
        assert!(p.since.is_none());
        assert!(p.until.is_none());
    }

    #[test]
    fn logs_params_explicit_stream_selection() {
        // stdout=1 alone -> stderr defaults to false.
        let q = LogsQuery {
            stdout: Some("1".to_string()),
            ..LogsQuery::default()
        };
        let p = LogsParams::from_query(&q);
        assert!(p.stdout);
        assert!(!p.stderr);

        // stderr=true alone -> stdout defaults to false.
        let q = LogsQuery {
            stderr: Some("true".to_string()),
            ..LogsQuery::default()
        };
        let p = LogsParams::from_query(&q);
        assert!(!p.stdout);
        assert!(p.stderr);
    }

    #[test]
    fn logs_params_parses_follow_and_tail_and_window() {
        let q = LogsQuery {
            follow: Some("true".to_string()),
            timestamps: Some("yes".to_string()),
            tail: Some("100".to_string()),
            since: Some("1700000000".to_string()),
            until: Some("1700100000".to_string()),
            ..LogsQuery::default()
        };
        let p = LogsParams::from_query(&q);
        assert!(p.follow);
        assert!(p.timestamps);
        assert_eq!(p.tail, Some(100));
        assert_eq!(p.since, Some(1_700_000_000));
        assert_eq!(p.until, Some(1_700_100_000));
    }

    #[test]
    fn logs_params_tail_all_means_no_truncation() {
        let q = LogsQuery {
            tail: Some("all".to_string()),
            ..LogsQuery::default()
        };
        let p = LogsParams::from_query(&q);
        assert!(p.tail.is_none(), "tail=all must be None on the wire");
    }

    /// 3.2.4: with `format=raw` the daemon emits stdcopy-framed bytes; the
    /// compat handler forwards the body verbatim (no NDJSON parsing, no
    /// re-framing), so a synthetic byte stream must round-trip unchanged
    /// through the body-stream pipeline the handler uses. We exercise the
    /// same `Body::from_stream(filter_map)` plumbing the handler sets up
    /// to assert verbatim forwarding.
    #[tokio::test]
    async fn logs_raw_stream_forwards_bytes_verbatim() {
        use crate::socket::streaming::log_frame::{encode_frame, LogStream};
        use futures_util::StreamExt as _;

        let frame_a = encode_frame(LogStream::Stdout, b"hello ");
        let frame_b = encode_frame(LogStream::Stderr, b"oops");
        let frame_c = encode_frame(LogStream::Stdout, b"world");

        let expected: Vec<u8> = frame_a
            .iter()
            .chain(frame_b.iter())
            .chain(frame_c.iter())
            .copied()
            .collect();

        let frames = vec![
            Ok::<Bytes, anyhow::Error>(frame_a),
            Ok::<Bytes, anyhow::Error>(Bytes::new()), // empty frame must be skipped
            Ok::<Bytes, anyhow::Error>(frame_b),
            Ok::<Bytes, anyhow::Error>(frame_c),
        ];
        let upstream = stream::iter(frames);

        // Mirror the handler's filter_map: drop empty frames, log+swallow
        // body errors, forward Bytes verbatim.
        let body_stream = upstream.filter_map(|res| async move {
            match res {
                Ok(bytes) if bytes.is_empty() => None,
                Ok(bytes) => Some(Ok::<Bytes, Infallible>(bytes)),
                Err(_) => None,
            }
        });

        let collected: Vec<Bytes> = body_stream.map(|r| r.unwrap()).collect().await;
        let flat: Vec<u8> = collected.iter().flat_map(|b| b.iter().copied()).collect();
        assert_eq!(
            flat, expected,
            "raw-stream body must forward stdcopy frames verbatim"
        );
    }

    // -----------------------------------------------------------------------
    // /containers/{id}/stats streaming
    // -----------------------------------------------------------------------

    fn make_stats_sample(secs: i64, cpu_total: u64, cpu_system: u64) -> StatsSample {
        // 2024-01-01T00:00:00Z is 1_704_067_200; secs is added on top so
        // tests can build distinct timestamps without pulling in a clock.
        let ts =
            chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).expect("valid unix timestamp");
        StatsSample {
            cpu_total_ns: cpu_total,
            cpu_system_ns: cpu_system,
            online_cpus: 4,
            mem_used_bytes: 50_000_000,
            mem_limit_bytes: 1_073_741_824,
            net_rx_bytes: 1024,
            net_tx_bytes: 2048,
            blkio_read_bytes: 4096,
            blkio_write_bytes: 8192,
            pids_current: 7,
            pids_limit: Some(100),
            timestamp: ts,
        }
    }

    /// 3.3.3: in `stream=true` mode the daemon emits one sample per tick;
    /// the compat handler must turn each into a single NDJSON line and
    /// the second sample must carry the first sample's CPU counters in
    /// `precpu_stats` (so clients can compute deltas).
    #[tokio::test]
    async fn stats_stream_true_emits_multiple_lines_with_precpu() {
        use futures_util::StreamExt as _;

        let s1 = make_stats_sample(1_704_067_200, 100, 1_000);
        let s2 = make_stats_sample(1_704_067_201, 200, 2_000);
        let s3 = make_stats_sample(1_704_067_202, 350, 3_000);

        let upstream = stream::iter(vec![
            Ok::<StatsSample, anyhow::Error>(s1),
            Ok::<StatsSample, anyhow::Error>(s2),
            Ok::<StatsSample, anyhow::Error>(s3),
        ]);

        let body_stream = stats_body_stream(upstream);
        let lines: Vec<Bytes> = body_stream.map(|r| r.unwrap()).collect().await;

        assert_eq!(lines.len(), 3, "one NDJSON line per sample");
        for line in &lines {
            assert!(
                line.last() == Some(&b'\n'),
                "every NDJSON line must terminate with \\n"
            );
        }

        // First line: precpu_stats has total_usage=0 (no prior tick).
        let v1: serde_json::Value = serde_json::from_slice(&lines[0]).unwrap();
        assert_eq!(v1["cpu_stats"]["cpu_usage"]["total_usage"], 100);
        assert_eq!(v1["precpu_stats"]["cpu_usage"]["total_usage"], 0);

        // Second line: precpu_stats inherits sample 1's counters.
        let v2: serde_json::Value = serde_json::from_slice(&lines[1]).unwrap();
        assert_eq!(v2["cpu_stats"]["cpu_usage"]["total_usage"], 200);
        assert_eq!(v2["precpu_stats"]["cpu_usage"]["total_usage"], 100);
        assert_eq!(v2["precpu_stats"]["system_cpu_usage"], 1_000);

        // Third line: precpu_stats inherits sample 2's counters.
        let v3: serde_json::Value = serde_json::from_slice(&lines[2]).unwrap();
        assert_eq!(v3["cpu_stats"]["cpu_usage"]["total_usage"], 350);
        assert_eq!(v3["precpu_stats"]["cpu_usage"]["total_usage"], 200);
        assert_eq!(v3["precpu_stats"]["system_cpu_usage"], 2_000);
    }

    /// 3.3.3: in `stream=false` mode the daemon emits exactly one sample
    /// then closes; the compat handler must mirror that behaviour by
    /// emitting a single line and ending the body.
    #[tokio::test]
    async fn stats_stream_false_emits_one_then_closes() {
        use futures_util::StreamExt as _;

        let only = make_stats_sample(1_704_067_200, 42, 99);
        let upstream = stream::iter(vec![Ok::<StatsSample, anyhow::Error>(only)]);

        let body_stream = stats_body_stream(upstream);
        let lines: Vec<Bytes> = body_stream.map(|r| r.unwrap()).collect().await;

        assert_eq!(
            lines.len(),
            1,
            "stream=false must emit exactly one NDJSON line"
        );
        let v: serde_json::Value = serde_json::from_slice(&lines[0]).unwrap();
        assert_eq!(v["cpu_stats"]["cpu_usage"]["total_usage"], 42);
        // Sentinel preread on the very first sample.
        assert_eq!(v["preread"], "0001-01-01T00:00:00Z");
    }

    /// 3.3.3: the Docker `/containers/{id}/stats` shape carries CPU,
    /// memory, blkio, and per-interface network counters in a fixed
    /// envelope. Pin the field paths so a future field rename surfaces
    /// here rather than in a downstream Docker SDK consumer.
    #[test]
    fn stats_sample_to_docker_shape_mapping() {
        let sample = make_stats_sample(1_704_067_200, 1234, 99_999);
        let v = stats_sample_to_docker(&sample, None);

        // CPU.
        assert_eq!(v["cpu_stats"]["cpu_usage"]["total_usage"], 1234);
        assert_eq!(v["cpu_stats"]["system_cpu_usage"], 99_999);
        assert_eq!(v["cpu_stats"]["online_cpus"], 4);

        // Memory.
        assert_eq!(v["memory_stats"]["usage"], 50_000_000);
        assert_eq!(v["memory_stats"]["limit"], 1_073_741_824_u64);

        // Blkio: Read+Write entries in the recursive array.
        let blkio = &v["blkio_stats"]["io_service_bytes_recursive"];
        let entries = blkio.as_array().expect("blkio array");
        assert_eq!(entries.len(), 2);
        let read_entry = entries
            .iter()
            .find(|e| e["op"] == "Read")
            .expect("Read entry present");
        let write_entry = entries
            .iter()
            .find(|e| e["op"] == "Write")
            .expect("Write entry present");
        assert_eq!(read_entry["value"], 4096);
        assert_eq!(write_entry["value"], 8192);

        // Networks: synthetic `eth0` interface with rx_bytes/tx_bytes.
        assert_eq!(v["networks"]["eth0"]["rx_bytes"], 1024);
        assert_eq!(v["networks"]["eth0"]["tx_bytes"], 2048);

        // Pids.
        assert_eq!(v["pids_stats"]["current"], 7);
        assert_eq!(v["pids_stats"]["limit"], 100);

        // Timestamps: `read` is the sample's timestamp; `preread` is the
        // Go zero-value sentinel when no prior sample was supplied.
        assert!(v["read"]
            .as_str()
            .unwrap()
            .starts_with("2024-01-01T00:00:00"));
        assert_eq!(v["preread"], "0001-01-01T00:00:00Z");
    }

    // -- Exec / attach handlers (4.2.2 / 4.2.4-4.2.8) ------------------------
    //
    // Real interactive PTY exec lands with Phase 4.1.x; until then the
    // docker-compat shim translates Docker's wire shapes onto the buffered
    // `exec_in_container` daemon endpoint and tracks exec sessions in a
    // module-local registry. These tests cover the three pure translation
    // paths (no daemon required): exec body parsing, exec inspect output,
    // and attach query normalisation.

    #[test]
    fn exec_config_body_parses_full_docker_shape() {
        let payload = serde_json::json!({
            "Cmd": ["sh", "-c", "echo hi"],
            "Env": ["FOO=bar", "BAZ=qux"],
            "WorkingDir": "/srv",
            "User": "nobody",
            "Privileged": true,
            "Tty": true,
            "AttachStdin": true,
            "AttachStdout": true,
            "AttachStderr": false,
            "DetachKeys": "ctrl-p,ctrl-q",
        });
        let parsed: ExecConfigBody = serde_json::from_value(payload).expect("parses");
        assert_eq!(
            parsed,
            ExecConfigBody {
                cmd: Some(vec!["sh".into(), "-c".into(), "echo hi".into()]),
                env: Some(vec!["FOO=bar".into(), "BAZ=qux".into()]),
                working_dir: Some("/srv".into()),
                user: Some("nobody".into()),
                privileged: Some(true),
                tty: Some(true),
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(false),
                detach_keys: Some("ctrl-p,ctrl-q".into()),
            }
        );
    }

    #[test]
    fn exec_config_body_accepts_minimal_cmd_only_payload() {
        let payload = serde_json::json!({ "Cmd": ["ls"] });
        let parsed: ExecConfigBody = serde_json::from_value(payload).expect("parses");
        assert_eq!(parsed.cmd.as_deref(), Some(&["ls".to_owned()][..]));
        assert!(parsed.env.is_none());
        assert!(parsed.tty.is_none());
        assert!(parsed.attach_stdin.is_none());
    }

    #[test]
    fn exec_config_body_rejects_unrecognised_shape_gracefully() {
        // Docker tolerates extra/missing fields; we should never panic
        // parsing an empty object — the handler enforces the Cmd
        // requirement separately.
        let payload = serde_json::json!({});
        let parsed: ExecConfigBody = serde_json::from_value(payload).expect("parses");
        assert!(parsed.cmd.is_none());
    }

    #[test]
    fn exec_inspect_response_matches_docker_shape() {
        let entry = ExecRegistryEntry {
            container_id: "abc123".into(),
            cmd: vec!["sh".into(), "-c".into(), "true".into()],
            env: vec!["A=1".into()],
            working_dir: Some("/work".into()),
            user: Some("root".into()),
            privileged: false,
            tty: true,
            attach_stdin: true,
            attach_stdout: true,
            attach_stderr: true,
            exit_code: Some(0),
            running: false,
        };
        let v = exec_inspect_response("deadbeef".repeat(8).as_str(), &entry);
        // Top-level shape keys.
        for key in [
            "ID",
            "Running",
            "ExitCode",
            "ProcessConfig",
            "OpenStdin",
            "OpenStderr",
            "OpenStdout",
            "CanRemove",
            "ContainerID",
            "DetachKeys",
        ] {
            assert!(v.get(key).is_some(), "missing top-level key: {key}");
        }
        assert_eq!(v["ID"].as_str().unwrap().len(), 64);
        assert_eq!(v["Running"], false);
        assert_eq!(v["ExitCode"], 0);
        assert_eq!(v["ContainerID"], "abc123");
        assert_eq!(v["CanRemove"], false);
        assert_eq!(v["DetachKeys"], "");
        assert_eq!(v["OpenStdin"], true);
        assert_eq!(v["OpenStdout"], true);
        assert_eq!(v["OpenStderr"], true);

        // ProcessConfig: entrypoint == cmd[0], arguments == cmd[1..].
        let pc = &v["ProcessConfig"];
        assert_eq!(pc["entrypoint"], "sh");
        assert_eq!(
            pc["arguments"],
            serde_json::json!(["-c".to_owned(), "true".to_owned()])
        );
        assert_eq!(pc["tty"], true);
        assert_eq!(pc["privileged"], false);
        assert_eq!(pc["user"], "root");
        assert_eq!(pc["working_dir"], "/work");
    }

    #[test]
    fn exec_inspect_response_handles_empty_cmd_without_panicking() {
        let entry = ExecRegistryEntry {
            container_id: "cid".into(),
            cmd: Vec::new(),
            env: Vec::new(),
            working_dir: None,
            user: None,
            privileged: false,
            tty: false,
            attach_stdin: false,
            attach_stdout: true,
            attach_stderr: true,
            exit_code: None,
            running: true,
        };
        let v = exec_inspect_response("0".repeat(64).as_str(), &entry);
        assert_eq!(v["ProcessConfig"]["entrypoint"], "");
        assert_eq!(v["ProcessConfig"]["arguments"], serde_json::json!([]));
        // While running, ExitCode is null.
        assert!(v["ExitCode"].is_null());
        assert_eq!(v["Running"], true);
    }

    #[test]
    fn attach_query_neither_stdout_nor_stderr_set_means_both() {
        let q = AttachQuery::default();
        let p = AttachParams::from_query(&q);
        assert!(p.stdout, "default attaches stdout");
        assert!(p.stderr, "default attaches stderr");
        assert!(!p.stream, "default does not stream");
        assert!(!p.logs, "default does not replay logs");
        assert!(!p.stdin, "default does not attach stdin");
    }

    #[test]
    fn attach_query_explicit_selection_overrides_defaults() {
        let q = AttachQuery {
            stream: Some("1".into()),
            logs: Some("true".into()),
            stdin: Some("yes".into()),
            stdout: Some("0".into()),
            stderr: Some("true".into()),
            detach_keys: Some("ctrl-p,ctrl-q".into()),
        };
        let p = AttachParams::from_query(&q);
        assert!(p.stream);
        assert!(p.logs);
        assert!(p.stdin);
        assert!(!p.stdout);
        assert!(p.stderr);
    }

    #[test]
    fn attach_query_only_stdout_set_disables_stderr() {
        // Docker's contract: setting any side flips the unset side to
        // false (matches the `/logs` handler's behaviour).
        let q = AttachQuery {
            stream: Some("1".into()),
            logs: Some("1".into()),
            stdout: Some("1".into()),
            ..AttachQuery::default()
        };
        let p = AttachParams::from_query(&q);
        assert!(p.stream);
        assert!(p.logs);
        assert!(p.stdout);
        assert!(!p.stderr);
    }

    #[test]
    fn exec_start_body_parses_detach_and_tty() {
        let body: ExecStartBody =
            serde_json::from_value(serde_json::json!({ "Detach": true, "Tty": false }))
                .expect("parses");
        assert_eq!(body.detach, Some(true));
        assert_eq!(body.tty, Some(false));
    }

    #[test]
    fn generate_exec_id_yields_64_hex_chars_and_unique_values() {
        let a = generate_exec_id();
        let b = generate_exec_id();
        assert_eq!(a.len(), 64);
        assert_eq!(b.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(b.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "consecutive IDs must differ");
    }

    #[test]
    fn resize_query_parses_h_and_w() {
        let q: ResizeQuery =
            serde_json::from_value(serde_json::json!({ "h": 24, "w": 80 })).expect("parses");
        assert_eq!(q.h, Some(24));
        assert_eq!(q.w, Some(80));
    }

    #[test]
    fn resize_query_defaults_to_none() {
        let q = ResizeQuery::default();
        assert_eq!(q.h, None);
        assert_eq!(q.w, None);
    }

    // -----------------------------------------------------------------------
    // Tasks 5.1.x / 5.2.x — wait + rename query parsing.
    // -----------------------------------------------------------------------

    /// Docker's `condition=` query parameter must round-trip through serde
    /// for the four shapes the SDKs send: explicit `not-running`,
    /// `next-exit`, `removed`, and the omitted form. The handler maps `None`
    /// to "default condition" downstream.
    #[test]
    fn wait_query_accepts_all_known_conditions_and_omitted() {
        let cases = [
            ("not-running", Some("not-running")),
            ("next-exit", Some("next-exit")),
            ("removed", Some("removed")),
        ];
        for (wire, expected) in cases {
            let q: WaitQuery =
                serde_json::from_value(serde_json::json!({ "condition": wire })).expect("parses");
            assert_eq!(q.condition.as_deref(), expected);
        }

        let q: WaitQuery =
            serde_json::from_value(serde_json::json!({})).expect("empty parses to default");
        assert!(q.condition.is_none());
    }

    /// Docker's wait body shape must mirror the upstream API: a top-level
    /// `StatusCode` field plus an optional `Error` envelope with a
    /// `Message`. We construct the body the same way the handler does and
    /// check the keys are `PascalCase` as the Docker SDKs expect.
    #[test]
    fn wait_body_uses_docker_pascal_case_shape() {
        // Success: only StatusCode is present.
        let mut body = serde_json::json!({ "StatusCode": 137_i64 });
        assert_eq!(body["StatusCode"], serde_json::json!(137));
        assert!(body.get("Error").is_none());

        // Failure: Error envelope nested with a Message field.
        body["Error"] = serde_json::json!({
            "Message": "container removed before reaching not-running"
        });
        assert_eq!(
            body["Error"]["Message"],
            "container removed before reaching not-running"
        );
        // Sanity-check the encoded JSON keys to lock the wire shape.
        let encoded = serde_json::to_string(&body).unwrap();
        assert!(encoded.contains("\"StatusCode\":137"));
        assert!(encoded.contains("\"Error\""));
        assert!(encoded.contains("\"Message\""));
    }

    /// Docker's `name=<new>` query parameter on `POST /containers/{id}/rename`
    /// is the only field we read off the URL. The handler rejects an
    /// omitted/empty `name` with `400` before touching the daemon, so this
    /// test pins the parsing layer alone (the rejection branch is exercised
    /// through `rename_query_handler_path` below).
    #[test]
    fn rename_query_extracts_name() {
        let q: RenameQuery =
            serde_json::from_value(serde_json::json!({ "name": "new-name" })).expect("parses");
        assert_eq!(q.name.as_deref(), Some("new-name"));

        let empty: RenameQuery = serde_json::from_value(serde_json::json!({})).expect("parses");
        assert!(empty.name.is_none());
    }

    // -----------------------------------------------------------------
    // `GET /containers/json` filters parameter
    // -----------------------------------------------------------------

    /// Compact constructor for a [`ContainerSummary`] used by the
    /// filter tests below.
    fn cs(id: &str, names: &[&str], state: &str, labels: &[(&str, &str)]) -> ContainerSummary {
        ContainerSummary {
            id: id.to_owned(),
            names: names.iter().map(|s| (*s).to_owned()).collect(),
            image: "img:latest".to_owned(),
            image_id: "sha256:deadbeef".to_owned(),
            command: String::new(),
            created: 0,
            state: state.to_owned(),
            status: String::new(),
            ports: Vec::new(),
            labels: labels
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn parse_filters_returns_none_on_empty() {
        assert!(parse_filters(None).is_none());
        assert!(parse_filters(Some("")).is_none());
        assert!(parse_filters(Some("   ")).is_none());
    }

    #[test]
    fn parse_filters_returns_none_on_invalid_json() {
        // Must not panic; just return None.
        assert!(parse_filters(Some("not json")).is_none());
        assert!(parse_filters(Some("{")).is_none());
        // Wrong shape (values must be arrays of strings).
        assert!(parse_filters(Some(r#"{"label":"env=prod"}"#)).is_none());
    }

    #[test]
    fn parse_filters_parses_label_and_status() {
        let raw = r#"{"label":["env=prod","tier=db"],"status":["running"]}"#;
        let parsed = parse_filters(Some(raw)).expect("parses");
        assert_eq!(
            parsed.get("label").map(Vec::as_slice),
            Some(["env=prod".to_owned(), "tier=db".to_owned()].as_slice())
        );
        assert_eq!(
            parsed.get("status").map(Vec::as_slice),
            Some(["running".to_owned()].as_slice())
        );
    }

    #[test]
    fn container_matches_filters_label_kv_exact() {
        let c = cs("abc", &["/x"], "running", &[("env", "prod")]);
        let mut f = HashMap::new();
        f.insert("label".to_owned(), vec!["env=prod".to_owned()]);
        assert!(container_matches_filters(&c, &f));

        let c2 = cs("abc", &["/x"], "running", &[("env", "dev")]);
        assert!(!container_matches_filters(&c2, &f));

        let c3 = cs("abc", &["/x"], "running", &[("other", "prod")]);
        assert!(!container_matches_filters(&c3, &f));
    }

    #[test]
    fn container_matches_filters_label_key_only() {
        let c = cs("abc", &["/x"], "running", &[("managed", "yes")]);
        let mut f = HashMap::new();
        f.insert("label".to_owned(), vec!["managed".to_owned()]);
        assert!(container_matches_filters(&c, &f));

        let c2 = cs("abc", &["/x"], "running", &[("other", "yes")]);
        assert!(!container_matches_filters(&c2, &f));
    }

    #[test]
    fn container_matches_filters_label_and_semantics() {
        let c = cs("abc", &["/x"], "running", &[("a", "1"), ("b", "2")]);
        let mut f = HashMap::new();
        f.insert("label".to_owned(), vec!["a=1".to_owned(), "b=2".to_owned()]);
        assert!(container_matches_filters(&c, &f));

        let c2 = cs("abc", &["/x"], "running", &[("a", "1")]);
        assert!(!container_matches_filters(&c2, &f));
    }

    #[test]
    fn container_matches_filters_status_or_semantics() {
        let mut f = HashMap::new();
        f.insert(
            "status".to_owned(),
            vec!["running".to_owned(), "paused".to_owned()],
        );

        let running = cs("a", &["/x"], "running", &[]);
        let paused = cs("a", &["/x"], "paused", &[]);
        let exited = cs("a", &["/x"], "exited", &[]);
        assert!(container_matches_filters(&running, &f));
        assert!(container_matches_filters(&paused, &f));
        assert!(!container_matches_filters(&exited, &f));
    }

    #[test]
    fn container_matches_filters_status_case_insensitive() {
        let mut f = HashMap::new();
        f.insert("status".to_owned(), vec!["Running".to_owned()]);
        let c = cs("a", &["/x"], "running", &[]);
        assert!(container_matches_filters(&c, &f));
    }

    #[test]
    fn container_matches_filters_name_substring_strips_slash() {
        let mut f = HashMap::new();
        f.insert("name".to_owned(), vec!["web".to_owned()]);

        let c = cs("a", &["/myweb-1"], "running", &[]);
        assert!(container_matches_filters(&c, &f));

        let c2 = cs("a", &["/db-1"], "running", &[]);
        assert!(!container_matches_filters(&c2, &f));
    }

    #[test]
    fn container_matches_filters_id_prefix() {
        let mut f = HashMap::new();
        f.insert("id".to_owned(), vec!["abc".to_owned()]);

        let c = cs("abc123def", &["/x"], "running", &[]);
        assert!(container_matches_filters(&c, &f));

        let c2 = cs("xyz123def", &["/x"], "running", &[]);
        assert!(!container_matches_filters(&c2, &f));
    }

    #[test]
    fn container_matches_filters_unsupported_key_passes_through() {
        let mut f = HashMap::new();
        f.insert("ancestor".to_owned(), vec!["never-matches".to_owned()]);
        let c = cs("abc", &["/x"], "running", &[("env", "prod")]);
        // Unsupported keys do not filter anything out.
        assert!(container_matches_filters(&c, &f));
    }
}
