//! Overlay network bootstrap functionality
//!
//! Provides initialization and joining capabilities for overlay networks,
//! including WireGuard keypair generation, interface creation, and peer management.

use crate::allocator::IpAllocator;
use crate::config::PeerInfo;
use crate::error::{OverlayError, Result};
use crate::wireguard::WireGuardManager;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, info, warn};

/// Default WireGuard interface name for ZLayer overlay
pub const DEFAULT_INTERFACE_NAME: &str = "wg-zlayer0";

/// Default WireGuard listen port
pub const DEFAULT_WG_PORT: u16 = 51820;

/// Default overlay network CIDR
pub const DEFAULT_OVERLAY_CIDR: &str = "10.200.0.0/16";

/// Default persistent keepalive interval (seconds)
pub const DEFAULT_KEEPALIVE_SECS: u16 = 25;

/// Overlay network bootstrap configuration
///
/// Contains all configuration needed to initialize and manage
/// an overlay network on a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapConfig {
    /// Network CIDR (e.g., "10.200.0.0/16")
    pub cidr: String,

    /// This node's overlay IP address
    pub node_ip: Ipv4Addr,

    /// WireGuard interface name
    pub interface: String,

    /// WireGuard listen port
    pub port: u16,

    /// This node's WireGuard private key
    pub private_key: String,

    /// This node's WireGuard public key
    pub public_key: String,

    /// Whether this node is the cluster leader
    pub is_leader: bool,

    /// Creation timestamp (Unix epoch seconds)
    pub created_at: u64,
}

impl BootstrapConfig {
    /// Get the overlay IP with /32 prefix for allowed IPs
    pub fn allowed_ip(&self) -> String {
        format!("{}/32", self.node_ip)
    }
}

/// Peer configuration for overlay network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerConfig {
    /// Peer's node ID (for identification)
    pub node_id: String,

    /// Peer's WireGuard public key
    pub public_key: String,

    /// Peer's public endpoint (host:port)
    pub endpoint: String,

    /// Peer's overlay IP address
    pub overlay_ip: Ipv4Addr,

    /// Optional persistent keepalive interval in seconds
    #[serde(default)]
    pub keepalive: Option<u16>,
}

impl PeerConfig {
    /// Create a new peer configuration
    pub fn new(
        node_id: String,
        public_key: String,
        endpoint: String,
        overlay_ip: Ipv4Addr,
    ) -> Self {
        Self {
            node_id,
            public_key,
            endpoint,
            overlay_ip,
            keepalive: Some(DEFAULT_KEEPALIVE_SECS),
        }
    }

    /// Convert to PeerInfo for WireGuard configuration
    pub fn to_peer_info(&self) -> std::result::Result<PeerInfo, Box<dyn std::error::Error>> {
        let endpoint: SocketAddr = self.endpoint.parse()?;
        let keepalive =
            Duration::from_secs(self.keepalive.unwrap_or(DEFAULT_KEEPALIVE_SECS) as u64);

        Ok(PeerInfo::new(
            self.public_key.clone(),
            endpoint,
            &format!("{}/32", self.overlay_ip),
            keepalive,
        ))
    }
}

/// Persistent state for the overlay bootstrap
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapState {
    /// Bootstrap configuration
    pub config: BootstrapConfig,

    /// List of configured peers
    pub peers: Vec<PeerConfig>,

    /// IP allocator state (only for leader)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocator_state: Option<crate::allocator::IpAllocatorState>,
}

/// Bootstrap manager for overlay network
///
/// Handles overlay network initialization, peer management,
/// and WireGuard interface configuration.
pub struct OverlayBootstrap {
    /// Bootstrap configuration
    config: BootstrapConfig,

    /// Configured peers
    peers: Vec<PeerConfig>,

    /// Data directory for persistent state
    data_dir: PathBuf,

    /// IP allocator (only for leader nodes)
    allocator: Option<IpAllocator>,
}

impl OverlayBootstrap {
    /// Initialize as cluster leader (first node in the overlay)
    ///
    /// This generates a new WireGuard keypair, allocates the first IP
    /// in the CIDR range, and prepares the node as the overlay leader.
    ///
    /// # Arguments
    /// * `cidr` - Overlay network CIDR (e.g., "10.200.0.0/16")
    /// * `port` - WireGuard listen port
    /// * `data_dir` - Directory for persistent state
    ///
    /// # Example
    /// ```ignore
    /// let bootstrap = OverlayBootstrap::init_leader(
    ///     "10.200.0.0/16",
    ///     51820,
    ///     Path::new("/var/lib/zlayer"),
    /// ).await?;
    /// ```
    pub async fn init_leader(cidr: &str, port: u16, data_dir: &Path) -> Result<Self> {
        // Check if already initialized
        let config_path = data_dir.join("overlay_bootstrap.json");
        if config_path.exists() {
            return Err(OverlayError::AlreadyInitialized(
                config_path.display().to_string(),
            ));
        }

        // Ensure data directory exists
        tokio::fs::create_dir_all(data_dir).await?;

        // Generate WireGuard keypair
        info!("Generating WireGuard keypair for leader");
        let (private_key, public_key) = WireGuardManager::generate_keys()
            .await
            .map_err(|e| OverlayError::WireGuardCommand(e.to_string()))?;

        // Initialize IP allocator and allocate first IP for leader
        let mut allocator = IpAllocator::new(cidr)?;
        let node_ip = allocator.allocate_first()?;

        info!(node_ip = %node_ip, cidr = cidr, "Allocated leader IP");

        // Create config
        let config = BootstrapConfig {
            cidr: cidr.to_string(),
            node_ip,
            interface: DEFAULT_INTERFACE_NAME.to_string(),
            port,
            private_key,
            public_key,
            is_leader: true,
            created_at: current_timestamp(),
        };

        let bootstrap = Self {
            config,
            peers: Vec::new(),
            data_dir: data_dir.to_path_buf(),
            allocator: Some(allocator),
        };

        // Persist state
        bootstrap.save().await?;

        Ok(bootstrap)
    }

    /// Join an existing overlay network
    ///
    /// Generates a new WireGuard keypair and configures this node
    /// to connect to an existing overlay network.
    ///
    /// # Arguments
    /// * `leader_cidr` - Leader's overlay network CIDR
    /// * `leader_endpoint` - Leader's public endpoint (host:port)
    /// * `leader_public_key` - Leader's WireGuard public key
    /// * `leader_overlay_ip` - Leader's overlay IP address
    /// * `allocated_ip` - IP address allocated for this node by the leader
    /// * `port` - WireGuard listen port for this node
    /// * `data_dir` - Directory for persistent state
    pub async fn join(
        leader_cidr: &str,
        leader_endpoint: &str,
        leader_public_key: &str,
        leader_overlay_ip: Ipv4Addr,
        allocated_ip: Ipv4Addr,
        port: u16,
        data_dir: &Path,
    ) -> Result<Self> {
        // Check if already initialized
        let config_path = data_dir.join("overlay_bootstrap.json");
        if config_path.exists() {
            return Err(OverlayError::AlreadyInitialized(
                config_path.display().to_string(),
            ));
        }

        // Ensure data directory exists
        tokio::fs::create_dir_all(data_dir).await?;

        // Generate WireGuard keypair for this node
        info!("Generating WireGuard keypair for joining node");
        let (private_key, public_key) = WireGuardManager::generate_keys()
            .await
            .map_err(|e| OverlayError::WireGuardCommand(e.to_string()))?;

        // Create config
        let config = BootstrapConfig {
            cidr: leader_cidr.to_string(),
            node_ip: allocated_ip,
            interface: DEFAULT_INTERFACE_NAME.to_string(),
            port,
            private_key,
            public_key,
            is_leader: false,
            created_at: current_timestamp(),
        };

        // Add leader as the first peer
        let leader_peer = PeerConfig {
            node_id: "leader".to_string(),
            public_key: leader_public_key.to_string(),
            endpoint: leader_endpoint.to_string(),
            overlay_ip: leader_overlay_ip,
            keepalive: Some(DEFAULT_KEEPALIVE_SECS),
        };

        info!(
            leader_endpoint = leader_endpoint,
            overlay_ip = %allocated_ip,
            "Configured leader as peer"
        );

        let bootstrap = Self {
            config,
            peers: vec![leader_peer],
            data_dir: data_dir.to_path_buf(),
            allocator: None, // Workers don't manage IP allocation
        };

        // Persist state
        bootstrap.save().await?;

        Ok(bootstrap)
    }

    /// Load existing bootstrap state from disk
    pub async fn load(data_dir: &Path) -> Result<Self> {
        let config_path = data_dir.join("overlay_bootstrap.json");

        if !config_path.exists() {
            return Err(OverlayError::NotInitialized);
        }

        let contents = tokio::fs::read_to_string(&config_path).await?;
        let state: BootstrapState = serde_json::from_str(&contents)?;

        let allocator = if let Some(alloc_state) = state.allocator_state {
            Some(IpAllocator::from_state(alloc_state)?)
        } else {
            None
        };

        Ok(Self {
            config: state.config,
            peers: state.peers,
            data_dir: data_dir.to_path_buf(),
            allocator,
        })
    }

    /// Save bootstrap state to disk
    pub async fn save(&self) -> Result<()> {
        let config_path = self.data_dir.join("overlay_bootstrap.json");

        let state = BootstrapState {
            config: self.config.clone(),
            peers: self.peers.clone(),
            allocator_state: self.allocator.as_ref().map(|a| a.to_state()),
        };

        let contents = serde_json::to_string_pretty(&state)?;
        tokio::fs::write(&config_path, contents).await?;

        debug!(path = %config_path.display(), "Saved bootstrap state");
        Ok(())
    }

    /// Start the overlay network (create and configure WireGuard interface)
    ///
    /// This creates the WireGuard interface, assigns the overlay IP,
    /// and configures all known peers.
    pub async fn start(&self) -> Result<()> {
        info!(
            interface = %self.config.interface,
            overlay_ip = %self.config.node_ip,
            port = self.config.port,
            "Starting overlay network"
        );

        // Convert our config to OverlayConfig
        let overlay_config = crate::config::OverlayConfig {
            local_endpoint: SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)),
                self.config.port,
            ),
            private_key: self.config.private_key.clone(),
            public_key: self.config.public_key.clone(),
            overlay_cidr: self.config.allowed_ip(),
            peer_discovery_interval: Duration::from_secs(30),
        };

        // Create WireGuard manager
        let wg_manager = WireGuardManager::new(overlay_config, self.config.interface.clone());

        // Create the interface
        wg_manager
            .create_interface()
            .await
            .map_err(|e| OverlayError::WireGuardCommand(e.to_string()))?;

        // Convert peers to PeerInfo
        let peer_infos: Vec<PeerInfo> = self
            .peers
            .iter()
            .filter_map(|p| match p.to_peer_info() {
                Ok(info) => Some(info),
                Err(e) => {
                    warn!(peer = %p.node_id, error = %e, "Failed to parse peer info");
                    None
                }
            })
            .collect();

        // Configure interface with peers
        wg_manager
            .configure_interface(&peer_infos)
            .await
            .map_err(|e| OverlayError::WireGuardCommand(e.to_string()))?;

        info!("Overlay network started successfully");
        Ok(())
    }

    /// Stop the overlay network (remove WireGuard interface)
    pub async fn stop(&self) -> Result<()> {
        info!(interface = %self.config.interface, "Stopping overlay network");

        let output = tokio::process::Command::new("ip")
            .args(["link", "delete", "dev", &self.config.interface])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "not found" errors
            if !stderr.contains("Cannot find device") {
                return Err(OverlayError::WireGuardCommand(stderr.to_string()));
            }
        }

        Ok(())
    }

    /// Add a new peer to the overlay network
    ///
    /// For leader nodes, this also allocates an IP address for the peer.
    pub async fn add_peer(&mut self, mut peer: PeerConfig) -> Result<Ipv4Addr> {
        // If we're the leader, allocate an IP for this peer
        let overlay_ip = if let Some(ref mut allocator) = self.allocator {
            let ip = allocator.allocate().ok_or(OverlayError::NoAvailableIps)?;
            peer.overlay_ip = ip;
            ip
        } else {
            peer.overlay_ip
        };

        // Add to WireGuard if interface is up
        let overlay_config = crate::config::OverlayConfig {
            local_endpoint: SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)),
                self.config.port,
            ),
            private_key: self.config.private_key.clone(),
            public_key: self.config.public_key.clone(),
            overlay_cidr: self.config.allowed_ip(),
            peer_discovery_interval: Duration::from_secs(30),
        };

        let wg_manager = WireGuardManager::new(overlay_config, self.config.interface.clone());

        if let Ok(peer_info) = peer.to_peer_info() {
            match wg_manager.add_peer(&peer_info).await {
                Ok(_) => debug!(peer = %peer.node_id, "Added peer to WireGuard"),
                Err(e) => {
                    warn!(peer = %peer.node_id, error = %e, "Failed to add peer to WireGuard (interface may not be up)")
                }
            }
        }

        // Add to peer list
        self.peers.push(peer);

        // Persist state
        self.save().await?;

        info!(peer_ip = %overlay_ip, "Added peer to overlay");
        Ok(overlay_ip)
    }

    /// Remove a peer from the overlay network
    pub async fn remove_peer(&mut self, public_key: &str) -> Result<()> {
        // Find the peer
        let peer_idx = self
            .peers
            .iter()
            .position(|p| p.public_key == public_key)
            .ok_or_else(|| OverlayError::PeerNotFound(public_key.to_string()))?;

        let peer = &self.peers[peer_idx];

        // Release IP if we're managing allocation
        if let Some(ref mut allocator) = self.allocator {
            allocator.release(peer.overlay_ip);
        }

        // Remove from WireGuard
        let overlay_config = crate::config::OverlayConfig {
            local_endpoint: SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)),
                self.config.port,
            ),
            private_key: self.config.private_key.clone(),
            public_key: self.config.public_key.clone(),
            overlay_cidr: self.config.allowed_ip(),
            peer_discovery_interval: Duration::from_secs(30),
        };

        let wg_manager = WireGuardManager::new(overlay_config, self.config.interface.clone());

        match wg_manager.remove_peer(public_key).await {
            Ok(_) => debug!(public_key = public_key, "Removed peer from WireGuard"),
            Err(e) => {
                warn!(public_key = public_key, error = %e, "Failed to remove peer from WireGuard")
            }
        }

        // Remove from peer list
        self.peers.remove(peer_idx);

        // Persist state
        self.save().await?;

        info!(public_key = public_key, "Removed peer from overlay");
        Ok(())
    }

    /// Get this node's public key
    pub fn public_key(&self) -> &str {
        &self.config.public_key
    }

    /// Get this node's overlay IP
    pub fn node_ip(&self) -> Ipv4Addr {
        self.config.node_ip
    }

    /// Get the overlay CIDR
    pub fn cidr(&self) -> &str {
        &self.config.cidr
    }

    /// Get the WireGuard interface name
    pub fn interface(&self) -> &str {
        &self.config.interface
    }

    /// Get the WireGuard listen port
    pub fn port(&self) -> u16 {
        self.config.port
    }

    /// Check if this node is the leader
    pub fn is_leader(&self) -> bool {
        self.config.is_leader
    }

    /// Get configured peers
    pub fn peers(&self) -> &[PeerConfig] {
        &self.peers
    }

    /// Get the bootstrap config
    pub fn config(&self) -> &BootstrapConfig {
        &self.config
    }

    /// Allocate an IP for a new peer (leader only)
    ///
    /// This is used by the control plane when processing join requests.
    pub fn allocate_peer_ip(&mut self) -> Result<Ipv4Addr> {
        let allocator = self
            .allocator
            .as_mut()
            .ok_or(OverlayError::Config("Not a leader node".to_string()))?;

        allocator.allocate().ok_or(OverlayError::NoAvailableIps)
    }

    /// Get IP allocation statistics (leader only)
    pub fn allocation_stats(&self) -> Option<(u32, u32)> {
        self.allocator
            .as_ref()
            .map(|a| (a.allocated_count() as u32, a.total_hosts()))
    }
}

/// Get current Unix timestamp
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bootstrap_config_allowed_ip() {
        let config = BootstrapConfig {
            cidr: "10.200.0.0/16".to_string(),
            node_ip: "10.200.0.1".parse().unwrap(),
            interface: DEFAULT_INTERFACE_NAME.to_string(),
            port: DEFAULT_WG_PORT,
            private_key: "test_private".to_string(),
            public_key: "test_public".to_string(),
            is_leader: true,
            created_at: 0,
        };

        assert_eq!(config.allowed_ip(), "10.200.0.1/32");
    }

    #[test]
    fn test_peer_config_new() {
        let peer = PeerConfig::new(
            "node-1".to_string(),
            "pubkey123".to_string(),
            "192.168.1.100:51820".to_string(),
            "10.200.0.5".parse().unwrap(),
        );

        assert_eq!(peer.node_id, "node-1");
        assert_eq!(peer.keepalive, Some(DEFAULT_KEEPALIVE_SECS));
    }

    #[test]
    fn test_peer_config_to_peer_info() {
        let peer = PeerConfig::new(
            "node-1".to_string(),
            "pubkey123".to_string(),
            "192.168.1.100:51820".to_string(),
            "10.200.0.5".parse().unwrap(),
        );

        let peer_info = peer.to_peer_info().unwrap();
        assert_eq!(peer_info.public_key, "pubkey123");
        assert_eq!(peer_info.allowed_ips, "10.200.0.5/32");
    }

    #[test]
    fn test_bootstrap_state_serialization() {
        let config = BootstrapConfig {
            cidr: "10.200.0.0/16".to_string(),
            node_ip: "10.200.0.1".parse().unwrap(),
            interface: DEFAULT_INTERFACE_NAME.to_string(),
            port: DEFAULT_WG_PORT,
            private_key: "private".to_string(),
            public_key: "public".to_string(),
            is_leader: true,
            created_at: 1234567890,
        };

        let state = BootstrapState {
            config,
            peers: vec![],
            allocator_state: None,
        };

        let json = serde_json::to_string_pretty(&state).unwrap();
        let deserialized: BootstrapState = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.config.cidr, "10.200.0.0/16");
        assert_eq!(deserialized.config.node_ip.to_string(), "10.200.0.1");
    }
}
