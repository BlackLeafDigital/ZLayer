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
    /// `WireGuard` overlay interface name (e.g. "zl-overlay0") for add-peer operations.
    /// `None` if overlay networking is not configured.
    pub overlay_interface: Option<String>,
}

impl InternalState {
    /// Create a new internal state
    pub fn new(service_manager: Arc<RwLock<ServiceManager>>, internal_token: String) -> Self {
        Self {
            service_manager,
            internal_token,
            overlay_interface: None,
        }
    }

    /// Create a new internal state with overlay interface for peer management
    pub fn with_overlay(
        service_manager: Arc<RwLock<ServiceManager>>,
        internal_token: String,
        overlay_interface: Option<String>,
    ) -> Self {
        Self {
            service_manager,
            internal_token,
            overlay_interface,
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
                ApiError::Unauthorized(format!("Missing {INTERNAL_AUTH_HEADER} header"))
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

/// Scale a service via internal scheduler request.
///
/// This endpoint is called by the distributed scheduler leader to trigger
/// scaling operations on agent nodes. It uses a shared secret for authentication.
///
/// # Errors
///
/// Returns an error if the service is not found, scaling fails, or authentication
/// is invalid.
#[allow(clippy::cast_possible_truncation)]
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
        .map_err(|e| ApiError::Internal(format!("Failed to scale service: {e}")))?;

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

/// Get the current replica count for a service.
///
/// This endpoint allows the scheduler to query the current state of a service.
///
/// # Errors
///
/// Returns an error if the service is not found or authentication is invalid.
#[allow(clippy::cast_possible_truncation)]
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
        .map_err(|_| ApiError::NotFound(format!("Service '{service}' not found")))?
        as u32;

    Ok(Json(InternalScaleResponse {
        success: true,
        service,
        replicas,
        message: None,
    }))
}

/// Request to add a `WireGuard` peer to the local overlay transport.
///
/// Sent by the leader to existing nodes when a new node joins the cluster,
/// so that all nodes learn about the new peer without waiting for periodic
/// reconciliation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InternalAddPeerRequest {
    /// New peer's `WireGuard` public key (base64)
    pub wg_public_key: String,
    /// New peer's overlay IP (e.g. "10.200.0.3")
    pub overlay_ip: String,
    /// New peer's `WireGuard` endpoint (e.g. "203.0.113.5:51820")
    pub endpoint: String,
}

/// Response from internal add-peer operation
#[derive(Debug, Serialize, ToSchema)]
pub struct InternalAddPeerResponse {
    /// Whether the operation succeeded
    pub success: bool,
    /// Optional message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Add a `WireGuard` peer to the local overlay transport.
///
/// Called by the cluster leader after a new node joins, so existing nodes
/// can immediately route traffic to the new peer.
///
/// `POST /api/v1/internal/add-peer`
///
/// # Errors
///
/// Returns an error if overlay networking is not configured, the endpoint
/// address is invalid, or the `WireGuard` peer cannot be added.
#[utoipa::path(
    post,
    path = "/api/v1/internal/add-peer",
    request_body = InternalAddPeerRequest,
    responses(
        (status = 200, description = "Peer added successfully", body = InternalAddPeerResponse),
        (status = 401, description = "Unauthorized - invalid or missing internal token"),
        (status = 500, description = "Internal error"),
    ),
    tag = "Internal"
)]
pub async fn add_peer_internal(
    _auth: InternalAuth,
    State(state): State<InternalState>,
    Json(request): Json<InternalAddPeerRequest>,
) -> Result<Json<InternalAddPeerResponse>> {
    let interface_name = state.overlay_interface.as_deref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Overlay networking not configured on this node".into())
    })?;

    info!(
        wg_public_key = %request.wg_public_key,
        overlay_ip = %request.overlay_ip,
        endpoint = %request.endpoint,
        "Internal add-peer request received"
    );

    // Parse the endpoint into a SocketAddr
    let endpoint: std::net::SocketAddr = request.endpoint.parse().map_err(|e| {
        ApiError::BadRequest(format!(
            "Invalid endpoint address '{}': {}",
            request.endpoint, e
        ))
    })?;

    // Build a PeerInfo for the WireGuard UAPI call
    let peer_info = zlayer_overlay::PeerInfo::new(
        request.wg_public_key.clone(),
        endpoint,
        &format!("{}/32", request.overlay_ip),
        std::time::Duration::from_secs(25),
    );

    // Create a temporary OverlayTransport pointing at the existing interface's
    // UAPI socket.  We only need the interface name — the UAPI socket path is
    // derived from it (`/var/run/wireguard/{name}.sock`).
    let transport = zlayer_overlay::OverlayTransport::new(
        zlayer_overlay::OverlayConfig::default(),
        interface_name.to_string(),
    );

    transport
        .add_peer(&peer_info)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to add WireGuard peer: {e}")))?;

    info!(
        wg_public_key = %request.wg_public_key,
        overlay_ip = %request.overlay_ip,
        "Successfully added WireGuard peer via internal endpoint"
    );

    Ok(Json(InternalAddPeerResponse {
        success: true,
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

    #[test]
    fn test_add_peer_request_deserialize() {
        let json = r#"{
            "wg_public_key": "abc123base64key==",
            "overlay_ip": "10.200.0.5",
            "endpoint": "203.0.113.5:51820"
        }"#;
        let request: InternalAddPeerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.wg_public_key, "abc123base64key==");
        assert_eq!(request.overlay_ip, "10.200.0.5");
        assert_eq!(request.endpoint, "203.0.113.5:51820");
    }

    #[test]
    fn test_add_peer_response_serialize() {
        let response = InternalAddPeerResponse {
            success: true,
            message: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("true"));
        assert!(!json.contains("message")); // skip_serializing_if
    }

    #[test]
    fn test_internal_state_with_overlay() {
        let runtime: Arc<dyn zlayer_agent::Runtime + Send + Sync> =
            Arc::new(zlayer_agent::MockRuntime::new());
        let service_manager = Arc::new(RwLock::new(ServiceManager::builder(runtime).build()));
        let state = InternalState::with_overlay(
            service_manager,
            "token".to_string(),
            Some("zl-overlay0".to_string()),
        );
        assert_eq!(state.overlay_interface.as_deref(), Some("zl-overlay0"));
    }
}
