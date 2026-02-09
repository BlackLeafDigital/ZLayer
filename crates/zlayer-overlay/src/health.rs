//! Health checking for overlay network peers
//!
//! Monitors overlay peer connectivity through handshake times
//! and optional ping tests. Uses UAPI to query the boringtun
//! device directly -- no external `wg` binary required.

use crate::error::{OverlayError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Default health check interval
pub const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum time since last handshake to consider peer healthy (3 minutes)
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 180;

/// Ping timeout in seconds
pub const PING_TIMEOUT_SECS: u64 = 5;

/// Status of a single peer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerStatus {
    /// Peer's public key
    pub public_key: String,

    /// Peer's overlay IP (if known)
    pub overlay_ip: Option<Ipv4Addr>,

    /// Whether the peer is considered healthy
    pub healthy: bool,

    /// Time since last overlay handshake (seconds)
    pub last_handshake_secs: Option<u64>,

    /// Last ping RTT in milliseconds
    pub last_ping_ms: Option<u64>,

    /// Number of consecutive health check failures
    pub failure_count: u32,

    /// Last check timestamp (Unix epoch)
    pub last_check: u64,
}

/// Aggregated health status for the overlay network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayHealth {
    /// Overlay interface name
    pub interface: String,

    /// Total number of configured peers
    pub total_peers: usize,

    /// Number of healthy peers
    pub healthy_peers: usize,

    /// Number of unhealthy peers
    pub unhealthy_peers: usize,

    /// Individual peer statuses
    pub peers: Vec<PeerStatus>,

    /// Last check timestamp (Unix epoch)
    pub last_check: u64,
}

/// Raw peer statistics from the overlay transport (via UAPI)
#[derive(Debug, Clone)]
pub struct WgPeerStats {
    pub public_key: String,
    pub endpoint: Option<String>,
    pub allowed_ips: Vec<String>,
    pub last_handshake_time: Option<u64>,
    pub transfer_rx: u64,
    pub transfer_tx: u64,
}

/// Overlay health checker
///
/// Monitors peer connectivity through overlay handshake times
/// and optional ping tests. Queries the boringtun device via UAPI.
pub struct OverlayHealthChecker {
    /// Overlay interface name
    interface: String,

    /// Health check interval
    check_interval: Duration,

    /// Handshake timeout threshold
    handshake_timeout: Duration,

    /// Peer status cache
    peer_status: RwLock<HashMap<String, PeerStatus>>,
}

impl OverlayHealthChecker {
    /// Create a new health checker for the given interface
    pub fn new(interface: &str, check_interval: Duration) -> Self {
        Self {
            interface: interface.to_string(),
            check_interval,
            handshake_timeout: Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
            peer_status: RwLock::new(HashMap::new()),
        }
    }

    /// Create with default settings
    pub fn default_for_interface(interface: &str) -> Self {
        Self::new(interface, DEFAULT_CHECK_INTERVAL)
    }

    /// Set the handshake timeout threshold
    pub fn with_handshake_timeout(mut self, timeout: Duration) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    /// Run continuous health check loop
    ///
    /// Calls the provided callback with peer status updates.
    pub async fn run<F>(&self, mut on_status_change: F)
    where
        F: FnMut(&str, bool) + Send + 'static,
    {
        info!(
            interface = %self.interface,
            interval_secs = self.check_interval.as_secs(),
            "Starting health check loop"
        );

        loop {
            match self.check_all().await {
                Ok(health) => {
                    for peer in &health.peers {
                        // Check for status change
                        let mut cache = self.peer_status.write().await;
                        let changed = cache
                            .get(&peer.public_key)
                            .map(|prev| prev.healthy != peer.healthy)
                            .unwrap_or(true);

                        if changed {
                            on_status_change(&peer.public_key, peer.healthy);
                        }

                        cache.insert(peer.public_key.clone(), peer.clone());
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Health check failed");
                }
            }

            tokio::time::sleep(self.check_interval).await;
        }
    }

    /// Check all peer connections once
    pub async fn check_all(&self) -> Result<OverlayHealth> {
        let now = current_timestamp();
        let stats = self.get_wg_stats().await?;

        let mut peers = Vec::with_capacity(stats.len());
        let mut healthy_count = 0;

        for stat in stats {
            let healthy = self.is_peer_healthy(&stat);

            if healthy {
                healthy_count += 1;
            }

            // Extract overlay IP from allowed IPs (first /32 address)
            let overlay_ip = stat.allowed_ips.iter().find_map(|ip| {
                if ip.ends_with("/32") {
                    ip.trim_end_matches("/32").parse().ok()
                } else {
                    None
                }
            });

            let status = PeerStatus {
                public_key: stat.public_key,
                overlay_ip,
                healthy,
                last_handshake_secs: stat.last_handshake_time.map(|t| now.saturating_sub(t)),
                last_ping_ms: None, // Ping is optional
                failure_count: if healthy { 0 } else { 1 },
                last_check: now,
            };

            peers.push(status);
        }

        let total = peers.len();
        Ok(OverlayHealth {
            interface: self.interface.clone(),
            total_peers: total,
            healthy_peers: healthy_count,
            unhealthy_peers: total - healthy_count,
            peers,
            last_check: now,
        })
    }

    /// Check if a specific peer is healthy based on its stats
    fn is_peer_healthy(&self, stats: &WgPeerStats) -> bool {
        let now = current_timestamp();
        let timeout_secs = self.handshake_timeout.as_secs();

        stats
            .last_handshake_time
            .map(|t| now.saturating_sub(t) < timeout_secs)
            .unwrap_or(false)
    }

    /// Ping a specific peer via its overlay IP
    ///
    /// Returns the RTT on success.
    pub async fn ping_peer(&self, overlay_ip: Ipv4Addr) -> Result<Duration> {
        let start = Instant::now();

        // Use ICMP ping via the ping command
        let output = tokio::time::timeout(
            Duration::from_secs(PING_TIMEOUT_SECS),
            Command::new("ping")
                .args([
                    "-c",
                    "1", // Single ping
                    "-W",
                    &PING_TIMEOUT_SECS.to_string(), // Timeout
                    &overlay_ip.to_string(),
                ])
                .output(),
        )
        .await;

        match output {
            Ok(Ok(result)) if result.status.success() => Ok(start.elapsed()),
            Ok(Ok(_)) => Err(OverlayError::PeerUnreachable {
                ip: overlay_ip,
                reason: "ping failed".to_string(),
            }),
            Ok(Err(e)) => Err(OverlayError::PeerUnreachable {
                ip: overlay_ip,
                reason: e.to_string(),
            }),
            Err(_) => Err(OverlayError::PeerUnreachable {
                ip: overlay_ip,
                reason: "timeout".to_string(),
            }),
        }
    }

    /// TCP connect test to a specific peer and port
    ///
    /// Returns the connection time on success.
    pub async fn tcp_check(&self, overlay_ip: Ipv4Addr, port: u16) -> Result<Duration> {
        let start = Instant::now();

        let result = tokio::time::timeout(
            Duration::from_secs(PING_TIMEOUT_SECS),
            tokio::net::TcpStream::connect((overlay_ip, port)),
        )
        .await;

        match result {
            Ok(Ok(_stream)) => Ok(start.elapsed()),
            Ok(Err(e)) => Err(OverlayError::PeerUnreachable {
                ip: overlay_ip,
                reason: e.to_string(),
            }),
            Err(_) => Err(OverlayError::PeerUnreachable {
                ip: overlay_ip,
                reason: "timeout".to_string(),
            }),
        }
    }

    /// Get raw overlay peer statistics via UAPI.
    ///
    /// Connects to the boringtun UAPI Unix socket and sends a `get=1`
    /// query. Parses the key=value response into [`WgPeerStats`] entries.
    /// No external `wg` binary is required.
    async fn get_wg_stats(&self) -> Result<Vec<WgPeerStats>> {
        let sock_path = format!("/var/run/wireguard/{}.sock", self.interface);

        let response = match uapi_get_raw(&sock_path).await {
            Ok(resp) => resp,
            Err(e) => {
                let msg = e.to_string();
                // If the socket doesn't exist, the interface isn't running
                if msg.contains("No such file")
                    || msg.contains("Connection refused")
                    || msg.contains("not found")
                {
                    return Ok(Vec::new());
                }
                return Err(OverlayError::TransportCommand(msg));
            }
        };

        let peers = parse_uapi_get_response(&response);

        debug!(interface = %self.interface, peer_count = peers.len(), "Retrieved overlay peer stats via UAPI");
        Ok(peers)
    }

    /// Get cached peer status
    pub async fn get_cached_status(&self, public_key: &str) -> Option<PeerStatus> {
        let cache = self.peer_status.read().await;
        cache.get(public_key).cloned()
    }

    /// Get the health check interval
    pub fn check_interval(&self) -> Duration {
        self.check_interval
    }

    /// Get the interface name
    pub fn interface(&self) -> &str {
        &self.interface
    }
}

/// Get current Unix timestamp
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Send a UAPI `get=1` query to the boringtun socket and return the raw response.
async fn uapi_get_raw(sock_path: &str) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let mut stream = tokio::net::UnixStream::connect(sock_path).await?;
    stream.write_all(b"get=1\n\n").await?;
    stream.shutdown().await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    Ok(response)
}

/// Convert a hex-encoded key (from UAPI) to base64 (standard WireGuard format).
fn hex_key_to_base64(hex_key: &str) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    match hex::decode(hex_key) {
        Ok(bytes) => STANDARD.encode(bytes),
        Err(_) => hex_key.to_string(), // fallback: return as-is
    }
}

/// Parse a UAPI `get=1` response into a list of [`WgPeerStats`].
///
/// The UAPI response is newline-delimited `key=value` pairs. Interface-level
/// fields come first (private_key, listen_port, fwmark). Each `public_key=`
/// line starts a new peer block. Peer fields include endpoint,
/// last_handshake_time_sec, last_handshake_time_nsec, rx_bytes, tx_bytes,
/// persistent_keepalive_interval, and allowed_ip.
fn parse_uapi_get_response(response: &str) -> Vec<WgPeerStats> {
    let mut peers = Vec::new();
    let mut current_peer: Option<WgPeerStats> = None;
    let mut in_peer = false;

    for line in response.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("errno=") {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        match key {
            "public_key" => {
                // Flush previous peer if any
                if let Some(peer) = current_peer.take() {
                    peers.push(peer);
                }
                in_peer = true;
                current_peer = Some(WgPeerStats {
                    public_key: hex_key_to_base64(value),
                    endpoint: None,
                    allowed_ips: Vec::new(),
                    last_handshake_time: None,
                    transfer_rx: 0,
                    transfer_tx: 0,
                });
            }
            "endpoint" if in_peer => {
                if let Some(ref mut peer) = current_peer {
                    if value != "(none)" {
                        peer.endpoint = Some(value.to_string());
                    }
                }
            }
            "allowed_ip" if in_peer => {
                if let Some(ref mut peer) = current_peer {
                    peer.allowed_ips.push(value.to_string());
                }
            }
            "last_handshake_time_sec" if in_peer => {
                if let Some(ref mut peer) = current_peer {
                    if let Ok(t) = value.parse::<u64>() {
                        if t > 0 {
                            peer.last_handshake_time = Some(t);
                        }
                    }
                }
            }
            "rx_bytes" if in_peer => {
                if let Some(ref mut peer) = current_peer {
                    peer.transfer_rx = value.parse().unwrap_or(0);
                }
            }
            "tx_bytes" if in_peer => {
                if let Some(ref mut peer) = current_peer {
                    peer.transfer_tx = value.parse().unwrap_or(0);
                }
            }
            // Skip interface-level fields and other peer fields we don't need
            _ => {}
        }
    }

    // Flush the last peer
    if let Some(peer) = current_peer {
        peers.push(peer);
    }

    peers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_status_serialization() {
        let status = PeerStatus {
            public_key: "test_key".to_string(),
            overlay_ip: Some("10.200.0.5".parse().unwrap()),
            healthy: true,
            last_handshake_secs: Some(10),
            last_ping_ms: Some(5),
            failure_count: 0,
            last_check: 1234567890,
        };

        let json = serde_json::to_string(&status).unwrap();
        let deserialized: PeerStatus = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.public_key, "test_key");
        assert!(deserialized.healthy);
    }

    #[test]
    fn test_overlay_health_serialization() {
        let health = OverlayHealth {
            interface: "zl-overlay0".to_string(),
            total_peers: 2,
            healthy_peers: 1,
            unhealthy_peers: 1,
            peers: vec![],
            last_check: 1234567890,
        };

        let json = serde_json::to_string_pretty(&health).unwrap();
        assert!(json.contains("zl-overlay0"));
    }

    #[test]
    fn test_health_checker_creation() {
        let checker = OverlayHealthChecker::new("wg0", Duration::from_secs(60));
        assert_eq!(checker.interface(), "wg0");
        assert_eq!(checker.check_interval(), Duration::from_secs(60));
    }

    #[test]
    fn test_is_peer_healthy_recent_handshake() {
        let checker = OverlayHealthChecker::new("wg0", Duration::from_secs(30));

        let now = current_timestamp();
        let stats = WgPeerStats {
            public_key: "key".to_string(),
            endpoint: None,
            allowed_ips: vec![],
            last_handshake_time: Some(now - 60), // 60 seconds ago
            transfer_rx: 0,
            transfer_tx: 0,
        };

        // Should be healthy (handshake within 180 seconds)
        assert!(checker.is_peer_healthy(&stats));
    }

    #[test]
    fn test_is_peer_healthy_stale_handshake() {
        let checker = OverlayHealthChecker::new("wg0", Duration::from_secs(30));

        let now = current_timestamp();
        let stats = WgPeerStats {
            public_key: "key".to_string(),
            endpoint: None,
            allowed_ips: vec![],
            last_handshake_time: Some(now - 300), // 5 minutes ago
            transfer_rx: 0,
            transfer_tx: 0,
        };

        // Should be unhealthy (handshake > 180 seconds ago)
        assert!(!checker.is_peer_healthy(&stats));
    }

    #[test]
    fn test_is_peer_healthy_no_handshake() {
        let checker = OverlayHealthChecker::new("wg0", Duration::from_secs(30));

        let stats = WgPeerStats {
            public_key: "key".to_string(),
            endpoint: None,
            allowed_ips: vec![],
            last_handshake_time: None,
            transfer_rx: 0,
            transfer_tx: 0,
        };

        // Should be unhealthy (no handshake ever)
        assert!(!checker.is_peer_healthy(&stats));
    }

    #[test]
    fn test_parse_uapi_get_response() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};

        // Simulate a hex-encoded 32-byte key for testing
        let key_bytes = [0xABu8; 32];
        let hex_key = hex::encode(key_bytes);
        let expected_b64 = STANDARD.encode(key_bytes);

        let response = format!(
            "private_key=0000000000000000000000000000000000000000000000000000000000000000\n\
             listen_port=51820\n\
             public_key={hex_key}\n\
             endpoint=192.168.1.5:51820\n\
             allowed_ip=10.200.0.2/32\n\
             last_handshake_time_sec=1700000000\n\
             last_handshake_time_nsec=0\n\
             rx_bytes=12345\n\
             tx_bytes=67890\n\
             persistent_keepalive_interval=25\n\
             errno=0\n"
        );

        let peers = parse_uapi_get_response(&response);
        assert_eq!(peers.len(), 1);

        let peer = &peers[0];
        assert_eq!(peer.public_key, expected_b64);
        assert_eq!(peer.endpoint, Some("192.168.1.5:51820".to_string()));
        assert_eq!(peer.allowed_ips, vec!["10.200.0.2/32".to_string()]);
        assert_eq!(peer.last_handshake_time, Some(1700000000));
        assert_eq!(peer.transfer_rx, 12345);
        assert_eq!(peer.transfer_tx, 67890);
    }

    #[test]
    fn test_parse_uapi_get_response_multiple_peers() {
        let key1 = hex::encode([0x01u8; 32]);
        let key2 = hex::encode([0x02u8; 32]);

        let response = format!(
            "private_key=0000000000000000000000000000000000000000000000000000000000000000\n\
             listen_port=51820\n\
             public_key={key1}\n\
             endpoint=10.0.0.1:51820\n\
             allowed_ip=10.200.0.2/32\n\
             rx_bytes=100\n\
             tx_bytes=200\n\
             public_key={key2}\n\
             endpoint=10.0.0.2:51821\n\
             allowed_ip=10.200.0.3/32\n\
             allowed_ip=10.200.1.0/24\n\
             rx_bytes=300\n\
             tx_bytes=400\n\
             errno=0\n"
        );

        let peers = parse_uapi_get_response(&response);
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].transfer_rx, 100);
        assert_eq!(peers[1].transfer_rx, 300);
        assert_eq!(peers[1].allowed_ips.len(), 2);
    }

    #[test]
    fn test_parse_uapi_get_response_empty() {
        let response = "private_key=0000\nlisten_port=51820\nerrno=0\n";
        let peers = parse_uapi_get_response(response);
        assert!(peers.is_empty());
    }

    #[test]
    fn test_hex_key_to_base64_roundtrip() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};

        let key_bytes = [0xCDu8; 32];
        let hex_key = hex::encode(key_bytes);
        let b64 = hex_key_to_base64(&hex_key);
        let expected = STANDARD.encode(key_bytes);
        assert_eq!(b64, expected);
    }
}
