//! Overlay network API DTOs.
//!
//! Wire types for the `/api/v1/overlay/*` HTTP surface, shared between
//! `zlayer-api` (server) and `zlayer-client` / `zlayer-manager` (clients).
//! These are the response shapes consumed by both the daemon and SDK
//! clients.
//!
//! See [`crate::overlay`] for the underlying configuration types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use utoipa::ToSchema;

use crate::overlay::OverlayMode;

/// Where on a specific node a service's overlay terminates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BridgeInfo {
    /// Linux bridge name (`br-svc-<hash>`), max 15 chars per IFNAMSIZ.
    pub name: String,
    /// CIDR of the service subnet assigned to this node.
    #[schema(value_type = String, example = "10.201.0.0/24")]
    pub subnet: ipnet::IpNet,
    /// Gateway IP within the subnet (first usable address); the bridge's
    /// own L3 address. Containers' default route points here.
    #[schema(value_type = String, example = "10.201.0.1")]
    pub gateway: IpAddr,
    /// Node identifier (stringified to avoid leaking `NodeId` type details
    /// across crate boundaries).
    pub node_id: String,
}

/// Status of a service's overlay across the cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ServiceOverlayStatus {
    pub service: String,
    /// Mode the daemon resolved for this service (after `resolve`).
    pub mode: OverlayMode,
    /// One entry per node where this service has at least one container
    /// attached. Keys are node IDs as stringified.
    pub bridges_by_node: HashMap<String, BridgeInfo>,
}

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

/// NAT traversal status response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NatStatusResponse {
    /// Whether NAT traversal is enabled in the daemon's config
    pub enabled: bool,
    /// Configured STUN servers (host:port)
    pub stun_servers: Vec<String>,
    /// Configured TURN/relay servers (host:port)
    pub turn_servers: Vec<String>,
    /// Address of the locally-bound built-in relay server, if running
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_server_bind: Option<String>,
    /// Locally gathered ICE candidates
    pub candidates: Vec<NatCandidateDto>,
    /// Per-peer NAT connectivity state
    pub peers: Vec<NatPeerDto>,
    /// Unix epoch seconds of the last successful STUN refresh
    pub last_refresh: u64,
}

/// Locally gathered NAT candidate (`Host` / `ServerReflexive` / `Relay`).
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NatCandidateDto {
    /// `Host` / `ServerReflexive` / `Relay`
    pub kind: String,
    /// Transport (e.g. "udp")
    pub transport: String,
    /// Address (host:port)
    pub address: String,
    /// Priority (higher = preferred)
    pub priority: u32,
}

/// Per-peer NAT connectivity entry.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NatPeerDto {
    /// Peer node ID
    pub node_id: String,
    /// Direct / `HolePunched` / Relayed / Unreachable
    pub connection_type: String,
    /// Selected remote endpoint, if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_endpoint: Option<String>,
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
