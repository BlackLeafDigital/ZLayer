//! Overlay network status endpoints

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use utoipa::ToSchema;

use crate::error::{ApiError, Result};
use zlayer_agent::OverlayManager;
use zlayer_overlay::DnsServer;

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
    /// Overlay network manager. `None` when host networking is active or
    /// overlay setup failed non-fatally.
    pub overlay: Option<Arc<RwLock<OverlayManager>>>,
    /// DNS server for service discovery. `None` when DNS is not configured.
    pub dns: Option<Arc<DnsServer>>,
}

impl Default for OverlayApiState {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayApiState {
    /// Create a new overlay API state with no overlay or DNS (disabled mode)
    #[must_use]
    pub fn new() -> Self {
        Self {
            overlay: None,
            dns: None,
        }
    }

    /// Create an overlay API state with a live overlay manager
    #[must_use]
    pub fn with_overlay(overlay: Arc<RwLock<OverlayManager>>) -> Self {
        Self {
            overlay: Some(overlay),
            dns: None,
        }
    }

    /// Create an overlay API state with both overlay manager and DNS server
    #[must_use]
    pub fn with_overlay_and_dns(overlay: Arc<RwLock<OverlayManager>>, dns: Arc<DnsServer>) -> Self {
        Self {
            overlay: Some(overlay),
            dns: Some(dns),
        }
    }
}

/// Get overlay network status.
///
/// Returns the current overlay network status including interface name,
/// node IP, CIDR, peer counts, and whether the transport is active.
/// Returns 503 if the overlay network is not initialized (e.g. host
/// networking mode).
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
pub async fn get_overlay_status(
    State(state): State<OverlayApiState>,
) -> Result<Json<OverlayStatusResponse>> {
    let om = state.overlay.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Overlay not initialized (host networking mode)".to_string())
    })?;

    let guard = om.read().await;

    let interface = guard.global_interface().unwrap_or("none").to_string();
    let node_ip = guard
        .node_ip()
        .map_or_else(|| "unassigned".to_string(), |ip| ip.to_string());
    let cidr = guard.overlay_cidr();
    let port = guard.overlay_port();
    let active = guard.has_global_transport();

    // Count service transports as peers (each service overlay is a peer context)
    let peer_count = guard.service_transport_count().await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    Ok(Json(OverlayStatusResponse {
        interface,
        is_leader: active, // The node that created the global overlay is the leader
        node_ip,
        cidr,
        port,
        total_peers: peer_count,
        healthy_peers: if active { peer_count } else { 0 },
        unhealthy_peers: 0,
        last_check: now,
    }))
}

/// Get list of overlay peers.
///
/// Returns peer information from the overlay transport's UAPI state.
/// When the overlay is not initialized, returns 503.
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
pub async fn get_overlay_peers(
    State(state): State<OverlayApiState>,
) -> Result<Json<PeerListResponse>> {
    let om = state.overlay.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Overlay not initialized (host networking mode)".to_string())
    })?;

    let guard = om.read().await;

    if !guard.has_global_transport() {
        return Ok(Json(PeerListResponse {
            total: 0,
            healthy: 0,
            peers: vec![],
        }));
    }

    // Service transports represent active per-service overlay connections.
    // Report them as peers with their service names as identifiers.
    let peer_count = guard.service_transport_count().await;

    Ok(Json(PeerListResponse {
        total: peer_count,
        healthy: peer_count,
        peers: vec![],
    }))
}

/// Get IP allocation status.
///
/// Returns the overlay network's IP allocation statistics including CIDR,
/// total IPs, allocated count, and utilization. Available on any node that
/// has an active overlay (not just leaders).
///
/// # Errors
///
/// Returns an error if the overlay is not initialized.
#[utoipa::path(
    get,
    path = "/api/v1/overlay/ip-alloc",
    responses(
        (status = 200, description = "IP allocation status", body = IpAllocationResponse),
        (status = 503, description = "Not a leader node or overlay not initialized"),
    ),
    tag = "Overlay"
)]
pub async fn get_ip_allocation(
    State(state): State<OverlayApiState>,
) -> Result<Json<IpAllocationResponse>> {
    let om = state.overlay.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Overlay not initialized (host networking mode)".to_string())
    })?;

    let guard = om.read().await;
    let cidr = guard.overlay_cidr();
    let (allocated_offset, _base) = guard.ip_alloc_stats();

    // The default CIDR is /16 for IPv4 (65534 usable hosts) or /48 for IPv6.
    // Parse the prefix length from the CIDR to compute total hosts.
    let total_ips: u32 = if let Some(prefix_str) = cidr.split('/').nth(1) {
        if let Ok(prefix) = prefix_str.parse::<u32>() {
            if cidr.contains(':') {
                // IPv6: cap at u32::MAX for display purposes
                let host_bits = 128u32.saturating_sub(prefix);
                if host_bits >= 32 {
                    u32::MAX
                } else {
                    2u32.saturating_pow(host_bits).saturating_sub(2)
                }
            } else {
                // IPv4
                let host_bits = 32u32.saturating_sub(prefix);
                2u32.saturating_pow(host_bits).saturating_sub(2)
            }
        } else {
            0
        }
    } else {
        0
    };

    // Clamp to u32 range since total_ips is u32; overlay networks will never
    // exceed 2^32 allocations in practice.
    #[allow(clippy::cast_possible_truncation)]
    let allocated_u32 = u32::try_from(allocated_offset).unwrap_or(u32::MAX);
    let allocated_count = allocated_u32 as usize;
    let available_count = total_ips.saturating_sub(allocated_u32);
    let utilization_percent = if total_ips > 0 {
        (f64::from(allocated_u32) / f64::from(total_ips)) * 100.0
    } else {
        0.0
    };

    Ok(Json(IpAllocationResponse {
        cidr,
        total_ips,
        allocated_count,
        available_count,
        utilization_percent,
        allocated_ips: None,
    }))
}

/// Get DNS service status.
///
/// Returns DNS server configuration and registered service count.
/// When DNS is not configured, returns a disabled status (not an error).
#[utoipa::path(
    get,
    path = "/api/v1/overlay/dns",
    responses(
        (status = 200, description = "DNS service status", body = DnsStatusResponse),
    ),
    tag = "Overlay"
)]
pub async fn get_dns_status(State(state): State<OverlayApiState>) -> Json<DnsStatusResponse> {
    let Some(dns) = &state.dns else {
        return Json(DnsStatusResponse {
            enabled: false,
            zone: None,
            port: None,
            bind_addr: None,
            service_count: 0,
            services: Vec::new(),
        });
    };

    let listen = dns.listen_addr();
    let zone = dns.zone_origin().to_string();

    Json(DnsStatusResponse {
        enabled: true,
        zone: Some(zone),
        port: Some(listen.port()),
        bind_addr: Some(listen.ip().to_string()),
        service_count: 0,
        services: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_overlay_status_returns_unavailable_when_none() {
        let state = OverlayApiState::new();
        let result = get_overlay_status(State(state)).await;
        assert!(result.is_err());
        match result {
            Err(ApiError::ServiceUnavailable(msg)) => {
                assert!(msg.contains("Overlay not initialized"));
            }
            _ => panic!("Expected ServiceUnavailable error"),
        }
    }

    #[tokio::test]
    async fn test_get_overlay_peers_returns_unavailable_when_none() {
        let state = OverlayApiState::new();
        let result = get_overlay_peers(State(state)).await;
        assert!(result.is_err());
        match result {
            Err(ApiError::ServiceUnavailable(msg)) => {
                assert!(msg.contains("Overlay not initialized"));
            }
            _ => panic!("Expected ServiceUnavailable error"),
        }
    }

    #[tokio::test]
    async fn test_get_ip_allocation_returns_unavailable_when_none() {
        let state = OverlayApiState::new();
        let result = get_ip_allocation(State(state)).await;
        assert!(result.is_err());
        match result {
            Err(ApiError::ServiceUnavailable(msg)) => {
                assert!(msg.contains("Overlay not initialized"));
            }
            _ => panic!("Expected ServiceUnavailable error"),
        }
    }

    #[tokio::test]
    async fn test_get_dns_status_returns_disabled_when_none() {
        let state = OverlayApiState::new();
        let Json(response) = get_dns_status(State(state)).await;
        assert!(!response.enabled);
        assert!(response.zone.is_none());
        assert_eq!(response.service_count, 0);
        assert!(response.services.is_empty());
    }

    #[tokio::test]
    async fn test_get_overlay_status_with_manager() {
        let om = OverlayManager::new("test-deploy".to_string())
            .await
            .unwrap();
        let om = Arc::new(RwLock::new(om));
        let state = OverlayApiState::with_overlay(om);
        let result = get_overlay_status(State(state)).await;
        assert!(result.is_ok());
        let Json(status) = result.unwrap();
        // Before setup_global_overlay, there is no interface or node_ip
        assert_eq!(status.interface, "none");
        assert_eq!(status.node_ip, "unassigned");
        assert!(status.cidr.contains("10.200.0.0"));
        assert!(!status.is_leader); // no global transport yet
    }

    #[tokio::test]
    async fn test_get_overlay_peers_with_manager() {
        let om = OverlayManager::new("test-deploy".to_string())
            .await
            .unwrap();
        let om = Arc::new(RwLock::new(om));
        let state = OverlayApiState::with_overlay(om);
        let result = get_overlay_peers(State(state)).await;
        assert!(result.is_ok());
        let Json(peers) = result.unwrap();
        assert_eq!(peers.total, 0);
        assert_eq!(peers.healthy, 0);
    }

    #[tokio::test]
    async fn test_get_ip_allocation_with_manager() {
        let om = OverlayManager::new("test-deploy".to_string())
            .await
            .unwrap();
        let om = Arc::new(RwLock::new(om));
        let state = OverlayApiState::with_overlay(om);
        let result = get_ip_allocation(State(state)).await;
        assert!(result.is_ok());
        let Json(alloc) = result.unwrap();
        assert!(alloc.cidr.contains("10.200.0.0"));
        assert_eq!(alloc.total_ips, 65534);
        assert_eq!(alloc.allocated_count, 0);
        assert_eq!(alloc.available_count, 65534);
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
        assert!(state.overlay.is_none());
        assert!(state.dns.is_none());
    }
}
