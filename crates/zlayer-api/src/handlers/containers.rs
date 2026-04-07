//! Raw container lifecycle endpoints
//!
//! Provides direct container management endpoints for use by CI runners and
//! other tooling that needs to manage containers independently of the
//! deployment/service abstraction.

use std::collections::HashMap;
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
use zlayer_agent::runtime::{ContainerId, ContainerState, Runtime};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// State for raw container endpoints.
///
/// Holds a reference to the container runtime so handlers can perform
/// container lifecycle operations without going through the service manager.
#[derive(Clone)]
pub struct ContainerApiState {
    /// Container runtime for lifecycle operations
    pub runtime: Arc<dyn Runtime + Send + Sync>,
    /// In-memory tracking of standalone containers (id -> metadata)
    pub containers: Arc<RwLock<HashMap<String, StandaloneContainer>>>,
}

/// Metadata for a standalone container (not managed by a deployment)
#[derive(Debug, Clone)]
pub struct StandaloneContainer {
    /// Container identifier used by the runtime
    pub container_id: ContainerId,
    /// OCI image reference
    pub image: String,
    /// Human-readable name (if provided)
    pub name: Option<String>,
    /// Labels for filtering/grouping
    pub labels: HashMap<String, String>,
    /// When the container was created
    pub created_at: String,
}

impl ContainerApiState {
    /// Create a new container API state with a runtime
    pub fn new(runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        Self {
            runtime,
            containers: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Resource limits for a container
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct ContainerResourceLimits {
    /// CPU limit in cores (e.g., 0.5, 1.0, 2.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<f64>,
    /// Memory limit (e.g., "256Mi", "1Gi")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
}

/// Volume mount specification
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct VolumeMount {
    /// Host path or volume name
    pub source: String,
    /// Container mount path
    pub target: String,
    /// Mount as read-only
    #[serde(default)]
    pub readonly: bool,
}

/// Request to create and start a container
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateContainerRequest {
    /// OCI image reference (e.g., "nginx:latest", "ubuntu:22.04")
    pub image: String,
    /// Optional human-readable name
    #[serde(default)]
    pub name: Option<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Command to run (overrides image entrypoint)
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// Labels for filtering and grouping
    #[serde(default)]
    pub labels: HashMap<String, String>,
    /// Resource limits (CPU, memory)
    #[serde(default)]
    pub resources: Option<ContainerResourceLimits>,
    /// Volume mounts
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,
    /// Working directory inside the container
    #[serde(default)]
    pub work_dir: Option<String>,
}

/// Container information returned by the API
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerInfo {
    /// Container identifier
    pub id: String,
    /// Human-readable name (if set)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// OCI image reference
    pub image: String,
    /// Container state (pending, running, exited, failed)
    pub state: String,
    /// Labels
    pub labels: HashMap<String, String>,
    /// Creation timestamp (ISO 8601)
    pub created_at: String,
    /// Process ID (if running)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Query parameters for listing containers
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListContainersQuery {
    /// Filter by label (key=value format)
    #[serde(default)]
    pub label: Option<String>,
}

/// Query parameters for container logs
#[derive(Debug, Deserialize, IntoParams)]
pub struct ContainerLogQuery {
    /// Number of tail lines to return
    #[serde(default = "default_tail")]
    pub tail: usize,
    /// Follow logs (SSE stream)
    #[serde(default)]
    pub follow: bool,
}

fn default_tail() -> usize {
    100
}

/// Exec request for running a command in a container
#[derive(Debug, Deserialize, ToSchema)]
pub struct ContainerExecRequest {
    /// Command and arguments to execute
    pub command: Vec<String>,
}

/// Exec response with command output
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerExecResponse {
    /// Exit code from the command
    pub exit_code: i32,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
}

/// Wait response with container exit code
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerWaitResponse {
    /// Container identifier
    pub id: String,
    /// Exit code (0 = success)
    pub exit_code: i32,
}

/// Container resource statistics
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerStatsResponse {
    /// Container identifier
    pub id: String,
    /// CPU usage in microseconds
    pub cpu_usage_usec: u64,
    /// Current memory usage in bytes
    pub memory_bytes: u64,
    /// Memory limit in bytes (`u64::MAX` if unlimited)
    pub memory_limit: u64,
    /// Memory usage as percentage of limit
    pub memory_percent: f64,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate a unique container ID string for standalone containers.
///
/// Format: `standalone-{name_or_uuid}-rep-0`
fn generate_container_id(name: Option<&str>) -> (String, ContainerId) {
    let service_name = match name {
        Some(n) => format!("standalone-{n}"),
        None => format!("standalone-{}", uuid::Uuid::new_v4().as_simple()),
    };
    let cid = ContainerId {
        service: service_name.clone(),
        replica: 0,
    };
    (service_name, cid)
}

/// Build a minimal `ServiceSpec` from the create request.
///
/// This converts the simplified container request into the full `ServiceSpec`
/// that the Runtime trait expects. Many fields default to sensible values
/// since standalone containers don't need scaling, health checks, etc.
fn build_service_spec(request: &CreateContainerRequest) -> zlayer_spec::ServiceSpec {
    use zlayer_spec::{
        CommandSpec, ErrorsSpec, HealthCheck, HealthSpec, ImageSpec, InitSpec, NodeMode,
        PullPolicy, ResourceType, ResourcesSpec, ScaleSpec, ServiceNetworkSpec, ServiceSpec,
        ServiceType, StorageSpec,
    };

    let command_spec = if let Some(ref cmd) = request.command {
        CommandSpec {
            entrypoint: if cmd.is_empty() {
                None
            } else {
                Some(vec![cmd[0].clone()])
            },
            args: if cmd.len() > 1 {
                Some(cmd[1..].to_vec())
            } else {
                None
            },
            workdir: request.work_dir.clone(),
        }
    } else {
        CommandSpec {
            entrypoint: None,
            args: None,
            workdir: request.work_dir.clone(),
        }
    };

    let resources = match &request.resources {
        Some(r) => ResourcesSpec {
            cpu: r.cpu,
            memory: r.memory.clone(),
            gpu: None,
        },
        None => ResourcesSpec::default(),
    };

    let storage: Vec<StorageSpec> = request
        .volumes
        .iter()
        .map(|v| StorageSpec::Bind {
            source: v.source.clone(),
            target: v.target.clone(),
            readonly: v.readonly,
        })
        .collect();

    ServiceSpec {
        rtype: ResourceType::Service,
        schedule: None,
        image: ImageSpec {
            name: request.image.clone(),
            pull_policy: PullPolicy::IfNotPresent,
        },
        resources,
        env: request.env.clone(),
        command: command_spec,
        network: ServiceNetworkSpec::default(),
        endpoints: Vec::new(),
        scale: ScaleSpec::Manual,
        depends: Vec::new(),
        health: HealthSpec {
            start_grace: None,
            interval: None,
            timeout: None,
            retries: 3,
            check: HealthCheck::Tcp { port: 0 },
        },
        init: InitSpec::default(),
        errors: ErrorsSpec::default(),
        devices: Vec::new(),
        storage,
        capabilities: Vec::new(),
        privileged: false,
        node_mode: NodeMode::default(),
        node_selector: None,
        service_type: ServiceType::default(),
        wasm: None,
        logs: None,
        host_network: false,
    }
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

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Create and start a container.
///
/// Pulls the image if needed, creates the container, and starts it.
/// Returns the container info including its assigned ID.
///
/// # Errors
///
/// Returns an error if image pull fails, container creation fails, or the
/// user lacks the operator role.
#[utoipa::path(
    post,
    path = "/api/v1/containers",
    request_body = CreateContainerRequest,
    responses(
        (status = 201, description = "Container created and started", body = ContainerInfo),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn create_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Json(request): Json<CreateContainerRequest>,
) -> Result<(axum::http::StatusCode, Json<ContainerInfo>)> {
    user.require_role("operator")?;

    // Validate image
    if request.image.is_empty() {
        return Err(ApiError::BadRequest("Image is required".to_string()));
    }

    // Validate name (if provided) - alphanumeric, hyphens, underscores only
    if let Some(ref name) = request.name {
        if name.is_empty() || name.len() > 128 {
            return Err(ApiError::BadRequest(
                "Name must be 1-128 characters".to_string(),
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ApiError::BadRequest(
                "Name must contain only alphanumeric characters, hyphens, and underscores"
                    .to_string(),
            ));
        }

        // Check for duplicate name
        let containers = state.containers.read().await;
        let duplicate = containers
            .values()
            .any(|c| c.name.as_deref() == Some(name.as_str()));
        if duplicate {
            return Err(ApiError::Conflict(format!(
                "Container with name '{name}' already exists"
            )));
        }
    }

    let (id_str, container_id) = generate_container_id(request.name.as_deref());
    let spec = build_service_spec(&request);

    info!(
        container_id = %container_id,
        image = %request.image,
        name = ?request.name,
        "Creating standalone container"
    );

    // Pull the image
    state
        .runtime
        .pull_image(&request.image)
        .await
        .map_err(|e| {
            ApiError::Internal(format!("Failed to pull image '{}': {e}", request.image))
        })?;

    // Create the container
    state
        .runtime
        .create_container(&container_id, &spec)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create container: {e}")))?;

    // Start the container
    state
        .runtime
        .start_container(&container_id)
        .await
        .map_err(|e| {
            // Best-effort cleanup: remove the created-but-not-started container
            let rt = state.runtime.clone();
            let cid = container_id.clone();
            tokio::spawn(async move {
                let _ = rt.remove_container(&cid).await;
            });
            ApiError::Internal(format!("Failed to start container: {e}"))
        })?;

    // Get PID
    let pid = state
        .runtime
        .get_container_pid(&container_id)
        .await
        .ok()
        .flatten();

    let now = chrono::Utc::now().to_rfc3339();

    // Track the container
    let standalone = StandaloneContainer {
        container_id: container_id.clone(),
        image: request.image.clone(),
        name: request.name.clone(),
        labels: request.labels.clone(),
        created_at: now.clone(),
    };

    state
        .containers
        .write()
        .await
        .insert(id_str.clone(), standalone);

    let info = ContainerInfo {
        id: id_str,
        name: request.name,
        image: request.image,
        state: "running".to_string(),
        labels: request.labels,
        created_at: now,
        pid,
    };

    Ok((axum::http::StatusCode::CREATED, Json(info)))
}

/// List standalone containers.
///
/// Returns all containers managed through this API. Optionally filter by
/// label using the `label` query parameter in `key=value` format.
///
/// # Errors
///
/// Returns an error if the user is not authenticated.
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
pub async fn list_containers(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Query(query): Query<ListContainersQuery>,
) -> Result<Json<Vec<ContainerInfo>>> {
    let containers = state.containers.read().await;

    // Parse label filter if provided
    let label_filter: Option<(String, String)> = query.label.and_then(|l| {
        let parts: Vec<&str> = l.splitn(2, '=').collect();
        if parts.len() == 2 {
            Some((parts[0].to_string(), parts[1].to_string()))
        } else {
            None
        }
    });

    let mut results = Vec::with_capacity(containers.len());

    for (id_str, meta) in containers.iter() {
        // Apply label filter
        if let Some((ref key, ref value)) = label_filter {
            match meta.labels.get(key) {
                Some(v) if v == value => {}
                _ => continue,
            }
        }

        // Query runtime state
        let runtime_state = state
            .runtime
            .container_state(&meta.container_id)
            .await
            .unwrap_or(ContainerState::Failed {
                reason: "state unavailable".to_string(),
            });

        let pid = state
            .runtime
            .get_container_pid(&meta.container_id)
            .await
            .ok()
            .flatten();

        results.push(ContainerInfo {
            id: id_str.clone(),
            name: meta.name.clone(),
            image: meta.image.clone(),
            state: state_to_string(&runtime_state),
            labels: meta.labels.clone(),
            created_at: meta.created_at.clone(),
            pid,
        });
    }

    Ok(Json(results))
}

/// Get details for a specific container.
///
/// # Errors
///
/// Returns an error if the container is not found or the user is not authenticated.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    responses(
        (status = 200, description = "Container details", body = ContainerInfo),
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
) -> Result<Json<ContainerInfo>> {
    let containers = state.containers.read().await;
    let meta = containers
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;

    let runtime_state = state
        .runtime
        .container_state(&meta.container_id)
        .await
        .unwrap_or(ContainerState::Failed {
            reason: "state unavailable".to_string(),
        });

    let pid = state
        .runtime
        .get_container_pid(&meta.container_id)
        .await
        .ok()
        .flatten();

    Ok(Json(ContainerInfo {
        id: id.clone(),
        name: meta.name.clone(),
        image: meta.image.clone(),
        state: state_to_string(&runtime_state),
        labels: meta.labels.clone(),
        created_at: meta.created_at.clone(),
        pid,
    }))
}

/// Stop and remove a container.
///
/// Sends a stop signal (with a 30-second timeout), then removes the container.
///
/// # Errors
///
/// Returns an error if the container is not found, stop/remove fails, or the
/// user lacks the operator role.
#[utoipa::path(
    delete,
    path = "/api/v1/containers/{id}",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    responses(
        (status = 204, description = "Container stopped and removed"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
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

    let container_id = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.container_id.clone()
    };

    info!(container_id = %container_id, "Stopping and removing standalone container");

    // Stop with 30s timeout (ignore errors if container is already stopped)
    let stop_result = state
        .runtime
        .stop_container(&container_id, Duration::from_secs(30))
        .await;
    if let Err(ref e) = stop_result {
        debug!(error = %e, "Stop returned error (container may already be stopped)");
    }

    // Remove the container
    state
        .runtime
        .remove_container(&container_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to remove container: {e}")))?;

    // Remove from tracking
    state.containers.write().await.remove(&id);

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Get container logs.
///
/// Returns the last N lines of container logs as plain text, or streams
/// logs as Server-Sent Events when `follow=true`.
///
/// # Errors
///
/// Returns an error if the container is not found or log retrieval fails.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/logs",
    params(
        ("id" = String, Path, description = "Container identifier"),
        ContainerLogQuery,
    ),
    responses(
        (status = 200, description = "Container logs (plain text or SSE stream)", body = String),
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
    let container_id = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.container_id.clone()
    };

    if query.follow {
        // SSE streaming mode
        let stream = container_log_follow_stream(state.runtime.clone(), container_id, query.tail);
        let sse = Sse::new(stream).keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        );
        Ok(sse.into_response())
    } else {
        // Plain text mode
        let entries = state
            .runtime
            .container_logs(&container_id, query.tail)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to get logs: {e}")))?;
        let logs: String = entries
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        Ok(logs.into_response())
    }
}

/// Internal state for the container log-follow polling stream.
struct ContainerLogFollowState {
    runtime: Arc<dyn Runtime + Send + Sync>,
    container_id: ContainerId,
    tail: usize,
    seen_line_count: usize,
    initial: bool,
    poll_interval: Duration,
}

/// Build an SSE stream that polls for new log output.
fn container_log_follow_stream(
    runtime: Arc<dyn Runtime + Send + Sync>,
    container_id: ContainerId,
    tail: usize,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    futures_util::stream::unfold(
        ContainerLogFollowState {
            runtime,
            container_id,
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

            let entries = state
                .runtime
                .container_logs(&state.container_id, fetch_tail)
                .await
                .unwrap_or_default();

            let all_lines: Vec<String> = entries.iter().map(ToString::to_string).collect();
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
                container_id = %state.container_id,
                new_events = events.len(),
                total_seen = state.seen_line_count,
                "container log follow poll"
            );

            Some((futures_util::stream::iter(events), state))
        },
    )
    .flatten()
}

/// Execute a command in a running container.
///
/// # Errors
///
/// Returns an error if the container is not found, the command is invalid,
/// execution fails, or the user lacks the operator role.
#[utoipa::path(
    post,
    path = "/api/v1/containers/{id}/exec",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    request_body = ContainerExecRequest,
    responses(
        (status = 200, description = "Command executed", body = ContainerExecResponse),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
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
    user.require_role("operator")?;

    if request.command.is_empty() {
        return Err(ApiError::BadRequest("Command cannot be empty".to_string()));
    }

    let container_id = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.container_id.clone()
    };

    let (exit_code, stdout, stderr) = state
        .runtime
        .exec(&container_id, &request.command)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            other => ApiError::Internal(format!("Exec failed: {other}")),
        })?;

    Ok(Json(ContainerExecResponse {
        exit_code,
        stdout,
        stderr,
    }))
}

/// Wait for a container to exit and return its exit code.
///
/// This endpoint blocks until the container exits. Useful for CI runners
/// that need to wait for a build/test container to complete.
///
/// # Errors
///
/// Returns an error if the container is not found or the wait fails.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/wait",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    responses(
        (status = 200, description = "Container exited", body = ContainerWaitResponse),
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
    let container_id = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.container_id.clone()
    };

    let exit_code = state
        .runtime
        .wait_container(&container_id)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            other => ApiError::Internal(format!("Wait failed: {other}")),
        })?;

    Ok(Json(ContainerWaitResponse { id, exit_code }))
}

/// Get container resource statistics.
///
/// Returns CPU and memory usage statistics for the specified container.
///
/// # Errors
///
/// Returns an error if the container is not found or stats retrieval fails.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/stats",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    responses(
        (status = 200, description = "Container statistics", body = ContainerStatsResponse),
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
    let container_id = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.container_id.clone()
    };

    let cstats = state
        .runtime
        .get_container_stats(&container_id)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            other => ApiError::Internal(format!("Failed to get stats: {other}")),
        })?;

    Ok(Json(ContainerStatsResponse {
        id,
        cpu_usage_usec: cstats.cpu_usage_usec,
        memory_bytes: cstats.memory_bytes,
        memory_limit: cstats.memory_limit,
        memory_percent: cstats.memory_percent(),
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_container_request_deserialize() {
        let json = r#"{
            "image": "nginx:latest",
            "name": "my-nginx",
            "env": {"PORT": "8080"},
            "command": ["nginx", "-g", "daemon off;"],
            "labels": {"app": "web", "ci": "true"},
            "resources": {"cpu": 0.5, "memory": "256Mi"},
            "volumes": [{"source": "/data", "target": "/app/data", "readonly": true}],
            "work_dir": "/app"
        }"#;
        let request: CreateContainerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.image, "nginx:latest");
        assert_eq!(request.name.as_deref(), Some("my-nginx"));
        assert_eq!(request.env.get("PORT").unwrap(), "8080");
        assert_eq!(request.command.as_ref().unwrap().len(), 3);
        assert_eq!(request.labels.get("ci").unwrap(), "true");
        assert!(request.resources.is_some());
        assert_eq!(request.volumes.len(), 1);
        assert!(request.volumes[0].readonly);
        assert_eq!(request.work_dir.as_deref(), Some("/app"));
    }

    #[test]
    fn test_create_container_request_minimal() {
        let json = r#"{"image": "alpine:3.19"}"#;
        let request: CreateContainerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.image, "alpine:3.19");
        assert!(request.name.is_none());
        assert!(request.env.is_empty());
        assert!(request.command.is_none());
        assert!(request.labels.is_empty());
        assert!(request.resources.is_none());
        assert!(request.volumes.is_empty());
        assert!(request.work_dir.is_none());
    }

    #[test]
    fn test_container_info_serialize() {
        let info = ContainerInfo {
            id: "standalone-test-rep-0".to_string(),
            name: Some("test".to_string()),
            image: "nginx:latest".to_string(),
            state: "running".to_string(),
            labels: HashMap::from([("app".to_string(), "web".to_string())]),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            pid: Some(12345),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("standalone-test-rep-0"));
        assert!(json.contains("running"));
        assert!(json.contains("12345"));
    }

    #[test]
    fn test_container_info_serialize_no_optional() {
        let info = ContainerInfo {
            id: "standalone-abc-rep-0".to_string(),
            name: None,
            image: "alpine:latest".to_string(),
            state: "exited(0)".to_string(),
            labels: HashMap::new(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            pid: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("name"));
        assert!(!json.contains("pid"));
    }

    #[test]
    fn test_exec_request_deserialize() {
        let json = r#"{"command": ["echo", "hello"]}"#;
        let request: ContainerExecRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.command, vec!["echo", "hello"]);
    }

    #[test]
    fn test_wait_response_serialize() {
        let response = ContainerWaitResponse {
            id: "test-container".to_string(),
            exit_code: 0,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("test-container"));
        assert!(json.contains('0'));
    }

    #[test]
    fn test_stats_response_serialize() {
        let response = ContainerStatsResponse {
            id: "test-container".to_string(),
            cpu_usage_usec: 1_000_000,
            memory_bytes: 50 * 1024 * 1024,
            memory_limit: 256 * 1024 * 1024,
            memory_percent: 19.53125,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("cpu_usage_usec"));
        assert!(json.contains("memory_percent"));
    }

    #[test]
    fn test_state_to_string() {
        assert_eq!(state_to_string(&ContainerState::Running), "running");
        assert_eq!(state_to_string(&ContainerState::Pending), "pending");
        assert_eq!(
            state_to_string(&ContainerState::Exited { code: 0 }),
            "exited(0)"
        );
        assert_eq!(
            state_to_string(&ContainerState::Failed {
                reason: "oom".to_string()
            }),
            "failed: oom"
        );
    }

    #[test]
    fn test_generate_container_id_with_name() {
        let (id, cid) = generate_container_id(Some("myapp"));
        assert_eq!(id, "standalone-myapp");
        assert_eq!(cid.service, "standalone-myapp");
        assert_eq!(cid.replica, 0);
    }

    #[test]
    fn test_generate_container_id_without_name() {
        let (id, cid) = generate_container_id(None);
        assert!(id.starts_with("standalone-"));
        assert_eq!(cid.service, id);
        assert_eq!(cid.replica, 0);
    }

    #[test]
    fn test_log_query_defaults() {
        let query: ContainerLogQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(query.tail, 100);
        assert!(!query.follow);
    }

    #[test]
    fn test_list_containers_query_no_label() {
        let query: ListContainersQuery = serde_json::from_str("{}").unwrap();
        assert!(query.label.is_none());
    }

    #[test]
    fn test_build_service_spec_minimal() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            name: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: Vec::new(),
            work_dir: None,
        };
        let spec = build_service_spec(&request);
        assert_eq!(spec.image.name, "alpine:latest");
        assert!(spec.env.is_empty());
        assert!(spec.storage.is_empty());
    }

    #[test]
    fn test_build_service_spec_full() {
        let request = CreateContainerRequest {
            image: "node:20".to_string(),
            name: Some("build-runner".to_string()),
            env: HashMap::from([("NODE_ENV".to_string(), "production".to_string())]),
            command: Some(vec!["node".to_string(), "server.js".to_string()]),
            labels: HashMap::from([("ci".to_string(), "true".to_string())]),
            resources: Some(ContainerResourceLimits {
                cpu: Some(2.0),
                memory: Some("1Gi".to_string()),
            }),
            volumes: vec![VolumeMount {
                source: "/workspace".to_string(),
                target: "/app".to_string(),
                readonly: false,
            }],
            work_dir: Some("/app".to_string()),
        };
        let spec = build_service_spec(&request);
        assert_eq!(spec.image.name, "node:20");
        assert_eq!(spec.env.get("NODE_ENV").unwrap(), "production");
        assert_eq!(
            spec.command.entrypoint.as_deref(),
            Some(["node".to_string()].as_slice())
        );
        assert_eq!(
            spec.command.args.as_deref(),
            Some(["server.js".to_string()].as_slice())
        );
        assert_eq!(spec.command.workdir.as_deref(), Some("/app"));
        assert_eq!(spec.resources.cpu, Some(2.0));
        assert_eq!(spec.resources.memory.as_deref(), Some("1Gi"));
        assert_eq!(spec.storage.len(), 1);
    }

    #[test]
    fn test_container_api_state_new() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ContainerApiState::new(runtime);
        // State builds without error
        assert!(Arc::strong_count(&state.containers) >= 1);
    }
}
