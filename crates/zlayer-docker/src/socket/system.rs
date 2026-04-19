//! Docker Engine API system endpoints.
//!
//! Provides `/_ping`, `/version`, `/info`, `/events`, and `/system/df`.

use std::collections::HashMap;
use std::convert::Infallible;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use futures_util::stream::{self, Stream, StreamExt};
use zlayer_client::{default_socket_path, DaemonClient};

use super::types::{SystemInfo, VersionInfo};

/// System API routes.
pub fn routes() -> Router {
    Router::new()
        .route("/_ping", get(ping))
        .route("/version", get(version))
        .route("/info", get(info))
        .route("/events", get(events))
        .route("/system/df", get(disk_usage))
}

/// `GET /_ping` — Health check. Returns `"OK"` as plain text.
async fn ping() -> &'static str {
    "OK"
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
async fn info() -> Json<serde_json::Value> {
    let (containers_total, containers_running, containers_paused, containers_stopped) =
        container_counts().await;
    let images_total = image_count().await;

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
        obj.insert(
            "Swarm".into(),
            serde_json::json!({
                "NodeID": "",
                "NodeAddr": "",
                "LocalNodeState": "inactive",
                "ControlAvailable": false,
                "Error": "",
                "RemoteManagers": null,
            }),
        );
        obj.insert("ExperimentalBuild".into(), false.into());
        obj.insert("Labels".into(), serde_json::Value::Array(Vec::new()));
        obj.insert("SystemTime".into(), current_time_rfc3339().into());
    }

    Json(value)
}

/// `GET /events` — Live server-sent event stream of container lifecycle
/// changes.
///
/// We don't have a push channel from the daemon yet, so this polls
/// `get_all_containers()` on a short interval and diffs the result to
/// emit Docker-shaped `{Type, Action, Actor, time, timeNano}` events.
/// A keep-alive comment is sent every 30 seconds so idle clients don't
/// drop the connection.
async fn events() -> impl IntoResponse {
    let stream = events_stream();
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keepalive"),
    )
}

/// `GET /system/df` — Disk usage (stub).
async fn disk_usage() -> impl IntoResponse {
    tracing::warn!("docker API: GET /system/df — stub");
    Json(serde_json::json!({
        "LayersSize": 0,
        "Images": [],
        "Containers": [],
        "Volumes": [],
        "BuildCache": []
    }))
}

// ---------------------------------------------------------------------------
// Event stream plumbing
// ---------------------------------------------------------------------------

/// Poll interval for diffing the container list.
const EVENTS_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// State snapshot used to diff container lifecycle events.
///
/// Maps container id -> (`state_lower`, image).
type ContainerState = HashMap<String, (String, String)>;

/// Build the SSE event stream. We emit one `Event::default().json_data(...)`
/// per container lifecycle change detected between polling ticks.
fn events_stream() -> impl Stream<Item = Result<Event, Infallible>> {
    let client_path = default_socket_path();
    stream::unfold(
        (None::<ContainerState>, client_path),
        |(prev, path)| async move {
            tokio::time::sleep(EVENTS_POLL_INTERVAL).await;

            let current = match DaemonClient::connect_to(&path).await {
                Ok(client) => snapshot_containers(&client).await,
                Err(err) => {
                    tracing::debug!("docker API: /events could not reach daemon at {path}: {err}");
                    ContainerState::new()
                }
            };

            let events = match prev {
                Some(prev_state) => diff_events(&prev_state, &current),
                None => Vec::new(),
            };

            // Batch all events for this tick into a single SSE envelope.
            // Docker's /events emits one JSON object per line; SSE allows
            // multi-line `data:` fields, so joining with `\n` matches the
            // Docker stream shape. Empty ticks yield `None` and are filtered
            // out downstream — axum's `KeepAlive` handles idle keep-alive.
            let event = if events.is_empty() {
                None
            } else {
                let joined = events
                    .iter()
                    .filter_map(|e| serde_json::to_string(e).ok())
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(Event::default().data(joined))
            };

            Some((event, (Some(current), path)))
        },
    )
    .filter_map(|maybe| async move { maybe.map(Ok) })
}

/// Snapshot current containers from the daemon as a lightweight
/// `id -> (state, image)` map suitable for diffing.
async fn snapshot_containers(client: &DaemonClient) -> ContainerState {
    let value = match client.get_all_containers().await {
        Ok(v) => v,
        Err(err) => {
            tracing::debug!("docker API: /events snapshot failed: {err}");
            return ContainerState::new();
        }
    };
    let Some(arr) = value.as_array() else {
        return ContainerState::new();
    };
    let mut out = ContainerState::with_capacity(arr.len());
    for c in arr {
        let Some(id) = c.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let state = c
            .get("status")
            .and_then(|v| v.as_str())
            .or_else(|| c.get("state").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_ascii_lowercase();
        let image = c
            .get("image")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        out.insert(id.to_owned(), (state, image));
    }
    out
}

/// Diff two container snapshots and produce Docker-shaped event JSON
/// objects for create/destroy/start/stop transitions.
fn diff_events(prev: &ContainerState, curr: &ContainerState) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    let (time_sec, time_nano) = now_unix();

    // New containers: create (+ start if already running).
    for (id, (state, image)) in curr {
        match prev.get(id) {
            None => {
                events.push(make_event(
                    "container",
                    "create",
                    id,
                    image,
                    time_sec,
                    time_nano,
                ));
                if is_running(state) {
                    events.push(make_event(
                        "container",
                        "start",
                        id,
                        image,
                        time_sec,
                        time_nano,
                    ));
                }
            }
            Some((prev_state, _)) => {
                let was_running = is_running(prev_state);
                let now_running = is_running(state);
                if !was_running && now_running {
                    events.push(make_event(
                        "container",
                        "start",
                        id,
                        image,
                        time_sec,
                        time_nano,
                    ));
                } else if was_running && !now_running {
                    // Map to `die` when the state shows exited/stopped,
                    // which mirrors Docker's event semantics.
                    let action = if state.contains("exit") || state.contains("dead") {
                        "die"
                    } else {
                        "stop"
                    };
                    events.push(make_event(
                        "container",
                        action,
                        id,
                        image,
                        time_sec,
                        time_nano,
                    ));
                }
            }
        }
    }

    // Removed containers: destroy.
    for (id, (_state, image)) in prev {
        if !curr.contains_key(id) {
            events.push(make_event(
                "container",
                "destroy",
                id,
                image,
                time_sec,
                time_nano,
            ));
        }
    }

    events
}

fn is_running(state: &str) -> bool {
    state == "running" || state.starts_with("up")
}

fn make_event(
    ty: &str,
    action: &str,
    id: &str,
    image: &str,
    time_sec: i64,
    time_nano: i64,
) -> serde_json::Value {
    serde_json::json!({
        "Type": ty,
        "Action": action,
        "Actor": {
            "ID": id,
            "Attributes": {
                "image": image,
            },
        },
        "scope": "local",
        "time": time_sec,
        "timeNano": time_nano,
    })
}

fn now_unix() -> (i64, i64) {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => (
            i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
            i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        ),
        Err(_) => (0, 0),
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

    #[test]
    fn diff_emits_create_start() {
        let prev = ContainerState::new();
        let mut curr = ContainerState::new();
        curr.insert(
            "abc123".to_owned(),
            ("running".to_owned(), "nginx:latest".to_owned()),
        );
        let events = diff_events(&prev, &curr);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["Action"], "create");
        assert_eq!(events[1]["Action"], "start");
        assert_eq!(events[0]["Actor"]["ID"], "abc123");
    }

    #[test]
    fn diff_emits_destroy() {
        let mut prev = ContainerState::new();
        prev.insert(
            "abc123".to_owned(),
            ("running".to_owned(), "nginx:latest".to_owned()),
        );
        let curr = ContainerState::new();
        let events = diff_events(&prev, &curr);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["Action"], "destroy");
    }

    #[test]
    fn diff_emits_stop_on_transition() {
        let mut prev = ContainerState::new();
        prev.insert(
            "abc".to_owned(),
            ("running".to_owned(), "alpine".to_owned()),
        );
        let mut curr = ContainerState::new();
        curr.insert("abc".to_owned(), ("exited".to_owned(), "alpine".to_owned()));
        let events = diff_events(&prev, &curr);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["Action"], "die");
    }
}
