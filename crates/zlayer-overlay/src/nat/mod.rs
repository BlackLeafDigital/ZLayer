//! NAT traversal for `ZLayer` overlay networking.
//!
//! Provides STUN endpoint discovery, UDP hole punching, and TURN relay
//! fallback for establishing overlay connections through NAT.

pub mod candidate;
pub mod config;
pub mod discovery;
pub mod relay;
pub mod stun;
pub mod traversal;
pub mod turn;

pub use candidate::{Candidate, CandidateType, ConnectionType};
pub use config::{NatConfig, RelayServerConfig, StunServerConfig, TurnServerConfig};
pub use discovery::RelayDiscovery;
pub use relay::RelayServer;
pub use stun::StunClient;
pub use traversal::NatTraversal;
pub use turn::RelayClient;
