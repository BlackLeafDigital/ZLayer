//! Node tunnel manager
//!
//! Manages node-to-node tunnels for a single node.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use uuid::Uuid;

use super::NodeTunnel;
use crate::{
    Result, ServiceConfig, TunnelAgent, TunnelClientConfig, TunnelError, TunnelRegistry,
    TunnelServerConfig,
};

/// Status of a node tunnel
#[derive(Debug, Clone)]
pub struct TunnelStatus {
    /// Tunnel name
    pub name: String,
    /// Source node name
    pub from: String,
    /// Destination node name
    pub to: String,
    /// Local port on source node
    pub local_port: u16,
    /// Remote port on destination node
    pub remote_port: u16,
    /// Current tunnel state
    pub state: TunnelState,
    /// When the tunnel was connected
    pub connected_at: Option<Instant>,
    /// Last activity timestamp
    pub last_activity: Option<Instant>,
    /// Bytes received through tunnel
    pub bytes_in: u64,
    /// Bytes sent through tunnel
    pub bytes_out: u64,
    /// Latency in milliseconds
    pub latency_ms: Option<u64>,
}

/// State of a tunnel connection
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum TunnelState {
    /// Tunnel is configured but not started
    #[default]
    Pending,
    /// Tunnel is attempting to connect
    Connecting,
    /// Tunnel is connected and active
    Connected,
    /// Tunnel is disconnected
    Disconnected,
    /// Tunnel connection failed
    Failed(String),
}

/// Outbound tunnel tracking
struct OutboundTunnel {
    /// Handle to the agent task (kept for potential future use like awaiting completion)
    #[allow(dead_code)]
    agent_handle: tokio::task::JoinHandle<()>,
    /// Abort handle for cancellation
    abort_handle: tokio::task::AbortHandle,
}

/// Manages tunnels for a node
///
/// The `NodeTunnelManager` handles both incoming and outgoing tunnel connections
/// for a single node in the `ZLayer` mesh network.
pub struct NodeTunnelManager {
    /// This node's name
    node_name: String,

    /// Server config for incoming tunnels
    server_config: TunnelServerConfig,

    /// Registry for server-side tunnels
    registry: Arc<TunnelRegistry>,

    /// Configured tunnels (both as source and destination)
    tunnels: DashMap<String, NodeTunnel>,

    /// Active outbound tunnel agents (when this node is source)
    outbound_agents: DashMap<String, OutboundTunnel>,

    /// Tunnel status tracking
    status: DashMap<String, TunnelStatus>,
}

impl NodeTunnelManager {
    /// Create a new node tunnel manager
    #[must_use]
    pub fn new(node_name: impl Into<String>, server_config: TunnelServerConfig) -> Self {
        let port_range = server_config.data_port_range;
        Self {
            node_name: node_name.into(),
            server_config,
            registry: Arc::new(TunnelRegistry::new(port_range)),
            tunnels: DashMap::new(),
            outbound_agents: DashMap::new(),
            status: DashMap::new(),
        }
    }

    /// Get this node's name
    #[must_use]
    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    /// Get the server configuration
    #[must_use]
    pub fn server_config(&self) -> &TunnelServerConfig {
        &self.server_config
    }

    /// Get the tunnel registry (for server integration)
    #[must_use]
    pub fn registry(&self) -> Arc<TunnelRegistry> {
        Arc::clone(&self.registry)
    }

    /// Add a tunnel configuration
    ///
    /// # Errors
    ///
    /// Returns an error if a tunnel with the same name already exists.
    pub fn add_tunnel(&self, mut tunnel: NodeTunnel) -> Result<()> {
        if self.tunnels.contains_key(&tunnel.name) {
            return Err(TunnelError::registry(format!(
                "Tunnel '{}' already exists",
                tunnel.name
            )));
        }

        // Generate token if not provided
        if tunnel.token.is_none() {
            tunnel.token = Some(format!("tun_{}", Uuid::new_v4()));
        }

        // Initialize status
        self.status.insert(
            tunnel.name.clone(),
            TunnelStatus {
                name: tunnel.name.clone(),
                from: tunnel.from.clone(),
                to: tunnel.to.clone(),
                local_port: tunnel.local_port,
                remote_port: tunnel.remote_port,
                state: TunnelState::Pending,
                connected_at: None,
                last_activity: None,
                bytes_in: 0,
                bytes_out: 0,
                latency_ms: None,
            },
        );

        self.tunnels.insert(tunnel.name.clone(), tunnel);
        Ok(())
    }

    /// Remove a tunnel
    ///
    /// # Errors
    ///
    /// Returns an error if the tunnel does not exist.
    pub fn remove_tunnel(&self, name: &str) -> Result<NodeTunnel> {
        // Stop outbound agent if running
        if let Some((_, outbound)) = self.outbound_agents.remove(name) {
            outbound.abort_handle.abort();
        }

        // Remove status
        self.status.remove(name);

        // Remove config
        self.tunnels
            .remove(name)
            .map(|(_, t)| t)
            .ok_or_else(|| TunnelError::registry(format!("Tunnel '{name}' not found")))
    }

    /// Get tunnel configuration by name
    #[must_use]
    pub fn get_tunnel(&self, name: &str) -> Option<NodeTunnel> {
        self.tunnels.get(name).map(|t| t.clone())
    }

    /// List all configured tunnels
    #[must_use]
    pub fn list_tunnels(&self) -> Vec<NodeTunnel> {
        self.tunnels.iter().map(|t| t.clone()).collect()
    }

    /// Get tunnel status by name
    #[must_use]
    pub fn get_status(&self, name: &str) -> Option<TunnelStatus> {
        self.status.get(name).map(|s| s.clone())
    }

    /// List all tunnel statuses
    #[must_use]
    pub fn list_status(&self) -> Vec<TunnelStatus> {
        self.status.iter().map(|s| s.clone()).collect()
    }

    /// Get tunnels where this node is the source
    #[must_use]
    pub fn outbound_tunnels(&self) -> Vec<NodeTunnel> {
        self.tunnels
            .iter()
            .filter(|t| t.from == self.node_name)
            .map(|t| t.clone())
            .collect()
    }

    /// Get tunnels where this node is the destination
    #[must_use]
    pub fn inbound_tunnels(&self) -> Vec<NodeTunnel> {
        self.tunnels
            .iter()
            .filter(|t| t.to == self.node_name)
            .map(|t| t.clone())
            .collect()
    }

    /// Start a tunnel where this node is the source (outbound)
    ///
    /// # Arguments
    ///
    /// * `name` - Name of the tunnel to start
    /// * `server_url` - WebSocket URL of the destination node's tunnel server
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The tunnel does not exist
    /// - This node is not the source for the tunnel
    /// - The tunnel has no authentication token
    pub fn start_outbound(&self, name: &str, server_url: String) -> Result<()> {
        let tunnel = self
            .tunnels
            .get(name)
            .ok_or_else(|| TunnelError::registry(format!("Tunnel '{name}' not found")))?
            .clone();

        if tunnel.from != self.node_name {
            return Err(TunnelError::config(format!(
                "Tunnel '{}' source is '{}', not this node '{}'",
                name, tunnel.from, self.node_name
            )));
        }

        let token = tunnel
            .token
            .clone()
            .ok_or_else(|| TunnelError::config("Tunnel has no token"))?;

        // Update status to connecting
        if let Some(mut status) = self.status.get_mut(name) {
            status.state = TunnelState::Connecting;
        }

        let config = TunnelClientConfig {
            server_url,
            token,
            reconnect_interval: Duration::from_secs(5),
            max_reconnect_interval: Duration::from_secs(60),
            services: vec![ServiceConfig {
                name: tunnel.name.clone(),
                protocol: tunnel.protocol,
                local_port: tunnel.local_port,
                remote_port: tunnel.remote_port,
            }],
        };

        let agent = TunnelAgent::new(config);
        let tunnel_name = name.to_string();
        let status_map = self.status.clone();

        let handle = tokio::spawn(async move {
            // Update status on connect
            if let Some(mut status) = status_map.get_mut(&tunnel_name) {
                status.state = TunnelState::Connected;
                status.connected_at = Some(Instant::now());
            }

            if let Err(e) = agent.run().await {
                tracing::error!(tunnel = %tunnel_name, error = %e, "Tunnel agent failed");
                if let Some(mut status) = status_map.get_mut(&tunnel_name) {
                    status.state = TunnelState::Failed(e.to_string());
                }
            } else if let Some(mut status) = status_map.get_mut(&tunnel_name) {
                status.state = TunnelState::Disconnected;
            }
        });

        self.outbound_agents.insert(
            name.to_string(),
            OutboundTunnel {
                abort_handle: handle.abort_handle(),
                agent_handle: handle,
            },
        );

        Ok(())
    }

    /// Stop an outbound tunnel
    ///
    /// # Errors
    ///
    /// Returns an error if no active outbound tunnel with the given name exists.
    pub fn stop_outbound(&self, name: &str) -> Result<()> {
        if let Some((_, outbound)) = self.outbound_agents.remove(name) {
            outbound.abort_handle.abort();

            if let Some(mut status) = self.status.get_mut(name) {
                status.state = TunnelState::Disconnected;
            }

            Ok(())
        } else {
            Err(TunnelError::registry(format!(
                "No active outbound tunnel '{name}'"
            )))
        }
    }

    /// Get count of active outbound tunnels
    #[must_use]
    pub fn outbound_count(&self) -> usize {
        self.outbound_agents.len()
    }

    /// Get count of configured tunnels
    #[must_use]
    pub fn tunnel_count(&self) -> usize {
        self.tunnels.len()
    }

    /// Check if a tunnel is active
    #[must_use]
    pub fn is_tunnel_active(&self, name: &str) -> bool {
        self.status
            .get(name)
            .is_some_and(|s| s.state == TunnelState::Connected)
    }

    /// Shutdown all tunnels
    pub fn shutdown(&self) {
        for item in &self.outbound_agents {
            item.abort_handle.abort();
        }
        self.outbound_agents.clear();

        for mut status in self.status.iter_mut() {
            status.state = TunnelState::Disconnected;
        }
    }
}

impl Drop for NodeTunnelManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_manager() -> NodeTunnelManager {
        let config = TunnelServerConfig::default();
        NodeTunnelManager::new("test-node", config)
    }

    fn create_test_tunnel(name: &str) -> NodeTunnel {
        NodeTunnel::new(name, "test-node", "remote-node").with_ports(22, 2222)
    }

    #[test]
    fn test_node_tunnel_manager_new() {
        let manager = create_test_manager();

        assert_eq!(manager.node_name(), "test-node");
        assert_eq!(manager.tunnel_count(), 0);
        assert_eq!(manager.outbound_count(), 0);
    }

    #[test]
    fn test_add_tunnel() {
        let manager = create_test_manager();
        let tunnel = create_test_tunnel("ssh-tunnel");

        manager.add_tunnel(tunnel).unwrap();

        assert_eq!(manager.tunnel_count(), 1);
        assert!(manager.get_tunnel("ssh-tunnel").is_some());
    }

    #[test]
    fn test_add_tunnel_generates_token() {
        let manager = create_test_manager();
        let tunnel = NodeTunnel::new("test-tunnel", "test-node", "remote").with_ports(22, 2222);

        assert!(tunnel.token.is_none());

        manager.add_tunnel(tunnel).unwrap();

        let stored = manager.get_tunnel("test-tunnel").unwrap();
        assert!(stored.token.is_some());
        assert!(stored.token.unwrap().starts_with("tun_"));
    }

    #[test]
    fn test_add_duplicate_tunnel() {
        let manager = create_test_manager();
        let tunnel1 = create_test_tunnel("ssh-tunnel");
        let tunnel2 = create_test_tunnel("ssh-tunnel");

        manager.add_tunnel(tunnel1).unwrap();
        let result = manager.add_tunnel(tunnel2);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn test_remove_tunnel() {
        let manager = create_test_manager();
        let tunnel = create_test_tunnel("ssh-tunnel");

        manager.add_tunnel(tunnel).unwrap();
        assert_eq!(manager.tunnel_count(), 1);

        let removed = manager.remove_tunnel("ssh-tunnel").unwrap();
        assert_eq!(removed.name, "ssh-tunnel");
        assert_eq!(manager.tunnel_count(), 0);
        assert!(manager.get_tunnel("ssh-tunnel").is_none());
    }

    #[test]
    fn test_remove_nonexistent_tunnel() {
        let manager = create_test_manager();

        let result = manager.remove_tunnel("nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_get_tunnel() {
        let manager = create_test_manager();
        let tunnel = create_test_tunnel("ssh-tunnel");

        manager.add_tunnel(tunnel).unwrap();

        let retrieved = manager.get_tunnel("ssh-tunnel");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "ssh-tunnel");

        assert!(manager.get_tunnel("nonexistent").is_none());
    }

    #[test]
    fn test_list_tunnels() {
        let manager = create_test_manager();

        manager.add_tunnel(create_test_tunnel("tunnel-1")).unwrap();
        manager.add_tunnel(create_test_tunnel("tunnel-2")).unwrap();
        manager.add_tunnel(create_test_tunnel("tunnel-3")).unwrap();

        let tunnels = manager.list_tunnels();
        assert_eq!(tunnels.len(), 3);
    }

    #[test]
    fn test_get_status() {
        let manager = create_test_manager();
        let tunnel = create_test_tunnel("ssh-tunnel");

        manager.add_tunnel(tunnel).unwrap();

        let status = manager.get_status("ssh-tunnel");
        assert!(status.is_some());

        let status = status.unwrap();
        assert_eq!(status.name, "ssh-tunnel");
        assert_eq!(status.state, TunnelState::Pending);
        assert!(status.connected_at.is_none());
    }

    #[test]
    fn test_list_status() {
        let manager = create_test_manager();

        manager.add_tunnel(create_test_tunnel("tunnel-1")).unwrap();
        manager.add_tunnel(create_test_tunnel("tunnel-2")).unwrap();

        let statuses = manager.list_status();
        assert_eq!(statuses.len(), 2);
    }

    #[test]
    fn test_outbound_tunnels() {
        let manager = create_test_manager();

        // Tunnel where this node is the source
        let outbound = NodeTunnel::new("outbound", "test-node", "remote").with_ports(22, 2222);
        manager.add_tunnel(outbound).unwrap();

        // Tunnel where this node is the destination
        let inbound = NodeTunnel::new("inbound", "remote", "test-node").with_ports(22, 2222);
        manager.add_tunnel(inbound).unwrap();

        let outbound_tunnels = manager.outbound_tunnels();
        assert_eq!(outbound_tunnels.len(), 1);
        assert_eq!(outbound_tunnels[0].name, "outbound");
    }

    #[test]
    fn test_inbound_tunnels() {
        let manager = create_test_manager();

        // Tunnel where this node is the source
        let outbound = NodeTunnel::new("outbound", "test-node", "remote").with_ports(22, 2222);
        manager.add_tunnel(outbound).unwrap();

        // Tunnel where this node is the destination
        let inbound = NodeTunnel::new("inbound", "remote", "test-node").with_ports(22, 2222);
        manager.add_tunnel(inbound).unwrap();

        let inbound_tunnels = manager.inbound_tunnels();
        assert_eq!(inbound_tunnels.len(), 1);
        assert_eq!(inbound_tunnels[0].name, "inbound");
    }

    #[test]
    fn test_tunnel_count() {
        let manager = create_test_manager();

        assert_eq!(manager.tunnel_count(), 0);

        manager.add_tunnel(create_test_tunnel("tunnel-1")).unwrap();
        assert_eq!(manager.tunnel_count(), 1);

        manager.add_tunnel(create_test_tunnel("tunnel-2")).unwrap();
        assert_eq!(manager.tunnel_count(), 2);

        manager.remove_tunnel("tunnel-1").unwrap();
        assert_eq!(manager.tunnel_count(), 1);
    }

    #[test]
    fn test_outbound_count() {
        let manager = create_test_manager();

        // Initially no outbound connections
        assert_eq!(manager.outbound_count(), 0);
    }

    #[test]
    fn test_is_tunnel_active() {
        let manager = create_test_manager();
        let tunnel = create_test_tunnel("ssh-tunnel");

        manager.add_tunnel(tunnel).unwrap();

        // Initially not active (Pending state)
        assert!(!manager.is_tunnel_active("ssh-tunnel"));

        // Nonexistent tunnel is not active
        assert!(!manager.is_tunnel_active("nonexistent"));
    }

    #[test]
    fn test_tunnel_state_default() {
        let state = TunnelState::default();
        assert_eq!(state, TunnelState::Pending);
    }

    #[test]
    fn test_tunnel_state_equality() {
        assert_eq!(TunnelState::Pending, TunnelState::Pending);
        assert_eq!(TunnelState::Connecting, TunnelState::Connecting);
        assert_eq!(TunnelState::Connected, TunnelState::Connected);
        assert_eq!(TunnelState::Disconnected, TunnelState::Disconnected);
        assert_eq!(
            TunnelState::Failed("error".to_string()),
            TunnelState::Failed("error".to_string())
        );

        assert_ne!(TunnelState::Pending, TunnelState::Connected);
        assert_ne!(
            TunnelState::Failed("error1".to_string()),
            TunnelState::Failed("error2".to_string())
        );
    }

    #[test]
    fn test_registry_access() {
        let manager = create_test_manager();
        let registry = manager.registry();

        // Verify we get a valid registry
        assert_eq!(registry.tunnel_count(), 0);
    }

    #[test]
    fn test_server_config_access() {
        let manager = create_test_manager();
        let config = manager.server_config();

        // Verify default config values
        assert!(config.enabled);
        assert_eq!(config.control_path, "/tunnel/v1");
    }

    #[test]
    fn test_shutdown() {
        let manager = create_test_manager();

        manager.add_tunnel(create_test_tunnel("tunnel-1")).unwrap();
        manager.add_tunnel(create_test_tunnel("tunnel-2")).unwrap();

        manager.shutdown();

        // All statuses should be Disconnected
        for status in manager.list_status() {
            assert_eq!(status.state, TunnelState::Disconnected);
        }
    }

    #[test]
    fn test_tunnel_status_fields() {
        let manager = create_test_manager();
        let tunnel = NodeTunnel::new("test", "test-node", "remote").with_ports(22, 2222);

        manager.add_tunnel(tunnel).unwrap();

        let status = manager.get_status("test").unwrap();

        assert_eq!(status.name, "test");
        assert_eq!(status.from, "test-node");
        assert_eq!(status.to, "remote");
        assert_eq!(status.local_port, 22);
        assert_eq!(status.remote_port, 2222);
        assert_eq!(status.state, TunnelState::Pending);
        assert!(status.connected_at.is_none());
        assert!(status.last_activity.is_none());
        assert_eq!(status.bytes_in, 0);
        assert_eq!(status.bytes_out, 0);
        assert!(status.latency_ms.is_none());
    }

    #[test]
    fn test_stop_outbound_not_running() {
        let manager = create_test_manager();
        let tunnel = create_test_tunnel("ssh-tunnel");

        manager.add_tunnel(tunnel).unwrap();

        // Try to stop a tunnel that isn't running
        let result = manager.stop_outbound("ssh-tunnel");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No active outbound"));
    }

    #[test]
    fn test_start_outbound_wrong_source() {
        let manager = create_test_manager();

        // Create tunnel where this node is NOT the source
        let tunnel = NodeTunnel::new("test", "other-node", "remote").with_ports(22, 2222);
        manager.add_tunnel(tunnel).unwrap();

        let result = manager.start_outbound("test", "ws://localhost:8080".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not this node"));
    }

    #[test]
    fn test_start_outbound_not_found() {
        let manager = create_test_manager();

        let result = manager.start_outbound("nonexistent", "ws://localhost:8080".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}
