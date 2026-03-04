//! Consensus configuration with production-ready defaults.
//!
//! The default values are tuned for a typical 3-5 node cluster on a LAN
//! with sub-millisecond latency. For WAN deployments, increase all timeouts.

use std::time::Duration;

use openraft::SnapshotPolicy;

/// Configuration for a consensus node.
///
/// Wraps both openraft's `Config` and network-level timeout settings.
#[derive(Debug, Clone)]
pub struct ConsensusConfig {
    /// Human-readable cluster name (used in logs).
    pub cluster_name: String,

    /// Minimum election timeout in milliseconds.
    ///
    /// A follower will start an election if it has not heard from the leader
    /// in at least this many milliseconds. Setting this to 7-15x the heartbeat
    /// interval prevents spurious elections while still detecting failures quickly.
    ///
    /// Default: 1500ms (7.5x default heartbeat).
    pub election_timeout_min_ms: u64,

    /// Maximum election timeout in milliseconds.
    ///
    /// The actual election timeout is randomized between min and max to prevent
    /// split-vote scenarios.
    ///
    /// Default: 3000ms (15x default heartbeat).
    pub election_timeout_max_ms: u64,

    /// Heartbeat interval in milliseconds.
    ///
    /// The leader sends heartbeats at this interval to maintain authority.
    ///
    /// Default: 200ms.
    pub heartbeat_interval_ms: u64,

    /// Number of log entries since the last snapshot before triggering a new one.
    ///
    /// Default: 10,000 entries.
    pub snapshot_logs_since_last: u64,

    /// Maximum number of entries per `AppendEntries` RPC payload.
    ///
    /// Default: 300.
    pub max_payload_entries: u64,

    /// Timeout for vote and `append_entries` RPCs.
    ///
    /// Default: 5 seconds.
    pub rpc_timeout: Duration,

    /// Timeout for snapshot transfer RPCs.
    ///
    /// Snapshots can be large, so this should be significantly longer than
    /// the normal RPC timeout.
    ///
    /// Default: 60 seconds.
    pub snapshot_timeout: Duration,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            cluster_name: "zlayer".to_string(),
            election_timeout_min_ms: 1500,
            election_timeout_max_ms: 3000,
            heartbeat_interval_ms: 200,
            snapshot_logs_since_last: 10_000,
            max_payload_entries: 300,
            rpc_timeout: Duration::from_secs(5),
            snapshot_timeout: Duration::from_secs(60),
        }
    }
}

impl ConsensusConfig {
    /// Build an openraft `Config` from this consensus config.
    ///
    /// # Errors
    ///
    /// Returns an error if the resulting openraft config fails validation
    /// (e.g., `election_timeout_min` > `election_timeout_max`).
    #[allow(clippy::result_large_err)]
    pub fn to_openraft_config(&self) -> Result<openraft::Config, openraft::ConfigError> {
        let config = openraft::Config {
            cluster_name: self.cluster_name.clone(),
            election_timeout_min: self.election_timeout_min_ms,
            election_timeout_max: self.election_timeout_max_ms,
            heartbeat_interval: self.heartbeat_interval_ms,
            max_payload_entries: self.max_payload_entries,
            snapshot_policy: SnapshotPolicy::LogsSinceLast(self.snapshot_logs_since_last),
            enable_tick: true,
            enable_heartbeat: true,
            enable_elect: true,
            ..Default::default()
        };

        config.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_validates() {
        let config = ConsensusConfig::default();
        let raft_config = config.to_openraft_config();
        assert!(
            raft_config.is_ok(),
            "Default config should validate: {raft_config:?}"
        );
    }

    #[test]
    fn invalid_config_detected() {
        let config = ConsensusConfig {
            election_timeout_min_ms: 5000,
            election_timeout_max_ms: 1000, // min > max
            ..Default::default()
        };
        assert!(config.to_openraft_config().is_err());
    }
}
