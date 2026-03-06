//! Connection candidate types for ICE-lite NAT traversal

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// Connection candidate for a peer (modeled after ICE candidates)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Candidate {
    /// Candidate type determines priority
    pub candidate_type: CandidateType,
    /// The endpoint address
    pub address: SocketAddr,
    /// Priority (higher = preferred)
    pub priority: u32,
    /// When this candidate was discovered (unix timestamp)
    #[serde(default)]
    pub discovered_at: u64,
}

/// The type of a connection candidate
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CandidateType {
    /// Direct LAN or public IP (highest priority)
    Host,
    /// STUN-discovered reflexive address
    ServerReflexive,
    /// TURN relay address (lowest priority, always works)
    Relay,
}

/// How this peer is currently connected
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConnectionType {
    /// Direct endpoint (original behavior)
    #[default]
    Direct,
    /// STUN-discovered reflexive address (hole-punched)
    HolePunched,
    /// TURN relay
    Relayed,
}

impl std::fmt::Display for ConnectionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Direct => write!(f, "direct"),
            Self::HolePunched => write!(f, "hole-punched"),
            Self::Relayed => write!(f, "relayed"),
        }
    }
}

impl Candidate {
    /// Create a new candidate with auto-computed priority
    #[must_use]
    pub fn new(candidate_type: CandidateType, address: SocketAddr) -> Self {
        let priority = match candidate_type {
            CandidateType::Host => 100,
            CandidateType::ServerReflexive => 50,
            CandidateType::Relay => 10,
        };
        Self {
            candidate_type,
            address,
            priority,
            discovered_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_candidate_new_host() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 51820);
        let candidate = Candidate::new(CandidateType::Host, addr);
        assert_eq!(candidate.candidate_type, CandidateType::Host);
        assert_eq!(candidate.priority, 100);
        assert_eq!(candidate.address, addr);
        assert!(candidate.discovered_at > 0);
    }

    #[test]
    fn test_candidate_new_server_reflexive() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5)), 51820);
        let candidate = Candidate::new(CandidateType::ServerReflexive, addr);
        assert_eq!(candidate.priority, 50);
    }

    #[test]
    fn test_candidate_new_relay() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 3478);
        let candidate = Candidate::new(CandidateType::Relay, addr);
        assert_eq!(candidate.priority, 10);
    }

    #[test]
    fn test_candidate_serialization_roundtrip() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 51820);
        let candidate = Candidate::new(CandidateType::Host, addr);
        let json = serde_json::to_string(&candidate).unwrap();
        let deserialized: Candidate = serde_json::from_str(&json).unwrap();
        assert_eq!(candidate.candidate_type, deserialized.candidate_type);
        assert_eq!(candidate.address, deserialized.address);
        assert_eq!(candidate.priority, deserialized.priority);
    }

    #[test]
    fn test_candidate_type_ordering() {
        assert!(CandidateType::Host < CandidateType::ServerReflexive);
        assert!(CandidateType::ServerReflexive < CandidateType::Relay);
    }

    #[test]
    fn test_connection_type_default() {
        let ct = ConnectionType::default();
        assert_eq!(ct, ConnectionType::Direct);
    }

    #[test]
    fn test_connection_type_display() {
        assert_eq!(ConnectionType::Direct.to_string(), "direct");
        assert_eq!(ConnectionType::HolePunched.to_string(), "hole-punched");
        assert_eq!(ConnectionType::Relayed.to_string(), "relayed");
    }

    #[test]
    fn test_connection_type_serialization_roundtrip() {
        for ct in [
            ConnectionType::Direct,
            ConnectionType::HolePunched,
            ConnectionType::Relayed,
        ] {
            let json = serde_json::to_string(&ct).unwrap();
            let deserialized: ConnectionType = serde_json::from_str(&json).unwrap();
            assert_eq!(ct, deserialized);
        }
    }
}
