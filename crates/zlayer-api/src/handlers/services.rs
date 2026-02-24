//! Service endpoints

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
use tracing::debug;
use utoipa::{IntoParams, ToSchema};

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use crate::storage::DeploymentStorage;
use zlayer_agent::ServiceManager;

/// Service summary
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ServiceSummary {
    /// Service name
    pub name: String,
    /// Deployment name
    pub deployment: String,
    /// Service status
    pub status: String,
    /// Current replica count
    pub replicas: u32,
    /// Desired replica count
    pub desired_replicas: u32,
    /// Service endpoints
    pub endpoints: Vec<ServiceEndpoint>,
}

/// Service details
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ServiceDetails {
    /// Service name
    pub name: String,
    /// Deployment name
    pub deployment: String,
    /// Service status
    pub status: String,
    /// Current replica count
    pub replicas: u32,
    /// Desired replica count
    pub desired_replicas: u32,
    /// Service endpoints
    pub endpoints: Vec<ServiceEndpoint>,
    /// Service metrics
    pub metrics: ServiceMetrics,
}

/// Service endpoint
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ServiceEndpoint {
    /// Endpoint name
    pub name: String,
    /// Protocol
    pub protocol: String,
    /// Port
    pub port: u16,
    /// URL (if public)
    pub url: Option<String>,
}

/// Service metrics
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ServiceMetrics {
    /// CPU usage percentage
    pub cpu_percent: f64,
    /// Memory usage percentage
    pub memory_percent: f64,
    /// Requests per second
    pub rps: Option<f64>,
}

/// Scale request
#[derive(Debug, Deserialize, ToSchema)]
pub struct ScaleRequest {
    /// Target replica count
    pub replicas: u32,
}

/// Log query parameters
#[derive(Debug, Deserialize, IntoParams)]
pub struct LogQuery {
    /// Number of lines to return
    #[serde(default = "default_lines")]
    pub lines: u32,
    /// Follow logs (streaming)
    #[serde(default)]
    pub follow: bool,
    /// Filter by container/instance
    pub instance: Option<String>,
}

fn default_lines() -> u32 {
    100
}

/// State for service endpoints
///
/// Holds references to the service manager and deployment storage,
/// allowing service handlers to perform scaling operations and
/// look up deployment/service information.
///
/// The service_manager is optional to allow read-only mode (viewing service
/// specs from storage) when no ServiceManager is available.
#[derive(Clone)]
pub struct ServiceState {
    /// Service manager for container lifecycle operations (optional)
    pub service_manager: Option<Arc<RwLock<ServiceManager>>>,
    /// Storage backend for deployment specifications
    pub storage: Arc<dyn DeploymentStorage + Send + Sync>,
}

impl ServiceState {
    /// Create a new service state with a service manager
    pub fn new(
        service_manager: Arc<RwLock<ServiceManager>>,
        storage: Arc<dyn DeploymentStorage + Send + Sync>,
    ) -> Self {
        Self {
            service_manager: Some(service_manager),
            storage,
        }
    }

    /// Create a service state without a service manager (read-only mode)
    ///
    /// In this mode, service details can be viewed from storage but
    /// scaling operations will return an error.
    pub fn read_only(storage: Arc<dyn DeploymentStorage + Send + Sync>) -> Self {
        Self {
            service_manager: None,
            storage,
        }
    }
}

/// List services in a deployment
#[utoipa::path(
    get,
    path = "/api/v1/deployments/{deployment}/services",
    params(
        ("deployment" = String, Path, description = "Deployment name"),
    ),
    responses(
        (status = 200, description = "List of services", body = Vec<ServiceSummary>),
        (status = 404, description = "Deployment not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Services"
)]
pub async fn list_services(
    _user: AuthUser,
    State(state): State<ServiceState>,
    Path(deployment): Path<String>,
) -> Result<Json<Vec<ServiceSummary>>> {
    // Get the deployment from storage
    let stored = state
        .storage
        .get(&deployment)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Deployment '{}' not found", deployment)))?;

    // Build service summaries from the deployment spec
    let mut summaries = Vec::new();
    for (service_name, spec) in &stored.spec.services {
        // Get current replica count from service manager if available
        let replicas = if let Some(ref manager) = state.service_manager {
            let manager = manager.read().await;
            manager
                .service_replica_count(service_name)
                .await
                .unwrap_or(0) as u32
        } else {
            0 // No service manager, assume not running
        };

        // Get desired replicas from spec
        let desired_replicas = match &spec.scale {
            zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
            zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
            zlayer_spec::ScaleSpec::Manual => 1,
        };

        // Determine status based on replica counts
        let status = if replicas == 0 {
            "pending".to_string()
        } else if replicas < desired_replicas {
            "scaling".to_string()
        } else {
            "running".to_string()
        };

        summaries.push(ServiceSummary {
            name: service_name.clone(),
            deployment: deployment.clone(),
            status,
            replicas,
            desired_replicas,
            endpoints: spec
                .endpoints
                .iter()
                .map(|ep| ServiceEndpoint {
                    name: ep.name.clone(),
                    protocol: format!("{:?}", ep.protocol).to_lowercase(),
                    port: ep.port,
                    url: ep.path.clone(),
                })
                .collect(),
        });
    }

    Ok(Json(summaries))
}

/// Get service details
#[utoipa::path(
    get,
    path = "/api/v1/deployments/{deployment}/services/{service}",
    params(
        ("deployment" = String, Path, description = "Deployment name"),
        ("service" = String, Path, description = "Service name"),
    ),
    responses(
        (status = 200, description = "Service details", body = ServiceDetails),
        (status = 404, description = "Service not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Services"
)]
pub async fn get_service(
    _user: AuthUser,
    State(state): State<ServiceState>,
    Path((deployment, service)): Path<(String, String)>,
) -> Result<Json<ServiceDetails>> {
    // Get the deployment from storage
    let stored = state
        .storage
        .get(&deployment)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Deployment '{}' not found", deployment)))?;

    // Get the service spec
    let spec = stored.spec.services.get(&service).ok_or_else(|| {
        ApiError::NotFound(format!("Service '{}/{}' not found", deployment, service))
    })?;

    // Get current replica count from service manager if available
    let replicas = if let Some(ref manager) = state.service_manager {
        let manager = manager.read().await;
        manager.service_replica_count(&service).await.unwrap_or(0) as u32
    } else {
        0 // No service manager, assume not running
    };

    // Get desired replicas from spec
    let desired_replicas = match &spec.scale {
        zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
        zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
        zlayer_spec::ScaleSpec::Manual => 1,
    };

    // Determine status based on replica counts
    let status = if replicas == 0 {
        "pending".to_string()
    } else if replicas < desired_replicas {
        "scaling".to_string()
    } else {
        "running".to_string()
    };

    // Build endpoints from spec
    let endpoints: Vec<ServiceEndpoint> = spec
        .endpoints
        .iter()
        .map(|ep| ServiceEndpoint {
            name: ep.name.clone(),
            protocol: format!("{:?}", ep.protocol).to_lowercase(),
            port: ep.port,
            url: ep.path.clone(),
        })
        .collect();

    // TODO: Get actual metrics from container runtime
    let metrics = ServiceMetrics {
        cpu_percent: 0.0,
        memory_percent: 0.0,
        rps: None,
    };

    Ok(Json(ServiceDetails {
        name: service,
        deployment,
        status,
        replicas,
        desired_replicas,
        endpoints,
        metrics,
    }))
}

/// Scale a service
#[utoipa::path(
    post,
    path = "/api/v1/deployments/{deployment}/services/{service}/scale",
    params(
        ("deployment" = String, Path, description = "Deployment name"),
        ("service" = String, Path, description = "Service name"),
    ),
    request_body = ScaleRequest,
    responses(
        (status = 200, description = "Service scaled", body = ServiceDetails),
        (status = 404, description = "Service not found"),
        (status = 401, description = "Unauthorized"),
        (status = 400, description = "Invalid scale request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Services"
)]
pub async fn scale_service(
    user: AuthUser,
    State(state): State<ServiceState>,
    Path((deployment, service)): Path<(String, String)>,
    Json(request): Json<ScaleRequest>,
) -> Result<Json<ServiceDetails>> {
    // Require admin or operator role
    user.require_role("operator")?;

    // Validate replica count
    if request.replicas > 100 {
        return Err(ApiError::BadRequest(
            "Replica count cannot exceed 100".to_string(),
        ));
    }

    // Get the deployment from storage to verify it exists and get service spec
    let stored = state
        .storage
        .get(&deployment)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Deployment '{}' not found", deployment)))?;

    // Get the service spec to verify it exists
    let spec = stored.spec.services.get(&service).ok_or_else(|| {
        ApiError::NotFound(format!("Service '{}/{}' not found", deployment, service))
    })?;

    // Require service manager for scaling operations
    let manager_arc = state.service_manager.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable(
            "Service scaling not available: no service manager configured".to_string(),
        )
    })?;

    // Scale the service via service manager
    let manager = manager_arc.read().await;

    // First ensure the service is registered with the manager
    // If not, we need to upsert it first
    if manager.service_replica_count(&service).await.is_err() {
        // Service not yet registered, upsert it
        manager
            .upsert_service(service.clone(), spec.clone())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to register service: {}", e)))?;
    }

    // Now scale to the requested replica count
    manager
        .scale_service(&service, request.replicas)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to scale service: {}", e)))?;

    // Get updated replica count
    let replicas = manager.service_replica_count(&service).await.unwrap_or(0) as u32;

    // Determine status
    let status = if replicas == 0 {
        "stopped".to_string()
    } else if replicas < request.replicas {
        "scaling".to_string()
    } else {
        "running".to_string()
    };

    // Build endpoints from spec
    let endpoints: Vec<ServiceEndpoint> = spec
        .endpoints
        .iter()
        .map(|ep| ServiceEndpoint {
            name: ep.name.clone(),
            protocol: format!("{:?}", ep.protocol).to_lowercase(),
            port: ep.port,
            url: ep.path.clone(),
        })
        .collect();

    // TODO: Get actual metrics from container runtime
    let metrics = ServiceMetrics {
        cpu_percent: 0.0,
        memory_percent: 0.0,
        rps: None,
    };

    Ok(Json(ServiceDetails {
        name: service,
        deployment,
        status,
        replicas,
        desired_replicas: request.replicas,
        endpoints,
        metrics,
    }))
}

/// Container summary for API responses
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerSummary {
    /// Container identifier (service-rep-N)
    pub id: String,
    /// Service name
    pub service: String,
    /// Replica number
    pub replica: u32,
    /// Container state
    pub state: String,
    /// Process ID (if running)
    pub pid: Option<u32>,
    /// Overlay IP (if assigned)
    pub overlay_ip: Option<String>,
}

/// List containers for a service
#[utoipa::path(
    get,
    path = "/api/v1/deployments/{deployment}/services/{service}/containers",
    params(
        ("deployment" = String, Path, description = "Deployment name"),
        ("service" = String, Path, description = "Service name"),
    ),
    responses(
        (status = 200, description = "List of containers", body = Vec<ContainerSummary>),
        (status = 404, description = "Service not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Services"
)]
pub async fn list_containers(
    _user: AuthUser,
    State(state): State<ServiceState>,
    Path((deployment, service)): Path<(String, String)>,
) -> Result<Json<Vec<ContainerSummary>>> {
    // Verify deployment and service exist in storage
    let stored = state
        .storage
        .get(&deployment)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Deployment '{}' not found", deployment)))?;

    if !stored.spec.services.contains_key(&service) {
        return Err(ApiError::NotFound(format!(
            "Service '{}/{}' not found",
            deployment, service
        )));
    }

    // Get container info from service manager if available
    let mut containers = Vec::new();
    if let Some(ref manager) = state.service_manager {
        let manager = manager.read().await;
        let container_ids = manager.get_service_containers(&service).await;
        for cid in container_ids {
            containers.push(ContainerSummary {
                id: cid.to_string(),
                service: cid.service.clone(),
                replica: cid.replica,
                state: "running".to_string(),
                pid: None,
                overlay_ip: None,
            });
        }
    }

    Ok(Json(containers))
}

/// Get service logs
///
/// Returns service logs as plain text when `follow=false`, or as a Server-Sent
/// Events stream when `follow=true`.  In follow mode the server first emits the
/// last N lines (controlled by the `lines` parameter) and then continuously
/// polls for new output, emitting each new line as a `data:` SSE event.
#[utoipa::path(
    get,
    path = "/api/v1/deployments/{deployment}/services/{service}/logs",
    params(
        ("deployment" = String, Path, description = "Deployment name"),
        ("service" = String, Path, description = "Service name"),
        LogQuery,
    ),
    responses(
        (status = 200, description = "Service logs (plain text or SSE stream)", body = String),
        (status = 404, description = "Service not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Services"
)]
pub async fn get_service_logs(
    _user: AuthUser,
    State(state): State<ServiceState>,
    Path((deployment, service)): Path<(String, String)>,
    Query(query): Query<LogQuery>,
) -> Result<Response> {
    // Verify deployment exists
    let stored = state
        .storage
        .get(&deployment)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Deployment '{}' not found", deployment)))?;

    // Verify service exists in deployment
    if !stored.spec.services.contains_key(&service) {
        return Err(ApiError::NotFound(format!(
            "Service '{}/{}' not found",
            deployment, service
        )));
    }

    // Require service manager for log retrieval
    let manager_arc = state
        .service_manager
        .as_ref()
        .ok_or_else(|| {
            ApiError::ServiceUnavailable(
                "Log retrieval not available: no service manager configured".to_string(),
            )
        })?
        .clone();

    let tail = query.lines as usize;
    let instance = query.instance.clone();

    if !query.follow {
        // ---- Non-follow mode: return plain text as before ----
        let manager = manager_arc.read().await;
        let logs = match manager
            .get_service_logs(&service, tail, instance.as_deref())
            .await
        {
            Ok(logs) => logs,
            Err(zlayer_agent::AgentError::NotFound { reason, .. }) => {
                format!(
                    "# Logs for {}/{}\n# No running containers ({})\n",
                    deployment, service, reason
                )
            }
            Err(e) => return Err(ApiError::Internal(format!("Failed to fetch logs: {}", e))),
        };
        Ok(logs.into_response())
    } else {
        // ---- Follow mode: SSE stream ----
        let stream = log_follow_stream(manager_arc, service, tail, instance);
        let sse = Sse::new(stream).keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        );
        Ok(sse.into_response())
    }
}

/// Build an SSE stream that first emits the last `tail` lines, then polls for
/// new log output every 500 ms and emits new lines as `data:` events.
///
/// The stream runs indefinitely until the client disconnects.  It works across
/// all container runtimes (youki, docker, wasm) because it uses the
/// `ServiceManager::get_service_logs()` abstraction rather than watching files
/// directly.
fn log_follow_stream(
    manager: Arc<RwLock<ServiceManager>>,
    service: String,
    tail: usize,
    instance: Option<String>,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    // We use an `async_stream`-style approach via `tokio_stream::wrappers` and a
    // manual async generator built on `futures_util::stream::unfold`.
    futures_util::stream::unfold(
        LogFollowState {
            manager,
            service,
            instance,
            tail,
            seen_line_count: 0,
            initial: true,
            poll_interval: Duration::from_millis(500),
        },
        |mut state| async move {
            // On the very first iteration we fetch the initial tail.
            // On subsequent iterations we sleep first, then poll for new lines.
            if !state.initial {
                tokio::time::sleep(state.poll_interval).await;
            }

            // Use a large tail value when polling for updates so we see all
            // output.  We track how many lines we have already sent and only
            // emit genuinely new ones.
            let fetch_tail = if state.initial { state.tail } else { 10_000 };

            let mgr = state.manager.read().await;
            let logs = mgr
                .get_service_logs(&state.service, fetch_tail, state.instance.as_deref())
                .await
                .unwrap_or_default();
            drop(mgr);

            let all_lines: Vec<&str> = logs.lines().collect();
            let total = all_lines.len();

            let new_lines = if state.initial {
                // First fetch: emit everything returned by the tail query
                state.initial = false;
                state.seen_line_count = total;
                all_lines
            } else if total > state.seen_line_count {
                // New lines appeared since last poll
                let new = all_lines[state.seen_line_count..].to_vec();
                state.seen_line_count = total;
                new
            } else {
                // Nothing new — yield an empty batch so the stream stays alive
                vec![]
            };

            // Build SSE events for each new line
            let events: Vec<std::result::Result<Event, Infallible>> = new_lines
                .into_iter()
                .map(|line| Ok(Event::default().data(line)))
                .collect();

            debug!(
                service = %state.service,
                new_events = events.len(),
                total_seen = state.seen_line_count,
                "log follow poll"
            );

            Some((futures_util::stream::iter(events), state))
        },
    )
    .flatten()
}

/// Internal state for the log-follow polling stream.
struct LogFollowState {
    manager: Arc<RwLock<ServiceManager>>,
    service: String,
    instance: Option<String>,
    tail: usize,
    /// Number of log lines already sent to the client.
    seen_line_count: usize,
    /// Whether this is the very first poll (initial tail fetch).
    initial: bool,
    /// How often to poll for new log data.
    poll_interval: Duration,
}

/// Exec request body
#[derive(Debug, Deserialize, ToSchema)]
pub struct ExecRequest {
    /// Command and arguments to execute
    pub command: Vec<String>,
    /// Optional replica number to target
    #[serde(default)]
    pub replica: Option<u32>,
}

/// Exec response body
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ExecResponse {
    /// Exit code from the command
    pub exit_code: i32,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
}

/// Execute a command in a service container
#[utoipa::path(
    post,
    path = "/api/v1/deployments/{deployment}/services/{service}/exec",
    params(
        ("deployment" = String, Path, description = "Deployment name"),
        ("service" = String, Path, description = "Service name"),
    ),
    request_body = ExecRequest,
    responses(
        (status = 200, description = "Command executed", body = ExecResponse),
        (status = 404, description = "Service or container not found"),
        (status = 400, description = "Invalid exec request"),
        (status = 401, description = "Unauthorized"),
        (status = 503, description = "Service manager not available"),
    ),
    security(("bearer_auth" = [])),
    tag = "Services"
)]
pub async fn exec_in_service(
    _user: AuthUser,
    State(state): State<ServiceState>,
    Path((deployment, service)): Path<(String, String)>,
    Json(request): Json<ExecRequest>,
) -> Result<Json<ExecResponse>> {
    // Validate command
    if request.command.is_empty() {
        return Err(ApiError::BadRequest("Command cannot be empty".to_string()));
    }

    // Verify deployment and service exist in storage
    let stored = state
        .storage
        .get(&deployment)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Deployment '{}' not found", deployment)))?;

    if !stored.spec.services.contains_key(&service) {
        return Err(ApiError::NotFound(format!(
            "Service '{}/{}' not found",
            deployment, service
        )));
    }

    // Require service manager for exec operations
    let manager_arc = state.service_manager.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable(
            "Exec not available: no service manager configured".to_string(),
        )
    })?;

    let manager = manager_arc.read().await;

    // Execute the command
    let (exit_code, stdout, stderr) = manager
        .exec_in_container(&service, request.replica, &request.command)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {}", reason))
            }
            other => ApiError::Internal(format!("Exec failed: {}", other)),
        })?;

    Ok(Json(ExecResponse {
        exit_code,
        stdout,
        stderr,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_summary_serialize() {
        let summary = ServiceSummary {
            name: "api".to_string(),
            deployment: "my-app".to_string(),
            status: "running".to_string(),
            replicas: 3,
            desired_replicas: 3,
            endpoints: vec![ServiceEndpoint {
                name: "http".to_string(),
                protocol: "http".to_string(),
                port: 8080,
                url: None,
            }],
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("api"));
        assert!(json.contains("my-app"));
    }

    #[test]
    fn test_scale_request_deserialize() {
        let json = r#"{"replicas": 5}"#;
        let request: ScaleRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.replicas, 5);
    }

    #[test]
    fn test_log_query_defaults() {
        let query: LogQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(query.lines, 100);
        assert!(!query.follow);
        assert!(query.instance.is_none());
    }
}
