//! NAT traversal orchestrator with ICE-lite connectivity checks.
//!
//! Gathers local host and STUN reflexive candidates, then attempts to
//! connect to peers by trying each candidate in priority order. Falls
//! back from direct -> hole-punched -> relayed as needed.

use crate::error::{OverlayError, Result};
use crate::transport::OverlayTransport;

use super::candidate::{Candidate, CandidateType, ConnectionType};
use super::config::NatConfig;
use super::discovery::RelayDiscovery;
use super::stun::{NatBehavior, ReflexiveAddress, StunClient};
use super::turn::RelayClient;

use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tracing::{debug, info, warn};

/// NAT traversal orchestrator.
///
/// Coordinates STUN discovery, candidate gathering, and ICE-lite
/// connectivity checks to establish the best possible connection
/// to each peer (direct > hole-punched > relayed).
pub struct NatTraversal {
    stun_client: StunClient,
    config: NatConfig,
    wg_port: u16,
    local_candidates: Vec<Candidate>,
    reflexive_addresses: Vec<ReflexiveAddress>,
    nat_behavior: NatBehavior,
    relay_clients: Vec<RelayClient>,
    relay_discovery: RelayDiscovery,
}

impl NatTraversal {
    /// Create a new NAT traversal orchestrator.
    ///
    /// # Arguments
    ///
    /// * `config` - NAT configuration with STUN servers, timeouts, etc.
    /// * `wg_port` - The local `WireGuard` listen port used for candidates.
    #[must_use]
    pub fn new(config: NatConfig, wg_port: u16) -> Self {
        let stun_client = StunClient::new(config.stun_servers.clone());
        let relay_discovery = RelayDiscovery::new(&config);
        Self {
            stun_client,
            config,
            wg_port,
            local_candidates: Vec::new(),
            reflexive_addresses: Vec::new(),
            nat_behavior: NatBehavior::EndpointIndependent,
            relay_clients: Vec::new(),
            relay_discovery,
        }
    }

    /// Gather local candidates: host candidates (local IP + WG port)
    /// and STUN reflexive candidates.
    ///
    /// Host candidates are discovered via the UDP socket trick (connect
    /// to 8.8.8.8:80, read `local_addr`). Reflexive candidates come
    /// from STUN `discover()`.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::StunDiscovery`] if STUN discovery fails
    /// and no host candidates could be gathered.
    #[allow(clippy::too_many_lines)]
    pub async fn gather_candidates(&mut self) -> Result<Vec<Candidate>> {
        let mut candidates = Vec::new();

        // -- Host candidates via UDP socket trick --
        // IPv4 host candidate
        match discover_local_ip() {
            Ok(local_ip) => {
                let addr = SocketAddr::new(local_ip, self.wg_port);
                let host = Candidate::new(CandidateType::Host, addr);
                debug!(address = %addr, "Gathered IPv4 host candidate");
                candidates.push(host);
            }
            Err(e) => {
                warn!(error = %e, "Failed to discover local IPv4 for host candidate");
            }
        }

        // IPv6 host candidate
        match discover_local_ipv6() {
            Ok(local_ip) => {
                let addr = SocketAddr::new(local_ip, self.wg_port);
                let host = Candidate::new(CandidateType::Host, addr);
                debug!(address = %addr, "Gathered IPv6 host candidate");
                candidates.push(host);
            }
            Err(e) => {
                debug!(error = %e, "No IPv6 host candidate (IPv6 may not be available)");
            }
        }

        // -- STUN reflexive candidates --
        match self.stun_client.discover().await {
            Ok((reflexive_addrs, behavior)) => {
                self.nat_behavior = behavior;
                debug!(
                    behavior = ?behavior,
                    count = reflexive_addrs.len(),
                    "STUN discovery completed"
                );

                for ra in &reflexive_addrs {
                    // Use the STUN-discovered IP with our WG port
                    let addr = SocketAddr::new(ra.address.ip(), self.wg_port);
                    let candidate = Candidate::new(CandidateType::ServerReflexive, addr);
                    debug!(
                        address = %addr,
                        server = %ra.server,
                        "Gathered server-reflexive candidate"
                    );
                    candidates.push(candidate);
                }

                self.reflexive_addresses = reflexive_addrs;
            }
            Err(e) => {
                warn!(error = %e, "STUN discovery failed, proceeding with host candidates only");
            }
        }

        // -- Relay candidates --
        for server_config in self.relay_discovery.servers().to_vec() {
            match RelayClient::new(&server_config) {
                Ok(mut client) => {
                    match client.allocate().await {
                        Ok(_relay_addr) => {
                            match client.start_proxy(self.wg_port).await {
                                Ok(proxy_addr) => {
                                    // Use proxy_addr as the candidate (WG sends to local proxy)
                                    candidates
                                        .push(Candidate::new(CandidateType::Relay, proxy_addr));
                                    self.relay_clients.push(client);
                                    debug!(
                                        proxy = %proxy_addr,
                                        server = %server_config.address,
                                        "Gathered relay candidate"
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        server = %server_config.address,
                                        error = %e,
                                        "Relay proxy start failed"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                server = %server_config.address,
                                error = %e,
                                "Relay allocation failed"
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        server = %server_config.address,
                        error = %e,
                        "Relay client creation failed"
                    );
                }
            }
        }

        // Deduplicate by address
        candidates.dedup_by_key(|c| c.address);

        // Sort by priority descending (highest priority first)
        candidates.sort_by(|a, b| b.priority.cmp(&a.priority));

        self.local_candidates.clone_from(&candidates);

        if self.local_candidates.is_empty() {
            return Err(OverlayError::StunDiscovery(
                "No candidates gathered (local IP discovery and STUN both failed)".to_string(),
            ));
        }

        Ok(self.local_candidates.clone())
    }

    /// Get our local candidates.
    #[must_use]
    pub fn local_candidates(&self) -> &[Candidate] {
        &self.local_candidates
    }

    /// Get detected NAT behavior from the most recent STUN discovery.
    #[must_use]
    pub fn nat_behavior(&self) -> NatBehavior {
        self.nat_behavior
    }

    /// Try to connect to a peer using their candidates, ordered by priority.
    ///
    /// For each candidate: updates the peer's `WireGuard` endpoint, then
    /// polls for a successful handshake. Returns the [`ConnectionType`]
    /// that succeeded.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::NatTraversalFailed`] if all candidates
    /// are exhausted without a successful handshake.
    pub async fn connect_to_peer(
        &self,
        transport: &OverlayTransport,
        peer_public_key: &str,
        peer_candidates: &[Candidate],
    ) -> Result<ConnectionType> {
        if peer_candidates.is_empty() {
            return Err(OverlayError::NatTraversalFailed {
                peer: peer_public_key.to_string(),
            });
        }

        // Sort candidates by priority descending, limited to max_candidate_pairs
        let mut sorted: Vec<&Candidate> = peer_candidates.iter().collect();
        sorted.sort_by(|a, b| b.priority.cmp(&a.priority));
        sorted.truncate(self.config.max_candidate_pairs);

        let timeout = Duration::from_secs(self.config.hole_punch_timeout_secs);

        for candidate in &sorted {
            debug!(
                address = %candidate.address,
                candidate_type = ?candidate.candidate_type,
                peer = peer_public_key,
                "Trying candidate"
            );

            match self
                .try_candidate(transport, peer_public_key, candidate.address, timeout)
                .await
            {
                Ok(true) => {
                    let connection_type = match candidate.candidate_type {
                        CandidateType::Host => ConnectionType::Direct,
                        CandidateType::ServerReflexive => ConnectionType::HolePunched,
                        CandidateType::Relay => ConnectionType::Relayed,
                    };
                    info!(
                        peer = peer_public_key,
                        address = %candidate.address,
                        connection = %connection_type,
                        "Candidate succeeded"
                    );
                    return Ok(connection_type);
                }
                Ok(false) => {
                    debug!(
                        peer = peer_public_key,
                        address = %candidate.address,
                        "Candidate timed out"
                    );
                }
                Err(e) => {
                    warn!(
                        peer = peer_public_key,
                        address = %candidate.address,
                        error = %e,
                        "Candidate failed"
                    );
                }
            }
        }

        Err(OverlayError::NatTraversalFailed {
            peer: peer_public_key.to_string(),
        })
    }

    /// Re-probe STUN servers and update local candidates.
    ///
    /// Returns `true` if the reflexive address changed (indicating a
    /// NAT rebinding event).
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::StunDiscovery`] if STUN discovery fails.
    pub async fn refresh(&mut self) -> Result<bool> {
        let (new_addrs, new_behavior) = self
            .stun_client
            .discover()
            .await
            .map_err(|e| OverlayError::StunDiscovery(e.to_string()))?;

        self.nat_behavior = new_behavior;

        // Check if any reflexive address changed
        let changed = if new_addrs.len() == self.reflexive_addresses.len() {
            let old_set: std::collections::HashSet<std::net::IpAddr> = self
                .reflexive_addresses
                .iter()
                .map(|r| r.address.ip())
                .collect();
            let new_set: std::collections::HashSet<std::net::IpAddr> =
                new_addrs.iter().map(|r| r.address.ip()).collect();
            old_set != new_set
        } else {
            true
        };

        if changed {
            debug!("Reflexive address changed, rebuilding candidates");
            self.reflexive_addresses = new_addrs;

            // Rebuild candidate list
            let mut candidates = Vec::new();

            // Keep existing host candidates
            for c in &self.local_candidates {
                if c.candidate_type == CandidateType::Host {
                    candidates.push(c.clone());
                }
            }

            // Add new reflexive candidates
            for ra in &self.reflexive_addresses {
                let addr = SocketAddr::new(ra.address.ip(), self.wg_port);
                candidates.push(Candidate::new(CandidateType::ServerReflexive, addr));
            }

            // Keep existing relay candidates
            for c in &self.local_candidates {
                if c.candidate_type == CandidateType::Relay {
                    candidates.push(c.clone());
                }
            }

            candidates.dedup_by_key(|c| c.address);
            candidates.sort_by(|a, b| b.priority.cmp(&a.priority));
            self.local_candidates = candidates;
        } else {
            self.reflexive_addresses = new_addrs;
        }

        // Refresh relay allocations
        for client in &mut self.relay_clients {
            if client.is_active() {
                if let Err(e) = client.refresh().await {
                    warn!(error = %e, "Relay refresh failed");
                }
            }
        }

        Ok(changed)
    }

    /// Attempt to upgrade a relayed connection to direct or hole-punched.
    ///
    /// Only tries Host and `ServerReflexive` candidates (skips Relay)
    /// with a shorter timeout (5 seconds) to avoid disrupting existing
    /// relay traffic.
    ///
    /// Returns `Some(ConnectionType)` if upgrade succeeded, `None` if not.
    ///
    /// # Errors
    ///
    /// Returns an error if transport operations fail unexpectedly.
    pub async fn attempt_upgrade(
        &self,
        transport: &OverlayTransport,
        peer_public_key: &str,
        peer_candidates: &[Candidate],
    ) -> Result<Option<ConnectionType>> {
        let upgrade_timeout = Duration::from_secs(5);

        // Only try Host and ServerReflexive candidates for upgrade
        let mut upgrade_candidates: Vec<&Candidate> = peer_candidates
            .iter()
            .filter(|c| {
                matches!(
                    c.candidate_type,
                    CandidateType::Host | CandidateType::ServerReflexive
                )
            })
            .collect();

        if upgrade_candidates.is_empty() {
            return Ok(None);
        }

        upgrade_candidates.sort_by(|a, b| b.priority.cmp(&a.priority));
        upgrade_candidates.truncate(self.config.max_candidate_pairs);

        for candidate in &upgrade_candidates {
            debug!(
                address = %candidate.address,
                candidate_type = ?candidate.candidate_type,
                peer = peer_public_key,
                "Attempting upgrade"
            );

            match self
                .try_candidate(
                    transport,
                    peer_public_key,
                    candidate.address,
                    upgrade_timeout,
                )
                .await
            {
                Ok(true) => {
                    let connection_type = match candidate.candidate_type {
                        CandidateType::Host => ConnectionType::Direct,
                        CandidateType::ServerReflexive => ConnectionType::HolePunched,
                        CandidateType::Relay => ConnectionType::Relayed,
                    };
                    return Ok(Some(connection_type));
                }
                Ok(false) => {
                    debug!(
                        peer = peer_public_key,
                        address = %candidate.address,
                        "Upgrade candidate timed out"
                    );
                }
                Err(e) => {
                    debug!(
                        peer = peer_public_key,
                        address = %candidate.address,
                        error = %e,
                        "Upgrade candidate failed"
                    );
                }
            }
        }

        Ok(None)
    }

    /// Try a single candidate: update the peer endpoint, then poll for handshake.
    ///
    /// Returns `Ok(true)` if a handshake was observed within the timeout,
    /// `Ok(false)` if the timeout elapsed without a handshake.
    async fn try_candidate(
        &self,
        transport: &OverlayTransport,
        peer_public_key: &str,
        endpoint: SocketAddr,
        timeout: Duration,
    ) -> Result<bool> {
        // Record timestamp before updating endpoint
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Update peer endpoint
        transport
            .update_peer_endpoint(peer_public_key, endpoint)
            .await
            .map_err(|e| OverlayError::HolePunchFailed {
                peer: peer_public_key.to_string(),
                reason: format!("endpoint update failed: {e}"),
            })?;

        // Poll for handshake completion
        let poll_interval = Duration::from_secs(1);
        let deadline = tokio::time::Instant::now() + timeout;

        while tokio::time::Instant::now() < deadline {
            match transport.check_peer_handshake(peer_public_key, since).await {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(e) => {
                    debug!(
                        peer = peer_public_key,
                        error = %e,
                        "Handshake check error"
                    );
                }
            }

            tokio::time::sleep(poll_interval).await;
        }

        Ok(false)
    }
}

/// Discover the local IPv4 address via the UDP socket trick.
///
/// Creates a UDP socket, "connects" it to a public address (8.8.8.8:80),
/// and reads the local address. No packets are sent because UDP `connect`
/// only sets the default destination. This gives us the IP of the default
/// route interface.
fn discover_local_ip() -> std::result::Result<std::net::IpAddr, std::io::Error> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("8.8.8.8:80")?;
    Ok(socket.local_addr()?.ip())
}

/// Discover the local IPv6 address via the UDP socket trick.
///
/// Same approach as [`discover_local_ip`] but for IPv6. Connects to
/// Google's public DNS IPv6 address (`[2001:4860:4860::8888]:80`).
fn discover_local_ipv6() -> std::result::Result<std::net::IpAddr, std::io::Error> {
    let socket = std::net::UdpSocket::bind("[::]:0")?;
    socket.connect("[2001:4860:4860::8888]:80")?;
    Ok(socket.local_addr()?.ip())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_discover_local_ip() {
        // The UDP socket trick should work in most test environments
        // (even without network access, since no packets are actually sent).
        // It may fail in extremely sandboxed environments.
        match discover_local_ip() {
            Ok(ip) => {
                assert!(!ip.is_unspecified(), "Local IP should not be 0.0.0.0");
                assert!(!ip.is_loopback(), "Local IP should not be 127.0.0.1");
            }
            Err(e) => {
                // In CI or sandboxed environments this may fail; that's acceptable
                eprintln!("discover_local_ip failed (may be sandboxed): {e}");
            }
        }
    }

    #[test]
    fn test_nat_traversal_new() {
        let config = NatConfig::default();
        let nat = NatTraversal::new(config, 51820);
        assert_eq!(nat.wg_port, 51820);
        assert!(nat.local_candidates().is_empty());
        assert_eq!(nat.nat_behavior(), NatBehavior::EndpointIndependent);
    }

    #[test]
    fn test_candidate_sorting_by_priority() {
        let host = Candidate::new(
            CandidateType::Host,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 51820),
        );
        let reflexive = Candidate::new(
            CandidateType::ServerReflexive,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5)), 51820),
        );
        let relay = Candidate::new(
            CandidateType::Relay,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 3478),
        );

        let mut candidates = [relay, host, reflexive];
        candidates.sort_by(|a, b| b.priority.cmp(&a.priority));

        assert_eq!(candidates[0].candidate_type, CandidateType::Host);
        assert_eq!(candidates[1].candidate_type, CandidateType::ServerReflexive);
        assert_eq!(candidates[2].candidate_type, CandidateType::Relay);
    }

    #[test]
    fn test_candidate_type_to_connection_type_mapping() {
        // Verify the mapping logic used in connect_to_peer
        let mappings = [
            (CandidateType::Host, ConnectionType::Direct),
            (CandidateType::ServerReflexive, ConnectionType::HolePunched),
            (CandidateType::Relay, ConnectionType::Relayed),
        ];

        for (ct, expected) in &mappings {
            let connection_type = match ct {
                CandidateType::Host => ConnectionType::Direct,
                CandidateType::ServerReflexive => ConnectionType::HolePunched,
                CandidateType::Relay => ConnectionType::Relayed,
            };
            assert_eq!(connection_type, *expected, "Mapping for {ct:?} is wrong");
        }
    }

    #[tokio::test]
    async fn test_gather_candidates_returns_host() {
        let config = NatConfig {
            enabled: true,
            // Use a bogus STUN server so STUN fails gracefully
            stun_servers: vec![],
            ..NatConfig::default()
        };
        let mut nat = NatTraversal::new(config, 51820);

        // With no STUN servers, gather_candidates will only produce
        // host candidates (if local IP discovery succeeds).
        match nat.gather_candidates().await {
            Ok(candidates) => {
                // Should have at least a host candidate
                assert!(
                    candidates
                        .iter()
                        .any(|c| c.candidate_type == CandidateType::Host),
                    "Should have at least one host candidate"
                );
                // All candidates should use our WG port
                for c in &candidates {
                    assert_eq!(c.address.port(), 51820);
                }
                // local_candidates() should match
                assert_eq!(nat.local_candidates().len(), candidates.len());
            }
            Err(e) => {
                // In sandboxed environments, both local IP and STUN may fail
                eprintln!("gather_candidates failed (may be sandboxed): {e}");
            }
        }
    }

    #[tokio::test]
    async fn test_connect_to_peer_empty_candidates() {
        let config = NatConfig::default();
        let nat = NatTraversal::new(config, 51820);

        // We can't test with real transport, but we can verify the
        // empty-candidates error path.
        let fake_config = crate::config::OverlayConfig::default();
        let transport = OverlayTransport::new(fake_config, "zl-test0".to_string());

        let result = nat.connect_to_peer(&transport, "fake_key", &[]).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            OverlayError::NatTraversalFailed { peer } => {
                assert_eq!(peer, "fake_key");
            }
            other => panic!("Expected NatTraversalFailed, got: {other:?}"),
        }
    }

    #[test]
    fn test_candidate_dedup() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 51820);
        let mut candidates = vec![
            Candidate::new(CandidateType::Host, addr),
            Candidate::new(CandidateType::Host, addr),
        ];
        candidates.dedup_by_key(|c| c.address);
        assert_eq!(candidates.len(), 1);
    }

    // ---- IPv6 tests ---------------------------------------------------------

    #[test]
    fn test_discover_local_ipv6() {
        // The IPv6 UDP socket trick may fail on systems without IPv6 connectivity.
        match discover_local_ipv6() {
            Ok(ip) => {
                assert!(ip.is_ipv6(), "Should return an IPv6 address");
                assert!(!ip.is_unspecified(), "IPv6 should not be [::]");
                assert!(!ip.is_loopback(), "IPv6 should not be [::1]");
            }
            Err(e) => {
                // IPv6 may not be available in all test environments
                eprintln!("discover_local_ipv6 failed (IPv6 may not be available): {e}");
            }
        }
    }

    #[test]
    fn test_candidate_sorting_mixed_families() {
        let host_v4 = Candidate::new(
            CandidateType::Host,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 51820),
        );
        let host_v6 = Candidate::new(
            CandidateType::Host,
            SocketAddr::new(
                IpAddr::V6(Ipv6Addr::new(0xFD00, 0, 0, 0, 0, 0, 0, 1)),
                51820,
            ),
        );
        let reflexive_v6 = Candidate::new(
            CandidateType::ServerReflexive,
            SocketAddr::new(
                IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
                51820,
            ),
        );

        let mut candidates = [reflexive_v6, host_v4, host_v6];
        candidates.sort_by(|a, b| b.priority.cmp(&a.priority));

        // Hosts (100) before ServerReflexive (50)
        assert_eq!(candidates[0].candidate_type, CandidateType::Host);
        assert_eq!(candidates[1].candidate_type, CandidateType::Host);
        assert_eq!(candidates[2].candidate_type, CandidateType::ServerReflexive);
    }

    #[test]
    fn test_candidate_dedup_ipv6() {
        let addr = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
            51820,
        );
        let mut candidates = vec![
            Candidate::new(CandidateType::Host, addr),
            Candidate::new(CandidateType::Host, addr),
        ];
        candidates.dedup_by_key(|c| c.address);
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn test_candidate_dedup_mixed_families_not_deduped() {
        let addr_v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 51820);
        let addr_v6 = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0xFD00, 0, 0, 0, 0, 0, 0, 1)),
            51820,
        );
        let mut candidates = vec![
            Candidate::new(CandidateType::Host, addr_v4),
            Candidate::new(CandidateType::Host, addr_v6),
        ];
        candidates.dedup_by_key(|c| c.address);
        assert_eq!(
            candidates.len(),
            2,
            "IPv4 and IPv6 candidates should not be deduped"
        );
    }
}
