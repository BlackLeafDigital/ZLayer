//! Deployment endpoints

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use dashmap::DashMap;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tracing::{debug, info, warn};
use utoipa::ToSchema;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use crate::storage::{DeploymentStatus, DeploymentStorage, StoredDeployment};
use zlayer_agent::ServiceManager;

/// Deployment summary
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DeploymentSummary {
    /// Deployment name
    pub name: String,
    /// Deployment status
    pub status: String,
    /// Number of services
    pub service_count: usize,
    /// Created timestamp
    pub created_at: String,
}

/// Per-service health info included in deployment details
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ServiceHealthInfo {
    /// Service name
    pub name: String,
    /// Running replica count
    pub replicas_running: u32,
    /// Desired replica count
    pub replicas_desired: u32,
    /// Health status ("healthy", "unhealthy", "unknown")
    pub health: String,
    /// Endpoint URLs for this service
    pub endpoints: Vec<String>,
}

/// Deployment details (enhanced with per-service health and endpoints)
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DeploymentDetails {
    /// Deployment name
    pub name: String,
    /// Deployment status
    pub status: String,
    /// Service names (for backwards compatibility)
    pub services: Vec<String>,
    /// Per-service health and endpoint info
    pub service_health: Vec<ServiceHealthInfo>,
    /// Created timestamp
    pub created_at: String,
    /// Updated timestamp
    pub updated_at: String,
}

/// Create deployment request
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateDeploymentRequest {
    /// Deployment specification (YAML content)
    pub spec: String,
}

// ============================================================================
// SSE Progress Events
// ============================================================================

/// Deployment progress event sent over SSE during orchestration.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeploymentProgressEvent {
    /// Deployment orchestration has started
    Started {
        /// Deployment name
        deployment: String,
        /// List of services being deployed
        services: Vec<String>,
    },
    /// A service was successfully registered with the service manager
    ServiceRegistered {
        /// Service name
        service: String,
    },
    /// A service failed to register
    ServiceRegistrationFailed {
        /// Service name
        service: String,
        /// Error message
        error: String,
    },
    /// Overlay network created for a service
    OverlayCreated {
        /// Service name
        service: String,
        /// Network interface name
        interface: String,
    },
    /// Overlay creation failed (non-fatal)
    OverlayFailed {
        /// Service name
        service: String,
        /// Error message
        error: String,
    },
    /// Proxy routes configured for a service
    ProxyConfigured {
        /// Service name
        service: String,
    },
    /// Proxy configuration failed (non-fatal)
    ProxyFailed {
        /// Service name
        service: String,
        /// Error message
        error: String,
    },
    /// Service scaling has started
    ServiceScaling {
        /// Service name
        service: String,
        /// Target replica count
        target: u32,
    },
    /// Service successfully scaled
    ServiceScaled {
        /// Service name
        service: String,
        /// Number of replicas running
        replicas: u32,
    },
    /// Service scaling failed
    ServiceScaleFailed {
        /// Service name
        service: String,
        /// Error message
        error: String,
    },
    /// Waiting for stabilization
    Stabilizing,
    /// Deployment is ready and running
    Ready,
    /// Deployment failed
    Failed {
        /// Error message describing the failure
        message: String,
    },
}

/// Wrapper for serializing deployment progress events as SSE.
///
/// Converts each [`DeploymentProgressEvent`] variant into an `event_type` string
/// and a JSON `data` payload, following the same pattern as [`super::build::BuildEventWrapper`].
#[derive(Debug, Clone, Serialize)]
pub struct DeploymentEventWrapper {
    /// SSE event type (used as the `event:` field)
    #[serde(rename = "type")]
    pub event_type: String,
    /// JSON data payload
    pub data: serde_json::Value,
}

impl From<DeploymentProgressEvent> for DeploymentEventWrapper {
    fn from(event: DeploymentProgressEvent) -> Self {
        match event {
            DeploymentProgressEvent::Started {
                deployment,
                services,
            } => DeploymentEventWrapper {
                event_type: "started".to_string(),
                data: serde_json::json!({
                    "deployment": deployment,
                    "services": services,
                }),
            },
            DeploymentProgressEvent::ServiceRegistered { service } => DeploymentEventWrapper {
                event_type: "service_registered".to_string(),
                data: serde_json::json!({ "service": service }),
            },
            DeploymentProgressEvent::ServiceRegistrationFailed { service, error } => {
                DeploymentEventWrapper {
                    event_type: "service_registration_failed".to_string(),
                    data: serde_json::json!({ "service": service, "error": error }),
                }
            }
            DeploymentProgressEvent::OverlayCreated { service, interface } => {
                DeploymentEventWrapper {
                    event_type: "overlay_created".to_string(),
                    data: serde_json::json!({ "service": service, "interface": interface }),
                }
            }
            DeploymentProgressEvent::OverlayFailed { service, error } => DeploymentEventWrapper {
                event_type: "overlay_failed".to_string(),
                data: serde_json::json!({ "service": service, "error": error }),
            },
            DeploymentProgressEvent::ProxyConfigured { service } => DeploymentEventWrapper {
                event_type: "proxy_configured".to_string(),
                data: serde_json::json!({ "service": service }),
            },
            DeploymentProgressEvent::ProxyFailed { service, error } => DeploymentEventWrapper {
                event_type: "proxy_failed".to_string(),
                data: serde_json::json!({ "service": service, "error": error }),
            },
            DeploymentProgressEvent::ServiceScaling { service, target } => DeploymentEventWrapper {
                event_type: "service_scaling".to_string(),
                data: serde_json::json!({ "service": service, "target": target }),
            },
            DeploymentProgressEvent::ServiceScaled { service, replicas } => {
                DeploymentEventWrapper {
                    event_type: "service_scaled".to_string(),
                    data: serde_json::json!({ "service": service, "replicas": replicas }),
                }
            }
            DeploymentProgressEvent::ServiceScaleFailed { service, error } => {
                DeploymentEventWrapper {
                    event_type: "service_scale_failed".to_string(),
                    data: serde_json::json!({ "service": service, "error": error }),
                }
            }
            DeploymentProgressEvent::Stabilizing => DeploymentEventWrapper {
                event_type: "stabilizing".to_string(),
                data: serde_json::json!({}),
            },
            DeploymentProgressEvent::Ready => DeploymentEventWrapper {
                event_type: "ready".to_string(),
                data: serde_json::json!({}),
            },
            DeploymentProgressEvent::Failed { message } => DeploymentEventWrapper {
                event_type: "failed".to_string(),
                data: serde_json::json!({ "message": message }),
            },
        }
    }
}

/// Helper to send a deployment progress event on the broadcast channel.
///
/// Silently ignores send errors (no subscribers, or channel closed).
fn emit_progress(
    tx: Option<&broadcast::Sender<DeploymentEventWrapper>>,
    event: DeploymentProgressEvent,
) {
    if let Some(tx) = tx {
        let _ = tx.send(DeploymentEventWrapper::from(event));
    }
}

/// Deployment state holding storage backend and optional orchestration handles.
///
/// When `service_manager` is `Some`, the `create_deployment` handler will
/// actually orchestrate (register services, set up networking, scale) rather
/// than just storing the spec.
#[derive(Clone)]
pub struct DeploymentState {
    /// Storage backend for deployments
    pub storage: Arc<dyn DeploymentStorage + Send + Sync>,
    /// Optional service manager for orchestration (behind `RwLock` for compatibility
    /// with the rest of the router, even though `ServiceManager` uses internal locking)
    pub service_manager: Option<Arc<RwLock<ServiceManager>>>,
    /// Optional overlay manager for network setup
    pub overlay: Option<Arc<RwLock<zlayer_agent::OverlayManager>>>,
    /// Optional proxy manager for route/port setup
    pub proxy: Option<Arc<zlayer_agent::ProxyManager>>,
    /// DNS handle for adding/removing service discovery records at runtime.
    /// Kept here to ensure the handle (and its background listener) stays alive
    /// for the lifetime of the API server.
    pub dns_handle: Option<zlayer_overlay::DnsHandle>,
    /// Active SSE event channels keyed by deployment name.
    ///
    /// A channel is inserted when `create_deployment` spawns orchestration and
    /// removed when the orchestration task finishes (sender is dropped).
    pub event_channels: Arc<DashMap<String, broadcast::Sender<DeploymentEventWrapper>>>,
}

impl DeploymentState {
    /// Create a new deployment state with the given storage backend (no orchestration)
    pub fn new(storage: Arc<dyn DeploymentStorage + Send + Sync>) -> Self {
        Self {
            storage,
            service_manager: None,
            overlay: None,
            proxy: None,
            dns_handle: None,
            event_channels: Arc::new(DashMap::new()),
        }
    }

    /// Create a deployment state wired for full orchestration
    pub fn with_orchestration(
        storage: Arc<dyn DeploymentStorage + Send + Sync>,
        service_manager: Arc<RwLock<ServiceManager>>,
        overlay: Option<Arc<RwLock<zlayer_agent::OverlayManager>>>,
        proxy: Arc<zlayer_agent::ProxyManager>,
        dns_handle: Option<zlayer_overlay::DnsHandle>,
    ) -> Self {
        Self {
            storage,
            service_manager: Some(service_manager),
            overlay,
            proxy: Some(proxy),
            dns_handle,
            event_channels: Arc::new(DashMap::new()),
        }
    }

    /// Build per-service health info from a stored deployment.
    ///
    /// If a service manager is available, queries live replica counts and health.
    /// Otherwise, returns static info from the spec.
    #[allow(clippy::cast_possible_truncation)]
    async fn build_service_health(&self, stored: &StoredDeployment) -> Vec<ServiceHealthInfo> {
        let mut infos = Vec::with_capacity(stored.spec.services.len());

        for (name, service_spec) in &stored.spec.services {
            let desired = match &service_spec.scale {
                zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
                zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
                zlayer_spec::ScaleSpec::Manual => 0,
            };

            let (running, health) = if let Some(ref mgr_lock) = self.service_manager {
                let mgr = mgr_lock.read().await;
                let running = mgr.service_replica_count(name).await.unwrap_or(0) as u32;

                let health_states = mgr.health_states();
                let states = health_states.read().await;
                let h = match states.get(name) {
                    Some(zlayer_agent::HealthState::Healthy) => "healthy".to_string(),
                    Some(zlayer_agent::HealthState::Unhealthy { reason, .. }) => {
                        format!("unhealthy: {reason}")
                    }
                    Some(zlayer_agent::HealthState::Checking) => "checking".to_string(),
                    _ => "unknown".to_string(),
                };
                (running, h)
            } else {
                // No service manager -- return spec-based info
                (desired, "unknown".to_string())
            };

            let endpoints: Vec<String> = service_spec
                .endpoints
                .iter()
                .map(|ep| {
                    let proto = match ep.protocol {
                        zlayer_spec::Protocol::Http => "http",
                        zlayer_spec::Protocol::Https => "https",
                        zlayer_spec::Protocol::Tcp => "tcp",
                        zlayer_spec::Protocol::Udp => "udp",
                        zlayer_spec::Protocol::Websocket => "ws",
                    };
                    format!("{}://localhost:{}", proto, ep.port)
                })
                .collect();

            infos.push(ServiceHealthInfo {
                name: name.clone(),
                replicas_running: running,
                replicas_desired: desired,
                health,
                endpoints,
            });
        }

        infos
    }
}

impl DeploymentDetails {
    /// Build deployment details from stored deployment and optional live health info.
    fn from_stored(d: &StoredDeployment, service_health: Vec<ServiceHealthInfo>) -> Self {
        Self {
            name: d.name.clone(),
            status: d.status.to_string(),
            services: d.spec.services.keys().cloned().collect(),
            service_health,
            created_at: d.created_at.to_rfc3339(),
            updated_at: d.updated_at.to_rfc3339(),
        }
    }
}

impl From<&StoredDeployment> for DeploymentDetails {
    fn from(d: &StoredDeployment) -> Self {
        // Backwards-compatible: no live health info, build from spec
        let service_health: Vec<ServiceHealthInfo> = d
            .spec
            .services
            .iter()
            .map(|(name, svc)| {
                let desired = match &svc.scale {
                    zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
                    zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
                    zlayer_spec::ScaleSpec::Manual => 0,
                };
                let endpoints: Vec<String> = svc
                    .endpoints
                    .iter()
                    .map(|ep| {
                        let proto = match ep.protocol {
                            zlayer_spec::Protocol::Http => "http",
                            zlayer_spec::Protocol::Https => "https",
                            zlayer_spec::Protocol::Tcp => "tcp",
                            zlayer_spec::Protocol::Udp => "udp",
                            zlayer_spec::Protocol::Websocket => "ws",
                        };
                        format!("{}://localhost:{}", proto, ep.port)
                    })
                    .collect();
                ServiceHealthInfo {
                    name: name.clone(),
                    replicas_running: desired,
                    replicas_desired: desired,
                    health: "unknown".to_string(),
                    endpoints,
                }
            })
            .collect();

        Self {
            name: d.name.clone(),
            status: d.status.to_string(),
            services: d.spec.services.keys().cloned().collect(),
            service_health,
            created_at: d.created_at.to_rfc3339(),
            updated_at: d.updated_at.to_rfc3339(),
        }
    }
}

impl From<&StoredDeployment> for DeploymentSummary {
    fn from(d: &StoredDeployment) -> Self {
        Self {
            name: d.name.clone(),
            status: d.status.to_string(),
            service_count: d.spec.services.len(),
            created_at: d.created_at.to_rfc3339(),
        }
    }
}

/// List all deployments.
///
/// # Errors
///
/// Returns an error if storage access fails.
#[utoipa::path(
    get,
    path = "/api/v1/deployments",
    responses(
        (status = 200, description = "List of deployments", body = Vec<DeploymentSummary>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Deployments"
)]
pub async fn list_deployments(
    _user: AuthUser,
    State(state): State<DeploymentState>,
) -> Result<Json<Vec<DeploymentSummary>>> {
    let deployments = state
        .storage
        .list()
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {e}")))?;

    let summaries: Vec<DeploymentSummary> =
        deployments.iter().map(DeploymentSummary::from).collect();
    Ok(Json(summaries))
}

/// Get deployment details (with live per-service health when available).
///
/// # Errors
///
/// Returns an error if the deployment is not found or storage access fails.
#[utoipa::path(
    get,
    path = "/api/v1/deployments/{name}",
    params(
        ("name" = String, Path, description = "Deployment name"),
    ),
    responses(
        (status = 200, description = "Deployment details", body = DeploymentDetails),
        (status = 404, description = "Deployment not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Deployments"
)]
pub async fn get_deployment(
    _user: AuthUser,
    State(state): State<DeploymentState>,
    Path(name): Path<String>,
) -> Result<Json<DeploymentDetails>> {
    let deployment = state
        .storage
        .get(&name)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Deployment '{name}' not found")))?;

    let service_health = state.build_service_health(&deployment).await;
    Ok(Json(DeploymentDetails::from_stored(
        &deployment,
        service_health,
    )))
}

/// Create a new deployment.
///
/// When the daemon has orchestration wired (service manager, proxy, overlay),
/// this handler:
///  1. Parses and validates the spec YAML
///  2. Stores the deployment with status `Deploying`
///  3. Spawns an async task that registers services, sets up overlays,
///     configures the proxy, and scales services
///  4. Returns immediately with `Deploying` status
///  5. The async task updates the stored status to `Running` or `Failed`
///
/// Without orchestration wired, it stores the spec with `Pending` status.
///
/// # Errors
///
/// Returns an error if the spec is invalid or storage fails.
#[utoipa::path(
    post,
    path = "/api/v1/deployments",
    request_body = CreateDeploymentRequest,
    responses(
        (status = 201, description = "Deployment created", body = DeploymentDetails),
        (status = 400, description = "Invalid specification"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Deployments"
)]
pub async fn create_deployment(
    user: AuthUser,
    State(state): State<DeploymentState>,
    Json(request): Json<CreateDeploymentRequest>,
) -> Result<(StatusCode, Json<DeploymentDetails>)> {
    user.require_role("operator")?;

    // Validate and parse spec
    let spec: zlayer_spec::DeploymentSpec = zlayer_spec::from_yaml_str(&request.spec)
        .map_err(|e| ApiError::BadRequest(format!("Invalid spec: {e}")))?;

    let deployment_name = spec.deployment.clone();

    // If we have orchestration, set status to Deploying and spawn background task
    if state.service_manager.is_some() {
        let mut deployment = StoredDeployment::new(spec.clone());
        deployment.update_status(DeploymentStatus::Deploying);

        state
            .storage
            .store(&deployment)
            .await
            .map_err(|e| ApiError::Internal(format!("Storage error: {e}")))?;

        let details = DeploymentDetails::from(&deployment);

        // Create a broadcast channel for SSE progress events
        let (event_tx, _) = broadcast::channel::<DeploymentEventWrapper>(256);
        state
            .event_channels
            .insert(deployment_name.clone(), event_tx.clone());

        // Spawn background orchestration task
        let state_clone = state.clone();
        let spec_clone = spec.clone();
        let deploy_name_clone = deployment_name.clone();
        tokio::spawn(async move {
            orchestrate_deployment(state_clone.clone(), spec_clone, Some(event_tx)).await;
            // Clean up the event channel once orchestration is done (sender dropped
            // by the function, so subscribers see the stream end).
            state_clone.event_channels.remove(&deploy_name_clone);
        });

        info!(deployment = %deployment_name, "Deployment submitted for orchestration");

        Ok((StatusCode::CREATED, Json(details)))
    } else {
        // No orchestration: just store with Pending status
        let deployment = StoredDeployment::new(spec);

        state
            .storage
            .store(&deployment)
            .await
            .map_err(|e| ApiError::Internal(format!("Storage error: {e}")))?;

        Ok((
            StatusCode::CREATED,
            Json(DeploymentDetails::from(&deployment)),
        ))
    }
}

/// Background orchestration task for a deployment.
///
/// Registers each service with the `ServiceManager`, sets up overlay networks,
/// configures proxy routes, scales to desired replicas, then waits for
/// stabilization. Updates the stored deployment status to Running or Failed.
///
/// When `event_tx` is `Some`, progress events are broadcast to any SSE
/// subscribers. The sender is consumed (dropped) at the end so that subscribers
/// see the stream close.
#[allow(clippy::too_many_lines)]
async fn orchestrate_deployment(
    state: DeploymentState,
    spec: zlayer_spec::DeploymentSpec,
    event_tx: Option<broadcast::Sender<DeploymentEventWrapper>>,
) {
    let deployment_name = spec.deployment.clone();
    info!(deployment = %deployment_name, "Starting deployment orchestration");

    let mgr_lock = if let Some(m) = &state.service_manager {
        Arc::clone(m)
    } else {
        warn!(deployment = %deployment_name, "No service manager available for orchestration");
        emit_progress(
            event_tx.as_ref(),
            DeploymentProgressEvent::Failed {
                message: "No service manager available".to_string(),
            },
        );
        return;
    };

    // Emit Started event
    let service_names: Vec<String> = spec.services.keys().cloned().collect();
    emit_progress(
        event_tx.as_ref(),
        DeploymentProgressEvent::Started {
            deployment: deployment_name.clone(),
            services: service_names,
        },
    );

    let mut errors: Vec<String> = Vec::new();

    for (name, service_spec) in &spec.services {
        // 1. Register the service with ServiceManager
        {
            let mgr = mgr_lock.read().await;
            if let Err(e) = mgr.upsert_service(name.clone(), service_spec.clone()).await {
                let msg = format!("{name}: failed to register: {e}");
                warn!(deployment = %deployment_name, service = %name, error = %e, "Service registration failed");
                emit_progress(
                    event_tx.as_ref(),
                    DeploymentProgressEvent::ServiceRegistrationFailed {
                        service: name.clone(),
                        error: e.to_string(),
                    },
                );
                errors.push(msg);
                continue;
            }
        }
        emit_progress(
            event_tx.as_ref(),
            DeploymentProgressEvent::ServiceRegistered {
                service: name.clone(),
            },
        );

        // 2. Set up service overlay network (non-fatal if unavailable)
        if let Some(om) = &state.overlay {
            let om_guard = om.read().await;
            match om_guard.setup_service_overlay(name).await {
                Ok(iface) => {
                    info!(
                        deployment = %deployment_name,
                        service = %name,
                        interface = %iface,
                        "Service overlay created"
                    );
                    emit_progress(
                        event_tx.as_ref(),
                        DeploymentProgressEvent::OverlayCreated {
                            service: name.clone(),
                            interface: iface,
                        },
                    );
                }
                Err(e) => {
                    warn!(
                        deployment = %deployment_name,
                        service = %name,
                        error = %e,
                        "Failed to create service overlay (non-fatal)"
                    );
                    emit_progress(
                        event_tx.as_ref(),
                        DeploymentProgressEvent::OverlayFailed {
                            service: name.clone(),
                            error: e.to_string(),
                        },
                    );
                }
            }
        }

        // 3. Register proxy routes and ensure listening ports
        if let Some(proxy) = &state.proxy {
            let overlay_ip: Option<std::net::IpAddr> = if let Some(om) = &state.overlay {
                om.read().await.node_ip().map(std::net::IpAddr::V4)
            } else {
                None
            };
            proxy.add_service(name, service_spec).await;
            if let Err(e) = proxy
                .ensure_ports_for_service(service_spec, overlay_ip)
                .await
            {
                warn!(
                    deployment = %deployment_name,
                    service = %name,
                    error = %e,
                    "Failed to setup proxy ports (non-fatal)"
                );
                emit_progress(
                    event_tx.as_ref(),
                    DeploymentProgressEvent::ProxyFailed {
                        service: name.clone(),
                        error: e.to_string(),
                    },
                );
            } else {
                emit_progress(
                    event_tx.as_ref(),
                    DeploymentProgressEvent::ProxyConfigured {
                        service: name.clone(),
                    },
                );
            }
        }

        // 4. Scale to the desired replica count
        let target_replicas = match &service_spec.scale {
            zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
            zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
            zlayer_spec::ScaleSpec::Manual => 0,
        };

        if target_replicas > 0 {
            emit_progress(
                event_tx.as_ref(),
                DeploymentProgressEvent::ServiceScaling {
                    service: name.clone(),
                    target: target_replicas,
                },
            );

            let mgr = mgr_lock.read().await;
            if let Err(e) = mgr.scale_service(name, target_replicas).await {
                let msg = format!("{name}: failed to scale to {target_replicas}: {e}");
                warn!(
                    deployment = %deployment_name,
                    service = %name,
                    target = target_replicas,
                    error = %e,
                    "Service scaling failed"
                );
                emit_progress(
                    event_tx.as_ref(),
                    DeploymentProgressEvent::ServiceScaleFailed {
                        service: name.clone(),
                        error: e.to_string(),
                    },
                );
                errors.push(msg);
            } else {
                info!(
                    deployment = %deployment_name,
                    service = %name,
                    replicas = target_replicas,
                    "Service scaled"
                );
                emit_progress(
                    event_tx.as_ref(),
                    DeploymentProgressEvent::ServiceScaled {
                        service: name.clone(),
                        replicas: target_replicas,
                    },
                );
            }
        }
    }

    // 5. Wait for stabilization (30s timeout)
    emit_progress(event_tx.as_ref(), DeploymentProgressEvent::Stabilizing);

    let final_status = if errors.is_empty() {
        let stabilization_timeout = Duration::from_secs(30);
        let mgr = mgr_lock.read().await;
        let result =
            zlayer_agent::stabilization::wait_for_stabilization(&mgr, &spec, stabilization_timeout)
                .await;
        drop(mgr);

        match result.outcome {
            zlayer_agent::stabilization::StabilizationOutcome::Ready => DeploymentStatus::Running,
            zlayer_agent::stabilization::StabilizationOutcome::TimedOut { message } => {
                DeploymentStatus::Failed { message }
            }
        }
    } else {
        DeploymentStatus::Failed {
            message: format!("{} service(s) failed: {}", errors.len(), errors.join("; ")),
        }
    };

    // Emit terminal progress event
    match &final_status {
        DeploymentStatus::Running => {
            emit_progress(event_tx.as_ref(), DeploymentProgressEvent::Ready);
        }
        DeploymentStatus::Failed { message } => {
            emit_progress(
                event_tx.as_ref(),
                DeploymentProgressEvent::Failed {
                    message: message.clone(),
                },
            );
        }
        _ => {}
    }

    // 6. Update stored deployment status
    match state.storage.get(&deployment_name).await {
        Ok(Some(mut stored)) => {
            stored.update_status(final_status.clone());
            if let Err(e) = state.storage.store(&stored).await {
                warn!(
                    deployment = %deployment_name,
                    error = %e,
                    "Failed to update deployment status in storage"
                );
            } else {
                info!(
                    deployment = %deployment_name,
                    status = %final_status,
                    "Deployment orchestration complete"
                );
            }
        }
        Ok(None) => {
            warn!(
                deployment = %deployment_name,
                "Deployment disappeared from storage during orchestration"
            );
        }
        Err(e) => {
            warn!(
                deployment = %deployment_name,
                error = %e,
                "Failed to fetch deployment from storage for status update"
            );
        }
    }

    // event_tx is dropped here, closing the broadcast channel and ending all
    // subscriber streams.
}

/// GET /api/v1/deployments/{name}/events
/// Stream deployment orchestration progress via Server-Sent Events.
///
/// If orchestration is currently in progress, subscribes to the live broadcast
/// channel and streams events as they occur. If orchestration has already
/// completed, checks the stored deployment status and emits a single terminal
/// event (`ready` or `failed`) before closing the stream.
///
/// # Errors
///
/// Returns 404 if the deployment does not exist at all.
#[utoipa::path(
    get,
    path = "/api/v1/deployments/{name}/events",
    params(
        ("name" = String, Path, description = "Deployment name"),
    ),
    responses(
        (status = 200, description = "SSE event stream"),
        (status = 404, description = "Deployment not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Deployments"
)]
pub async fn stream_deployment_events(
    _user: AuthUser,
    State(state): State<DeploymentState>,
    Path(name): Path<String>,
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>> {
    // Try to subscribe to an active orchestration channel
    if let Some(tx) = state.event_channels.get(&name) {
        let rx = tx.subscribe();
        drop(tx); // release DashMap ref

        debug!(deployment = %name, "SSE client subscribed to deployment events");

        let stream = BroadcastStream::new(rx).map(|result| {
            let wrapper = match result {
                Ok(w) => w,
                Err(_) => DeploymentEventWrapper {
                    event_type: "error".to_string(),
                    data: serde_json::json!({"message": "Stream error"}),
                },
            };

            Ok::<_, Infallible>(
                Event::default()
                    .event(&wrapper.event_type)
                    .json_data(&wrapper.data)
                    .unwrap_or_else(|_| Event::default().data("error")),
            )
        });

        let boxed: futures_util::stream::BoxStream<
            'static,
            std::result::Result<Event, Infallible>,
        > = Box::pin(stream);
        return Ok(Sse::new(boxed).keep_alive(KeepAlive::default()));
    }

    // No active channel -- check if the deployment exists and has a terminal status
    let stored = state
        .storage
        .get(&name)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Deployment '{name}' not found")))?;

    debug!(
        deployment = %name,
        status = %stored.status,
        "No active SSE channel; sending terminal status"
    );

    // Build a single terminal event based on stored status
    let terminal_event = match &stored.status {
        DeploymentStatus::Running => DeploymentEventWrapper::from(DeploymentProgressEvent::Ready),
        DeploymentStatus::Failed { message } => {
            DeploymentEventWrapper::from(DeploymentProgressEvent::Failed {
                message: message.clone(),
            })
        }
        other => DeploymentEventWrapper {
            event_type: "status".to_string(),
            data: serde_json::json!({ "status": other.to_string() }),
        },
    };

    // Return a single-item stream
    let stream = futures_util::stream::once(async move {
        Ok::<_, Infallible>(
            Event::default()
                .event(&terminal_event.event_type)
                .json_data(&terminal_event.data)
                .unwrap_or_else(|_| Event::default().data("error")),
        )
    });

    let boxed: futures_util::stream::BoxStream<'static, std::result::Result<Event, Infallible>> =
        Box::pin(stream);
    Ok(Sse::new(boxed).keep_alive(KeepAlive::default()))
}

/// Delete a deployment.
///
/// # Errors
///
/// Returns an error if the deployment is not found, storage access fails, or teardown
/// encounters critical failures.
#[utoipa::path(
    delete,
    path = "/api/v1/deployments/{name}",
    params(
        ("name" = String, Path, description = "Deployment name"),
    ),
    responses(
        (status = 204, description = "Deployment deleted"),
        (status = 404, description = "Deployment not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Deployments"
)]
pub async fn delete_deployment(
    user: AuthUser,
    State(state): State<DeploymentState>,
    Path(name): Path<String>,
) -> Result<StatusCode> {
    user.require_role("operator")?;

    // Load the deployment to get service specs for full teardown
    let stored = state
        .storage
        .get(&name)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {e}")))?;

    if let Some(stored) = &stored {
        for service_name in stored.spec.services.keys() {
            // 1. Remove proxy routes/ports BEFORE scaling down so in-flight
            //    requests get clean connection-refused instead of routing to
            //    half-dead containers.
            if let Some(ref proxy) = state.proxy {
                proxy.remove_service(service_name).await;
                info!(
                    deployment = %name,
                    service = %service_name,
                    "Removed proxy routes for service"
                );
            }

            // 2. Tear down the service overlay network interface
            if let Some(ref overlay) = state.overlay {
                let om = overlay.read().await;
                om.teardown_service_overlay(service_name).await;
                info!(
                    deployment = %name,
                    service = %service_name,
                    "Tore down service overlay"
                );
            }

            // 3. Scale to 0 and remove from service manager
            if let Some(ref mgr_lock) = state.service_manager {
                let mgr = mgr_lock.read().await;

                if let Err(e) = mgr.scale_service(service_name, 0).await {
                    warn!(
                        deployment = %name,
                        service = %service_name,
                        error = %e,
                        "Failed to scale service to 0 during deletion"
                    );
                }

                if let Err(e) = mgr.remove_service(service_name).await {
                    warn!(
                        deployment = %name,
                        service = %service_name,
                        error = %e,
                        "Failed to remove service during deletion"
                    );
                }
            }
        }
    }

    let deleted = state
        .storage
        .delete(&name)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {e}")))?;

    if deleted {
        info!(deployment = %name, "Deployment deleted");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("Deployment '{name}' not found")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deployment_summary_serialize() {
        let summary = DeploymentSummary {
            name: "test-app".to_string(),
            status: "running".to_string(),
            service_count: 3,
            created_at: "2025-01-22T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("test-app"));
        assert!(json.contains("running"));
    }

    #[test]
    fn test_create_deployment_request_deserialize() {
        let json = r#"{"spec": "version: v1\ndeployment: test"}"#;
        let request: CreateDeploymentRequest = serde_json::from_str(json).unwrap();
        assert!(request.spec.contains("v1"));
    }

    #[test]
    fn test_service_health_info_serialize() {
        let info = ServiceHealthInfo {
            name: "web".to_string(),
            replicas_running: 2,
            replicas_desired: 3,
            health: "healthy".to_string(),
            endpoints: vec!["http://localhost:8080".to_string()],
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("web"));
        assert!(json.contains("replicas_running"));
        assert!(json.contains("http://localhost:8080"));
    }

    #[test]
    fn test_deployment_details_service_health_field() {
        let details = DeploymentDetails {
            name: "test".to_string(),
            status: "running".to_string(),
            services: vec!["web".to_string()],
            service_health: vec![ServiceHealthInfo {
                name: "web".to_string(),
                replicas_running: 1,
                replicas_desired: 1,
                health: "healthy".to_string(),
                endpoints: vec![],
            }],
            created_at: "2025-01-22T00:00:00Z".to_string(),
            updated_at: "2025-01-22T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&details).unwrap();
        assert!(json.contains("service_health"));
    }
}
