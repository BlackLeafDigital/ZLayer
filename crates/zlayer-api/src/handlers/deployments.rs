//! Deployment endpoints

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use crate::storage::{DeploymentStorage, StoredDeployment};

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

/// Deployment state holding storage backend
#[derive(Clone)]
pub struct DeploymentState {
    /// Storage backend for deployments
    pub storage: Arc<dyn DeploymentStorage + Send + Sync>,
}

impl DeploymentState {
    /// Create a new deployment state with the given storage backend
    pub fn new(storage: Arc<dyn DeploymentStorage + Send + Sync>) -> Self {
        Self { storage }
    }
}

impl From<&StoredDeployment> for DeploymentDetails {
    fn from(d: &StoredDeployment) -> Self {
        Self {
            name: d.name.clone(),
            status: d.status.to_string(),
            services: d.spec.services.keys().cloned().collect(),
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
    State(state): State<DeploymentState>,
    Path(name): Path<String>,
) -> Result<Json<DeploymentDetails>> {
    let deployment = state
        .storage
        .get(&name)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Deployment '{}' not found", name)))?;

    Ok(Json(DeploymentDetails::from(&deployment)))
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
    State(state): State<DeploymentState>,
    Json(request): Json<CreateDeploymentRequest>,
) -> Result<(StatusCode, Json<DeploymentDetails>)> {
    // Validate and parse spec
    let spec: zlayer_spec::DeploymentSpec = zlayer_spec::from_yaml_str(&request.spec)
        .map_err(|e| ApiError::BadRequest(format!("Invalid spec: {}", e)))?;

    // Create stored deployment
    let deployment = StoredDeployment::new(spec);

    // Store
    state
        .storage
        .store(&deployment)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?;

    // Return details
    Ok((
        StatusCode::CREATED,
        Json(DeploymentDetails::from(&deployment)),
    ))
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
    let deleted = state
        .storage
        .delete(&name)
        .await
        .map_err(|e| ApiError::Internal(format!("Storage error: {}", e)))?;

    if deleted {
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
}
