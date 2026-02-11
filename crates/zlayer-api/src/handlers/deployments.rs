//! Deployment endpoints

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};
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

/// Deployment state holding storage backend and optional orchestration handles.
///
/// When `service_manager` is `Some`, the `create_deployment` handler will
/// actually orchestrate (register services, set up networking, scale) rather
/// than just storing the spec.
#[derive(Clone)]
pub struct DeploymentState {
    /// Storage backend for deployments
    pub storage: Arc<dyn DeploymentStorage + Send + Sync>,
    /// Optional service manager for orchestration (behind RwLock for compatibility
    /// with the rest of the router, even though ServiceManager uses internal locking)
    pub service_manager: Option<Arc<RwLock<ServiceManager>>>,
    /// Optional overlay manager for network setup
    pub overlay: Option<Arc<RwLock<zlayer_agent::OverlayManager>>>,
    /// Optional proxy manager for route/port setup
    pub proxy: Option<Arc<zlayer_agent::ProxyManager>>,
    /// DNS handle for adding/removing service discovery records at runtime.
    /// Kept here to ensure the handle (and its background listener) stays alive
    /// for the lifetime of the API server.
    pub dns_handle: Option<zlayer_overlay::DnsHandle>,
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
        }
    }

    /// Build per-service health info from a stored deployment.
    ///
    /// If a service manager is available, queries live replica counts and health.
    /// Otherwise, returns static info from the spec.
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
                        format!("unhealthy: {}", reason)
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

/// List all deployments
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
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?;

    let summaries: Vec<DeploymentSummary> =
        deployments.iter().map(DeploymentSummary::from).collect();
    Ok(Json(summaries))
}

/// Get deployment details (with live per-service health when available)
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
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Deployment '{}' not found", name)))?;

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
    _user: AuthUser,
    State(state): State<DeploymentState>,
    Json(request): Json<CreateDeploymentRequest>,
) -> Result<(StatusCode, Json<DeploymentDetails>)> {
    // Validate and parse spec
    let spec: zlayer_spec::DeploymentSpec = zlayer_spec::from_yaml_str(&request.spec)
        .map_err(|e| ApiError::BadRequest(format!("Invalid spec: {}", e)))?;

    let deployment_name = spec.deployment.clone();

    // If we have orchestration, set status to Deploying and spawn background task
    if state.service_manager.is_some() {
        let mut deployment = StoredDeployment::new(spec.clone());
        deployment.update_status(DeploymentStatus::Deploying);

        state
            .storage
            .store(&deployment)
            .await
            .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?;

        let details = DeploymentDetails::from(&deployment);

        // Spawn background orchestration task
        let state_clone = state.clone();
        let spec_clone = spec.clone();
        tokio::spawn(async move {
            orchestrate_deployment(state_clone, spec_clone).await;
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
            .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?;

        Ok((
            StatusCode::CREATED,
            Json(DeploymentDetails::from(&deployment)),
        ))
    }
}

/// Background orchestration task for a deployment.
///
/// Registers each service with the ServiceManager, sets up overlay networks,
/// configures proxy routes, scales to desired replicas, then waits for
/// stabilization. Updates the stored deployment status to Running or Failed.
async fn orchestrate_deployment(state: DeploymentState, spec: zlayer_spec::DeploymentSpec) {
    let deployment_name = spec.deployment.clone();
    info!(deployment = %deployment_name, "Starting deployment orchestration");

    let mgr_lock = match &state.service_manager {
        Some(m) => Arc::clone(m),
        None => {
            warn!(deployment = %deployment_name, "No service manager available for orchestration");
            return;
        }
    };

    let mut errors: Vec<String> = Vec::new();

    for (name, service_spec) in &spec.services {
        // 1. Register the service with ServiceManager
        {
            let mgr = mgr_lock.read().await;
            if let Err(e) = mgr.upsert_service(name.clone(), service_spec.clone()).await {
                let msg = format!("{name}: failed to register: {e}");
                warn!(deployment = %deployment_name, service = %name, error = %e, "Service registration failed");
                errors.push(msg);
                continue;
            }
        }

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
                }
                Err(e) => {
                    warn!(
                        deployment = %deployment_name,
                        service = %name,
                        error = %e,
                        "Failed to create service overlay (non-fatal)"
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
            }
        }

        // 4. Scale to the desired replica count
        let target_replicas = match &service_spec.scale {
            zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
            zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
            zlayer_spec::ScaleSpec::Manual => 0,
        };

        if target_replicas > 0 {
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
                errors.push(msg);
            } else {
                info!(
                    deployment = %deployment_name,
                    service = %name,
                    replicas = target_replicas,
                    "Service scaled"
                );
            }
        }
    }

    // 5. Wait for stabilization (30s timeout)
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
}

/// Delete a deployment
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
    _user: AuthUser,
    State(state): State<DeploymentState>,
    Path(name): Path<String>,
) -> Result<StatusCode> {
    // Load the deployment to get service specs for full teardown
    let stored = state
        .storage
        .get(&name)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?;

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
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?;

    if deleted {
        info!(deployment = %name, "Deployment deleted");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!(
            "Deployment '{}' not found",
            name
        )))
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
