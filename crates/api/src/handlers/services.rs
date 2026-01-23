//! Service endpoints

use axum::{
    extract::{Path, Query},
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};

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
    Path(_deployment): Path<String>,
) -> Result<Json<Vec<ServiceSummary>>> {
    // TODO: Get from scheduler/storage
    Ok(Json(vec![]))
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
    Path((deployment, service)): Path<(String, String)>,
) -> Result<Json<ServiceDetails>> {
    // TODO: Get from scheduler/storage
    Err(ApiError::NotFound(format!(
        "Service '{}/{}' not found",
        deployment, service
    )))
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
    ),
    security(("bearer_auth" = [])),
    tag = "Services"
)]
pub async fn scale_service(
    user: AuthUser,
    Path((deployment, service)): Path<(String, String)>,
    Json(_request): Json<ScaleRequest>,
) -> Result<Json<ServiceDetails>> {
    // Require admin or operator role
    user.require_role("operator")?;

    // TODO: Scale via scheduler
    Err(ApiError::NotFound(format!(
        "Service '{}/{}' not found",
        deployment, service
    )))
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
    Path((deployment, service)): Path<(String, String)>,
    Query(_query): Query<LogQuery>,
) -> Result<String> {
    // TODO: Get logs from container runtime
    Err(ApiError::NotFound(format!(
        "Service '{}/{}' not found",
        deployment, service
    )))
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
