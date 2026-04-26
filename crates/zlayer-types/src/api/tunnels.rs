//! Tunnel management API DTOs.
//!
//! Wire types for the tunnel management endpoints.

use serde::{Deserialize, Serialize};
#[cfg(feature = "utoipa")]
use utoipa::ToSchema;

/// Request to create a new tunnel token
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct SuccessResponse {
    /// Success message
    pub message: String,
}
