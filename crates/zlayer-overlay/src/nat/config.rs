//! NAT traversal configuration types

use serde::{Deserialize, Serialize};

/// NAT traversal configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NatConfig {
    /// Whether NAT traversal is enabled
    #[serde(default)]
    pub enabled: bool,
    /// STUN servers for reflexive address discovery
    #[serde(default = "default_stun_servers")]
    pub stun_servers: Vec<StunServerConfig>,
    /// TURN relay servers (fallback when direct + hole punch fails)
    #[serde(default)]
    pub turn_servers: Vec<TurnServerConfig>,
    /// Seconds to attempt hole punching before falling back to relay
    #[serde(default = "default_hole_punch_timeout")]
    pub hole_punch_timeout_secs: u64,
    /// Seconds between STUN reflexive address refreshes
    #[serde(default = "default_stun_refresh_interval")]
    pub stun_refresh_interval_secs: u64,
    /// Max candidate pairs to test per peer
    #[serde(default = "default_max_candidate_pairs")]
    pub max_candidate_pairs: usize,
    /// If this node should run a relay server
    #[serde(default)]
    pub relay_server: Option<RelayServerConfig>,
}

/// STUN server endpoint configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StunServerConfig {
    /// STUN server address (host:port)
    pub address: String,
    /// Optional label for logging
    #[serde(default)]
    pub label: Option<String>,
}

/// TURN relay server configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnServerConfig {
    /// TURN server address (host:port)
    pub address: String,
    /// Auth username
    pub username: String,
    /// Auth credential
    pub credential: String,
    /// Region label for proximity selection
    #[serde(default)]
    pub region: Option<String>,
}

/// Configuration for running a relay server on this node
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RelayServerConfig {
    /// Port for relay connections
    pub listen_port: u16,
    /// External address other nodes use to reach this relay
    pub external_addr: String,
    /// Max concurrent relay sessions
    #[serde(default = "default_max_relay_sessions")]
    pub max_sessions: usize,
}

fn default_stun_servers() -> Vec<StunServerConfig> {
    vec![
        StunServerConfig {
            address: "stun.l.google.com:19302".to_string(),
            label: Some("Google STUN 1".to_string()),
        },
        StunServerConfig {
            address: "stun1.l.google.com:19302".to_string(),
            label: Some("Google STUN 2".to_string()),
        },
    ]
}

fn default_hole_punch_timeout() -> u64 {
    15
}

fn default_stun_refresh_interval() -> u64 {
    60
}

fn default_max_candidate_pairs() -> usize {
    10
}

fn default_max_relay_sessions() -> usize {
    100
}

impl Default for NatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            stun_servers: default_stun_servers(),
            turn_servers: Vec::new(),
            hole_punch_timeout_secs: default_hole_punch_timeout(),
            stun_refresh_interval_secs: default_stun_refresh_interval(),
            max_candidate_pairs: default_max_candidate_pairs(),
            relay_server: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nat_config_default() {
        let config = NatConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.stun_servers.len(), 2);
        assert!(config.turn_servers.is_empty());
        assert_eq!(config.hole_punch_timeout_secs, 15);
        assert_eq!(config.stun_refresh_interval_secs, 60);
        assert_eq!(config.max_candidate_pairs, 10);
        assert!(config.relay_server.is_none());
    }

    #[test]
    fn test_nat_config_serialization_roundtrip() {
        let config = NatConfig::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        let deserialized: NatConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.stun_servers.len(), 2);
        assert_eq!(deserialized.hole_punch_timeout_secs, 15);
    }

    #[test]
    fn test_nat_config_deserialize_empty_uses_defaults() {
        let json = r#"{"enabled": true}"#;
        let config: NatConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.stun_servers.len(), 2);
        assert_eq!(config.hole_punch_timeout_secs, 15);
    }

    #[test]
    fn test_turn_server_config() {
        let json = r#"{
            "address": "turn.example.com:3478",
            "username": "user",
            "credential": "pass",
            "region": "us-east"
        }"#;
        let config: TurnServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.address, "turn.example.com:3478");
        assert_eq!(config.region, Some("us-east".to_string()));
    }

    #[test]
    fn test_relay_server_config() {
        let json = r#"{
            "listen_port": 3478,
            "external_addr": "1.2.3.4:3478"
        }"#;
        let config: RelayServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.listen_port, 3478);
        assert_eq!(config.max_sessions, 100);
    }
}
