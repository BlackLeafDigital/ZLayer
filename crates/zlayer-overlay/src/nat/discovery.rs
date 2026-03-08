//! Static relay server discovery.
//!
//! Phase 4 implementation: static configuration-based discovery only.
//! Future phases may add dynamic discovery (DNS-SD, gossip, etc.).

use crate::nat::config::{NatConfig, TurnServerConfig};

/// Discovers and manages available relay servers.
///
/// Currently supports only static configuration. The server list
/// is populated from [`NatConfig::turn_servers`].
pub struct RelayDiscovery {
    static_servers: Vec<TurnServerConfig>,
}

impl RelayDiscovery {
    /// Create a new discovery instance from NAT configuration.
    #[must_use]
    pub fn new(config: &NatConfig) -> Self {
        Self {
            static_servers: config.turn_servers.clone(),
        }
    }

    /// Get the list of known relay servers.
    #[must_use]
    pub fn servers(&self) -> &[TurnServerConfig] {
        &self.static_servers
    }

    /// Add a relay server to the static list.
    pub fn add_server(&mut self, server: TurnServerConfig) {
        // Deduplicate by address
        if !self
            .static_servers
            .iter()
            .any(|s| s.address == server.address)
        {
            self.static_servers.push(server);
        }
    }

    /// Remove a relay server by address.
    pub fn remove_server(&mut self, address: &str) {
        self.static_servers.retain(|s| s.address != address);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config_with_servers(n: usize) -> NatConfig {
        let mut config = NatConfig::default();
        for i in 0..n {
            config.turn_servers.push(TurnServerConfig {
                address: format!("relay{i}.example.com:3478"),
                username: format!("user{i}"),
                credential: format!("cred{i}"),
                region: Some(format!("region-{i}")),
            });
        }
        config
    }

    #[test]
    fn test_discovery_new_empty() {
        let config = NatConfig::default();
        let discovery = RelayDiscovery::new(&config);
        assert!(discovery.servers().is_empty());
    }

    #[test]
    fn test_discovery_new_with_servers() {
        let config = make_config_with_servers(3);
        let discovery = RelayDiscovery::new(&config);
        assert_eq!(discovery.servers().len(), 3);
        assert_eq!(discovery.servers()[0].address, "relay0.example.com:3478");
        assert_eq!(discovery.servers()[2].address, "relay2.example.com:3478");
    }

    #[test]
    fn test_discovery_add_server() {
        let config = NatConfig::default();
        let mut discovery = RelayDiscovery::new(&config);

        discovery.add_server(TurnServerConfig {
            address: "new.example.com:3478".to_string(),
            username: "user".to_string(),
            credential: "cred".to_string(),
            region: None,
        });

        assert_eq!(discovery.servers().len(), 1);
        assert_eq!(discovery.servers()[0].address, "new.example.com:3478");
    }

    #[test]
    fn test_discovery_add_server_dedup() {
        let config = NatConfig::default();
        let mut discovery = RelayDiscovery::new(&config);

        let server = TurnServerConfig {
            address: "relay.example.com:3478".to_string(),
            username: "user".to_string(),
            credential: "cred".to_string(),
            region: None,
        };

        discovery.add_server(server.clone());
        discovery.add_server(server);

        assert_eq!(
            discovery.servers().len(),
            1,
            "Duplicate server should not be added"
        );
    }

    #[test]
    fn test_discovery_remove_server() {
        let config = make_config_with_servers(3);
        let mut discovery = RelayDiscovery::new(&config);

        discovery.remove_server("relay1.example.com:3478");

        assert_eq!(discovery.servers().len(), 2);
        assert_eq!(discovery.servers()[0].address, "relay0.example.com:3478");
        assert_eq!(discovery.servers()[1].address, "relay2.example.com:3478");
    }

    #[test]
    fn test_discovery_remove_nonexistent() {
        let config = make_config_with_servers(2);
        let mut discovery = RelayDiscovery::new(&config);

        discovery.remove_server("nonexistent:3478");

        assert_eq!(
            discovery.servers().len(),
            2,
            "Removing nonexistent server should be a no-op"
        );
    }
}
