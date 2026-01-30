//! Service endpoints

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
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

/// Get service logs
#[utoipa::path(
    get,
    path = "/api/v1/deployments/{deployment}/services/{service}/logs",
    params(
        ("deployment" = String, Path, description = "Deployment name"),
        ("service" = String, Path, description = "Service name"),
        LogQuery,
    ),
    responses(
        (status = 200, description = "Service logs", body = String),
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
    Query(_query): Query<LogQuery>,
) -> Result<String> {
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

    // TODO: Get actual logs from container runtime
    // For now, return placeholder indicating logs are not yet implemented
    Ok(format!(
        "# Logs for {}/{}\n# Log streaming not yet implemented\n",
        deployment, service
    ))
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
