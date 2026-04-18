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
//! - `GET    /containers/{id}/logs`      — fetch logs (non-follow, plain text)
//! - `POST   /containers/{id}/start`     — start a stopped container
//! - `POST   /containers/{id}/stop`      — graceful stop (with `t=<secs>`)
//! - `POST   /containers/{id}/kill`      — signal (default `SIGKILL`)
//! - `POST   /containers/{id}/restart`   — restart (with `t=<secs>`)
//! - `DELETE /containers/{id}`           — remove (with `force=1`)
//!
//! The remaining stubs (`create`, `stats`, `wait`, `exec`, `exec/start`)
//! still return `501 Not Implemented`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use zlayer_client::DaemonClient;

use super::types::{ContainerCreateResponse, ContainerSummary};
use super::SocketState;

/// Container API routes.
pub(crate) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/containers/json", get(list_containers))
        .route("/containers/create", post(create_container))
        .route("/containers/{id}/start", post(start_container))
        .route("/containers/{id}/stop", post(stop_container))
        .route("/containers/{id}/kill", post(kill_container))
        .route("/containers/{id}/restart", post(restart_container))
        .route("/containers/{id}", delete(remove_container))
        .route("/containers/{id}/json", get(inspect_container))
        .route("/containers/{id}/logs", get(container_logs))
        .route("/containers/{id}/stats", get(container_stats))
        .route("/containers/{id}/wait", post(wait_container))
        .route("/containers/{id}/exec", post(create_exec))
        .route("/exec/{id}/start", post(start_exec))
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
/// `filters` and `size` are parsed but not honored — the daemon's
/// container list doesn't expose size info, and filter translation
/// (e.g. `label=foo`) would require invoking the daemon repeatedly.
/// `all` and `limit` are honored.
#[derive(Debug, Default, Deserialize)]
struct ListContainersQuery {
    #[serde(default)]
    all: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    filters: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    size: Option<String>,
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

    if let Some(limit) = query.limit {
        if limit > 0 {
            let n = usize::try_from(limit).unwrap_or(usize::MAX);
            summaries.truncate(n);
        }
    }

    Json(summaries).into_response()
}

/// `POST /containers/create` — Create a container (stub).
///
/// The zlayer daemon doesn't currently expose a raw Docker-style
/// container-create endpoint; creation happens via deployments. We
/// leave this as a stub so the rest of the surface still loads.
async fn create_container() -> Json<ContainerCreateResponse> {
    tracing::warn!("docker API: POST /containers/create — stub");
    Json(ContainerCreateResponse {
        id: "zlayer-stub-container-id".to_owned(),
        warnings: vec!["ZLayer Docker API emulation: container create is a stub".to_owned()],
    })
}

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

/// `GET /containers/{id}/json` — Inspect a container.
///
/// Returns a minimum-viable subset of Docker's `ContainerInspect` shape:
/// `Id`, `Created`, `Image`, `Name`, `State { Status, Running, Pid,
/// ExitCode, StartedAt, FinishedAt }`, `HostConfig`, `Config { Image,
/// Labels }`, `NetworkSettings`, `Mounts`. Fields we don't track are
/// filled with reasonable defaults (empty strings, `0`, empty maps).
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
        "NetworkSettings": {
            "Bridge": "",
            "SandboxID": "",
            "HairpinMode": false,
            "LinkLocalIPv6Address": "",
            "LinkLocalIPv6PrefixLen": 0,
            "Ports": {},
            "SandboxKey": "",
            "SecondaryIPAddresses": null,
            "SecondaryIPv6Addresses": null,
            "EndpointID": "",
            "Gateway": "",
            "GlobalIPv6Address": "",
            "GlobalIPv6PrefixLen": 0,
            "IPAddress": "",
            "IPPrefixLen": 0,
            "IPv6Gateway": "",
            "MacAddress": "",
            "Networks": {},
        },
        // Also echo zlayer's derived Docker-style short status so
        // clients that ignore `State.Status` still get something useful.
        "ZLayerStatus": status_str,
    });

    Json(inspect).into_response()
}

/// Query params for `GET /containers/{id}/logs`.
///
/// Docker accepts: `follow`, `stdout`, `stderr`, `since`, `until`,
/// `timestamps`, `tail`. We currently honor only `tail` (as a count or
/// `"all"`). `follow=1` is accepted but ignored — a single snapshot
/// is returned.
#[derive(Debug, Default, Deserialize)]
struct LogsQuery {
    #[serde(default)]
    #[allow(dead_code)]
    follow: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    stdout: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    stderr: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    since: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    until: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    timestamps: Option<String>,
    #[serde(default)]
    tail: Option<String>,
}

/// `GET /containers/{id}/logs` — Fetch container logs.
///
/// Returns the logs as `application/octet-stream` so the Docker CLI is
/// happy to accept them. **Log-frame multiplexing is not implemented
/// here.** Docker normally multiplexes stdout and stderr into a stream
/// of 8-byte-framed chunks (`[stream_type, 0,0,0, size_be_u32, payload...]`)
/// when a container was started without a TTY; without the frame the
/// CLI can still render output, and the CLI's `-t`/`tty` fallback makes
/// this acceptable for v1.
///
/// Follow-mode streaming (`follow=1`) is likewise not implemented — the
/// handler always returns a single snapshot.
async fn container_logs(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<LogsQuery>,
) -> Response {
    // `tail` can be "all" or a number. The daemon accepts an optional u32.
    let tail: Option<u32> = match q.tail.as_deref() {
        Some("all") | None => None,
        Some(n) => n.parse::<u32>().ok(),
    };

    match state.client.get_container_logs(&id, tail).await {
        Ok(text) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/octet-stream")],
            text,
        )
            .into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

/// `GET /containers/{id}/stats` — Get container stats (stub).
async fn container_stats(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: GET /containers/{id}/stats — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("container stats not yet implemented for {id}")
        })),
    )
}

/// `POST /containers/{id}/wait` — Wait for a container (stub).
async fn wait_container(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /containers/{id}/wait — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("container wait not yet implemented for {id}")
        })),
    )
}

/// `POST /containers/{id}/exec` — Create an exec instance (stub).
async fn create_exec(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /containers/{id}/exec — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("exec create not yet implemented for container {id}")
        })),
    )
}

/// `POST /exec/{id}/start` — Start an exec instance (stub).
async fn start_exec(Path(id): Path<String>) -> impl IntoResponse {
    tracing::warn!("docker API: POST /exec/{id}/start — stub");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "message": format!("exec start not yet implemented for {id}")
        })),
    )
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
}
