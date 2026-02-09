//! Configuration structures for ZLayer

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Main agent configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AgentConfig {
    /// Deployment name this agent belongs to
    pub deployment_name: String,

    /// Unique node identifier
    pub node_id: String,

    /// Raft consensus configuration
    #[serde(default)]
    pub raft: RaftConfig,

    /// Overlay network configuration
    #[serde(default)]
    pub overlay: OverlayAgentConfig,

    /// Data directory for state persistence
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    /// Metrics configuration
    #[serde(default)]
    pub metrics: MetricsConfig,

    /// Logging configuration
    #[serde(default)]
    pub logging: LoggingConfig,

    /// Registry authentication configuration
    #[serde(default)]
    pub auth: crate::auth::AuthConfig,
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("/var/lib/zlayer")
}

/// Raft consensus configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RaftConfig {
    /// Address to bind for Raft RPC
    pub raft_addr: String,

    /// Advertised address for other nodes to connect
    pub advertise_addr: Option<String>,

    /// Snapshot threshold (number of logs before snapshot)
    #[serde(default = "default_snapshot_threshold")]
    pub snapshot_threshold: u64,

    /// Snapshot policy (logs since last snapshot)
    #[serde(default = "default_snapshot_policy_count")]
    pub snapshot_policy_count: u64,

    /// Election timeout minimum
    #[serde(default = "default_election_timeout_min")]
    pub election_timeout_min: u64,

    /// Election timeout maximum
    #[serde(default = "default_election_timeout_max")]
    pub election_timeout_max: u64,

    /// Heartbeat interval
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval: u64,
}

impl Default for RaftConfig {
    fn default() -> Self {
        Self {
            raft_addr: "0.0.0.0:27001".to_string(),
            advertise_addr: None,
            snapshot_threshold: default_snapshot_threshold(),
            snapshot_policy_count: default_snapshot_policy_count(),
            election_timeout_min: default_election_timeout_min(),
            election_timeout_max: default_election_timeout_max(),
            heartbeat_interval: default_heartbeat_interval(),
        }
    }
}

fn default_snapshot_threshold() -> u64 {
    10000
}

fn default_snapshot_policy_count() -> u64 {
    8000
}

fn default_election_timeout_min() -> u64 {
    150
}

fn default_election_timeout_max() -> u64 {
    300
}

fn default_heartbeat_interval() -> u64 {
    50
}

/// Overlay network configuration for agents
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OverlayAgentConfig {
    /// Overlay private key (x25519)
    pub private_key: String,

    /// Overlay public key (derived from private key)
    pub public_key: Option<String>,

    /// Listen port for overlay network (WireGuard protocol)
    #[serde(default = "default_wg_port")]
    pub wg_port: u16,

    /// Global overlay network configuration
    #[serde(default)]
    pub global: GlobalOverlayConfig,

    /// DNS configuration
    #[serde(default)]
    pub dns: DnsConfig,
}

impl Default for OverlayAgentConfig {
    fn default() -> Self {
        Self {
            private_key: String::new(),
            public_key: None,
            wg_port: default_wg_port(),
            global: Default::default(),
            dns: Default::default(),
        }
    }
}

fn default_wg_port() -> u16 {
    51820
}

/// Global overlay network configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlobalOverlayConfig {
    /// Overlay network CIDR (e.g., "10.0.0.0/8")
    #[serde(default = "default_overlay_cidr")]
    pub overlay_cidr: String,

    /// Peer discovery interval
    #[serde(default = "default_peer_discovery")]
    pub peer_discovery_interval: Duration,
}

impl Default for GlobalOverlayConfig {
    fn default() -> Self {
        Self {
            overlay_cidr: default_overlay_cidr(),
            peer_discovery_interval: default_peer_discovery(),
        }
    }
}

fn default_overlay_cidr() -> String {
    "10.0.0.0/8".to_string()
}

fn default_peer_discovery() -> Duration {
    Duration::from_secs(30)
}

/// DNS configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DnsConfig {
    /// DNS listen address
    #[serde(default = "default_dns_addr")]
    pub listen_addr: String,

    /// DNS TLD for global overlay
    #[serde(default = "default_global_tld")]
    pub global_tld: String,

    /// DNS TLD for service-scoped overlay
    #[serde(default = "default_service_tld")]
    pub service_tld: String,
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            listen_addr: default_dns_addr(),
            global_tld: default_global_tld(),
            service_tld: default_service_tld(),
        }
    }
}

fn default_dns_addr() -> String {
    "0.0.0.0:53".to_string()
}

fn default_global_tld() -> String {
    "global".to_string()
}

fn default_service_tld() -> String {
    "service".to_string()
}

/// Metrics configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricsConfig {
    /// Enable metrics collection
    #[serde(default = "default_metrics_enabled")]
    pub enabled: bool,

    /// Metrics exposition address
    #[serde(default = "default_metrics_addr")]
    pub listen_addr: String,

    /// Metrics path
    #[serde(default = "default_metrics_path")]
    pub path: String,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: default_metrics_enabled(),
            listen_addr: default_metrics_addr(),
            path: default_metrics_path(),
        }
    }
}

fn default_metrics_enabled() -> bool {
    true
}

fn default_metrics_addr() -> String {
    "0.0.0.0:9090".to_string()
}

fn default_metrics_path() -> String {
    "/metrics".to_string()
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Log format (json, pretty)
    #[serde(default = "default_log_format")]
    pub format: String,

    /// Log to stdout
    #[serde(default = "default_log_stdout")]
    pub stdout: bool,

    /// Log file path
    pub file: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
            stdout: default_log_stdout(),
            file: None,
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "json".to_string()
}

fn default_log_stdout() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig {
            deployment_name: "test".to_string(),
            node_id: "node-1".to_string(),
            ..Default::default()
        };
        assert_eq!(config.deployment_name, "test");
        assert_eq!(config.node_id, "node-1");
    }
}
