//! Overlay network status endpoints

use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{ApiError, Result};

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

/// Overlay API state holding overlay network references
#[derive(Clone)]
pub struct OverlayApiState {
    // Placeholder - will be filled when integrated with zlayer-overlay
}

impl Default for OverlayApiState {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayApiState {
    /// Create a new overlay API state
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

/// Get overlay network status.
///
/// # Errors
///
/// Returns an error if the overlay network is not initialized.
#[utoipa::path(
    get,
    path = "/api/v1/overlay/status",
    responses(
        (status = 200, description = "Overlay network status", body = OverlayStatusResponse),
        (status = 503, description = "Overlay not initialized"),
    ),
    tag = "Overlay"
)]
pub async fn get_overlay_status() -> Result<Json<OverlayStatusResponse>> {
    // Overlay not yet initialized - return service unavailable
    Err(ApiError::ServiceUnavailable(
        "Overlay not initialized".to_string(),
    ))
}

/// Get list of overlay peers.
///
/// # Errors
///
/// Returns an error if the overlay network is not initialized.
#[utoipa::path(
    get,
    path = "/api/v1/overlay/peers",
    responses(
        (status = 200, description = "List of overlay peers", body = PeerListResponse),
        (status = 503, description = "Overlay not initialized"),
    ),
    tag = "Overlay"
)]
pub async fn get_overlay_peers() -> Result<Json<PeerListResponse>> {
    // Overlay not yet initialized - return service unavailable
    Err(ApiError::ServiceUnavailable(
        "Overlay not initialized".to_string(),
    ))
}

/// Get IP allocation status.
///
/// # Errors
///
/// Returns an error if this is not a leader node or the overlay is not initialized.
#[utoipa::path(
    get,
    path = "/api/v1/overlay/ip-alloc",
    responses(
        (status = 200, description = "IP allocation status", body = IpAllocationResponse),
        (status = 503, description = "Not a leader node or overlay not initialized"),
    ),
    tag = "Overlay"
)]
pub async fn get_ip_allocation() -> Result<Json<IpAllocationResponse>> {
    // Not a leader node - return service unavailable
    Err(ApiError::ServiceUnavailable(
        "Not a leader node".to_string(),
    ))
}

/// Get DNS service status
#[utoipa::path(
    get,
    path = "/api/v1/overlay/dns",
    responses(
        (status = 200, description = "DNS service status", body = DnsStatusResponse),
    ),
    tag = "Overlay"
)]
pub async fn get_dns_status() -> Json<DnsStatusResponse> {
    // DNS not enabled - return disabled status
    Json(DnsStatusResponse {
        enabled: false,
        zone: None,
        port: None,
        bind_addr: None,
        service_count: 0,
        services: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_overlay_status_returns_unavailable() {
        let result = get_overlay_status().await;
        assert!(result.is_err());
        match result {
            Err(ApiError::ServiceUnavailable(msg)) => {
                assert!(msg.contains("Overlay not initialized"));
            }
            _ => panic!("Expected ServiceUnavailable error"),
        }
    }

    #[tokio::test]
    async fn test_get_overlay_peers_returns_unavailable() {
        let result = get_overlay_peers().await;
        assert!(result.is_err());
        match result {
            Err(ApiError::ServiceUnavailable(msg)) => {
                assert!(msg.contains("Overlay not initialized"));
            }
            _ => panic!("Expected ServiceUnavailable error"),
        }
    }

    #[tokio::test]
    async fn test_get_ip_allocation_returns_unavailable() {
        let result = get_ip_allocation().await;
        assert!(result.is_err());
        match result {
            Err(ApiError::ServiceUnavailable(msg)) => {
                assert!(msg.contains("Not a leader node"));
            }
            _ => panic!("Expected ServiceUnavailable error"),
        }
    }

    #[tokio::test]
    async fn test_get_dns_status_returns_disabled() {
        let response = get_dns_status().await;
        assert!(!response.enabled);
        assert!(response.zone.is_none());
        assert_eq!(response.service_count, 0);
        assert!(response.services.is_empty());
    }

    #[test]
    fn test_overlay_status_response_serialize() {
        let status = OverlayStatusResponse {
            interface: "wg0".to_string(),
            is_leader: true,
            node_ip: "10.200.0.1".to_string(),
            cidr: "10.200.0.0/16".to_string(),
            port: 51820,
            total_peers: 5,
            healthy_peers: 4,
            unhealthy_peers: 1,
            last_check: 1_706_900_000,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("wg0"));
        assert!(json.contains("10.200.0.1"));
    }

    #[test]
    fn test_peer_info_serialize() {
        let peer = PeerInfo {
            public_key: "abc123".to_string(),
            overlay_ip: Some("10.200.0.2".to_string()),
            healthy: true,
            last_handshake_secs: Some(30),
            last_ping_ms: Some(5),
            failure_count: 0,
            last_check: 1_706_900_000,
        };
        let json = serde_json::to_string(&peer).unwrap();
        assert!(json.contains("abc123"));
        assert!(json.contains("10.200.0.2"));
    }

    #[test]
    fn test_peer_list_response_serialize() {
        let response = PeerListResponse {
            total: 2,
            healthy: 1,
            peers: vec![],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"total\":2"));
        assert!(json.contains("\"healthy\":1"));
    }

    #[test]
    fn test_ip_allocation_response_serialize() {
        let response = IpAllocationResponse {
            cidr: "10.200.0.0/16".to_string(),
            total_ips: 65534,
            allocated_count: 100,
            available_count: 65434,
            utilization_percent: 0.15,
            allocated_ips: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("10.200.0.0/16"));
        assert!(json.contains("65534"));
    }

    #[test]
    fn test_dns_status_response_serialize() {
        let response = DnsStatusResponse {
            enabled: true,
            zone: Some("zlayer.local".to_string()),
            port: Some(53),
            bind_addr: Some("10.200.0.1".to_string()),
            service_count: 3,
            services: vec!["web".to_string(), "api".to_string(), "db".to_string()],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("zlayer.local"));
        assert!(json.contains("\"enabled\":true"));
    }

    // =========================================================================
    // IPv6 dual-stack serialization tests
    // =========================================================================

    #[test]
    fn test_overlay_status_response_serialize_ipv6() {
        let status = OverlayStatusResponse {
            interface: "wg0".to_string(),
            is_leader: false,
            node_ip: "fd00:200::1".to_string(),
            cidr: "fd00:200::/48".to_string(),
            port: 51820,
            total_peers: 3,
            healthy_peers: 3,
            unhealthy_peers: 0,
            last_check: 1_706_900_000,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("fd00:200::1"));
        assert!(json.contains("fd00:200::/48"));

        // Verify roundtrip
        let parsed: OverlayStatusResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.node_ip, "fd00:200::1");
        assert_eq!(parsed.cidr, "fd00:200::/48");
    }

    #[test]
    fn test_peer_info_serialize_ipv6() {
        let peer = PeerInfo {
            public_key: "def456".to_string(),
            overlay_ip: Some("fd00:200::2".to_string()),
            healthy: true,
            last_handshake_secs: Some(15),
            last_ping_ms: Some(3),
            failure_count: 0,
            last_check: 1_706_900_000,
        };
        let json = serde_json::to_string(&peer).unwrap();
        assert!(json.contains("def456"));
        assert!(json.contains("fd00:200::2"));

        // Verify roundtrip
        let parsed: PeerInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.overlay_ip.as_deref(), Some("fd00:200::2"));
    }

    #[test]
    fn test_ip_allocation_response_serialize_ipv6() {
        let response = IpAllocationResponse {
            cidr: "fd00:200::/48".to_string(),
            total_ips: 65534,
            allocated_count: 50,
            available_count: 65484,
            utilization_percent: 0.08,
            allocated_ips: Some(vec!["fd00:200::1".to_string(), "fd00:200::2".to_string()]),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("fd00:200::/48"));
        assert!(json.contains("fd00:200::1"));
        assert!(json.contains("fd00:200::2"));

        // Verify roundtrip
        let parsed: IpAllocationResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.cidr, "fd00:200::/48");
        assert_eq!(parsed.allocated_ips.unwrap().len(), 2);
    }

    #[test]
    fn test_dns_status_response_serialize_ipv6() {
        let response = DnsStatusResponse {
            enabled: true,
            zone: Some("overlay.local.".to_string()),
            port: Some(53),
            bind_addr: Some("fd00:200::1".to_string()),
            service_count: 2,
            services: vec!["web-v6".to_string(), "api-v6".to_string()],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("fd00:200::1"));
        assert!(json.contains("web-v6"));

        // Verify roundtrip
        let parsed: DnsStatusResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.bind_addr.as_deref(), Some("fd00:200::1"));
        assert_eq!(parsed.service_count, 2);
    }

    #[test]
    fn test_overlay_status_response_dual_stack() {
        // Verify both IPv4 and IPv6 representations work as node_ip
        let v4_status = OverlayStatusResponse {
            interface: "wg0".to_string(),
            is_leader: true,
            node_ip: "10.200.0.1".to_string(),
            cidr: "10.200.0.0/16".to_string(),
            port: 51820,
            total_peers: 2,
            healthy_peers: 2,
            unhealthy_peers: 0,
            last_check: 1_706_900_000,
        };

        let v6_status = OverlayStatusResponse {
            interface: "wg0".to_string(),
            is_leader: true,
            node_ip: "fd00:200::1".to_string(),
            cidr: "fd00:200::/48".to_string(),
            port: 51820,
            total_peers: 2,
            healthy_peers: 2,
            unhealthy_peers: 0,
            last_check: 1_706_900_000,
        };

        let v4_json = serde_json::to_string(&v4_status).unwrap();
        let v6_json = serde_json::to_string(&v6_status).unwrap();

        // Both should be valid JSON
        let _: OverlayStatusResponse = serde_json::from_str(&v4_json).unwrap();
        let _: OverlayStatusResponse = serde_json::from_str(&v6_json).unwrap();

        // They should be different
        assert_ne!(v4_json, v6_json);
    }

    #[test]
    fn test_overlay_api_state_default() {
        let state = OverlayApiState::default();
        // Just ensure it can be created
        let _ = state;
    }
}
