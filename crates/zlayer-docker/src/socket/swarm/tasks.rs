//! `/tasks` endpoints — Swarm task bridge.
//!
//! A Swarm "task" is one running replica of a service. `ZLayer` models the
//! same primitive as a per-replica container under a deployment+service
//! pair, so each [`zlayer_types::api::services::ContainerSummary`] returned
//! by the daemon maps to exactly one Docker [`Task`].
//!
//! Endpoints implemented here:
//!
//! * `GET /tasks`            — iterate every deployment+service in the
//!   daemon and emit one task per replica. Honours the Docker
//!   `?filters=<json>` query param (see [`apply_task_filters`] for the
//!   supported keys).
//! * `GET /tasks/{id}`       — resolve `{id}` against the per-replica
//!   container id (`{service}-rep-{replica}`) and shape the matching
//!   record as a [`Task`]. 404 when no replica matches.
//! * `GET /tasks/{id}/logs`  — stream logs for one replica via the same
//!   `stream_container_logs` plumbing the `/containers/{id}/logs` endpoint
//!   uses, so multiplexed stdcopy framing matches the Docker wire format.
//!
//! `ZLayer`'s native primitives carry less metadata than Docker's Swarm
//! `Task` — there is no `Task.Version.Index`, no node-id label on a
//! container, no per-task creation timestamp distinct from the underlying
//! container. Those fields are populated with reasonable defaults
//! (`Version.Index = 0`, container id reused as Node id when no other is
//! known) so Docker tooling sees a well-formed payload.

use std::collections::HashMap;
use std::convert::Infallible;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use zlayer_types::api::services::ContainerSummary;

use super::shape::{ContainerSpec, Task, TaskStatus, TaskTemplate, Version};
use crate::socket::system::error_response;
use crate::socket::SocketState;

/// `/tasks` route table.
pub(super) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/tasks", get(list_tasks))
        .route("/tasks/{id}", get(inspect_task))
        .route("/tasks/{id}/logs", get(task_logs))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Query / shape helpers
// ---------------------------------------------------------------------------

/// Query parameters for `GET /tasks`. Docker passes filters as a single
/// URL-encoded JSON object whose values are arrays of strings, e.g.
/// `?filters={"service":["nginx.web"]}`.
#[derive(Debug, Deserialize, Default)]
struct ListQuery {
    #[serde(default)]
    filters: Option<String>,
}

/// Query parameters for `GET /tasks/{id}/logs`. Mirrors the subset of the
/// Docker `/containers/{id}/logs` query string that `stream_container_logs`
/// understands. Boolean flags use Docker's lax spellings
/// (`1`/`0`/`true`/`false`); `tail` accepts `all` or a positive integer;
/// `since` is Unix seconds.
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
    tail: Option<String>,
    #[serde(default)]
    timestamps: Option<String>,
}

/// Resolved log-stream parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // mirrors Docker's logs query 1:1
struct LogsParams {
    follow: bool,
    stdout: bool,
    stderr: bool,
    timestamps: bool,
    tail: Option<u64>,
    since: Option<i64>,
}

impl LogsParams {
    /// Resolve a Docker [`LogsQuery`] into typed parameters. Defaults match
    /// the equivalent code in `socket/containers.rs::container_logs`:
    ///   * `stdout` / `stderr` default to `true` if neither is set; if one
    ///     is set, the other defaults to `false`.
    ///   * `follow`, `timestamps` default to `false`.
    ///   * `tail` defaults to `None` (no truncation); `tail=all` and a
    ///     missing value are equivalent.
    fn from_query(q: &LogsQuery) -> Self {
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
        Self {
            follow,
            stdout,
            stderr,
            timestamps,
            tail,
            since,
        }
    }
}

/// Parse Docker's lax boolean flags (`1`/`true`/`yes`).
fn parse_bool(s: Option<&str>) -> bool {
    matches!(
        s.map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes")
    )
}

// ---------------------------------------------------------------------------
// Filter parsing & application
// ---------------------------------------------------------------------------

/// Parse Docker's URL-encoded JSON filter blob into a multi-valued map.
///
/// Returns `Ok(map)` (possibly empty) on success, or `Err(message)` if the
/// blob is non-empty and not valid JSON / not an object-of-string-arrays.
/// An absent or empty blob yields an empty map (matching Docker's "no
/// filters" semantics).
fn parse_task_filters(raw: Option<&str>) -> Result<HashMap<String, Vec<String>>, String> {
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

/// Apply the supported subset of Docker task filters to a list of tasks.
///
/// Honoured filter keys (mirrors Docker Engine's documented `task ls`
/// filters):
///
/// - `id`            — exact match against `Task.ID`.
/// - `name`          — exact match against the synthetic task name
///   (`{service}.{slot}`, matching the wording `docker service ps` prints).
/// - `service`       — exact match against `Task.ServiceID`
///   (`{deployment}.{service}`).
/// - `node`          — exact match against `Task.NodeID`.
/// - `desired-state` — exact match against `Task.DesiredState`
///   (`running` or `shutdown`).
/// - `label`         — `key` or `key=value` match against `Task.Labels`.
///
/// Unknown filter keys are ignored (matching Docker's permissive semantics).
fn apply_task_filters(tasks: Vec<Task>, filters: &HashMap<String, Vec<String>>) -> Vec<Task> {
    if filters.is_empty() {
        return tasks;
    }
    tasks
        .into_iter()
        .filter(|t| task_matches_filters(t, filters))
        .collect()
}

fn task_matches_filters(task: &Task, filters: &HashMap<String, Vec<String>>) -> bool {
    for (key, values) in filters {
        if values.is_empty() {
            continue;
        }
        let pass = match key.as_str() {
            "id" => values.iter().any(|v| v == &task.id),
            "name" => {
                let synthetic = format!("{}.{}", task.service_id, task.slot);
                values.iter().any(|v| v == &synthetic)
            }
            "service" => values.iter().any(|v| v == &task.service_id),
            "node" => values.iter().any(|v| v == &task.node_id),
            "desired-state" => values.iter().any(|v| v == &task.desired_state),
            "label" => values
                .iter()
                .any(|v| label_matches(&task.spec.container_spec.labels, v)),
            // Unknown filter keys are ignored (Docker semantics).
            _ => true,
        };
        if !pass {
            return false;
        }
    }
    true
}

/// Match a `key` or `key=value` label spec against a JSON-shaped label map.
fn label_matches(labels: &Value, spec: &str) -> bool {
    let Some(obj) = labels.as_object() else {
        return false;
    };
    if let Some((k, v)) = spec.split_once('=') {
        obj.get(k)
            .and_then(Value::as_str)
            .is_some_and(|got| got.contains(v))
    } else {
        obj.contains_key(spec)
    }
}

// ---------------------------------------------------------------------------
// Shape mapping
// ---------------------------------------------------------------------------

/// Map `ContainerSummary.state` to Docker's task-state vocabulary.
///
/// Docker recognises: `new, allocated, pending, assigned, accepted, ready,
/// preparing, starting, running, complete, shutdown, failed, rejected,
/// remove, orphaned`. `ZLayer`'s container lifecycle is narrower; the
/// mapping below picks the closest documented Docker state for each
/// `ZLayer` lifecycle phase. Docker has no per-task "paused" verb (the
/// flag lives on the underlying container only), so a paused replica
/// stays reported as `running` — matching what `docker service ps` shows
/// when a swarm task's container is paused. Unknown / future `ZLayer`
/// states fall through to `"orphaned"` so Docker tooling renders them as
/// "lost contact" rather than crashing on an unrecognised verb.
fn map_task_state(zlayer_state: &str) -> &'static str {
    match zlayer_state {
        // Paused containers report as `running` because Swarm has no
        // dedicated `paused` task-state verb (see fn docs).
        "running" | "paused" => "running",
        "exited" | "stopped" => "shutdown",
        "failed" => "failed",
        "created" | "pending" => "preparing",
        "starting" => "starting",
        _ => "orphaned",
    }
}

/// Whether a Docker task state should be reported as `desired-state =
/// "running"` (anything else maps to `"shutdown"`).
fn desired_state_for(state: &str) -> &'static str {
    match state {
        "running" | "starting" | "preparing" | "ready" | "accepted" | "assigned" | "pending"
        | "allocated" | "new" => "running",
        _ => "shutdown",
    }
}

/// Build a Docker [`Task`] from a daemon-side
/// [`ContainerSummary`] plus the deployment+service it belongs to.
///
/// The mapping follows the rules documented at the module level. The image is
/// carried through from the [`ContainerSummary`] when the daemon reports it;
/// other fields it does not surface (command, labels, timestamps) are
/// populated with their Docker defaults so the JSON shape stays well-formed
/// regardless of which fields the caller's tooling reads.
fn task_from_replica(deployment: &str, service: &str, summary: &ContainerSummary) -> Task {
    let docker_state = map_task_state(&summary.state);
    let desired = desired_state_for(docker_state);
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let container_status = serde_json::json!({
        "ContainerID": summary.id,
        "PID": summary.pid.unwrap_or(0),
        "ExitCode": 0,
    });

    Task {
        id: summary.id.clone(),
        version: Version::default(),
        // `ContainerSummary` does not carry timestamps; reuse "now" so
        // Docker tooling has a parseable RFC 3339 string. The wire shape
        // requires a string here, not a nullable.
        created_at: now.clone(),
        updated_at: now.clone(),
        spec: TaskTemplate {
            container_spec: ContainerSpec {
                image: summary.image.clone(),
                labels: Value::Object(serde_json::Map::new()),
                command: Vec::new(),
                args: Vec::new(),
                env: Vec::new(),
                mounts: Value::Array(Vec::new()),
            },
            resources: Value::Object(serde_json::Map::new()),
            restart_policy: Value::Object(serde_json::Map::new()),
            placement: Value::Object(serde_json::Map::new()),
            networks: Value::Array(Vec::new()),
            log_driver: Value::Null,
            force_update: 0,
            runtime: "container".to_string(),
        },
        service_id: format!("{deployment}.{service}"),
        slot: u64::from(summary.replica.max(1)),
        // `ZLayer` does not stamp the per-container record with the node
        // id that runs it (the scheduler tracks placement separately and
        // the wire DTO drops it). Reuse the container id so the field is
        // stable per-replica and Docker tooling has something unique to
        // group by.
        node_id: summary.id.clone(),
        status: TaskStatus {
            timestamp: now,
            state: docker_state.to_string(),
            message: String::new(),
            container_status,
        },
        desired_state: desired.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Daemon iteration helpers
// ---------------------------------------------------------------------------

/// Pull every `(deployment, service)` pair the daemon currently knows about.
///
/// Iterates `list_deployments()` and, for each, calls `list_services()` to
/// pick service names out of the response. Tolerates per-deployment
/// failures: a single service-list call going sideways logs a warning and
/// drops that deployment from the iteration rather than failing the whole
/// `/tasks` request.
async fn collect_deployment_service_pairs(
    state: &SocketState,
) -> Result<Vec<(String, String)>, String> {
    let deployments = state
        .client
        .list_deployments()
        .await
        .map_err(|e| format!("failed to list deployments: {e}"))?;

    let mut out: Vec<(String, String)> = Vec::new();
    for dep_value in deployments {
        let Some(name) = dep_value.get("name").and_then(Value::as_str) else {
            continue;
        };
        let dep_name = name.to_owned();
        match state.client.list_services(&dep_name).await {
            Ok(services) => {
                for svc_value in services {
                    if let Some(svc_name) = svc_value.get("name").and_then(Value::as_str) {
                        out.push((dep_name.clone(), svc_name.to_owned()));
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    deployment = %dep_name,
                    error = %e,
                    "docker /tasks: failed to list services for deployment, skipping",
                );
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /tasks` — List every replica across every deployment+service as a
/// Docker [`Task`].
async fn list_tasks(State(state): State<SocketState>, Query(q): Query<ListQuery>) -> Response {
    let filters = match parse_task_filters(q.filters.as_deref()) {
        Ok(f) => f,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, msg),
    };

    let pairs = match collect_deployment_service_pairs(&state).await {
        Ok(p) => p,
        Err(msg) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, msg),
    };

    let mut tasks: Vec<Task> = Vec::new();
    for (deployment, service) in pairs {
        match state
            .client
            .deployment_replicas(&deployment, &service)
            .await
        {
            Ok(replicas) => {
                for replica in &replicas {
                    tasks.push(task_from_replica(&deployment, &service, replica));
                }
            }
            Err(e) => {
                tracing::warn!(
                    deployment = %deployment,
                    service = %service,
                    error = %e,
                    "docker /tasks: failed to fetch replicas, skipping service",
                );
            }
        }
    }

    let filtered = apply_task_filters(tasks, &filters);
    (StatusCode::OK, Json(filtered)).into_response()
}

/// `GET /tasks/{id}` — Inspect one task by container id.
///
/// Iterates every deployment+service pair and matches `{id}` against the
/// per-replica container id (`{service}-rep-{replica}`). Returns 404 when
/// no replica matches.
async fn inspect_task(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    match resolve_task(&state, &id).await {
        Ok(Some(task)) => (StatusCode::OK, Json(task)).into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, format!("task {id} not found")),
        Err(msg) => error_response(StatusCode::INTERNAL_SERVER_ERROR, msg),
    }
}

/// Walk the daemon's deployment+service tree to find the replica matching
/// `id`. Returns `Ok(None)` when no replica matches and `Err` only on
/// daemon errors that aren't the simple "deployment had no services" case.
async fn resolve_task(state: &SocketState, id: &str) -> Result<Option<Task>, String> {
    let pairs = collect_deployment_service_pairs(state).await?;
    for (deployment, service) in pairs {
        match state
            .client
            .deployment_replicas(&deployment, &service)
            .await
        {
            Ok(replicas) => {
                if let Some(replica) = replicas.iter().find(|r| r.id == id) {
                    return Ok(Some(task_from_replica(&deployment, &service, replica)));
                }
            }
            Err(e) => {
                tracing::warn!(
                    deployment = %deployment,
                    service = %service,
                    error = %e,
                    "docker /tasks/{{id}}: failed to fetch replicas, skipping service",
                );
            }
        }
    }
    Ok(None)
}

/// `GET /tasks/{id}/logs` — Stream logs for one replica.
///
/// Forwards the daemon's logs stream verbatim using Docker's framed
/// multiplexed wire format (the daemon honours `format=raw`, which emits
/// the 8-byte stdcopy header per chunk). The handler mirrors
/// `socket/containers.rs::container_logs`: the framed body is labelled
/// `application/vnd.docker.multiplexed-stream` so Content-Type-driven
/// clients demux instead of leaking the 8-byte headers into the text.
async fn task_logs(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<LogsQuery>,
) -> Response {
    // 404 fast for ids that don't correspond to any replica, so callers
    // get the same shape as `GET /tasks/{id}` rather than a 500 from the
    // log endpoint.
    let exists = match resolve_task(&state, &id).await {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(msg) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, msg);
        }
    };
    if !exists {
        return error_response(StatusCode::NOT_FOUND, format!("task {id} not found"));
    }

    let params = LogsParams::from_query(&q);

    let stream = match state
        .client
        .stream_container_logs(
            &id,
            params.follow,
            params.tail,
            params.since,
            None,
            params.timestamps,
            params.stdout,
            params.stderr,
            true,
        )
        .await
    {
        Ok(s) => s,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to open task log stream: {e}"),
            );
        }
    };

    let body_stream = stream.filter_map(|res| async move {
        match res {
            Ok(bytes) if bytes.is_empty() => None,
            Ok(bytes) => Some(Ok::<Bytes, Infallible>(bytes)),
            Err(err) => {
                tracing::warn!(error = %err, "docker /tasks/.../logs: dropping body chunk");
                None
            }
        }
    });

    let body = Body::from_stream(body_stream);

    let mut response = Response::builder().status(StatusCode::OK);
    if let Some(headers) = response.headers_mut() {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.docker.multiplexed-stream"),
        );
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        );
        headers.insert("X-Accel-Buffering", HeaderValue::from_static("no"));
    }
    response.body(body).map_or_else(
        |e| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to build /tasks/.../logs response: {e}"),
            )
        },
        IntoResponse::into_response,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_summary(service: &str, replica: u32, state: &str) -> ContainerSummary {
        ContainerSummary {
            id: format!("{service}-rep-{replica}"),
            service: service.to_string(),
            replica,
            image: "docker.io/library/nginx:latest".to_string(),
            state: state.to_string(),
            pid: Some(1234),
            overlay_ip: None,
            node_id: None,
        }
    }

    #[test]
    fn map_task_state_covers_known_lifecycle() {
        assert_eq!(map_task_state("running"), "running");
        assert_eq!(map_task_state("exited"), "shutdown");
        assert_eq!(map_task_state("stopped"), "shutdown");
        assert_eq!(map_task_state("failed"), "failed");
        assert_eq!(map_task_state("paused"), "running");
        assert_eq!(map_task_state("created"), "preparing");
        assert_eq!(map_task_state("pending"), "preparing");
        assert_eq!(map_task_state("starting"), "starting");
        assert_eq!(map_task_state("something-new"), "orphaned");
    }

    #[test]
    fn desired_state_active_states_remain_running() {
        assert_eq!(desired_state_for("running"), "running");
        assert_eq!(desired_state_for("starting"), "running");
        assert_eq!(desired_state_for("preparing"), "running");
        assert_eq!(desired_state_for("shutdown"), "shutdown");
        assert_eq!(desired_state_for("failed"), "shutdown");
        assert_eq!(desired_state_for("orphaned"), "shutdown");
    }

    #[test]
    fn task_from_replica_maps_core_fields() {
        let summary = fixture_summary("web", 2, "running");
        let task = task_from_replica("nginx", "web", &summary);
        assert_eq!(task.id, "web-rep-2");
        assert_eq!(task.service_id, "nginx.web");
        assert_eq!(task.slot, 2);
        assert_eq!(task.status.state, "running");
        assert_eq!(task.desired_state, "running");
        assert_eq!(task.spec.runtime, "container");
        let cs = &task.status.container_status;
        assert_eq!(cs["ContainerID"].as_str(), Some("web-rep-2"));
        assert_eq!(cs["PID"].as_u64(), Some(1234));
    }

    #[test]
    fn task_from_replica_replica_zero_clamps_to_slot_one() {
        // Docker's `Task.Slot` is 1-indexed; `ZLayer`'s `replica` index can
        // be 0 for single-replica services. The mapping clamps to 1 so
        // Docker tooling never sees `Slot: 0`.
        let summary = fixture_summary("svc", 0, "running");
        let task = task_from_replica("dep", "svc", &summary);
        assert_eq!(task.slot, 1);
    }

    #[test]
    fn task_from_replica_exited_marks_shutdown() {
        let summary = fixture_summary("svc", 1, "exited");
        let task = task_from_replica("dep", "svc", &summary);
        assert_eq!(task.status.state, "shutdown");
        assert_eq!(task.desired_state, "shutdown");
    }

    #[test]
    fn parse_filters_handles_well_formed_json() {
        let out = parse_task_filters(Some(r#"{"service":["nginx.web"]}"#)).unwrap();
        assert_eq!(
            out.get("service").map(Vec::as_slice),
            Some(&["nginx.web".to_string()][..])
        );
    }

    #[test]
    fn parse_filters_empty_input_yields_empty_map() {
        assert!(parse_task_filters(None).unwrap().is_empty());
        assert!(parse_task_filters(Some("")).unwrap().is_empty());
        assert!(parse_task_filters(Some("   ")).unwrap().is_empty());
    }

    #[test]
    fn parse_filters_rejects_garbage() {
        assert!(parse_task_filters(Some("not-json")).is_err());
        assert!(parse_task_filters(Some(r#"["not","object"]"#)).is_err());
    }

    #[test]
    fn parse_filters_accepts_map_form() {
        let out = parse_task_filters(Some(
            r#"{"desired-state":{"running":true,"shutdown":false}}"#,
        ))
        .unwrap();
        let v = out.get("desired-state").unwrap();
        assert!(v.contains(&"running".to_string()));
        assert!(!v.contains(&"shutdown".to_string()));
    }

    fn fixture_task(id: &str, service_id: &str, slot: u64, state: &str, desired: &str) -> Task {
        Task {
            id: id.to_string(),
            service_id: service_id.to_string(),
            node_id: id.to_string(),
            slot,
            desired_state: desired.to_string(),
            status: TaskStatus {
                state: state.to_string(),
                ..TaskStatus::default()
            },
            ..Task::default()
        }
    }

    #[test]
    fn apply_filters_id_match() {
        let tasks = vec![
            fixture_task("a-rep-0", "dep.a", 1, "running", "running"),
            fixture_task("b-rep-0", "dep.b", 1, "running", "running"),
        ];
        let mut filters = HashMap::new();
        filters.insert("id".to_string(), vec!["a-rep-0".to_string()]);
        let out = apply_task_filters(tasks, &filters);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "a-rep-0");
    }

    #[test]
    fn apply_filters_service_match() {
        let tasks = vec![
            fixture_task("a-rep-0", "dep.a", 1, "running", "running"),
            fixture_task("b-rep-0", "dep.b", 1, "running", "running"),
        ];
        let mut filters = HashMap::new();
        filters.insert("service".to_string(), vec!["dep.a".to_string()]);
        let out = apply_task_filters(tasks, &filters);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].service_id, "dep.a");
    }

    #[test]
    fn apply_filters_desired_state_match() {
        let tasks = vec![
            fixture_task("a-rep-0", "dep.a", 1, "running", "running"),
            fixture_task("a-rep-1", "dep.a", 2, "shutdown", "shutdown"),
        ];
        let mut filters = HashMap::new();
        filters.insert("desired-state".to_string(), vec!["running".to_string()]);
        let out = apply_task_filters(tasks, &filters);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].desired_state, "running");
    }

    #[test]
    fn apply_filters_name_uses_synthetic_form() {
        let tasks = vec![fixture_task("a-rep-0", "dep.a", 3, "running", "running")];
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), vec!["dep.a.3".to_string()]);
        let out = apply_task_filters(tasks, &filters);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn apply_filters_unknown_key_passes_through() {
        let tasks = vec![fixture_task("a-rep-0", "dep.a", 1, "running", "running")];
        let mut filters = HashMap::new();
        filters.insert("totally-unknown".to_string(), vec!["whatever".to_string()]);
        let out = apply_task_filters(tasks, &filters);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn apply_filters_empty_values_dont_drop() {
        let tasks = vec![fixture_task("a-rep-0", "dep.a", 1, "running", "running")];
        let mut filters = HashMap::new();
        filters.insert("service".to_string(), Vec::new());
        let out = apply_task_filters(tasks, &filters);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn label_matches_handles_present_and_missing() {
        let labels = serde_json::json!({"tier": "edge"});
        assert!(label_matches(&labels, "tier"));
        assert!(label_matches(&labels, "tier=edge"));
        assert!(!label_matches(&labels, "missing"));
        assert!(!label_matches(&labels, "tier=other"));
    }

    #[test]
    fn parse_bool_truthy() {
        assert!(parse_bool(Some("1")));
        assert!(parse_bool(Some("true")));
        assert!(parse_bool(Some("yes")));
        assert!(parse_bool(Some("TRUE")));
        assert!(!parse_bool(None));
        assert!(!parse_bool(Some("0")));
        assert!(!parse_bool(Some("nope")));
    }

    #[test]
    fn logs_params_default_streams_both() {
        let q = LogsQuery::default();
        let p = LogsParams::from_query(&q);
        assert!(p.stdout);
        assert!(p.stderr);
        assert!(!p.follow);
        assert!(!p.timestamps);
        assert!(p.tail.is_none());
        assert!(p.since.is_none());
    }

    #[test]
    fn logs_params_setting_one_disables_the_other() {
        let q = LogsQuery {
            stdout: Some("1".to_string()),
            ..LogsQuery::default()
        };
        let p = LogsParams::from_query(&q);
        assert!(p.stdout);
        assert!(!p.stderr);
    }

    #[test]
    fn logs_params_tail_all_is_no_truncation() {
        let q = LogsQuery {
            tail: Some("all".to_string()),
            ..LogsQuery::default()
        };
        let p = LogsParams::from_query(&q);
        assert!(p.tail.is_none());

        let q = LogsQuery {
            tail: Some("100".to_string()),
            ..LogsQuery::default()
        };
        let p = LogsParams::from_query(&q);
        assert_eq!(p.tail, Some(100));

        let q = LogsQuery {
            tail: Some("garbage".to_string()),
            ..LogsQuery::default()
        };
        let p = LogsParams::from_query(&q);
        assert!(p.tail.is_none());
    }

    #[test]
    fn logs_params_since_parses_unix_seconds() {
        let q = LogsQuery {
            since: Some("1700000000".to_string()),
            ..LogsQuery::default()
        };
        let p = LogsParams::from_query(&q);
        assert_eq!(p.since, Some(1_700_000_000));
    }
}
