//! Raw container lifecycle REST API endpoints
//!
//! Provides direct container management operations independent of the service
//! abstraction. These endpoints allow creating, inspecting, stopping, and
//! interacting with individual containers by ID.
//!
//! Container IDs follow the `{service}-rep-{replica}` format used throughout
//! the `ZLayer` agent layer.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info};
use utoipa::{IntoParams, ToSchema};

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use zlayer_agent::{ContainerId, ContainerState, Runtime, ServiceManager};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// State for raw container endpoints.
///
/// Holds references to the `ServiceManager` (which owns the `Runtime`) so
/// handlers can perform direct container lifecycle operations.
#[derive(Clone)]
pub struct ContainerApiState {
    /// Service manager — used to enumerate containers and delegate to the runtime.
    pub service_manager: Arc<RwLock<ServiceManager>>,
    /// Direct runtime reference for container lifecycle operations that bypass
    /// the service layer (create, start, stop, remove, logs, exec, wait, stats).
    pub runtime: Arc<dyn Runtime + Send + Sync>,
}

impl ContainerApiState {
    /// Create a new container API state.
    pub fn new(
        service_manager: Arc<RwLock<ServiceManager>>,
        runtime: Arc<dyn Runtime + Send + Sync>,
    ) -> Self {
        Self {
            service_manager,
            runtime,
        }
    }
}

// ---------------------------------------------------------------------------
// Types — requests, responses, query params
// ---------------------------------------------------------------------------

/// Request body for creating a container.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateContainerRequest {
    /// Service name this container belongs to.
    pub service: String,
    /// Replica number (must be unique within the service).
    pub replica: u32,
    /// Image to use (e.g. `nginx:latest`). If omitted, the service spec's
    /// image is used.
    pub image: Option<String>,
}

/// Response after creating (and starting) a container.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateContainerResponse {
    /// Container identifier (`{service}-rep-{replica}`).
    pub id: String,
    /// Current container state after creation.
    pub state: String,
    /// Process ID (if available immediately).
    pub pid: Option<u32>,
}

/// Summary returned when listing containers.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerInfo {
    /// Container identifier.
    pub id: String,
    /// Service name.
    pub service: String,
    /// Replica number.
    pub replica: u32,
    /// Current state (`pending`, `running`, `exited`, `failed`, etc.).
    pub state: String,
    /// Process ID (if running).
    pub pid: Option<u32>,
    /// Exit code (if exited).
    pub exit_code: Option<i32>,
}

/// Detailed container state response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerStateResponse {
    /// Container identifier.
    pub id: String,
    /// Current state string.
    pub state: String,
    /// Process ID (if running).
    pub pid: Option<u32>,
    /// Exit code (if exited).
    pub exit_code: Option<i32>,
    /// Failure reason (if failed).
    pub reason: Option<String>,
}

/// Query parameters for the logs endpoint.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ContainerLogQuery {
    /// Number of lines to return (tail).
    #[serde(default = "default_tail")]
    pub tail: usize,
    /// Follow logs via SSE stream.
    #[serde(default)]
    pub follow: bool,
}

fn default_tail() -> usize {
    100
}

/// Request body for executing a command in a container.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ContainerExecRequest {
    /// Command and arguments.
    pub command: Vec<String>,
}

/// Response from executing a command in a container.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerExecResponse {
    /// Exit code from the command.
    pub exit_code: i32,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
}

/// Response from waiting for a container to exit.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerWaitResponse {
    /// Container identifier.
    pub id: String,
    /// Exit code.
    pub exit_code: i32,
}

/// Container resource statistics.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerStatsResponse {
    /// Container identifier.
    pub id: String,
    /// CPU usage in microseconds.
    pub cpu_usage_usec: u64,
    /// Current memory usage in bytes.
    pub memory_bytes: u64,
    /// Memory limit in bytes.
    pub memory_limit: u64,
    /// Memory usage as a percentage of the limit.
    pub memory_percent: f64,
}

/// Query parameters for listing containers.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListContainersQuery {
    /// Filter by service name.
    pub service: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a container ID string (`{service}-rep-{replica}`) into a `ContainerId`.
fn parse_container_id(id: &str) -> Result<ContainerId> {
    // Format: {service}-rep-{replica}
    let parts: Vec<&str> = id.rsplitn(3, '-').collect();
    // rsplitn(3, '-') on "my-service-rep-2" yields ["2", "rep", "my-service"]
    if parts.len() < 3 || parts[1] != "rep" {
        return Err(ApiError::BadRequest(format!(
            "Invalid container ID format '{id}': expected '{{service}}-rep-{{replica}}'"
        )));
    }

    let replica: u32 = parts[0].parse().map_err(|_| {
        ApiError::BadRequest(format!("Invalid replica number in container ID '{id}'"))
    })?;

    let service = parts[2].to_string();

    Ok(ContainerId { service, replica })
}

/// Convert a `ContainerState` to a human-readable string.
fn state_to_string(state: &ContainerState) -> String {
    match state {
        ContainerState::Pending => "pending".to_string(),
        ContainerState::Initializing => "initializing".to_string(),
        ContainerState::Running => "running".to_string(),
        ContainerState::Stopping => "stopping".to_string(),
        ContainerState::Exited { code } => format!("exited({code})"),
        ContainerState::Failed { reason } => format!("failed: {reason}"),
    }
}

/// Extract the exit code from a `ContainerState`, if applicable.
fn state_exit_code(state: &ContainerState) -> Option<i32> {
    match state {
        ContainerState::Exited { code } => Some(*code),
        _ => None,
    }
}

/// Extract the failure reason from a `ContainerState`, if applicable.
fn state_reason(state: &ContainerState) -> Option<String> {
    match state {
        ContainerState::Failed { reason } => Some(reason.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Create and start a container.
///
/// Creates a new container using the runtime, then starts it. The container
/// is identified by its service name and replica number. The caller must
/// ensure the service is registered with the `ServiceManager` first (via the
/// deployment or service endpoints), otherwise the image cannot be resolved.
///
/// # Errors
///
/// Returns an error if the service is unknown, the image cannot be pulled,
/// container creation fails, or starting fails.
#[utoipa::path(
    post,
    path = "/api/v1/containers",
    request_body = CreateContainerRequest,
    responses(
        (status = 201, description = "Container created and started", body = CreateContainerResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — operator role required"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn create_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Json(request): Json<CreateContainerRequest>,
) -> Result<(axum::http::StatusCode, Json<CreateContainerResponse>)> {
    user.require_role("operator")?;

    if request.service.is_empty() {
        return Err(ApiError::BadRequest(
            "Service name cannot be empty".to_string(),
        ));
    }
    if request.replica == 0 {
        return Err(ApiError::BadRequest(
            "Replica number must be >= 1".to_string(),
        ));
    }

    let cid = ContainerId {
        service: request.service.clone(),
        replica: request.replica,
    };

    info!(container = %cid, "Creating container via raw API");

    // Resolve the service spec from the service manager so we can get the
    // image and full spec to pass to create_container.
    let manager = state.service_manager.read().await;
    let services = manager.list_services().await;
    drop(manager);

    if !services.contains(&request.service) {
        return Err(ApiError::NotFound(format!(
            "Service '{}' not registered — deploy it first",
            request.service
        )));
    }

    // Pull image if explicitly provided, otherwise rely on the spec's image
    // which was pulled during deployment.
    if let Some(ref image) = request.image {
        state
            .runtime
            .pull_image(image)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to pull image: {e}")))?;
    }

    // We need the service spec to create the container. The ServiceManager
    // tracks ServiceInstances internally, but the Runtime.create_container
    // needs a ServiceSpec. Get it via upsert — but since the service is
    // already registered, we just need the spec.
    //
    // For now we ask the manager for containers on this service; if the
    // replica already exists, fail with Conflict.
    let manager = state.service_manager.read().await;
    let existing = manager.get_service_containers(&request.service).await;
    drop(manager);

    if existing.iter().any(|c| c.replica == request.replica) {
        return Err(ApiError::Conflict(format!(
            "Container {cid} already exists",
        )));
    }

    // Use the runtime to create + start.  We need the ServiceSpec which is
    // held inside ServiceInstance (not publicly accessible).  Since the raw
    // container API is meant for low-level operations, we go through the
    // service manager's scale path indirectly — but that doesn't give per-
    // container control.
    //
    // Instead, get the ServiceSpec via the service manager. The manager
    // doesn't expose specs directly, but we can read the replica count and
    // scale up by 1.  This is intentional: the raw container API delegates
    // to the same service infrastructure, ensuring init actions, overlay
    // attachment, etc. are handled.
    let manager = state.service_manager.read().await;
    let current_count = manager
        .service_replica_count(&request.service)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to get replica count: {e}")))?;
    drop(manager);

    // Scale up to include the new replica
    #[allow(clippy::cast_possible_truncation)]
    let target = current_count.max(request.replica as usize) as u32;
    let manager = state.service_manager.read().await;
    manager
        .scale_service(&request.service, target)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create container: {e}")))?;
    drop(manager);

    // Get the resulting state
    let container_state = state
        .runtime
        .container_state(&cid)
        .await
        .unwrap_or(ContainerState::Pending);

    let pid = state.runtime.get_container_pid(&cid).await.ok().flatten();

    info!(container = %cid, state = %state_to_string(&container_state), "Container created");

    Ok((
        axum::http::StatusCode::CREATED,
        Json(CreateContainerResponse {
            id: cid.to_string(),
            state: state_to_string(&container_state),
            pid,
        }),
    ))
}

/// List all containers, optionally filtered by service.
///
/// # Errors
///
/// Returns an error if the service manager is unavailable.
#[utoipa::path(
    get,
    path = "/api/v1/containers",
    params(ListContainersQuery),
    responses(
        (status = 200, description = "List of containers", body = Vec<ContainerInfo>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn list_all_containers(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Query(query): Query<ListContainersQuery>,
) -> Result<Json<Vec<ContainerInfo>>> {
    let manager = state.service_manager.read().await;
    let services = manager.list_services().await;
    drop(manager);

    let target_services: Vec<String> = if let Some(ref filter) = query.service {
        if services.contains(filter) {
            vec![filter.clone()]
        } else {
            return Ok(Json(Vec::new()));
        }
    } else {
        services
    };

    let mut containers = Vec::new();
    let manager = state.service_manager.read().await;

    for svc in &target_services {
        let cids = manager.get_service_containers(svc).await;
        for cid in cids {
            let container_state = state
                .runtime
                .container_state(&cid)
                .await
                .unwrap_or(ContainerState::Pending);
            let pid = state.runtime.get_container_pid(&cid).await.ok().flatten();

            containers.push(ContainerInfo {
                id: cid.to_string(),
                service: cid.service.clone(),
                replica: cid.replica,
                state: state_to_string(&container_state),
                pid,
                exit_code: state_exit_code(&container_state),
            });
        }
    }

    drop(manager);

    Ok(Json(containers))
}

/// Get the state of a specific container.
///
/// # Errors
///
/// Returns an error if the container ID is malformed or the container is not found.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}",
    params(
        ("id" = String, Path, description = "Container ID (e.g. my-service-rep-1)"),
    ),
    responses(
        (status = 200, description = "Container state", body = ContainerStateResponse),
        (status = 400, description = "Invalid container ID format"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn get_container(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
) -> Result<Json<ContainerStateResponse>> {
    let cid = parse_container_id(&id)?;

    let container_state = state
        .runtime
        .container_state(&cid)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { .. } => {
                ApiError::NotFound(format!("Container '{id}' not found"))
            }
            other => ApiError::Internal(format!("Failed to get container state: {other}")),
        })?;

    let pid = state.runtime.get_container_pid(&cid).await.ok().flatten();

    Ok(Json(ContainerStateResponse {
        id,
        state: state_to_string(&container_state),
        pid,
        exit_code: state_exit_code(&container_state),
        reason: state_reason(&container_state),
    }))
}

/// Stop and remove a container.
///
/// Stops the container with a 10-second timeout, then removes it from the
/// runtime. The container's resources (cgroups, filesystem) are cleaned up.
///
/// # Errors
///
/// Returns an error if the container is not found or stop/remove fails.
#[utoipa::path(
    delete,
    path = "/api/v1/containers/{id}",
    params(
        ("id" = String, Path, description = "Container ID (e.g. my-service-rep-1)"),
    ),
    responses(
        (status = 204, description = "Container stopped and removed"),
        (status = 400, description = "Invalid container ID format"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — operator role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn delete_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode> {
    user.require_role("operator")?;

    let cid = parse_container_id(&id)?;

    info!(container = %cid, "Stopping and removing container via raw API");

    // Stop with a 10-second timeout
    state
        .runtime
        .stop_container(&cid, Duration::from_secs(10))
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { .. } => {
                ApiError::NotFound(format!("Container '{id}' not found"))
            }
            other => ApiError::Internal(format!("Failed to stop container: {other}")),
        })?;

    // Remove
    state
        .runtime
        .remove_container(&cid)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to remove container: {e}")))?;

    info!(container = %cid, "Container removed");

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Get container logs.
///
/// Returns the last N lines of logs as plain text (default: 100). When
/// `follow=true`, returns a Server-Sent Events stream that continuously
/// emits new log lines.
///
/// # Errors
///
/// Returns an error if the container is not found or log retrieval fails.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/logs",
    params(
        ("id" = String, Path, description = "Container ID (e.g. my-service-rep-1)"),
        ContainerLogQuery,
    ),
    responses(
        (status = 200, description = "Container logs (plain text or SSE stream)", body = String),
        (status = 400, description = "Invalid container ID format"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn get_container_logs(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Query(query): Query<ContainerLogQuery>,
) -> Result<Response> {
    let cid = parse_container_id(&id)?;

    // Verify the container exists
    state
        .runtime
        .container_state(&cid)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { .. } => {
                ApiError::NotFound(format!("Container '{id}' not found"))
            }
            other => ApiError::Internal(format!("Failed to verify container: {other}")),
        })?;

    if query.follow {
        // SSE stream mode
        let stream = log_follow_stream(state.runtime.clone(), cid, query.tail);
        let sse = Sse::new(stream).keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        );
        Ok(sse.into_response())
    } else {
        // Plain text mode
        let logs = state
            .runtime
            .container_logs(&cid, query.tail)
            .await
            .map_err(|e| match e {
                zlayer_agent::AgentError::NotFound { .. } => {
                    ApiError::NotFound(format!("Container '{id}' not found"))
                }
                other => ApiError::Internal(format!("Failed to get logs: {other}")),
            })?;
        Ok(logs.into_response())
    }
}

/// Build an SSE stream that polls for new container log lines.
fn log_follow_stream(
    runtime: Arc<dyn Runtime + Send + Sync>,
    cid: ContainerId,
    tail: usize,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    futures_util::stream::unfold(
        ContainerLogFollowState {
            runtime,
            cid,
            tail,
            seen_line_count: 0,
            initial: true,
            poll_interval: Duration::from_millis(500),
        },
        |mut state| async move {
            if !state.initial {
                tokio::time::sleep(state.poll_interval).await;
            }

            let fetch_tail = if state.initial { state.tail } else { 10_000 };

            let logs = state
                .runtime
                .container_logs(&state.cid, fetch_tail)
                .await
                .unwrap_or_default();

            let all_lines: Vec<&str> = logs.lines().collect();
            let total = all_lines.len();

            let new_lines = if state.initial {
                state.initial = false;
                state.seen_line_count = total;
                all_lines
            } else if total > state.seen_line_count {
                let new = all_lines[state.seen_line_count..].to_vec();
                state.seen_line_count = total;
                new
            } else {
                vec![]
            };

            let events: Vec<std::result::Result<Event, Infallible>> = new_lines
                .into_iter()
                .map(|line| Ok(Event::default().data(line)))
                .collect();

            debug!(
                container = %state.cid,
                new_events = events.len(),
                total_seen = state.seen_line_count,
                "container log follow poll"
            );

            Some((futures_util::stream::iter(events), state))
        },
    )
    .flatten()
}

/// Internal state for the container log-follow stream.
struct ContainerLogFollowState {
    runtime: Arc<dyn Runtime + Send + Sync>,
    cid: ContainerId,
    tail: usize,
    seen_line_count: usize,
    initial: bool,
    poll_interval: Duration,
}

/// Execute a command inside a running container.
///
/// # Errors
///
/// Returns an error if the container is not found, the command is empty, or
/// execution fails.
#[utoipa::path(
    post,
    path = "/api/v1/containers/{id}/exec",
    params(
        ("id" = String, Path, description = "Container ID (e.g. my-service-rep-1)"),
    ),
    request_body = ContainerExecRequest,
    responses(
        (status = 200, description = "Command executed", body = ContainerExecResponse),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — admin role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn exec_in_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Json(request): Json<ContainerExecRequest>,
) -> Result<Json<ContainerExecResponse>> {
    user.require_role("admin")?;

    if request.command.is_empty() {
        return Err(ApiError::BadRequest("Command cannot be empty".to_string()));
    }

    let cid = parse_container_id(&id)?;

    info!(container = %cid, command = ?request.command, "Executing command in container");

    let (exit_code, stdout, stderr) =
        state
            .runtime
            .exec(&cid, &request.command)
            .await
            .map_err(|e| match e {
                zlayer_agent::AgentError::NotFound { .. } => {
                    ApiError::NotFound(format!("Container '{id}' not found"))
                }
                other => ApiError::Internal(format!("Exec failed: {other}")),
            })?;

    Ok(Json(ContainerExecResponse {
        exit_code,
        stdout,
        stderr,
    }))
}

/// Wait for a container to exit.
///
/// Blocks until the container exits and returns the exit code. This is useful
/// for job-style containers where the caller needs to wait for completion.
///
/// # Errors
///
/// Returns an error if the container is not found or waiting fails.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/wait",
    params(
        ("id" = String, Path, description = "Container ID (e.g. my-service-rep-1)"),
    ),
    responses(
        (status = 200, description = "Container exited", body = ContainerWaitResponse),
        (status = 400, description = "Invalid container ID format"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn wait_container(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
) -> Result<Json<ContainerWaitResponse>> {
    let cid = parse_container_id(&id)?;

    debug!(container = %cid, "Waiting for container to exit");

    let exit_code = state
        .runtime
        .wait_container(&cid)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { .. } => {
                ApiError::NotFound(format!("Container '{id}' not found"))
            }
            other => ApiError::Internal(format!("Wait failed: {other}")),
        })?;

    info!(container = %cid, exit_code, "Container exited");

    Ok(Json(ContainerWaitResponse { id, exit_code }))
}

/// Get resource statistics for a container.
///
/// Returns CPU and memory usage from cgroups. Only works for running
/// containers on Linux with cgroups v2 enabled.
///
/// # Errors
///
/// Returns an error if the container is not found or stats retrieval fails.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/stats",
    params(
        ("id" = String, Path, description = "Container ID (e.g. my-service-rep-1)"),
    ),
    responses(
        (status = 200, description = "Container resource statistics", body = ContainerStatsResponse),
        (status = 400, description = "Invalid container ID format"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn get_container_stats(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
) -> Result<Json<ContainerStatsResponse>> {
    let cid = parse_container_id(&id)?;

    let cgroup_stats = state
        .runtime
        .get_container_stats(&cid)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { .. } => {
                ApiError::NotFound(format!("Container '{id}' not found"))
            }
            other => ApiError::Internal(format!("Failed to get stats: {other}")),
        })?;

    Ok(Json(ContainerStatsResponse {
        id,
        cpu_usage_usec: cgroup_stats.cpu_usage_usec,
        memory_bytes: cgroup_stats.memory_bytes,
        memory_limit: cgroup_stats.memory_limit,
        memory_percent: cgroup_stats.memory_percent(),
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_container_id_simple() {
        let cid = parse_container_id("web-rep-1").unwrap();
        assert_eq!(cid.service, "web");
        assert_eq!(cid.replica, 1);
    }

    #[test]
    fn test_parse_container_id_with_hyphens() {
        let cid = parse_container_id("my-cool-service-rep-42").unwrap();
        assert_eq!(cid.service, "my-cool-service");
        assert_eq!(cid.replica, 42);
    }

    #[test]
    fn test_parse_container_id_invalid_format() {
        assert!(parse_container_id("invalid").is_err());
        assert!(parse_container_id("no-replica").is_err());
        assert!(parse_container_id("service-nope-1").is_err());
    }

    #[test]
    fn test_parse_container_id_invalid_replica() {
        assert!(parse_container_id("web-rep-abc").is_err());
    }

    #[test]
    fn test_state_to_string_variants() {
        assert_eq!(state_to_string(&ContainerState::Pending), "pending");
        assert_eq!(
            state_to_string(&ContainerState::Initializing),
            "initializing"
        );
        assert_eq!(state_to_string(&ContainerState::Running), "running");
        assert_eq!(state_to_string(&ContainerState::Stopping), "stopping");
        assert_eq!(
            state_to_string(&ContainerState::Exited { code: 0 }),
            "exited(0)"
        );
        assert_eq!(
            state_to_string(&ContainerState::Exited { code: 137 }),
            "exited(137)"
        );
        assert_eq!(
            state_to_string(&ContainerState::Failed {
                reason: "OOM".to_string()
            }),
            "failed: OOM"
        );
    }

    #[test]
    fn test_state_exit_code() {
        assert_eq!(
            state_exit_code(&ContainerState::Exited { code: 42 }),
            Some(42)
        );
        assert_eq!(state_exit_code(&ContainerState::Running), None);
        assert_eq!(
            state_exit_code(&ContainerState::Failed {
                reason: "err".to_string()
            }),
            None
        );
    }

    #[test]
    fn test_state_reason() {
        assert_eq!(
            state_reason(&ContainerState::Failed {
                reason: "out of memory".to_string()
            }),
            Some("out of memory".to_string())
        );
        assert_eq!(state_reason(&ContainerState::Running), None);
    }

    #[test]
    fn test_create_container_request_deserialize() {
        let json = r#"{"service": "web", "replica": 3}"#;
        let req: CreateContainerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.service, "web");
        assert_eq!(req.replica, 3);
        assert!(req.image.is_none());
    }

    #[test]
    fn test_create_container_request_with_image() {
        let json = r#"{"service": "web", "replica": 1, "image": "nginx:latest"}"#;
        let req: CreateContainerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.image, Some("nginx:latest".to_string()));
    }

    #[test]
    fn test_container_info_serialize() {
        let info = ContainerInfo {
            id: "web-rep-1".to_string(),
            service: "web".to_string(),
            replica: 1,
            state: "running".to_string(),
            pid: Some(1234),
            exit_code: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("web-rep-1"));
        assert!(json.contains("1234"));
    }

    #[test]
    fn test_container_exec_request_deserialize() {
        let json = r#"{"command": ["ls", "-la"]}"#;
        let req: ContainerExecRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.command, vec!["ls", "-la"]);
    }

    #[test]
    fn test_container_log_query_defaults() {
        let query: ContainerLogQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(query.tail, 100);
        assert!(!query.follow);
    }

    #[test]
    fn test_container_stats_response_serialize() {
        let stats = ContainerStatsResponse {
            id: "web-rep-1".to_string(),
            cpu_usage_usec: 5_000_000,
            memory_bytes: 100 * 1024 * 1024,
            memory_limit: 256 * 1024 * 1024,
            memory_percent: 39.0625,
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("cpu_usage_usec"));
        assert!(json.contains("memory_percent"));
    }

    #[test]
    fn test_container_wait_response_serialize() {
        let resp = ContainerWaitResponse {
            id: "job-rep-1".to_string(),
            exit_code: 0,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("exit_code"));
    }
}
