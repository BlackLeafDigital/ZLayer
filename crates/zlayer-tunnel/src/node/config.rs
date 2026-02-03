//! Node tunnel configuration

use serde::{Deserialize, Serialize};

use crate::ServiceProtocol;

/// Definition of a node-to-node tunnel
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeTunnel {
    /// Tunnel name (unique identifier)
    pub name: String,

    /// Source node name (runs tunnel client)
    pub from: String,

    /// Destination node name (runs tunnel server, exposes port)
    pub to: String,

    /// Local port on source node
    pub local_port: u16,

    /// Remote port exposed on destination node
    pub remote_port: u16,

    /// Protocol (defaults to TCP)
    #[serde(default)]
    pub protocol: ServiceProtocol,

    /// Exposure type
    #[serde(default)]
    pub expose: ExposeType,

    /// Auto-generated token for this tunnel
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Exposure type for tunneled ports
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ExposeType {
    /// Only accessible within the mesh network
    #[default]
    Internal,
    /// Publicly accessible from internet
    Public,
}

impl NodeTunnel {
    /// Create a new node tunnel definition
    #[must_use]
    pub fn new(name: impl Into<String>, from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            from: from.into(),
            to: to.into(),
            local_port: 0,
            remote_port: 0,
            protocol: ServiceProtocol::Tcp,
            expose: ExposeType::Internal,
            token: None,
        }
    }

    /// Set the local and remote ports
    #[must_use]
    pub const fn with_ports(mut self, local: u16, remote: u16) -> Self {
        self.local_port = local;
        self.remote_port = remote;
        self
    }

    /// Set the protocol
    #[must_use]
    pub const fn with_protocol(mut self, protocol: ServiceProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Set the exposure type
    #[must_use]
    pub const fn with_expose(mut self, expose: ExposeType) -> Self {
        self.expose = expose;
        self
    }

    /// Set the authentication token
    #[must_use]
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_tunnel_new() {
        let tunnel = NodeTunnel::new("test-tunnel", "node-a", "node-b");

        assert_eq!(tunnel.name, "test-tunnel");
        assert_eq!(tunnel.from, "node-a");
        assert_eq!(tunnel.to, "node-b");
        assert_eq!(tunnel.local_port, 0);
        assert_eq!(tunnel.remote_port, 0);
        assert_eq!(tunnel.protocol, ServiceProtocol::Tcp);
        assert_eq!(tunnel.expose, ExposeType::Internal);
        assert!(tunnel.token.is_none());
    }

    #[test]
    fn test_node_tunnel_builder() {
        let tunnel = NodeTunnel::new("ssh-tunnel", "client-node", "server-node")
            .with_ports(22, 2222)
            .with_protocol(ServiceProtocol::Tcp)
            .with_expose(ExposeType::Public)
            .with_token("secret-token-123");

        assert_eq!(tunnel.name, "ssh-tunnel");
        assert_eq!(tunnel.from, "client-node");
        assert_eq!(tunnel.to, "server-node");
        assert_eq!(tunnel.local_port, 22);
        assert_eq!(tunnel.remote_port, 2222);
        assert_eq!(tunnel.protocol, ServiceProtocol::Tcp);
        assert_eq!(tunnel.expose, ExposeType::Public);
        assert_eq!(tunnel.token, Some("secret-token-123".to_string()));
    }

    #[test]
    fn test_node_tunnel_with_udp() {
        let tunnel = NodeTunnel::new("game-tunnel", "game-client", "game-server")
            .with_ports(27015, 27015)
            .with_protocol(ServiceProtocol::Udp);

        assert_eq!(tunnel.protocol, ServiceProtocol::Udp);
    }

    #[test]
    fn test_expose_type_default() {
        let expose = ExposeType::default();
        assert_eq!(expose, ExposeType::Internal);
    }

    #[test]
    fn test_node_tunnel_equality() {
        let tunnel1 = NodeTunnel::new("test", "a", "b").with_ports(22, 2222);
        let tunnel2 = NodeTunnel::new("test", "a", "b").with_ports(22, 2222);
        let tunnel3 = NodeTunnel::new("test", "a", "b").with_ports(22, 3333);

        assert_eq!(tunnel1, tunnel2);
        assert_ne!(tunnel1, tunnel3);
    }

    #[test]
    fn test_node_tunnel_clone() {
        let tunnel = NodeTunnel::new("test", "a", "b")
            .with_ports(22, 2222)
            .with_token("token");

        let cloned = tunnel.clone();
        assert_eq!(tunnel, cloned);
    }

    #[test]
    fn test_node_tunnel_serialization() {
        let tunnel = NodeTunnel::new("test-tunnel", "node-a", "node-b")
            .with_ports(22, 2222)
            .with_protocol(ServiceProtocol::Tcp)
            .with_expose(ExposeType::Public)
            .with_token("token123");

        let toml_str = toml::to_string(&tunnel).expect("serialize");
        let parsed: NodeTunnel = toml::from_str(&toml_str).expect("deserialize");

        assert_eq!(tunnel, parsed);
    }

    #[test]
    fn test_node_tunnel_serialization_without_token() {
        let tunnel = NodeTunnel::new("test-tunnel", "node-a", "node-b").with_ports(22, 2222);

        let toml_str = toml::to_string(&tunnel).expect("serialize");

        // Token should not appear in serialized output when None
        assert!(!toml_str.contains("token"));

        let parsed: NodeTunnel = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(tunnel, parsed);
    }

    #[test]
    fn test_expose_type_serialization() {
        // Test Internal
        let tunnel_internal = NodeTunnel::new("test", "a", "b").with_expose(ExposeType::Internal);
        let toml_str = toml::to_string(&tunnel_internal).expect("serialize");
        assert!(toml_str.contains("internal"));

        // Test Public
        let tunnel_public = NodeTunnel::new("test", "a", "b").with_expose(ExposeType::Public);
        let toml_str = toml::to_string(&tunnel_public).expect("serialize");
        assert!(toml_str.contains("public"));
    }
}
