//! Network re-exports from zlayer-consensus
//!
//! This module previously contained an inline HTTP client for Raft RPCs.
//! Networking is now provided by `zlayer_consensus::network::http_client`.
//!
//! This module is kept for backward-compatibility re-exports only.

pub use zlayer_consensus::network::http_client::HttpConnection;
pub use zlayer_consensus::network::http_client::HttpNetwork;
pub use zlayer_consensus::network::http_client::RaftHttpClient;
