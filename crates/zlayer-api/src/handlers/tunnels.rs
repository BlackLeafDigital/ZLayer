//! Tunnel management endpoints
//!
//! Provides REST API endpoints for creating, managing, and revoking tunnels.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request to create a new tunnel token
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTunnelRequest {
    /// Name for this tunnel (for identification)
    pub name: String,
    /// Optional list of services this tunnel is allowed to expose
    #[serde(default)]
    pub services: Vec<String>,
    /// Time-to-live in seconds (default: 24 hours)
    #[serde(default = "default_ttl")]
    pub ttl_secs: u64,
}

fn default_ttl() -> u64 {
    86400 // 24 hours
}

/// Response after creating a tunnel token
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateTunnelResponse {
    /// Unique tunnel identifier
    pub id: String,
    /// Name of the tunnel
    pub name: String,
    /// The tunnel token to use for authentication
    pub token: String,
    /// Services this tunnel can expose
    pub services: Vec<String>,
    /// When the token expires (Unix timestamp)
    pub expires_at: u64,
    /// When the tunnel was created (Unix timestamp)
    pub created_at: u64,
}

/// Tunnel summary for list operations
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
pub struct TunnelSummary {
    /// Unique tunnel identifier
    pub id: String,
    /// Name of the tunnel
    pub name: String,
    /// Current status (active, disconnected, expired)
    pub status: String,
    /// Services this tunnel can expose
    pub services: Vec<String>,
    /// When the tunnel was created (Unix timestamp)
    pub created_at: u64,
    /// When the token expires (Unix timestamp)
    pub expires_at: u64,
    /// Last time the tunnel connected (Unix timestamp, if ever)
    pub last_connected: Option<u64>,
}

/// Detailed tunnel status
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TunnelStatus {
    /// Unique tunnel identifier
    pub id: String,
    /// Name of the tunnel
    pub name: String,
    /// Current status (active, disconnected, expired)
    pub status: String,
    /// Services this tunnel can expose
    pub allowed_services: Vec<String>,
    /// Currently registered services
    pub registered_services: Vec<RegisteredServiceInfo>,
    /// When the tunnel was created (Unix timestamp)
    pub created_at: u64,
    /// When the token expires (Unix timestamp)
    pub expires_at: u64,
    /// Last time the tunnel connected (Unix timestamp, if ever)
    pub last_connected: Option<u64>,
    /// Client IP address (if connected)
    pub client_addr: Option<String>,
    /// Number of active connections
    pub active_connections: u32,
}

/// Information about a registered service in a tunnel
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RegisteredServiceInfo {
    /// Service name
    pub name: String,
    /// Service ID
    pub service_id: String,
    /// Protocol (tcp or udp)
    pub protocol: String,
    /// Local port on the client
    pub local_port: u16,
    /// Remote port exposed on the server
    pub remote_port: u16,
    /// Current status
    pub status: String,
}

/// Request to create a node-to-node tunnel
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateNodeTunnelRequest {
    /// Name for this tunnel
    pub name: String,
    /// Source node ID
    pub from_node: String,
    /// Destination node ID
    pub to_node: String,
    /// Local port on the source node
    pub local_port: u16,
    /// Remote port on the destination node
    pub remote_port: u16,
    /// Exposure level (public, internal)
    #[serde(default = "default_expose")]
    pub expose: String,
}

fn default_expose() -> String {
    "internal".to_string()
}

/// Response after creating a node-to-node tunnel
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateNodeTunnelResponse {
    /// Tunnel name
    pub name: String,
    /// Source node ID
    pub from_node: String,
    /// Destination node ID
    pub to_node: String,
    /// Local port
    pub local_port: u16,
    /// Remote port
    pub remote_port: u16,
    /// Exposure level
    pub expose: String,
    /// Current status
    pub status: String,
}

/// Generic success response
#[derive(Debug, Serialize, ToSchema)]
pub struct SuccessResponse {
    /// Success message
    pub message: String,
}

// =============================================================================
// State
// =============================================================================

/// In-memory storage for tunnel configurations
/// In production, this would be backed by persistent cluster state
#[derive(Debug, Clone)]
pub struct StoredTunnel {
    pub id: String,
    pub name: String,
    pub token_hash: String,
    pub services: Vec<String>,
    pub created_at: u64,
    pub expires_at: u64,
    pub created_by: String,
    pub status: String,
    pub last_connected: Option<u64>,
}

/// State for tunnel endpoints
#[derive(Clone)]
pub struct TunnelApiState {
    /// Stored tunnel configurations
    tunnels: Arc<RwLock<HashMap<String, StoredTunnel>>>,
    /// Node-to-node tunnels
    node_tunnels: Arc<RwLock<HashMap<String, CreateNodeTunnelResponse>>>,
}

impl TunnelApiState {
    /// Create a new tunnel API state
    #[must_use]
    pub fn new() -> Self {
        Self {
            tunnels: Arc::new(RwLock::new(HashMap::new())),
            node_tunnels: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for TunnelApiState {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Generate a tunnel token with prefix
fn generate_tunnel_token() -> String {
    format!("tun_{}", Uuid::new_v4().to_string().replace('-', ""))
}

/// Hash a token for storage
fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Get current Unix timestamp
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}

// =============================================================================
// Handlers
// =============================================================================

/// Create a new tunnel token
#[utoipa::path(
    post,
    path = "/api/v1/tunnels",
    request_body = CreateTunnelRequest,
    responses(
        (status = 201, description = "Tunnel token created", body = CreateTunnelResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Tunnels"
)]
pub async fn create_tunnel(
    user: AuthUser,
    State(state): State<TunnelApiState>,
    Json(request): Json<CreateTunnelRequest>,
) -> Result<Json<CreateTunnelResponse>> {
    // Validate request
    if request.name.is_empty() {
        return Err(ApiError::BadRequest("Tunnel name is required".to_string()));
    }

    if request.name.len() > 64 {
        return Err(ApiError::BadRequest(
            "Tunnel name must be 64 characters or less".to_string(),
        ));
    }

    // Generate token and ID
    let token = generate_tunnel_token();
    let id = Uuid::new_v4().to_string();
    let now = current_timestamp();
    let expires_at = now + request.ttl_secs;

    // Store tunnel config
    let stored = StoredTunnel {
        id: id.clone(),
        name: request.name.clone(),
        token_hash: hash_token(&token),
        services: request.services.clone(),
        created_at: now,
        expires_at,
        created_by: user.subject().to_string(),
        status: "pending".to_string(),
        last_connected: None,
    };

    {
        let mut tunnels = state.tunnels.write();
        tunnels.insert(id.clone(), stored);
    }

    tracing::info!(
        tunnel_id = %id,
        name = %request.name,
        created_by = %user.subject(),
        expires_at = expires_at,
        "Created tunnel token"
    );

    Ok(Json(CreateTunnelResponse {
        id,
        name: request.name,
        token,
        services: request.services,
        expires_at,
        created_at: now,
    }))
}

/// List all tunnels
#[utoipa::path(
    get,
    path = "/api/v1/tunnels",
    responses(
        (status = 200, description = "List of tunnels", body = Vec<TunnelSummary>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Tunnels"
)]
pub async fn list_tunnels(
    _user: AuthUser,
    State(state): State<TunnelApiState>,
) -> Result<Json<Vec<TunnelSummary>>> {
    let tunnels = state.tunnels.read();
    let now = current_timestamp();

    let summaries: Vec<TunnelSummary> = tunnels
        .values()
        .map(|t| {
            let status = if t.expires_at < now {
                "expired"
            } else {
                &t.status
            };
            TunnelSummary {
                id: t.id.clone(),
                name: t.name.clone(),
                status: status.to_string(),
                services: t.services.clone(),
                created_at: t.created_at,
                expires_at: t.expires_at,
                last_connected: t.last_connected,
            }
        })
        .collect();

    Ok(Json(summaries))
}

/// Revoke (delete) a tunnel
#[utoipa::path(
    delete,
    path = "/api/v1/tunnels/{id}",
    params(
        ("id" = String, Path, description = "Tunnel identifier"),
    ),
    responses(
        (status = 200, description = "Tunnel revoked", body = SuccessResponse),
        (status = 404, description = "Tunnel not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Tunnels"
)]
pub async fn revoke_tunnel(
    _user: AuthUser,
    State(state): State<TunnelApiState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>> {
    let mut tunnels = state.tunnels.write();

    if tunnels.remove(&id).is_some() {
        tracing::info!(tunnel_id = %id, "Revoked tunnel");
        Ok(Json(SuccessResponse {
            message: format!("Tunnel '{}' revoked successfully", id),
        }))
    } else {
        Err(ApiError::NotFound(format!("Tunnel '{}' not found", id)))
    }
}

/// Get tunnel status
#[utoipa::path(
    get,
    path = "/api/v1/tunnels/{id}/status",
    params(
        ("id" = String, Path, description = "Tunnel identifier"),
    ),
    responses(
        (status = 200, description = "Tunnel status", body = TunnelStatus),
        (status = 404, description = "Tunnel not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Tunnels"
)]
pub async fn get_tunnel_status(
    _user: AuthUser,
    State(state): State<TunnelApiState>,
    Path(id): Path<String>,
) -> Result<Json<TunnelStatus>> {
    let tunnels = state.tunnels.read();
    let now = current_timestamp();

    match tunnels.get(&id) {
        Some(tunnel) => {
            let status = if tunnel.expires_at < now {
                "expired"
            } else {
                &tunnel.status
            };

            Ok(Json(TunnelStatus {
                id: tunnel.id.clone(),
                name: tunnel.name.clone(),
                status: status.to_string(),
                allowed_services: tunnel.services.clone(),
                registered_services: Vec::new(), // Would be populated from TunnelRegistry in production
                created_at: tunnel.created_at,
                expires_at: tunnel.expires_at,
                last_connected: tunnel.last_connected,
                client_addr: None,     // Would come from active connection
                active_connections: 0, // Would come from TunnelRegistry
            }))
        }
        None => Err(ApiError::NotFound(format!("Tunnel '{}' not found", id))),
    }
}

/// Create a node-to-node tunnel
#[utoipa::path(
    post,
    path = "/api/v1/tunnels/node",
    request_body = CreateNodeTunnelRequest,
    responses(
        (status = 201, description = "Node tunnel created", body = CreateNodeTunnelResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - admin role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Tunnels"
)]
pub async fn create_node_tunnel(
    user: AuthUser,
    State(state): State<TunnelApiState>,
    Json(request): Json<CreateNodeTunnelRequest>,
) -> Result<Json<CreateNodeTunnelResponse>> {
    // Require admin role for node tunnels
    user.require_role("admin")?;

    // Validate request
    if request.name.is_empty() {
        return Err(ApiError::BadRequest("Tunnel name is required".to_string()));
    }

    if request.from_node == request.to_node {
        return Err(ApiError::BadRequest(
            "Source and destination nodes must be different".to_string(),
        ));
    }

    if request.expose != "public" && request.expose != "internal" {
        return Err(ApiError::BadRequest(
            "Expose must be 'public' or 'internal'".to_string(),
        ));
    }

    let response = CreateNodeTunnelResponse {
        name: request.name.clone(),
        from_node: request.from_node,
        to_node: request.to_node,
        local_port: request.local_port,
        remote_port: request.remote_port,
        expose: request.expose,
        status: "pending".to_string(),
    };

    {
        let mut node_tunnels = state.node_tunnels.write();
        node_tunnels.insert(request.name.clone(), response.clone());
    }

    tracing::info!(
        tunnel_name = %request.name,
        "Created node-to-node tunnel"
    );

    Ok(Json(response))
}

/// Remove a node-to-node tunnel
#[utoipa::path(
    delete,
    path = "/api/v1/tunnels/node/{name}",
    params(
        ("name" = String, Path, description = "Tunnel name"),
    ),
    responses(
        (status = 200, description = "Node tunnel removed", body = SuccessResponse),
        (status = 404, description = "Tunnel not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - admin role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Tunnels"
)]
pub async fn remove_node_tunnel(
    user: AuthUser,
    State(state): State<TunnelApiState>,
    Path(name): Path<String>,
) -> Result<Json<SuccessResponse>> {
    // Require admin role
    user.require_role("admin")?;

    let mut node_tunnels = state.node_tunnels.write();

    if node_tunnels.remove(&name).is_some() {
        tracing::info!(tunnel_name = %name, "Removed node tunnel");
        Ok(Json(SuccessResponse {
            message: format!("Node tunnel '{}' removed successfully", name),
        }))
    } else {
        Err(ApiError::NotFound(format!(
            "Node tunnel '{}' not found",
            name
        )))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_tunnel_token() {
        let token = generate_tunnel_token();
        assert!(token.starts_with("tun_"));
        assert_eq!(token.len(), 36); // "tun_" + 32 hex chars
    }

    #[test]
    fn test_hash_token() {
        let token = "tun_abc123";
        let hash1 = hash_token(token);
        let hash2 = hash_token(token);
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA256 hex

        // Different tokens produce different hashes
        let hash3 = hash_token("tun_xyz789");
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_tunnel_api_state_default() {
        let state = TunnelApiState::default();
        assert!(state.tunnels.read().is_empty());
        assert!(state.node_tunnels.read().is_empty());
    }

    #[test]
    fn test_create_tunnel_request_defaults() {
        let json = r#"{"name": "test-tunnel"}"#;
        let request: CreateTunnelRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.name, "test-tunnel");
        assert!(request.services.is_empty());
        assert_eq!(request.ttl_secs, 86400);
    }

    #[test]
    fn test_create_tunnel_request_with_services() {
        let json = r#"{"name": "test-tunnel", "services": ["ssh", "postgres"], "ttl_secs": 3600}"#;
        let request: CreateTunnelRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.name, "test-tunnel");
        assert_eq!(request.services, vec!["ssh", "postgres"]);
        assert_eq!(request.ttl_secs, 3600);
    }

    #[test]
    fn test_tunnel_summary_serialize() {
        let summary = TunnelSummary {
            id: "abc123".to_string(),
            name: "my-tunnel".to_string(),
            status: "active".to_string(),
            services: vec!["ssh".to_string()],
            created_at: 1706745600,
            expires_at: 1706832000,
            last_connected: Some(1706750000),
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("my-tunnel"));
        assert!(json.contains("active"));
    }

    #[test]
    fn test_create_node_tunnel_request_defaults() {
        let json = r#"{"name": "test", "from_node": "node1", "to_node": "node2", "local_port": 8080, "remote_port": 80}"#;
        let request: CreateNodeTunnelRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.expose, "internal");
    }
}
