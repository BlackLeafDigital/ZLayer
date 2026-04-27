//! Overlay network API DTOs.
//!
//! Wire types for the overlay network status endpoints. These are the
//! response shapes consumed by both the daemon and SDK clients.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Overlay network status response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct OverlayStatusResponse {
    /// Overlay interface name
    pub interface: String,
    /// Whether this node is the cluster leader
    pub is_leader: bool,
    /// Node's overlay IP address
    pub node_ip: String,
    /// Overlay network CIDR
    pub cidr: String,
    /// Overlay listen port (`WireGuard` protocol)
    pub port: u16,
    /// Total number of peers
    pub total_peers: usize,
    /// Number of healthy peers
    pub healthy_peers: usize,
    /// Number of unhealthy peers
    pub unhealthy_peers: usize,
    /// Last health check timestamp (unix epoch seconds)
    pub last_check: u64,
}

/// Peer information
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PeerInfo {
    /// Peer's public key
    pub public_key: String,
    /// Peer's overlay IP address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay_ip: Option<String>,
    /// Whether the peer is healthy
    pub healthy: bool,
    /// Seconds since last handshake
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_handshake_secs: Option<u64>,
    /// Last ping latency in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ping_ms: Option<u64>,
    /// Number of consecutive health check failures
    pub failure_count: u32,
    /// Last health check timestamp (unix epoch seconds)
    pub last_check: u64,
}

/// Peer list response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PeerListResponse {
    /// Total number of peers
    pub total: usize,
    /// Number of healthy peers
    pub healthy: usize,
    /// List of peer information
    pub peers: Vec<PeerInfo>,
}

/// IP allocation status response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct IpAllocationResponse {
    /// Overlay network CIDR
    pub cidr: String,
    /// Total available IPs in the range
    pub total_ips: u32,
    /// Number of allocated IPs
    pub allocated_count: usize,
    /// Number of available IPs
    pub available_count: u32,
    /// Utilization percentage (0.0 - 100.0)
    pub utilization_percent: f64,
    /// List of allocated IP addresses (only included if requested)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocated_ips: Option<Vec<String>>,
}

/// DNS service status response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DnsStatusResponse {
    /// Whether DNS service is enabled
    pub enabled: bool,
    /// DNS zone name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
    /// DNS server port
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// DNS server bind address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_addr: Option<String>,
    /// Number of registered services
    pub service_count: usize,
    /// List of registered service names
    pub services: Vec<String>,
}
