//! Configuration types for tunnel server and client

use crate::protocol::ServiceProtocol;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// =============================================================================
// Default value functions for serde
// =============================================================================

const fn default_enabled() -> bool {
    true
}

fn default_control_path() -> String {
    "/tunnel/v1".to_string()
}

const fn default_port_range() -> (u16, u16) {
    (30000, 31000)
}

const fn default_heartbeat_interval() -> Duration {
    Duration::from_secs(30)
}

const fn default_heartbeat_timeout() -> Duration {
    Duration::from_secs(40)
}

const fn default_max_tunnels() -> usize {
    1000
}

const fn default_max_services() -> usize {
    50
}

const fn default_reconnect_interval() -> Duration {
    Duration::from_secs(5)
}

const fn default_max_reconnect_interval() -> Duration {
    Duration::from_secs(60)
}

// =============================================================================
// Server Configuration
// =============================================================================

/// Server-side tunnel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelServerConfig {
    /// Enable tunnel server
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// WebSocket path for control channel
    #[serde(default = "default_control_path")]
    pub control_path: String,

    /// Port range for dynamic data channels (inclusive)
    #[serde(default = "default_port_range")]
    pub data_port_range: (u16, u16),

    /// Heartbeat send interval
    #[serde(default = "default_heartbeat_interval", with = "humantime_serde")]
    pub heartbeat_interval: Duration,

    /// Heartbeat timeout (no response = dead)
    #[serde(default = "default_heartbeat_timeout", with = "humantime_serde")]
    pub heartbeat_timeout: Duration,

    /// Maximum concurrent tunnels
    #[serde(default = "default_max_tunnels")]
    pub max_tunnels: usize,

    /// Maximum services per tunnel
    #[serde(default = "default_max_services")]
    pub max_services_per_tunnel: usize,
}

impl Default for TunnelServerConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            control_path: default_control_path(),
            data_port_range: default_port_range(),
            heartbeat_interval: default_heartbeat_interval(),
            heartbeat_timeout: default_heartbeat_timeout(),
            max_tunnels: default_max_tunnels(),
            max_services_per_tunnel: default_max_services(),
        }
    }
}

impl TunnelServerConfig {
    /// Create a new server config with default values
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate the configuration
    ///
    /// # Errors
    ///
    /// Returns an error string if:
    /// - `control_path` is empty or doesn't start with '/'
    /// - `data_port_range` start > end or start < 1024
    /// - `heartbeat_timeout` <= `heartbeat_interval`
    /// - `max_tunnels` or `max_services_per_tunnel` is 0
    pub fn validate(&self) -> Result<(), String> {
        if self.control_path.is_empty() {
            return Err("control_path cannot be empty".to_string());
        }

        if !self.control_path.starts_with('/') {
            return Err("control_path must start with '/'".to_string());
        }

        let (start, end) = self.data_port_range;
        if start > end {
            return Err(format!(
                "data_port_range start ({start}) must be <= end ({end})"
            ));
        }

        if start < 1024 {
            return Err(format!(
                "data_port_range start ({start}) should be >= 1024 (privileged ports)"
            ));
        }

        if self.heartbeat_timeout <= self.heartbeat_interval {
            return Err(format!(
                "heartbeat_timeout ({:?}) must be > heartbeat_interval ({:?})",
                self.heartbeat_timeout, self.heartbeat_interval
            ));
        }

        if self.max_tunnels == 0 {
            return Err("max_tunnels must be > 0".to_string());
        }

        if self.max_services_per_tunnel == 0 {
            return Err("max_services_per_tunnel must be > 0".to_string());
        }

        Ok(())
    }
}

// =============================================================================
// Client Configuration
// =============================================================================

/// Client-side tunnel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelClientConfig {
    /// Server WebSocket URL (e.g., `wss://tunnel.example.com/tunnel/v1`)
    pub server_url: String,

    /// Authentication token
    pub token: String,

    /// Initial reconnect interval
    #[serde(default = "default_reconnect_interval", with = "humantime_serde")]
    pub reconnect_interval: Duration,

    /// Maximum reconnect interval (exponential backoff cap)
    #[serde(default = "default_max_reconnect_interval", with = "humantime_serde")]
    pub max_reconnect_interval: Duration,

    /// Services to expose through the tunnel
    #[serde(default)]
    pub services: Vec<ServiceConfig>,
}

impl TunnelClientConfig {
    /// Create a new client config
    #[must_use]
    pub fn new(server_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            server_url: server_url.into(),
            token: token.into(),
            reconnect_interval: default_reconnect_interval(),
            max_reconnect_interval: default_max_reconnect_interval(),
            services: Vec::new(),
        }
    }

    /// Add a service to the configuration
    #[must_use]
    pub fn with_service(mut self, service: ServiceConfig) -> Self {
        self.services.push(service);
        self
    }

    /// Validate the configuration
    ///
    /// # Errors
    ///
    /// Returns an error string if:
    /// - `server_url` is empty or doesn't start with `ws://` or `wss://`
    /// - `token` is empty
    /// - `max_reconnect_interval` < `reconnect_interval`
    /// - Any service configuration is invalid
    /// - Duplicate service names exist
    pub fn validate(&self) -> Result<(), String> {
        if self.server_url.is_empty() {
            return Err("server_url cannot be empty".to_string());
        }

        // Basic URL validation
        if !self.server_url.starts_with("ws://") && !self.server_url.starts_with("wss://") {
            return Err("server_url must start with ws:// or wss://".to_string());
        }

        if self.token.is_empty() {
            return Err("token cannot be empty".to_string());
        }

        if self.max_reconnect_interval < self.reconnect_interval {
            return Err(format!(
                "max_reconnect_interval ({:?}) must be >= reconnect_interval ({:?})",
                self.max_reconnect_interval, self.reconnect_interval
            ));
        }

        // Validate each service
        for (i, service) in self.services.iter().enumerate() {
            service
                .validate()
                .map_err(|e| format!("services[{i}]: {e}"))?;
        }

        // Check for duplicate service names
        let mut names: Vec<&str> = self.services.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        for i in 1..names.len() {
            if names[i] == names[i - 1] {
                return Err(format!("duplicate service name: {}", names[i]));
            }
        }

        Ok(())
    }
}

// =============================================================================
// Service Configuration
// =============================================================================

/// Service to expose through the tunnel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// Service name (for identification)
    pub name: String,

    /// Protocol (tcp/udp)
    #[serde(default)]
    pub protocol: ServiceProtocol,

    /// Local port to forward from
    pub local_port: u16,

    /// Remote port to expose (0 = auto-assign)
    #[serde(default)]
    pub remote_port: u16,
}

impl ServiceConfig {
    /// Create a new TCP service configuration
    #[must_use]
    pub fn tcp(name: impl Into<String>, local_port: u16) -> Self {
        Self {
            name: name.into(),
            protocol: ServiceProtocol::Tcp,
            local_port,
            remote_port: 0,
        }
    }

    /// Create a new UDP service configuration
    #[must_use]
    pub fn udp(name: impl Into<String>, local_port: u16) -> Self {
        Self {
            name: name.into(),
            protocol: ServiceProtocol::Udp,
            local_port,
            remote_port: 0,
        }
    }

    /// Set the remote port
    #[must_use]
    pub const fn with_remote_port(mut self, port: u16) -> Self {
        self.remote_port = port;
        self
    }

    /// Validate the service configuration
    ///
    /// # Errors
    ///
    /// Returns an error string if:
    /// - `name` is empty or longer than 255 characters
    /// - `name` contains invalid characters (only alphanumeric, hyphens, underscores allowed)
    /// - `local_port` is 0
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("name cannot be empty".to_string());
        }

        if self.name.len() > 255 {
            return Err(format!("name too long: {} chars, max 255", self.name.len()));
        }

        // Check for invalid characters in name
        if !self
            .name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(
                "name must contain only alphanumeric characters, hyphens, and underscores"
                    .to_string(),
            );
        }

        if self.local_port == 0 {
            return Err("local_port cannot be 0".to_string());
        }

        Ok(())
    }
}

// =============================================================================
// humantime_serde module for Duration serialization
// =============================================================================

mod humantime_serde {
    use serde::{self, Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = format!("{}s", duration.as_secs());
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        parse_duration(&s).map_err(serde::de::Error::custom)
    }

    fn parse_duration(s: &str) -> Result<Duration, String> {
        let s = s.trim();

        // Try to parse as plain seconds
        if let Ok(secs) = s.parse::<u64>() {
            return Ok(Duration::from_secs(secs));
        }

        // Parse with suffix using strip_suffix
        if let Some(num_str) = s.strip_suffix("ms") {
            let num: u64 = num_str
                .trim()
                .parse()
                .map_err(|_| format!("invalid duration number: {num_str}"))?;
            return Ok(Duration::from_millis(num));
        }

        if let Some(num_str) = s.strip_suffix('s') {
            let num: u64 = num_str
                .trim()
                .parse()
                .map_err(|_| format!("invalid duration number: {num_str}"))?;
            return Ok(Duration::from_secs(num));
        }

        if let Some(num_str) = s.strip_suffix('m') {
            let num: u64 = num_str
                .trim()
                .parse()
                .map_err(|_| format!("invalid duration number: {num_str}"))?;
            return Ok(Duration::from_secs(num * 60));
        }

        if let Some(num_str) = s.strip_suffix('h') {
            let num: u64 = num_str
                .trim()
                .parse()
                .map_err(|_| format!("invalid duration number: {num_str}"))?;
            return Ok(Duration::from_secs(num * 3600));
        }

        Err(format!("invalid duration format: {s}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_defaults() {
        let config = TunnelServerConfig::default();
        assert!(config.enabled);
        assert_eq!(config.control_path, "/tunnel/v1");
        assert_eq!(config.data_port_range, (30000, 31000));
        assert_eq!(config.heartbeat_interval, Duration::from_secs(30));
        assert_eq!(config.heartbeat_timeout, Duration::from_secs(40));
        assert_eq!(config.max_tunnels, 1000);
        assert_eq!(config.max_services_per_tunnel, 50);
    }

    #[test]
    fn test_server_config_validation() {
        let config = TunnelServerConfig::default();
        assert!(config.validate().is_ok());

        // Empty control path
        let bad = TunnelServerConfig {
            control_path: String::new(),
            ..Default::default()
        };
        assert!(bad.validate().is_err());

        // Control path without leading slash
        let bad = TunnelServerConfig {
            control_path: "tunnel/v1".to_string(),
            ..Default::default()
        };
        assert!(bad.validate().is_err());

        // Invalid port range
        let bad = TunnelServerConfig {
            data_port_range: (31000, 30000),
            ..Default::default()
        };
        assert!(bad.validate().is_err());

        // Privileged port range
        let bad = TunnelServerConfig {
            data_port_range: (80, 100),
            ..Default::default()
        };
        assert!(bad.validate().is_err());

        // Heartbeat timeout <= interval
        let bad = TunnelServerConfig {
            heartbeat_timeout: Duration::from_secs(30),
            ..Default::default()
        };
        assert!(bad.validate().is_err());

        // Zero max tunnels
        let bad = TunnelServerConfig {
            max_tunnels: 0,
            ..Default::default()
        };
        assert!(bad.validate().is_err());

        // Zero max services
        let bad = TunnelServerConfig {
            max_services_per_tunnel: 0,
            ..Default::default()
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn test_client_config_new() {
        let config = TunnelClientConfig::new("wss://example.com/tunnel/v1", "my-token");
        assert_eq!(config.server_url, "wss://example.com/tunnel/v1");
        assert_eq!(config.token, "my-token");
        assert_eq!(config.reconnect_interval, Duration::from_secs(5));
        assert_eq!(config.max_reconnect_interval, Duration::from_secs(60));
        assert!(config.services.is_empty());
    }

    #[test]
    fn test_client_config_with_service() {
        let config = TunnelClientConfig::new("wss://example.com/tunnel/v1", "my-token")
            .with_service(ServiceConfig::tcp("ssh", 22))
            .with_service(ServiceConfig::udp("game", 27015));

        assert_eq!(config.services.len(), 2);
        assert_eq!(config.services[0].name, "ssh");
        assert_eq!(config.services[1].name, "game");
    }

    #[test]
    fn test_client_config_validation() {
        let config = TunnelClientConfig::new("wss://example.com/tunnel/v1", "my-token")
            .with_service(ServiceConfig::tcp("ssh", 22));
        assert!(config.validate().is_ok());

        // Empty server URL
        let mut bad = TunnelClientConfig::new("wss://example.com", "token");
        bad.server_url = String::new();
        assert!(bad.validate().is_err());

        // Invalid URL scheme
        let bad = TunnelClientConfig::new("http://example.com", "token");
        assert!(bad.validate().is_err());

        // Empty token
        let mut bad = TunnelClientConfig::new("wss://example.com", "token");
        bad.token = String::new();
        assert!(bad.validate().is_err());

        // Invalid reconnect intervals
        let mut bad = TunnelClientConfig::new("wss://example.com", "token");
        bad.reconnect_interval = Duration::from_secs(60);
        bad.max_reconnect_interval = Duration::from_secs(30);
        assert!(bad.validate().is_err());

        // Duplicate service names
        let bad = TunnelClientConfig::new("wss://example.com", "token")
            .with_service(ServiceConfig::tcp("ssh", 22))
            .with_service(ServiceConfig::tcp("ssh", 2222));
        assert!(bad.validate().is_err());
    }

    #[test]
    fn test_service_config_tcp() {
        let service = ServiceConfig::tcp("ssh", 22);
        assert_eq!(service.name, "ssh");
        assert_eq!(service.protocol, ServiceProtocol::Tcp);
        assert_eq!(service.local_port, 22);
        assert_eq!(service.remote_port, 0);
    }

    #[test]
    fn test_service_config_udp() {
        let service = ServiceConfig::udp("game", 27015);
        assert_eq!(service.name, "game");
        assert_eq!(service.protocol, ServiceProtocol::Udp);
        assert_eq!(service.local_port, 27015);
        assert_eq!(service.remote_port, 0);
    }

    #[test]
    fn test_service_config_with_remote_port() {
        let service = ServiceConfig::tcp("ssh", 22).with_remote_port(2222);
        assert_eq!(service.remote_port, 2222);
    }

    #[test]
    fn test_service_config_validation() {
        let service = ServiceConfig::tcp("ssh", 22);
        assert!(service.validate().is_ok());

        // Empty name
        let mut bad = ServiceConfig::tcp("ssh", 22);
        bad.name = String::new();
        assert!(bad.validate().is_err());

        // Name too long
        let mut bad = ServiceConfig::tcp("ssh", 22);
        bad.name = "a".repeat(256);
        assert!(bad.validate().is_err());

        // Invalid characters in name
        let mut bad = ServiceConfig::tcp("ssh", 22);
        bad.name = "ssh service".to_string(); // Space not allowed
        assert!(bad.validate().is_err());

        // Zero local port
        let mut bad = ServiceConfig::tcp("ssh", 22);
        bad.local_port = 0;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn test_service_config_name_characters() {
        // Valid names
        assert!(ServiceConfig::tcp("ssh", 22).validate().is_ok());
        assert!(ServiceConfig::tcp("my-service", 22).validate().is_ok());
        assert!(ServiceConfig::tcp("my_service", 22).validate().is_ok());
        assert!(ServiceConfig::tcp("service123", 22).validate().is_ok());
        assert!(ServiceConfig::tcp("SSH", 22).validate().is_ok());

        // Invalid names
        let mut bad = ServiceConfig::tcp("test", 22);
        bad.name = "my service".to_string();
        assert!(bad.validate().is_err());

        bad.name = "my.service".to_string();
        assert!(bad.validate().is_err());

        bad.name = "my/service".to_string();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn test_server_config_toml_roundtrip() {
        let config = TunnelServerConfig::default();
        let toml_str = toml::to_string(&config).expect("serialize");
        let parsed: TunnelServerConfig = toml::from_str(&toml_str).expect("deserialize");

        assert_eq!(parsed.enabled, config.enabled);
        assert_eq!(parsed.control_path, config.control_path);
        assert_eq!(parsed.data_port_range, config.data_port_range);
        assert_eq!(parsed.heartbeat_interval, config.heartbeat_interval);
        assert_eq!(parsed.heartbeat_timeout, config.heartbeat_timeout);
        assert_eq!(parsed.max_tunnels, config.max_tunnels);
        assert_eq!(
            parsed.max_services_per_tunnel,
            config.max_services_per_tunnel
        );
    }

    #[test]
    fn test_client_config_toml_roundtrip() {
        let config = TunnelClientConfig::new("wss://example.com/tunnel/v1", "my-token")
            .with_service(ServiceConfig::tcp("ssh", 22).with_remote_port(2222))
            .with_service(ServiceConfig::udp("game", 27015));

        let toml_str = toml::to_string(&config).expect("serialize");
        let parsed: TunnelClientConfig = toml::from_str(&toml_str).expect("deserialize");

        assert_eq!(parsed.server_url, config.server_url);
        assert_eq!(parsed.token, config.token);
        assert_eq!(parsed.reconnect_interval, config.reconnect_interval);
        assert_eq!(parsed.max_reconnect_interval, config.max_reconnect_interval);
        assert_eq!(parsed.services.len(), config.services.len());
        assert_eq!(parsed.services[0].name, "ssh");
        assert_eq!(parsed.services[0].local_port, 22);
        assert_eq!(parsed.services[0].remote_port, 2222);
        assert_eq!(parsed.services[1].name, "game");
        assert_eq!(parsed.services[1].protocol, ServiceProtocol::Udp);
    }

    #[test]
    fn test_duration_parsing() {
        let toml_str = r#"
server_url = "wss://example.com"
token = "test"
reconnect_interval = "10s"
max_reconnect_interval = "2m"
"#;
        let config: TunnelClientConfig = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(config.reconnect_interval, Duration::from_secs(10));
        assert_eq!(config.max_reconnect_interval, Duration::from_secs(120));
    }

    #[test]
    fn test_duration_parsing_variants() {
        // Test milliseconds
        let toml_str = r#"
server_url = "wss://example.com"
token = "test"
reconnect_interval = "500ms"
max_reconnect_interval = "1000ms"
"#;
        let config: TunnelClientConfig = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(config.reconnect_interval, Duration::from_millis(500));
        assert_eq!(config.max_reconnect_interval, Duration::from_millis(1000));

        // Test hours
        let toml_str = r#"
server_url = "wss://example.com"
token = "test"
reconnect_interval = "1h"
max_reconnect_interval = "2h"
"#;
        let config: TunnelClientConfig = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(config.reconnect_interval, Duration::from_secs(3600));
        assert_eq!(config.max_reconnect_interval, Duration::from_secs(7200));
    }
}
