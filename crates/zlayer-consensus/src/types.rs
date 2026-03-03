//! Core type aliases and re-exports for the consensus layer.

pub use openraft::BasicNode;

/// Raft node identifier. Every node in a cluster must have a unique `NodeId`.
pub type NodeId = u64;
