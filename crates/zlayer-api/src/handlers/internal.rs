//! Internal API endpoints for scheduler-to-agent communication
//!
//! These endpoints are used by the distributed scheduler to trigger operations
//! on agents. They use a shared secret for authentication rather than JWT tokens.

use std::sync::Arc;

use axum::{
    extract::{FromRequestParts, State},
    http::{header::HeaderValue, request::Parts},
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};
use utoipa::ToSchema;

use crate::error::{ApiError, Result};
use zlayer_agent::ServiceManager;

/// Header name for internal API authentication
pub const INTERNAL_AUTH_HEADER: &str = "X-ZLayer-Internal-Token";

/// State for internal endpoints
#[derive(Clone)]
pub struct InternalState {
    /// Service manager for container lifecycle operations
    pub service_manager: Arc<RwLock<ServiceManager>>,
    /// Shared secret for authenticating internal calls
    pub internal_token: String,
}

impl InternalState {
    /// Create a new internal state
    pub fn new(service_manager: Arc<RwLock<ServiceManager>>, internal_token: String) -> Self {
        Self {
            service_manager,
            internal_token,
        }
    }
}

/// Internal authentication extractor
///
/// Validates the X-ZLayer-Internal-Token header against the configured secret.
pub struct InternalAuth;

impl<S> FromRequestParts<S> for InternalAuth
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        // Get internal state from extensions
        let internal_state = parts
            .extensions
            .get::<InternalState>()
            .cloned()
            .ok_or_else(|| ApiError::Internal("Internal state not configured".to_string()))?;

        // Extract the internal token header
        let token = parts
            .headers
            .get(INTERNAL_AUTH_HEADER)
            .and_then(|value: &HeaderValue| value.to_str().ok())
            .ok_or_else(|| {
                warn!("Missing internal authentication header");
                ApiError::Unauthorized(format!("Missing {} header", INTERNAL_AUTH_HEADER))
            })?;

        // Verify the token
        if token != internal_state.internal_token {
            warn!("Invalid internal authentication token");
            return Err(ApiError::Unauthorized("Invalid internal token".to_string()));
        }

        Ok(InternalAuth)
    }
}

/// Request to scale a service
#[derive(Debug, Deserialize, ToSchema)]
pub struct InternalScaleRequest {
    /// Service name to scale
    pub service: String,
    /// Target replica count
    pub replicas: u32,
}

/// Response from internal scale operation
#[derive(Debug, Serialize, ToSchema)]
pub struct InternalScaleResponse {
    /// Whether the operation succeeded
    pub success: bool,
    /// Service name that was scaled
    pub service: String,
    /// New replica count
    pub replicas: u32,
    /// Optional message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Scale a service via internal scheduler request
///
/// This endpoint is called by the distributed scheduler leader to trigger
/// scaling operations on agent nodes. It uses a shared secret for authentication.
#[utoipa::path(
    post,
    path = "/api/v1/internal/scale",
    request_body = InternalScaleRequest,
    responses(
        (status = 200, description = "Service scaled successfully", body = InternalScaleResponse),
        (status = 401, description = "Unauthorized - invalid or missing internal token"),
        (status = 404, description = "Service not found"),
        (status = 500, description = "Internal error"),
    ),
    tag = "Internal"
)]
pub async fn scale_service_internal(
    _auth: InternalAuth,
    State(state): State<InternalState>,
    Json(request): Json<InternalScaleRequest>,
) -> Result<Json<InternalScaleResponse>> {
    // Validate replica count
    if request.replicas > 100 {
        return Err(ApiError::BadRequest(
            "Replica count cannot exceed 100".to_string(),
        ));
    }

    info!(
        service = %request.service,
        replicas = request.replicas,
        "Internal scale request received"
    );

    // Get the service manager
    let manager = state.service_manager.read().await;

    // Check if service exists
    if manager
        .service_replica_count(&request.service)
        .await
        .is_err()
    {
        return Err(ApiError::NotFound(format!(
            "Service '{}' not found or not registered",
            request.service
        )));
    }

    // Scale the service
    manager
        .scale_service(&request.service, request.replicas)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to scale service: {}", e)))?;

    // Get updated replica count
    let actual_replicas = manager
        .service_replica_count(&request.service)
        .await
        .unwrap_or(request.replicas as usize) as u32;

    info!(
        service = %request.service,
        replicas = actual_replicas,
        "Internal scale completed"
    );

    Ok(Json(InternalScaleResponse {
        success: true,
        service: request.service,
        replicas: actual_replicas,
        message: None,
    }))
}

/// Get the current replica count for a service
///
/// This endpoint allows the scheduler to query the current state of a service.
#[utoipa::path(
    get,
    path = "/api/v1/internal/replicas/{service}",
    params(
        ("service" = String, Path, description = "Service name"),
    ),
    responses(
        (status = 200, description = "Current replica count", body = InternalScaleResponse),
        (status = 401, description = "Unauthorized - invalid or missing internal token"),
        (status = 404, description = "Service not found"),
    ),
    tag = "Internal"
)]
pub async fn get_replicas_internal(
    _auth: InternalAuth,
    State(state): State<InternalState>,
    axum::extract::Path(service): axum::extract::Path<String>,
) -> Result<Json<InternalScaleResponse>> {
    let manager = state.service_manager.read().await;

    let replicas = manager
        .service_replica_count(&service)
        .await
        .map_err(|_| ApiError::NotFound(format!("Service '{}' not found", service)))?
        as u32;

    Ok(Json(InternalScaleResponse {
        success: true,
        service,
        replicas,
        message: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scale_request_deserialize() {
        let json = r#"{"service": "web", "replicas": 5}"#;
        let request: InternalScaleRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.service, "web");
        assert_eq!(request.replicas, 5);
    }

    #[test]
    fn test_scale_response_serialize() {
        let response = InternalScaleResponse {
            success: true,
            service: "web".to_string(),
            replicas: 5,
            message: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("web"));
        assert!(json.contains("5"));
        assert!(!json.contains("message")); // skip_serializing_if
    }

    #[test]
    fn test_scale_response_with_message() {
        let response = InternalScaleResponse {
            success: true,
            service: "web".to_string(),
            replicas: 5,
            message: Some("Scaled successfully".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("message"));
        assert!(json.contains("Scaled successfully"));
    }
}
