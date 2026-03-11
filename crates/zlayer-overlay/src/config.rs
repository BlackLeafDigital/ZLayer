//! Overlay network configuration

#[cfg(feature = "nat")]
use crate::nat::NatConfig;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

/// Overlay network configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OverlayConfig {
    /// Local overlay endpoint (`WireGuard` protocol)
    pub local_endpoint: SocketAddr,

    /// Private key (x25519)
    pub private_key: String,

    /// Public key (derived from private key)
    #[serde(default = "OverlayConfig::default_public_key")]
    pub public_key: String,

    /// Overlay network CIDR (supports both IPv4 e.g. "10.0.0.0/8" and IPv6 e.g. "`fd00::/48`")
    #[serde(default = "OverlayConfig::default_cidr")]
    pub overlay_cidr: String,

    /// Peer discovery interval
    #[serde(default = "OverlayConfig::default_discovery")]
    pub peer_discovery_interval: Duration,

    /// NAT traversal configuration (requires "nat" feature)
    #[cfg(feature = "nat")]
    #[serde(default)]
    pub nat: NatConfig,
}

impl OverlayConfig {
    fn default_public_key() -> String {
        String::new()
    }

    fn default_cidr() -> String {
        "10.0.0.0/8".to_string()
    }

    fn default_discovery() -> Duration {
        Duration::from_secs(30)
    }
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            local_endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 51820),
            private_key: String::new(),
            public_key: String::new(),
            overlay_cidr: "10.0.0.0/8".to_string(),
            peer_discovery_interval: Duration::from_secs(30),
            #[cfg(feature = "nat")]
            nat: NatConfig::default(),
        }
    }
}

/// Peer information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeerInfo {
    /// Peer public key
    pub public_key: String,

    /// Endpoint address
    pub endpoint: SocketAddr,

    /// Allowed IPs
    pub allowed_ips: String,

    /// Persistent keepalive interval
    pub persistent_keepalive_interval: Duration,
}

impl PeerInfo {
    /// Create a new peer info
    #[must_use]
    pub fn new(
        public_key: String,
        endpoint: SocketAddr,
        allowed_ips: &str,
        persistent_keepalive_interval: Duration,
    ) -> Self {
        Self {
            public_key,
            endpoint,
            allowed_ips: allowed_ips.to_string(),
            persistent_keepalive_interval,
        }
    }

    /// Create a peer config block (`WireGuard` protocol format)
    #[must_use]
    pub fn to_peer_config(&self) -> String {
        format!(
            "[Peer]\n\
             PublicKey = {}\n\
             Endpoint = {}\n\
             AllowedIPs = {}\n\
             PersistentKeepalive = {}\n",
            self.public_key,
            self.endpoint,
            self.allowed_ips,
            self.persistent_keepalive_interval.as_secs()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_info_to_peer_config() {
        let peer = PeerInfo::new(
            "public_key_here".to_string(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 51820),
            "10.0.0.2/32",
            Duration::from_secs(25),
        );

        let config = peer.to_peer_config();
        assert!(config.contains("PublicKey = public_key_here"));
        assert!(config.contains("Endpoint = 192.168.1.1:51820"));
    }

    #[test]
    fn test_overlay_config_default() {
        let config = OverlayConfig::default();
        assert_eq!(config.local_endpoint.port(), 51820);
        assert_eq!(config.overlay_cidr, "10.0.0.0/8");
    }

    #[test]
    fn test_peer_info_to_peer_config_v6() {
        use std::net::Ipv6Addr;

        let peer = PeerInfo::new(
            "public_key_here".to_string(),
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 51820),
            "fd00::2/128",
            Duration::from_secs(25),
        );

        let config = peer.to_peer_config();
        assert!(config.contains("PublicKey = public_key_here"));
        assert!(config.contains("Endpoint = [::1]:51820"));
        assert!(config.contains("AllowedIPs = fd00::2/128"));
    }

    #[test]
    fn test_overlay_config_accepts_ipv6_cidr() {
        let config = OverlayConfig {
            overlay_cidr: "fd00:200::/48".to_string(),
            ..OverlayConfig::default()
        };
        assert_eq!(config.overlay_cidr, "fd00:200::/48");
    }
}
