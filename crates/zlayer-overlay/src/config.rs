//! Overlay network configuration

#[cfg(feature = "nat")]
use crate::nat::NatConfig;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
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
    ///
    /// Historically stores the per-node slice / host IP (e.g. `10.200.0.0/28`
    /// or `10.200.0.1/32`) that the local TUN/Wintun adapter is assigned.
    /// It is *not* the full cluster CIDR — use [`Self::cluster_cidr`] for that.
    #[serde(default = "OverlayConfig::default_cidr")]
    pub overlay_cidr: String,

    /// Full cluster CIDR (e.g. `10.200.0.0/16`).
    ///
    /// Used on Windows to install a catch-all host route pointing the
    /// entire cluster range at the Wintun adapter so traffic to remote-node
    /// container IPs flows through the overlay (HCN auto-installs the more
    /// specific local /28 → vSwitch route, and longest-prefix-match routes
    /// local traffic to the vSwitch). `None` on pre-cluster-CIDR configs;
    /// callers should fall back to skipping the route install in that case.
    #[serde(default)]
    pub cluster_cidr: Option<String>,

    /// Peer discovery interval
    #[serde(default = "OverlayConfig::default_discovery")]
    pub peer_discovery_interval: Duration,

    /// NAT traversal configuration (requires "nat" feature)
    #[cfg(feature = "nat")]
    #[serde(default)]
    pub nat: NatConfig,

    /// Directory containing per-interface `WireGuard` UAPI sockets
    /// (`<dir>/<interface_name>.sock`). Defaults to `/var/run/wireguard`
    /// on Linux for `wg(8)` interop; overridden to `{data_dir}/run/wireguard`
    /// by the daemon when running with a non-default `--data-dir` to keep
    /// a test/dev daemon hermetic.
    #[serde(default = "OverlayConfig::default_uapi_sock_dir")]
    pub uapi_sock_dir: PathBuf,

    /// MTU applied to the overlay tunnel interface.
    ///
    /// Defaults to `1420`: the standard `WireGuard` tunnel MTU (1500-byte
    /// underlay MTU minus 80 bytes of `WireGuard` encapsulation overhead).
    /// Matches the Windows Wintun default in `transport.rs`.
    ///
    /// Applied to the TUN interface on Linux/macOS at configure time. On
    /// Windows, Wintun exposes no per-adapter MTU setter, so this value is
    /// advisory there (IP Helper may override it). Lowering it matters when
    /// running `WireGuard`-in-`WireGuard` — e.g. the overlay riding over
    /// another mesh such as netbird — where stacked encapsulation shrinks
    /// the usable payload and an over-large MTU causes silent blackholing
    /// of oversized packets that can't be fragmented.
    #[serde(default = "default_mtu")]
    pub mtu: u32,
}

/// Default overlay tunnel MTU: 1500-byte underlay minus 80 bytes of
/// `WireGuard` encapsulation overhead.
fn default_mtu() -> u32 {
    1420
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

    /// Platform-default `WireGuard` UAPI socket directory.
    ///
    /// Linux: `/var/run/wireguard` (FHS, matches `wg(8)`).
    /// macOS / Windows: `/var/run/wireguard` is the historical literal
    /// the transport used; the daemon overrides this to a data-dir-aware
    /// path via [`zlayer_paths::ZLayerDirs::wireguard`] when a non-default
    /// `--data-dir` is in play.
    fn default_uapi_sock_dir() -> PathBuf {
        PathBuf::from("/var/run/wireguard")
    }
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            local_endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 51820),
            private_key: String::new(),
            public_key: String::new(),
            overlay_cidr: "10.0.0.0/8".to_string(),
            cluster_cidr: None,
            peer_discovery_interval: Duration::from_secs(30),
            #[cfg(feature = "nat")]
            nat: NatConfig::default(),
            uapi_sock_dir: Self::default_uapi_sock_dir(),
            mtu: default_mtu(),
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

    #[test]
    fn test_overlay_config_default_mtu() {
        let config = OverlayConfig::default();
        assert_eq!(config.mtu, 1420);
    }

    #[test]
    fn test_overlay_config_mtu_serde_round_trip() {
        let config = OverlayConfig {
            mtu: 1280,
            ..OverlayConfig::default()
        };
        let json = serde_json::to_string(&config).expect("serialize");
        assert!(json.contains("\"mtu\":1280"));
        let decoded: OverlayConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, config);
        assert_eq!(decoded.mtu, 1280);
    }

    #[test]
    fn test_overlay_config_missing_mtu_defaults() {
        // An on-disk config written before the `mtu` field existed must
        // still deserialize cleanly and pick up the 1420 default.
        let json = r#"{
            "local_endpoint": "0.0.0.0:51820",
            "private_key": ""
        }"#;
        let config: OverlayConfig = serde_json::from_str(json).expect("deserialize without mtu");
        assert_eq!(config.mtu, 1420);
    }
}
