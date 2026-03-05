//! Error types for the consensus layer.

use crate::types::NodeId;

/// Errors that can occur in the consensus layer.
#[derive(Debug, thiserror::Error)]
pub enum ConsensusError {
    /// `OpenRaft` configuration error.
    #[error("invalid raft config: {0}")]
    Config(#[from] openraft::ConfigError),

    /// `OpenRaft` fatal error.
    #[error("raft fatal error: {0}")]
    Fatal(String),

    /// `OpenRaft` client write error (forwarded as string to avoid generic explosion).
    #[error("raft write error: {0}")]
    Write(String),

    /// `OpenRaft` initialization error.
    #[error("raft init error: {0}")]
    Init(String),

    /// `OpenRaft` membership change error.
    #[error("membership change error: {0}")]
    Membership(String),

    /// Network / RPC error.
    #[error("network error: {0}")]
    Network(String),

    /// Storage error.
    #[error("storage error: {0}")]
    Storage(#[from] openraft::StorageError<NodeId>),

    /// Serialization / deserialization error.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// The node is not the current leader.
    #[error("not leader (leader is {leader:?})")]
    NotLeader {
        /// The node ID of the current leader, if known.
        leader: Option<NodeId>,
    },

    /// Redb storage error.
    #[cfg(feature = "redb-store")]
    #[error("redb error: {0}")]
    Redb(String),
}

/// Result alias for consensus operations.
pub type Result<T> = std::result::Result<T, ConsensusError>;
