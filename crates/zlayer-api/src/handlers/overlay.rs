//! Overlay network status endpoints

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use tokio::sync::RwLock;
pub use zlayer_types::api::overlay::*;
use zlayer_types::overlay::OverlayMode;

use crate::error::{ApiError, Result};
use zlayer_agent::OverlayManager;
use zlayer_overlay::DnsServer;

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

/// Map a [`zlayer_overlay::CandidateType`] to its API string form.
fn candidate_kind_str(kind: zlayer_overlay::CandidateType) -> &'static str {
    match kind {
        zlayer_overlay::CandidateType::Host => "host",
        zlayer_overlay::CandidateType::ServerReflexive => "server-reflexive",
        zlayer_overlay::CandidateType::Relay => "relay",
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

    // Count per-service bridge contexts (one per service overlay). Renamed
    // from `service_transport_count` in v0.51 alongside the bridge rewrite
    // (P9a) — local variable name and the underlying method now agree.
    let service_bridge_count = guard.service_bridge_count().await;

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
        total_peers: service_bridge_count,
        healthy_peers: if active { service_bridge_count } else { 0 },
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

    // Per-service bridge contexts (each service overlay is one entry).
    let service_bridge_count = guard.service_bridge_count().await;

    Ok(Json(PeerListResponse {
        total: service_bridge_count,
        healthy: service_bridge_count,
        peers: vec![],
    }))
}

/// Get the overlay status for a single service.
///
/// Returns the resolved overlay mode (after `resolve_v0_51`) and the
/// per-node bridge map describing where on each node the service's
/// overlay terminates.
///
/// In v0.51 this endpoint is a stub: the bridge map is always empty and
/// the mode is always `Shared` (the only implemented mode). Real wiring
/// lands in P9a once `OverlayManager` exposes a per-service bridge view.
///
/// # Errors
///
/// Returns 503 if the overlay manager is not initialised (host
/// networking mode). The 404 case (service unknown to this node) is
/// reserved for the post-P9a implementation; today the stub always
/// responds with an empty bridge map for any name.
#[utoipa::path(
    get,
    path = "/api/v1/overlay/services/{name}",
    params(
        ("name" = String, Path, description = "Service name")
    ),
    responses(
        (status = 200, description = "Service overlay status", body = ServiceOverlayStatus),
        (status = 503, description = "Overlay not initialized"),
    ),
    tag = "Overlay"
)]
pub async fn get_service_overlay_status(
    State(state): State<OverlayApiState>,
    Path(name): Path<String>,
) -> Result<Json<ServiceOverlayStatus>> {
    // The endpoint is reachable even in host-networking mode (the manager
    // UI hits it on every service detail page) but we surface 503 when no
    // overlay is configured rather than lying with an empty payload.
    if state.overlay.is_none() {
        return Err(ApiError::ServiceUnavailable(
            "Overlay not initialized (host networking mode)".to_string(),
        ));
    }

    Ok(Json(ServiceOverlayStatus {
        service: name,
        mode: OverlayMode::Auto.resolve_v0_51(),
        bridges_by_node: HashMap::new(),
    }))
}

/// Get the bridge attachment for a service on a specific node.
///
/// In v0.51 this endpoint is a stub: the bridge map is not yet populated
/// by the overlay manager, so every call returns 404. The endpoint
/// contract is defined now so adapter crates (zlayer-docker, manager UI,
/// Python SDK) can consume it day-one once P9a wires the real bridges.
///
/// # Errors
///
/// Returns 503 if the overlay manager is not initialised, and 404 in all
/// other cases until P9a lands.
#[utoipa::path(
    get,
    path = "/api/v1/overlay/services/{name}/bridges/{node_id}",
    params(
        ("name" = String, Path, description = "Service name"),
        ("node_id" = String, Path, description = "Node ID")
    ),
    responses(
        (status = 200, description = "Bridge info for this service on this node", body = BridgeInfo),
        (status = 404, description = "No bridge found for this service on this node"),
        (status = 503, description = "Overlay not initialized"),
    ),
    tag = "Overlay"
)]
pub async fn get_service_bridge(
    State(state): State<OverlayApiState>,
    Path((name, node_id)): Path<(String, String)>,
) -> Result<Json<BridgeInfo>> {
    if state.overlay.is_none() {
        return Err(ApiError::ServiceUnavailable(
            "Overlay not initialized (host networking mode)".to_string(),
        ));
    }

    Err(ApiError::NotFound(format!(
        "service '{name}' has no bridge on node '{node_id}' yet \
         (service bridges not yet populated in v0.51)",
    )))
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

/// Get NAT traversal status.
///
/// Returns the daemon's current NAT traversal configuration plus the live
/// runtime state from the overlay manager: configured STUN/TURN servers,
/// the bound relay-server address (if running), the locally gathered ICE
/// candidates, per-peer connection types, and the timestamp of the last
/// successful STUN refresh.
///
/// When the overlay manager is not initialised (host-networking mode) the
/// response reports `enabled: false` with empty server / candidate lists
/// rather than 503 — the Manager UI uses this endpoint to surface "NAT
/// disabled" badges and shouldn't have to special-case missing overlays.
#[utoipa::path(
    get,
    path = "/api/v1/overlay/nat/status",
    responses(
        (status = 200, description = "NAT traversal status", body = NatStatusResponse),
    ),
    tag = "Overlay"
)]
pub async fn get_nat_status(State(state): State<OverlayApiState>) -> Json<NatStatusResponse> {
    let Some(om) = state.overlay.as_ref() else {
        return Json(NatStatusResponse {
            enabled: false,
            stun_servers: Vec::new(),
            turn_servers: Vec::new(),
            relay_server_bind: None,
            candidates: Vec::new(),
            peers: Vec::new(),
            last_refresh: 0,
        });
    };

    let guard = om.read().await;
    let enabled = guard.nat_enabled();
    let nat_config = guard.nat_config().unwrap_or_default();
    let stun_servers = nat_config
        .stun_servers
        .iter()
        .map(|s| s.address.clone())
        .collect();
    let turn_servers = nat_config
        .turn_servers
        .iter()
        .map(|s| s.address.clone())
        .collect();
    let relay_server_bind = nat_config.relay_server.as_ref().map(|r| {
        // The configured external address is what other nodes use to reach
        // this relay; surface it as the human-readable bind for the UI.
        r.external_addr.clone()
    });

    let snapshot = guard.nat_status_snapshot().await;

    let candidates = snapshot
        .candidates
        .into_iter()
        .map(|c| NatCandidateDto {
            kind: candidate_kind_str(c.candidate_type).to_string(),
            transport: "udp".to_string(),
            address: c.address.to_string(),
            priority: c.priority,
        })
        .collect();

    let peers = snapshot
        .peers
        .into_iter()
        .map(|p| NatPeerDto {
            node_id: p.node_id,
            connection_type: p.connection_type,
            remote_endpoint: p.remote_endpoint,
        })
        .collect();

    Json(NatStatusResponse {
        enabled,
        stun_servers,
        turn_servers,
        relay_server_bind,
        candidates,
        peers,
        last_refresh: snapshot.last_refresh,
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
        let om = OverlayManager::new("test-deploy".to_string(), "test".to_string())
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
        let om = OverlayManager::new("test-deploy".to_string(), "test".to_string())
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
        let om = OverlayManager::new("test-deploy".to_string(), "test".to_string())
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

    #[test]
    fn test_overlay_api_state_default() {
        let state = OverlayApiState::default();
        assert!(state.overlay.is_none());
        assert!(state.dns.is_none());
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

    // =========================================================================
    // NAT status endpoint
    // =========================================================================

    /// When no overlay manager is wired in (host-networking mode), the NAT
    /// endpoint returns an `enabled: false` payload instead of 503.
    #[tokio::test]
    async fn test_get_nat_status_returns_disabled_when_none() {
        let state = OverlayApiState::new();
        let Json(response) = get_nat_status(State(state)).await;
        assert!(!response.enabled);
        assert!(response.stun_servers.is_empty());
        assert!(response.turn_servers.is_empty());
        assert!(response.relay_server_bind.is_none());
        assert!(response.candidates.is_empty());
        assert!(response.peers.is_empty());
        assert_eq!(response.last_refresh, 0);
    }

    /// With a live overlay manager but no NAT bootstrap, the endpoint
    /// surfaces the configured STUN defaults and an empty candidate list.
    #[tokio::test]
    async fn test_get_nat_status_with_manager_no_traversal() {
        let om = OverlayManager::new("test-deploy".to_string(), "test".to_string())
            .await
            .unwrap();
        let om = Arc::new(RwLock::new(om));
        let state = OverlayApiState::with_overlay(om);
        let Json(response) = get_nat_status(State(state)).await;
        // Default NatConfig has enabled = true
        assert!(response.enabled);
        // STUN defaults populate when no override is provided
        assert!(!response.stun_servers.is_empty());
        // Candidates / peers empty until start_nat_traversal runs
        assert!(response.candidates.is_empty());
        assert!(response.peers.is_empty());
    }

    #[test]
    fn test_nat_status_response_serialise_round_trip() {
        let response = NatStatusResponse {
            enabled: true,
            stun_servers: vec!["stun.l.google.com:19302".to_string()],
            turn_servers: vec![],
            relay_server_bind: Some("203.0.113.5:3478".to_string()),
            candidates: vec![NatCandidateDto {
                kind: "host".to_string(),
                transport: "udp".to_string(),
                address: "192.168.1.5:51820".to_string(),
                priority: 100,
            }],
            peers: vec![NatPeerDto {
                node_id: "node-1".to_string(),
                connection_type: "direct".to_string(),
                remote_endpoint: Some("203.0.113.6:51820".to_string()),
            }],
            last_refresh: 1_706_900_000,
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: NatStatusResponse = serde_json::from_str(&json).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.stun_servers.len(), 1);
        assert_eq!(parsed.candidates.len(), 1);
        assert_eq!(parsed.peers.len(), 1);
        assert_eq!(parsed.last_refresh, 1_706_900_000);
    }

    #[test]
    fn test_candidate_kind_str_mapping() {
        use zlayer_overlay::CandidateType;
        assert_eq!(candidate_kind_str(CandidateType::Host), "host");
        assert_eq!(
            candidate_kind_str(CandidateType::ServerReflexive),
            "server-reflexive"
        );
        assert_eq!(candidate_kind_str(CandidateType::Relay), "relay");
    }

    // =========================================================================
    // Service overlay status / bridge stubs (P4.5b)
    //
    // These cover the day-one contract for `GET
    // /api/v1/overlay/services/{name}` and
    // `GET /api/v1/overlay/services/{name}/bridges/{node_id}`. The real
    // wiring lands in P9a once the bridge map is populated by the overlay
    // manager; these tests pin down what adapter crates can rely on today.
    // =========================================================================

    #[tokio::test]
    async fn test_get_service_overlay_status_returns_unavailable_when_none() {
        let state = OverlayApiState::new();
        let result = get_service_overlay_status(State(state), Path("test-svc".to_string())).await;
        assert!(result.is_err());
        match result {
            Err(ApiError::ServiceUnavailable(msg)) => {
                assert!(msg.contains("Overlay not initialized"));
            }
            _ => panic!("Expected ServiceUnavailable error"),
        }
    }

    #[tokio::test]
    async fn test_get_service_overlay_status_returns_shared_stub_with_manager() {
        let om = OverlayManager::new("test-deploy".to_string(), "test".to_string())
            .await
            .unwrap();
        let om = Arc::new(RwLock::new(om));
        let state = OverlayApiState::with_overlay(om);
        let result = get_service_overlay_status(State(state), Path("test-svc".to_string())).await;
        assert!(result.is_ok(), "stub should succeed with a live manager");
        let Json(status) = result.unwrap();
        assert_eq!(status.service, "test-svc");
        // v0.51 always resolves Auto -> Shared
        assert_eq!(status.mode, OverlayMode::Shared);
        assert!(
            status.bridges_by_node.is_empty(),
            "bridge map is unpopulated until P9a lands"
        );

        // Pin the wire shape: mode must serialise as "shared", and
        // `bridges_by_node` must be an empty object.
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"service\":\"test-svc\""));
        assert!(json.contains("\"mode\":\"shared\""));
        assert!(json.contains("\"bridges_by_node\":{}"));
    }

    #[tokio::test]
    async fn test_get_service_bridge_returns_unavailable_when_none() {
        let state = OverlayApiState::new();
        let result = get_service_bridge(
            State(state),
            Path(("test-svc".to_string(), "node-xyz".to_string())),
        )
        .await;
        match result {
            Err(ApiError::ServiceUnavailable(msg)) => {
                assert!(msg.contains("Overlay not initialized"));
            }
            other => panic!("Expected ServiceUnavailable error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_get_service_bridge_returns_not_found_with_manager() {
        let om = OverlayManager::new("test-deploy".to_string(), "test".to_string())
            .await
            .unwrap();
        let om = Arc::new(RwLock::new(om));
        let state = OverlayApiState::with_overlay(om);
        let result = get_service_bridge(
            State(state),
            Path(("test-svc".to_string(), "node-xyz".to_string())),
        )
        .await;
        match result {
            Err(ApiError::NotFound(msg)) => {
                assert!(msg.contains("test-svc"));
                assert!(msg.contains("node-xyz"));
            }
            other => panic!("Expected NotFound error, got {other:?}"),
        }
    }
}
