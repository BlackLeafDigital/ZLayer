//! Runtime helpers for NAT traversal status reporting.
//!
//! Provides plain-data snapshot types that can be consumed by the daemon's
//! API layer without exposing the full [`NatTraversal`] state.
//!
//! [`NatTraversal`]: super::traversal::NatTraversal

use serde::{Deserialize, Serialize};

use super::candidate::Candidate;

/// Plain-data snapshot of NAT traversal state suitable for API responses.
///
/// Produced by [`crate::OverlayManager`]-style components that own a live
/// `NatTraversal`. The snapshot is decoupled from the runtime types so it
/// can be serialised over JSON without cross-crate generic plumbing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatStatusSnapshot {
    /// Locally gathered ICE candidates.
    pub candidates: Vec<Candidate>,
    /// Per-peer NAT connectivity entries.
    pub peers: Vec<NatPeerSnapshot>,
    /// Unix epoch seconds of the last successful candidate gather / refresh.
    pub last_refresh: u64,
}

impl NatStatusSnapshot {
    /// Construct an empty snapshot. Useful for tests and disabled runtimes.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            candidates: Vec::new(),
            peers: Vec::new(),
            last_refresh: 0,
        }
    }
}

impl Default for NatStatusSnapshot {
    fn default() -> Self {
        Self::empty()
    }
}

/// Per-peer NAT connectivity entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatPeerSnapshot {
    /// Peer node id (or public key when no node id is available).
    pub node_id: String,
    /// Current connection type as a lowercase string
    /// (`"direct"` / `"hole-punched"` / `"relayed"` / `"unreachable"`).
    pub connection_type: String,
    /// Selected remote endpoint (`host:port`), if any has been negotiated.
    pub remote_endpoint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_has_no_peers_or_candidates() {
        let s = NatStatusSnapshot::empty();
        assert!(s.candidates.is_empty());
        assert!(s.peers.is_empty());
        assert_eq!(s.last_refresh, 0);
    }

    #[test]
    fn snapshot_default_matches_empty() {
        let a = NatStatusSnapshot::default();
        let b = NatStatusSnapshot::empty();
        assert_eq!(a.candidates.len(), b.candidates.len());
        assert_eq!(a.peers.len(), b.peers.len());
        assert_eq!(a.last_refresh, b.last_refresh);
    }

    #[test]
    fn peer_snapshot_serialises_and_round_trips() {
        let peer = NatPeerSnapshot {
            node_id: "node-1".to_string(),
            connection_type: "direct".to_string(),
            remote_endpoint: Some("203.0.113.5:51820".to_string()),
        };
        let json = serde_json::to_string(&peer).unwrap();
        let parsed: NatPeerSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.node_id, "node-1");
        assert_eq!(parsed.connection_type, "direct");
        assert_eq!(parsed.remote_endpoint.as_deref(), Some("203.0.113.5:51820"));
    }
}
