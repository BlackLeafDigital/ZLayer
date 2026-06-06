//! `/services` endpoints — Swarm service bridge to `ZLayer` deployments.
//!
//! Docker's Swarm `Service` is the closest analogue to a `ZLayer` service
//! living inside a `DeploymentSpec`. The bridge here projects each
//! `(deployment, service)` pair from the running daemon onto a single
//! Docker `Service` whose `ID` is `"{deployment}.{service}"`. That naming
//! convention is reversible: every endpoint here splits an inbound `id`
//! back into the two halves before dispatching to
//! [`zlayer_client::DaemonClient`].
//!
//! Endpoint family (Docker Engine API v1.43):
//!
//! - `GET    /services`                — list all services across deployments
//!   (with `?filters=`).
//! - `GET    /services/{id}`           — inspect one `(deployment, service)`
//!   pair.
//! - `POST   /services/create`         — translate a Docker [`ServiceSpec`]
//!   into a `ZLayer` [`DeploymentSpec`] containing exactly one service and
//!   POST it to `/api/v1/deployments`.
//! - `POST   /services/{id}/update`    — fast-path replica-only changes onto
//!   `scale_service`; otherwise re-fetch the deployment, splice the new
//!   service spec in, and re-POST the entire deployment.
//! - `DELETE /services/{id}`           — when the deployment has only this
//!   service, undeploy. Otherwise re-POST the deployment with the service
//!   removed.
//! - `GET    /services/{id}/logs`      — bridge to `get_logs_with_instance`
//!   and frame stdout/stderr with the Docker stdcopy multiplex header so
//!   `docker service logs` clients can decode the stream.
//!
//! Translation tables for Docker -> `ZLayer` and `ZLayer` -> Docker live
//! at the bottom of this file ([`docker_to_zlayer_spec`] /
//! [`zlayer_to_docker_service`]). Fields without a `ZLayer` analogue
//! (`UpdateConfig`, `RollbackConfig`, `Networks`, `EndpointSpec`,
//! `Placement` constraints) are silently dropped on the way in and emitted as empty
//! values on the way out — matching Docker's permissive treatment of
//! unknown extras.

use std::collections::HashMap;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::stream;
use serde::Deserialize;
use serde_json::{json, Value};
use zlayer_spec::{
    CommandSpec, DeploymentSpec, ImageSpec, NodeMode, PullPolicy, ScaleSpec,
    ServiceSpec as ZServiceSpec,
};

use super::shape::{
    iso_timestamp, ContainerSpec, Replicated, Service, ServiceMode, ServiceSpec, TaskTemplate,
    Version,
};
use crate::socket::streaming::log_frame::{multiplex, LogStream};
use crate::socket::system::error_response;
use crate::socket::SocketState;

/// Vec of framed log chunks. Pulled to module scope so the typed
/// destructuring inside [`service_logs`] keeps clippy's
/// `type_complexity` lint quiet without adding a `let`-scoped alias
/// (which would itself trip `items_after_statements`).
type FrameVec = Vec<Result<Bytes, std::io::Error>>;

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/// `/services` route table.
pub(super) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/services", get(list_services))
        .route("/services/create", post(create_service))
        .route(
            "/services/{id}",
            get(inspect_service).delete(delete_service),
        )
        .route("/services/{id}/update", post(update_service))
        .route("/services/{id}/logs", get(service_logs))
        // Older Docker clients sometimes send DELETE without an explicit
        // method override, so we register the same path on both `get` and
        // `delete` above. Provide a fallback for the rare client that
        // POSTs to `/services/{id}/delete`.
        .route("/services/{id}/delete", delete(delete_service))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// ID parsing
// ---------------------------------------------------------------------------

/// Split a Docker service id of the form `"{deployment}.{service}"` into its
/// two halves. The first dot is the separator: deployment names are
/// validated by `ZLayer` as DNS-style identifiers (no dots), but service
/// names within a deployment may legally contain further dots, so we keep
/// the right-hand side intact.
///
/// Returns `None` if the input lacks a dot.
fn parse_service_id(id: &str) -> Option<(&str, &str)> {
    let (deployment, service) = id.split_once('.')?;
    if deployment.is_empty() || service.is_empty() {
        return None;
    }
    Some((deployment, service))
}

// ---------------------------------------------------------------------------
// Filter parsing
// ---------------------------------------------------------------------------

/// Parse Docker's URL-encoded JSON filter blob into a multi-valued map.
///
/// Mirrors the helper used by the other swarm submodules: an absent or
/// empty blob yields an empty map. Garbage is rejected with a `BAD_REQUEST`
/// so the caller can surface a clean error to Docker.
fn parse_service_filters(raw: Option<&str>) -> Result<HashMap<String, Vec<String>>, String> {
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

/// Apply Docker's `id` / `name` / `label` / `mode` filters to a list of
/// services.
///
/// Honoured filter keys:
///
/// - `id` / `name` — exact match against `Service.ID` and `Service.Spec.Name`.
///   `ZLayer` uses `"{deployment}.{service}"` for both, so the two filters
///   accept the same shape.
/// - `mode` — `replicated` or `global`, matched against the service's
///   resolved mode.
/// - `label` — exact match against either `key` or `key=value` against the
///   service's labels object. `ZLayer` does not yet propagate Docker labels
///   into the service spec, so the filter rejects every service when set.
///
/// Unknown keys are ignored (Docker's permissive semantics).
fn apply_service_filters(
    services: Vec<Service>,
    filters: &HashMap<String, Vec<String>>,
) -> Vec<Service> {
    if filters.is_empty() {
        return services;
    }
    services
        .into_iter()
        .filter(|s| service_matches_filters(s, filters))
        .collect()
}

fn service_matches_filters(svc: &Service, filters: &HashMap<String, Vec<String>>) -> bool {
    for (key, values) in filters {
        if values.is_empty() {
            continue;
        }
        let matched = match key.as_str() {
            "id" => values.iter().any(|v| v == &svc.id),
            "name" => values.iter().any(|v| v == &svc.spec.name),
            "mode" => {
                let resolved = if svc.spec.mode.replicated.is_some() {
                    "replicated"
                } else if svc.spec.mode.global.is_some() {
                    "global"
                } else {
                    ""
                };
                values.iter().any(|v| v == resolved)
            }
            "label" => values.iter().any(|v| label_matches(&svc.spec.labels, v)),
            // Unknown filter keys are ignored (Docker semantics).
            _ => true,
        };
        if !matched {
            return false;
        }
    }
    true
}

fn label_matches(labels: &Value, spec: &str) -> bool {
    let Some(obj) = labels.as_object() else {
        return false;
    };
    if let Some((k, v)) = spec.split_once('=') {
        obj.get(k).and_then(Value::as_str) == Some(v)
    } else {
        obj.contains_key(spec)
    }
}

// ---------------------------------------------------------------------------
// Daemon error mapping
// ---------------------------------------------------------------------------

/// Map a [`zlayer_client::DaemonClient`] error into a Docker-shape response.
///
/// The daemon returns errors as anyhow strings; we pattern-match on the
/// well-known prefixes used by `DaemonClient::check_status` so 404s and
/// 409s surface with the right status code.
fn map_daemon_error(err: &anyhow::Error) -> Response {
    let msg = format!("{err:#}");
    let status = classify_daemon_error(&msg);
    error_response(status, msg)
}

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
// Stored deployment fetch
// ---------------------------------------------------------------------------

/// Fetch the full stored deployment (including the [`DeploymentSpec`]) for
/// a given name, returning a typed pair on success.
///
/// The daemon's `/api/v1/deployments/{name}/spec` endpoint returns a
/// [`zlayer_types::storage::StoredDeployment`] whose `spec` field is the
/// raw `DeploymentSpec`. We extract just the parts we need to surface as
/// Docker `Service` payloads: the spec itself plus the `created_at` /
/// `updated_at` timestamps for the wire object.
struct StoredDeploymentView {
    spec: DeploymentSpec,
    created_at: String,
    updated_at: String,
}

async fn fetch_stored(state: &SocketState, name: &str) -> Result<StoredDeploymentView, Response> {
    let value = state
        .client
        .get_deployment_stored(name)
        .await
        .map_err(|e| map_daemon_error(&e))?;
    let spec_value = value
        .get("spec")
        .cloned()
        .ok_or_else(|| error_response(StatusCode::INTERNAL_SERVER_ERROR, "missing spec field"))?;
    let spec: DeploymentSpec = serde_json::from_value(spec_value).map_err(|e| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("daemon returned malformed spec: {e}"),
        )
    })?;
    let created_at = value
        .get("created_at")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let updated_at = value
        .get("updated_at")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(StoredDeploymentView {
        spec,
        created_at,
        updated_at,
    })
}

/// Submit a [`DeploymentSpec`] to the daemon by serialising it to YAML and
/// calling `create_deployment`.
async fn submit_deployment(state: &SocketState, spec: &DeploymentSpec) -> Result<(), Response> {
    let yaml = serde_yaml::to_string(spec).map_err(|e| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to serialise deployment spec: {e}"),
        )
    })?;
    state
        .client
        .create_deployment(&yaml)
        .await
        .map_err(|e| map_daemon_error(&e))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers — list / inspect
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ListServicesQuery {
    /// Raw JSON-encoded filter map; see [`parse_service_filters`].
    filters: Option<String>,
    /// Docker also accepts `status=true` to enrich the response with
    /// runtime state. We accept and ignore it: the bridged
    /// `RunningTasks` / `DesiredTasks` fields are best-effort populated
    /// from the spec already and Docker clients tolerate the simpler shape.
    #[allow(dead_code)]
    status: Option<String>,
}

/// `GET /services` — List all swarm services across deployments.
async fn list_services(
    State(state): State<SocketState>,
    Query(query): Query<ListServicesQuery>,
) -> Response {
    let filters = match parse_service_filters(query.filters.as_deref()) {
        Ok(f) => f,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, msg),
    };

    let summaries = match state.client.list_deployments().await {
        Ok(s) => s,
        Err(e) => return map_daemon_error(&e),
    };

    let mut docker: Vec<Service> = Vec::new();
    for entry in summaries {
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        // A deployment may vanish between list and per-name fetch. Skip it
        // rather than fail the whole list — matches stock dockerd's
        // behaviour when a service is removed mid-list.
        let Ok(view) = fetch_stored(&state, name).await else {
            continue;
        };
        for (svc_name, svc_spec) in &view.spec.services {
            docker.push(zlayer_to_docker_service(
                &view.spec.deployment,
                svc_name,
                svc_spec,
                &view.created_at,
                &view.updated_at,
            ));
        }
    }

    let filtered = apply_service_filters(docker, &filters);
    (StatusCode::OK, Json(filtered)).into_response()
}

/// `GET /services/{id}` — Inspect a single service.
async fn inspect_service(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    let Some((deployment, service)) = parse_service_id(&id) else {
        return error_response(StatusCode::NOT_FOUND, format!("service {id} not found"));
    };

    let view = match fetch_stored(&state, deployment).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let Some(svc_spec) = view.spec.services.get(service) else {
        return error_response(StatusCode::NOT_FOUND, format!("service {id} not found"));
    };
    let docker = zlayer_to_docker_service(
        &view.spec.deployment,
        service,
        svc_spec,
        &view.created_at,
        &view.updated_at,
    );
    (StatusCode::OK, Json(docker)).into_response()
}

// ---------------------------------------------------------------------------
// Handlers — create / update / delete
// ---------------------------------------------------------------------------

/// Docker's `ServiceSpec` body for `POST /services/create`. We deserialise
/// only the subset `ZLayer` honours; everything else is captured as
/// `serde_json::Value` so unknown fields don't reject the request.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct DockerServiceSpec {
    name: String,
    #[serde(default)]
    labels: Option<HashMap<String, String>>,
    #[serde(default)]
    task_template: Option<DockerTaskTemplate>,
    #[serde(default)]
    mode: Option<DockerServiceMode>,
    /// Catch-all for fields `ZLayer` doesn't model — `UpdateConfig`,
    /// `RollbackConfig`, `Networks`, `EndpointSpec`, etc. Captured so the
    /// deserialiser doesn't reject Docker's verbose payloads.
    #[serde(flatten)]
    #[allow(dead_code)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct DockerTaskTemplate {
    container_spec: Option<DockerContainerSpec>,
    #[serde(flatten)]
    #[allow(dead_code)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct DockerContainerSpec {
    image: String,
    command: Option<Vec<String>>,
    args: Option<Vec<String>>,
    env: Option<Vec<String>>,
    #[serde(flatten)]
    #[allow(dead_code)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct DockerServiceMode {
    replicated: Option<DockerReplicated>,
    global: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct DockerReplicated {
    replicas: Option<u32>,
}

/// `POST /services/create` — Create a swarm service by translating to a
/// `ZLayer` deployment with one service.
async fn create_service(
    State(state): State<SocketState>,
    Json(body): Json<DockerServiceSpec>,
) -> Response {
    let (deployment_name, service_name, deployment_spec) = match docker_to_zlayer_spec(&body) {
        Ok(triple) => triple,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, msg),
    };

    if let Err(resp) = submit_deployment(&state, &deployment_spec).await {
        return resp;
    }
    let id = format!("{deployment_name}.{service_name}");
    (StatusCode::OK, Json(json!({ "ID": id }))).into_response()
}

/// Query parameters for `POST /services/{id}/update`.
///
/// Docker uses `version` for optimistic concurrency: clients pass back the
/// `Version.Index` they last observed and the daemon refuses the update if
/// the stored index has advanced. `ZLayer` does not yet implement spec
/// versioning, so we accept the parameter and ignore its value — matching
/// the contract in the task spec ("accept and ignore").
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UpdateServiceQuery {
    #[allow(dead_code)]
    version: Option<u64>,
    /// Docker also accepts `registryAuthFrom` and `rollback`; both are
    /// no-ops on `ZLayer` (the daemon manages registry auth itself; there
    /// is no rollback history yet). Accept-and-ignore.
    #[allow(dead_code)]
    registry_auth_from: Option<String>,
    #[allow(dead_code)]
    rollback: Option<String>,
}

/// `POST /services/{id}/update` — Update an existing service.
///
/// Two paths:
///
/// 1. **Replica-only** updates (Docker `Mode.Replicated.Replicas` is the only
///    field that differs from the stored spec) hit `scale_service` directly.
///    This is the hot path for `docker service scale`.
/// 2. Anything else triggers a full re-POST: we fetch the current spec,
///    splice the translated service in place of the existing one, and
///    submit the updated deployment.
async fn update_service(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(_query): Query<UpdateServiceQuery>,
    Json(body): Json<DockerServiceSpec>,
) -> Response {
    let Some((deployment, service)) = parse_service_id(&id) else {
        return error_response(StatusCode::NOT_FOUND, format!("service {id} not found"));
    };

    let view = match fetch_stored(&state, deployment).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let Some(existing) = view.spec.services.get(service) else {
        return error_response(StatusCode::NOT_FOUND, format!("service {id} not found"));
    };

    // Fast path: replicas-only change.
    if let Some(replicas) = replica_only_change(&body, existing) {
        return match state
            .client
            .scale_service(deployment, service, replicas)
            .await
        {
            Ok(_) => StatusCode::OK.into_response(),
            Err(e) => map_daemon_error(&e),
        };
    }

    // Full re-POST: translate the incoming Docker spec into a ZLayer
    // ServiceSpec and splice it into the stored deployment, replacing the
    // existing service in place.
    let translated = match translate_service_only(&body) {
        Ok(s) => s,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, msg),
    };
    let mut new_spec = view.spec;
    new_spec.services.insert(service.to_string(), translated);
    if let Err(resp) = submit_deployment(&state, &new_spec).await {
        return resp;
    }
    StatusCode::OK.into_response()
}

/// Detect a Docker update body whose only material change is the desired
/// replica count and return the new value, or `None` if the body asks for
/// any other change.
fn replica_only_change(body: &DockerServiceSpec, existing: &ZServiceSpec) -> Option<u32> {
    // The body must specify Mode.Replicated.Replicas and nothing else
    // ZLayer cares about. We inspect the only fields we can map back to
    // existing service state: image, command, args, env. If any of those
    // is present and different, this is not a replica-only update.
    if let Some(tt) = &body.task_template {
        if let Some(cs) = &tt.container_spec {
            // Compare on the parsed canonical form so a user-supplied
            // short name ("nginx") matches an existing reference stored
            // verbatim. `ImageRef` preserves the original string in
            // `to_string()`, so we must reach through to `.parsed()` here.
            // Unparseable images count as a change — the daemon would
            // reject them anyway.
            if !cs.image.is_empty() {
                let existing_canonical = existing.image.name.parsed().to_string();
                let incoming_canonical = cs
                    .image
                    .parse::<zlayer_types::ImageReference>()
                    .ok()
                    .map(|r| r.to_string());
                if incoming_canonical.as_deref() != Some(existing_canonical.as_str()) {
                    return None;
                }
            }
            if let Some(cmd) = &cs.command {
                let existing_cmd: Vec<String> =
                    existing.command.entrypoint.clone().unwrap_or_default();
                if cmd != &existing_cmd {
                    return None;
                }
            }
            if let Some(args) = &cs.args {
                let existing_args: Vec<String> = existing.command.args.clone().unwrap_or_default();
                if args != &existing_args {
                    return None;
                }
            }
            if let Some(env) = &cs.env {
                let mut existing_env: Vec<String> = existing
                    .env
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                let mut got = env.clone();
                existing_env.sort();
                got.sort();
                if got != existing_env {
                    return None;
                }
            }
        }
    }

    let mode = body.mode.as_ref()?;
    let replicated = mode.replicated.as_ref()?;
    replicated.replicas
}

/// `DELETE /services/{id}` — Remove a service.
///
/// If this is the only service in its deployment, undeploy the entire
/// deployment so the user does not end up with a `Pending` empty record.
/// Otherwise re-POST the deployment with the service removed from the
/// `services` map.
async fn delete_service(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    let Some((deployment, service)) = parse_service_id(&id) else {
        return error_response(StatusCode::NOT_FOUND, format!("service {id} not found"));
    };

    let view = match fetch_stored(&state, deployment).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    if !view.spec.services.contains_key(service) {
        return error_response(StatusCode::NOT_FOUND, format!("service {id} not found"));
    }

    if view.spec.services.len() <= 1 {
        return match state.client.delete_deployment(deployment).await {
            Ok(()) => StatusCode::OK.into_response(),
            Err(e) => map_daemon_error(&e),
        };
    }

    let mut new_spec = view.spec;
    new_spec.services.remove(service);
    if let Err(resp) = submit_deployment(&state, &new_spec).await {
        return resp;
    }
    StatusCode::OK.into_response()
}

// ---------------------------------------------------------------------------
// Handlers — logs
// ---------------------------------------------------------------------------

/// Query parameters for `GET /services/{id}/logs`.
///
/// Mirrors the container `/logs` query: Docker accepts permissive boolean
/// spellings (`1`/`0`/`true`/`false`) for `stdout`, `stderr`, `follow`,
/// `timestamps`, and a numeric or `all` `tail`. We ignore `since` / `until`
/// because the daemon's service logs endpoint does not yet support a
/// time-range filter.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LogsQuery {
    follow: Option<String>,
    stdout: Option<String>,
    stderr: Option<String>,
    #[allow(dead_code)]
    since: Option<String>,
    #[allow(dead_code)]
    until: Option<String>,
    #[allow(dead_code)]
    timestamps: Option<String>,
    tail: Option<String>,
    /// Optional replica/instance pin (`?instance=0`). Honoured by passing
    /// straight to the daemon's per-service logs query.
    #[serde(alias = "instance")]
    details: Option<String>,
}

fn parse_bool(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "true" | "True" | "TRUE"))
}

/// `GET /services/{id}/logs` — Stream service logs.
///
/// The daemon's per-service log endpoint returns plain text (one line per
/// log entry). We frame each chunk with Docker's stdcopy multiplex header
/// so clients that decode the `application/vnd.docker.raw-stream` content
/// type continue to work. Lines from the daemon all originate from the
/// container's stdout — `ZLayer` does not split stdout/stderr at the API
/// boundary today — so when only `stderr=1` is requested with no `stdout`,
/// we still emit on stdout.
async fn service_logs(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Query(q): Query<LogsQuery>,
) -> Response {
    let Some((deployment, service)) = parse_service_id(&id) else {
        return error_response(StatusCode::NOT_FOUND, format!("service {id} not found"));
    };

    // Default tail mirrors `docker service logs`: when no value is given
    // the daemon returns all available lines (`tail=all` -> u32::MAX).
    let lines = match q.tail.as_deref() {
        Some("all") | None => 1_000,
        Some(n) => n.parse::<u32>().unwrap_or(1_000),
    };
    let follow = parse_bool(q.follow.as_deref());
    let want_stdout = parse_bool(q.stdout.as_deref());
    let want_stderr = parse_bool(q.stderr.as_deref());
    // Docker's contract: when both flags are unset the daemon emits both
    // streams. Otherwise the unset side defaults to false.
    let (emit_stdout, _emit_stderr) = match (q.stdout.as_deref(), q.stderr.as_deref()) {
        (None, None) => (true, true),
        _ => (want_stdout, want_stderr),
    };

    let body_bytes = match state
        .client
        .get_logs_with_instance(deployment, service, lines, follow, q.details.as_deref())
        .await
    {
        Ok(s) => s,
        Err(e) => return map_daemon_error(&e),
    };

    // Pick the channel to frame on. ZLayer does not yet split stdout/stderr,
    // so all lines flow on the single channel the caller asked for; if both
    // were requested we use stdout, which is what stock `dockerd` does for
    // services that don't classify their output.
    let stream_kind = if emit_stdout {
        LogStream::Stdout
    } else {
        LogStream::Stderr
    };

    let chunks: Vec<Result<Bytes, std::io::Error>> = body_bytes
        .lines()
        .map(|line| {
            let mut framed = line.to_owned();
            framed.push('\n');
            Ok::<Bytes, std::io::Error>(Bytes::from(framed))
        })
        .collect();

    // Multiplex through the shared helper so the wire format matches
    // `/containers/{id}/logs`. We feed everything down a single channel
    // (the one the caller requested); the other side is empty.
    let (stdout_iter, stderr_iter): (FrameVec, FrameVec) =
        if matches!(stream_kind, LogStream::Stdout) {
            (chunks, Vec::new())
        } else {
            (Vec::new(), chunks)
        };
    let body_stream = multiplex(stream::iter(stdout_iter), stream::iter(stderr_iter));
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
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("body build: {e}"),
            )
        },
        IntoResponse::into_response,
    )
}

// ---------------------------------------------------------------------------
// Translation: Docker ServiceSpec -> ZLayer DeploymentSpec
// ---------------------------------------------------------------------------

/// Translate a Docker [`DockerServiceSpec`] into a `ZLayer`
/// [`DeploymentSpec`] containing exactly one service. The deployment's
/// `name` and the inner service's name are both set to Docker's
/// `Spec.Name` — Docker swarm services are flat (no parent grouping), so
/// the bridge collapses both halves of the `(deployment, service)` pair
/// onto the same string.
///
/// Returns `(deployment_name, service_name, spec)` so the caller can build
/// the `"{deployment}.{service}"` ID without re-deriving it.
fn docker_to_zlayer_spec(
    body: &DockerServiceSpec,
) -> Result<(String, String, DeploymentSpec), String> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err("service name is required".to_string());
    }
    let zlayer_service = translate_service_only(body)?;

    let mut services = HashMap::new();
    services.insert(name.to_string(), zlayer_service);

    let deployment = DeploymentSpec {
        version: "v1".to_string(),
        deployment: name.to_string(),
        services,
        externals: HashMap::new(),
        tunnels: HashMap::new(),
        api: zlayer_spec::ApiSpec::default(),
    };
    Ok((name.to_string(), name.to_string(), deployment))
}

/// Translate just the Docker `ServiceSpec` body into a `ZLayer`
/// [`ZServiceSpec`]. Used by both create (which then wraps it in a fresh
/// `DeploymentSpec`) and update (which splices it back into the stored
/// deployment).
///
/// Length is dominated by the `ZServiceSpec` struct-literal at the bottom:
/// every field has to be set explicitly because `ZServiceSpec` does not
/// implement `Default` (the type has too many tunables to pick safe
/// defaults for). Allow `too_many_lines` rather than split the literal
/// into a helper that would just push the same boilerplate around.
#[allow(clippy::too_many_lines)]
fn translate_service_only(body: &DockerServiceSpec) -> Result<ZServiceSpec, String> {
    let container = body
        .task_template
        .as_ref()
        .and_then(|t| t.container_spec.as_ref());
    let image_str = container.map_or("", |c| c.image.as_str()).trim();
    if image_str.is_empty() {
        return Err("Spec.TaskTemplate.ContainerSpec.Image is required".to_string());
    }
    let image_ref: zlayer_types::ImageRef = image_str
        .parse()
        .map_err(|e| format!("invalid image reference '{image_str}': {e}"))?;

    let scale = match &body.mode {
        Some(mode) => {
            if mode.global.is_some() && !mode.global.as_ref().unwrap().is_null() {
                // Global mode: one container per agent. ZLayer's nearest
                // analogue is `NodeMode::Dedicated`, but that lives on the
                // service spec rather than the scale enum. Encode "one per
                // node" as Fixed(1) and let the node-mode dedicated flag
                // (set further down) carry the placement intent.
                ScaleSpec::Fixed { replicas: 1 }
            } else if let Some(replicated) = &mode.replicated {
                ScaleSpec::Fixed {
                    replicas: replicated.replicas.unwrap_or(1),
                }
            } else {
                ScaleSpec::default()
            }
        }
        None => ScaleSpec::default(),
    };

    // Map Docker's `command` (entrypoint override) and `args` (cmd override)
    // onto ZLayer's CommandSpec.
    let entrypoint = container
        .and_then(|c| c.command.clone())
        .filter(|v| !v.is_empty());
    let args = container
        .and_then(|c| c.args.clone())
        .filter(|v| !v.is_empty());
    let command = CommandSpec {
        entrypoint,
        args,
        workdir: None,
    };

    let env = container
        .and_then(|c| c.env.clone())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect();

    let labels = body.labels.clone().unwrap_or_default();

    let node_mode = if let Some(mode) = &body.mode {
        if mode.global.is_some() && !mode.global.as_ref().unwrap().is_null() {
            NodeMode::Dedicated
        } else {
            NodeMode::default()
        }
    } else {
        NodeMode::default()
    };

    Ok(ZServiceSpec {
        rtype: zlayer_spec::ResourceType::Service,
        schedule: None,
        image: ImageSpec {
            name: image_ref,
            pull_policy: PullPolicy::IfNotPresent,
            source_policy: None,
        },
        resources: zlayer_spec::ResourcesSpec::default(),
        env,
        command,
        network: zlayer_spec::ServiceNetworkSpec::default(),
        endpoints: Vec::new(),
        scale,
        depends: Vec::new(),
        health: zlayer_spec::HealthSpec {
            start_grace: Some(std::time::Duration::from_secs(5)),
            interval: None,
            timeout: None,
            retries: 3,
            check: zlayer_spec::HealthCheck::Tcp { port: 0 },
        },
        init: zlayer_spec::InitSpec::default(),
        errors: zlayer_spec::ErrorsSpec::default(),
        lifecycle: zlayer_spec::LifecycleSpec::default(),
        devices: Vec::new(),
        storage: Vec::new(),
        port_mappings: Vec::new(),
        capabilities: Vec::new(),
        cap_drop: Vec::new(),
        privileged: false,
        node_mode,
        node_selector: None,
        affinity: None,
        platform: None,
        service_type: zlayer_spec::ServiceType::default(),
        wasm: None,
        logs: None,
        host_network: false,
        hostname: None,
        dns: Vec::new(),
        extra_hosts: Vec::new(),
        restart_policy: None,
        labels,
        user: None,
        stop_signal: None,
        stop_grace_period: None,
        sysctls: HashMap::new(),
        ulimits: HashMap::new(),
        security_opt: Vec::new(),
        pid_mode: None,
        ipc_mode: None,
        network_mode: zlayer_spec::NetworkMode::default(),
        extra_groups: Vec::new(),
        read_only_root_fs: false,
        init_container: None,
        tty: false,
        stdin_open: false,
        userns_mode: None,
        cgroup_parent: None,
        expose: Vec::new(),
        replica_groups: None,
        isolation: None,
        overlay: None,
        localhost_reachability: zlayer_spec::LocalhostReachability::default(),
    })
}

// ---------------------------------------------------------------------------
// Translation: ZLayer DeploymentSpec/ServiceSpec -> Docker Service
// ---------------------------------------------------------------------------

/// Build a Docker [`Service`] from a `(deployment, service_name,
/// service_spec)` triple plus the deployment's stored timestamps.
///
/// The wire contract:
///
/// - `Service.ID` is `"{deployment}.{service}"`.
/// - `Service.Spec.Name` mirrors `Service.ID` so `docker service ls` shows
///   the same string in both columns.
/// - `Service.Spec.Mode` is `Replicated{Replicas: N}` for fixed / adaptive
///   scaling and `Global` when the underlying spec uses
///   `NodeMode::Dedicated` (mapped from Docker `Global` on the way in).
/// - `Service.CreatedAt` / `UpdatedAt` reuse the deployment's timestamps —
///   `ZLayer` does not track a per-service `created_at` today.
/// - `Service.Endpoint` and `Service.UpdateStatus` are emitted as empty /
///   null because the underlying primitives do not exist yet.
fn zlayer_to_docker_service(
    deployment: &str,
    service_name: &str,
    spec: &ZServiceSpec,
    created_at: &str,
    updated_at: &str,
) -> Service {
    let id = format!("{deployment}.{service_name}");

    let (mode, replicas_for_global): (ServiceMode, Option<u32>) =
        match (&spec.scale, spec.node_mode) {
            // Global semantics: a daemon-set / dedicated-per-node placement.
            (_, NodeMode::Dedicated) => (
                ServiceMode {
                    replicated: None,
                    global: Some(Value::Object(serde_json::Map::new())),
                },
                Some(1),
            ),
            (ScaleSpec::Fixed { replicas }, _) => (
                ServiceMode {
                    replicated: Some(Replicated {
                        replicas: u64::from(*replicas),
                    }),
                    global: None,
                },
                None,
            ),
            (ScaleSpec::Adaptive { min, .. }, _) => (
                ServiceMode {
                    replicated: Some(Replicated {
                        replicas: u64::from(*min),
                    }),
                    global: None,
                },
                None,
            ),
            (ScaleSpec::Manual, _) => (
                ServiceMode {
                    replicated: Some(Replicated { replicas: 0 }),
                    global: None,
                },
                None,
            ),
        };
    let _ = replicas_for_global; // intentionally unused beyond the match

    let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let command = spec.command.entrypoint.clone().unwrap_or_default();
    let args = spec.command.args.clone().unwrap_or_default();

    let labels_value = serde_json::to_value(&spec.labels)
        .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));

    Service {
        id: id.clone(),
        version: Version::default(),
        created_at: if created_at.is_empty() {
            iso_timestamp(0)
        } else {
            created_at.to_string()
        },
        updated_at: if updated_at.is_empty() {
            iso_timestamp(0)
        } else {
            updated_at.to_string()
        },
        spec: ServiceSpec {
            name: id,
            labels: labels_value,
            task_template: TaskTemplate {
                container_spec: ContainerSpec {
                    image: format!("{}", spec.image.name),
                    labels: Value::Object(serde_json::Map::new()),
                    command,
                    args,
                    env,
                    mounts: Value::Array(Vec::new()),
                },
                resources: Value::Object(serde_json::Map::new()),
                restart_policy: Value::Object(serde_json::Map::new()),
                placement: Value::Object(serde_json::Map::new()),
                networks: Value::Array(Vec::new()),
                log_driver: Value::Object(serde_json::Map::new()),
                force_update: 0,
                runtime: "container".to_string(),
            },
            mode,
            update_config: Value::Object(serde_json::Map::new()),
            rollback_config: Value::Object(serde_json::Map::new()),
            networks: Value::Array(Vec::new()),
            endpoint_spec: Value::Object(serde_json::Map::new()),
        },
        previous_spec: None,
        endpoint: Value::Object(serde_json::Map::new()),
        update_status: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_zservice(image: &str) -> ZServiceSpec {
        ZServiceSpec {
            rtype: zlayer_spec::ResourceType::Service,
            schedule: None,
            image: ImageSpec {
                name: image.parse().expect("valid image"),
                pull_policy: PullPolicy::IfNotPresent,
                source_policy: None,
            },
            resources: zlayer_spec::ResourcesSpec::default(),
            env: HashMap::from([
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "qux".to_string()),
            ]),
            command: CommandSpec {
                entrypoint: Some(vec!["/bin/sh".to_string()]),
                args: Some(vec!["-c".to_string(), "echo hi".to_string()]),
                workdir: None,
            },
            network: zlayer_spec::ServiceNetworkSpec::default(),
            endpoints: Vec::new(),
            scale: ScaleSpec::Fixed { replicas: 3 },
            replica_groups: None,
            depends: Vec::new(),
            health: zlayer_spec::HealthSpec {
                start_grace: None,
                interval: None,
                timeout: None,
                retries: 3,
                check: zlayer_spec::HealthCheck::Tcp { port: 0 },
            },
            init: zlayer_spec::InitSpec::default(),
            errors: zlayer_spec::ErrorsSpec::default(),
            lifecycle: zlayer_spec::LifecycleSpec::default(),
            devices: Vec::new(),
            storage: Vec::new(),
            port_mappings: Vec::new(),
            capabilities: Vec::new(),
            cap_drop: Vec::new(),
            privileged: false,
            node_mode: NodeMode::default(),
            node_selector: None,
            affinity: None,
            platform: None,
            service_type: zlayer_spec::ServiceType::default(),
            wasm: None,
            logs: None,
            host_network: false,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: None,
            labels: HashMap::new(),
            user: None,
            stop_signal: None,
            stop_grace_period: None,
            sysctls: HashMap::new(),
            ulimits: HashMap::new(),
            security_opt: Vec::new(),
            pid_mode: None,
            ipc_mode: None,
            network_mode: zlayer_spec::NetworkMode::default(),
            extra_groups: Vec::new(),
            read_only_root_fs: false,
            init_container: None,
            tty: false,
            stdin_open: false,
            userns_mode: None,
            cgroup_parent: None,
            expose: Vec::new(),
            isolation: None,
            overlay: None,
            localhost_reachability: zlayer_spec::LocalhostReachability::default(),
        }
    }

    #[test]
    fn parse_service_id_splits_on_first_dot() {
        assert_eq!(parse_service_id("dep.svc"), Some(("dep", "svc")));
        // Service names may contain dots; deployment names may not, so the
        // first dot is always the separator.
        assert_eq!(
            parse_service_id("dep.svc.with.dots"),
            Some(("dep", "svc.with.dots"))
        );
        assert!(parse_service_id("nodot").is_none());
        assert!(parse_service_id(".missing-dep").is_none());
        assert!(parse_service_id("missing-svc.").is_none());
    }

    #[test]
    fn parse_service_filters_handles_well_formed_json() {
        let out =
            parse_service_filters(Some(r#"{"name":["api"],"mode":["replicated"]}"#)).expect("ok");
        assert_eq!(
            out.get("name").map(Vec::as_slice),
            Some(&["api".to_string()][..])
        );
        assert_eq!(
            out.get("mode").map(Vec::as_slice),
            Some(&["replicated".to_string()][..])
        );
    }

    #[test]
    fn parse_service_filters_empty_input_is_ok() {
        assert!(parse_service_filters(None).expect("ok").is_empty());
        assert!(parse_service_filters(Some("")).expect("ok").is_empty());
        assert!(parse_service_filters(Some("   ")).expect("ok").is_empty());
    }

    #[test]
    fn parse_service_filters_rejects_garbage() {
        assert!(parse_service_filters(Some("not-json")).is_err());
        assert!(parse_service_filters(Some(r#"["array"]"#)).is_err());
    }

    #[test]
    fn apply_filters_id_and_name_match_same_string() {
        let zsvc = fixture_zservice("nginx:latest");
        let docker = zlayer_to_docker_service("dep", "svc", &zsvc, "", "");
        let services = vec![docker];

        let mut filters = HashMap::new();
        filters.insert("id".to_string(), vec!["dep.svc".to_string()]);
        assert_eq!(apply_service_filters(services.clone(), &filters).len(), 1);

        filters.clear();
        filters.insert("name".to_string(), vec!["dep.svc".to_string()]);
        assert_eq!(apply_service_filters(services.clone(), &filters).len(), 1);

        filters.clear();
        filters.insert("name".to_string(), vec!["other".to_string()]);
        assert!(apply_service_filters(services, &filters).is_empty());
    }

    #[test]
    fn apply_filters_mode_distinguishes_replicated_global() {
        let mut zsvc = fixture_zservice("nginx:latest");
        zsvc.scale = ScaleSpec::Fixed { replicas: 2 };
        let replicated = zlayer_to_docker_service("dep", "svc", &zsvc, "", "");

        zsvc.node_mode = NodeMode::Dedicated;
        let global = zlayer_to_docker_service("dep", "global-svc", &zsvc, "", "");

        let services = vec![replicated, global];

        let mut filters = HashMap::new();
        filters.insert("mode".to_string(), vec!["replicated".to_string()]);
        let kept = apply_service_filters(services.clone(), &filters);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "dep.svc");

        filters.insert("mode".to_string(), vec!["global".to_string()]);
        let kept = apply_service_filters(services, &filters);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "dep.global-svc");
    }

    #[test]
    fn translate_service_only_requires_image() {
        let body = DockerServiceSpec {
            name: "svc".to_string(),
            ..Default::default()
        };
        let err = translate_service_only(&body).unwrap_err();
        assert!(err.contains("Image"));
    }

    #[test]
    fn translate_service_only_maps_image_command_args_env() {
        let body = DockerServiceSpec {
            name: "svc".to_string(),
            task_template: Some(DockerTaskTemplate {
                container_spec: Some(DockerContainerSpec {
                    image: "nginx:1.25".to_string(),
                    command: Some(vec!["/usr/bin/nginx".to_string()]),
                    args: Some(vec!["-g".to_string(), "daemon off;".to_string()]),
                    env: Some(vec!["FOO=bar".to_string(), "BAZ=qux".to_string()]),
                    extra: HashMap::new(),
                }),
                extra: HashMap::new(),
            }),
            mode: Some(DockerServiceMode {
                replicated: Some(DockerReplicated { replicas: Some(4) }),
                global: None,
            }),
            ..Default::default()
        };
        let zspec = translate_service_only(&body).expect("translate");
        // `ImageRef` preserves the user's original string verbatim; the
        // parsed canonical form lives behind `.parsed()` if needed.
        assert_eq!(format!("{}", zspec.image.name), "nginx:1.25");
        assert_eq!(
            zspec.command.entrypoint,
            Some(vec!["/usr/bin/nginx".to_string()])
        );
        assert_eq!(
            zspec.command.args,
            Some(vec!["-g".to_string(), "daemon off;".to_string()])
        );
        assert_eq!(zspec.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(zspec.env.get("BAZ").map(String::as_str), Some("qux"));
        assert!(matches!(zspec.scale, ScaleSpec::Fixed { replicas: 4 }));
        assert_eq!(zspec.node_mode, NodeMode::Shared);
    }

    #[test]
    fn translate_service_only_global_mode_uses_dedicated_node_mode() {
        let body = DockerServiceSpec {
            name: "svc".to_string(),
            task_template: Some(DockerTaskTemplate {
                container_spec: Some(DockerContainerSpec {
                    image: "redis:7".to_string(),
                    ..Default::default()
                }),
                extra: HashMap::new(),
            }),
            mode: Some(DockerServiceMode {
                replicated: None,
                global: Some(json!({})),
            }),
            ..Default::default()
        };
        let zspec = translate_service_only(&body).expect("translate");
        assert_eq!(zspec.node_mode, NodeMode::Dedicated);
        assert!(matches!(zspec.scale, ScaleSpec::Fixed { replicas: 1 }));
    }

    #[test]
    fn docker_to_zlayer_spec_wraps_in_single_service_deployment() {
        let body = DockerServiceSpec {
            name: "api".to_string(),
            task_template: Some(DockerTaskTemplate {
                container_spec: Some(DockerContainerSpec {
                    image: "my/api:latest".to_string(),
                    ..Default::default()
                }),
                extra: HashMap::new(),
            }),
            ..Default::default()
        };
        let (dep, svc, spec) = docker_to_zlayer_spec(&body).expect("translate");
        assert_eq!(dep, "api");
        assert_eq!(svc, "api");
        assert_eq!(spec.deployment, "api");
        assert_eq!(spec.services.len(), 1);
        assert!(spec.services.contains_key("api"));
    }

    #[test]
    fn zlayer_to_docker_service_builds_replicated_mode() {
        let zsvc = fixture_zservice("nginx:latest");
        let docker = zlayer_to_docker_service("dep", "svc", &zsvc, "", "");
        assert_eq!(docker.id, "dep.svc");
        assert_eq!(docker.spec.name, "dep.svc");
        let replicated = docker.spec.mode.replicated.expect("replicated mode");
        assert_eq!(replicated.replicas, 3);
        assert!(docker.spec.mode.global.is_none());
        // `ImageRef` preserves the user's original string verbatim across the
        // ZLayer<->Docker round trip; no silent `docker.io/library/` prefix.
        assert_eq!(
            docker.spec.task_template.container_spec.image,
            "nginx:latest"
        );
        assert_eq!(
            docker.spec.task_template.container_spec.command,
            vec!["/bin/sh"]
        );
        // env survives round-trip (order independent).
        let mut got = docker.spec.task_template.container_spec.env.clone();
        got.sort();
        assert_eq!(got, vec!["BAZ=qux".to_string(), "FOO=bar".to_string()]);
    }

    #[test]
    fn zlayer_to_docker_service_builds_global_mode_when_dedicated() {
        let mut zsvc = fixture_zservice("redis:7");
        zsvc.node_mode = NodeMode::Dedicated;
        let docker = zlayer_to_docker_service("dep", "global-svc", &zsvc, "", "");
        assert!(docker.spec.mode.global.is_some());
        assert!(docker.spec.mode.replicated.is_none());
    }

    #[test]
    fn zlayer_to_docker_service_handles_manual_scale() {
        let mut zsvc = fixture_zservice("nginx:latest");
        zsvc.scale = ScaleSpec::Manual;
        let docker = zlayer_to_docker_service("dep", "svc", &zsvc, "", "");
        let replicated = docker.spec.mode.replicated.expect("replicated mode");
        assert_eq!(replicated.replicas, 0);
    }

    #[test]
    fn zlayer_to_docker_service_uses_adaptive_min_for_replicas() {
        let mut zsvc = fixture_zservice("nginx:latest");
        zsvc.scale = ScaleSpec::Adaptive {
            min: 2,
            max: 10,
            cooldown: None,
            targets: zlayer_spec::ScaleTargets::default(),
        };
        let docker = zlayer_to_docker_service("dep", "svc", &zsvc, "", "");
        let replicated = docker.spec.mode.replicated.expect("replicated mode");
        assert_eq!(replicated.replicas, 2);
    }

    #[test]
    fn replica_only_change_detects_pure_replica_update() {
        let zsvc = fixture_zservice("nginx:latest");
        let body = DockerServiceSpec {
            name: "svc".to_string(),
            task_template: Some(DockerTaskTemplate {
                container_spec: Some(DockerContainerSpec {
                    image: "nginx:latest".to_string(),
                    command: Some(vec!["/bin/sh".to_string()]),
                    args: Some(vec!["-c".to_string(), "echo hi".to_string()]),
                    env: Some(vec!["FOO=bar".to_string(), "BAZ=qux".to_string()]),
                    extra: HashMap::new(),
                }),
                extra: HashMap::new(),
            }),
            mode: Some(DockerServiceMode {
                replicated: Some(DockerReplicated { replicas: Some(7) }),
                global: None,
            }),
            ..Default::default()
        };
        assert_eq!(replica_only_change(&body, &zsvc), Some(7));
    }

    #[test]
    fn replica_only_change_rejects_image_drift() {
        let zsvc = fixture_zservice("nginx:latest");
        let body = DockerServiceSpec {
            name: "svc".to_string(),
            task_template: Some(DockerTaskTemplate {
                container_spec: Some(DockerContainerSpec {
                    image: "nginx:1.27".to_string(),
                    ..Default::default()
                }),
                extra: HashMap::new(),
            }),
            mode: Some(DockerServiceMode {
                replicated: Some(DockerReplicated { replicas: Some(2) }),
                global: None,
            }),
            ..Default::default()
        };
        assert!(replica_only_change(&body, &zsvc).is_none());
    }

    #[test]
    fn classify_daemon_error_maps_status_codes() {
        assert_eq!(
            classify_daemon_error("404 Not Found: deployment 'x' missing"),
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
}
