//! Deployment endpoints

use axum::{extract::Path, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};

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

/// Deployment details
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DeploymentDetails {
    /// Deployment name
    pub name: String,
    /// Deployment status
    pub status: String,
    /// Service names
    pub services: Vec<String>,
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
pub async fn list_deployments(_user: AuthUser) -> Result<Json<Vec<DeploymentSummary>>> {
    // TODO: Get from storage
    Ok(Json(vec![]))
}

/// Get deployment details
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
    Path(name): Path<String>,
) -> Result<Json<DeploymentDetails>> {
    // TODO: Get from storage
    Err(ApiError::NotFound(format!(
        "Deployment '{}' not found",
        name
    )))
}

/// Create a new deployment
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
    Json(request): Json<CreateDeploymentRequest>,
) -> Result<Json<DeploymentDetails>> {
    // Validate and parse spec
    let _spec: spec::DeploymentSpec = spec::from_yaml_str(&request.spec)
        .map_err(|e| ApiError::BadRequest(format!("Invalid spec: {}", e)))?;

    // TODO: Store and deploy
    Err(ApiError::Internal("Not implemented".to_string()))
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
    Path(name): Path<String>,
) -> Result<axum::http::StatusCode> {
    // TODO: Delete from storage
    Err(ApiError::NotFound(format!(
        "Deployment '{}' not found",
        name
    )))
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
}
