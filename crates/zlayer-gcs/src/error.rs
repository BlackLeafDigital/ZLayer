//! Error type for the GCS bridge.
//!
//! All public surface returns [`GcsResult<T>`]; transport, framing, protocol
//! negotiation, and RPC dispatch all funnel their failure modes through the
//! single [`GcsError`] enum so callers can match on the failure class without
//! caring which sub-layer produced it.

use thiserror::Error;

/// Errors that can occur while talking to a UVM's Guest Compute Service.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GcsError {
    /// Underlying I/O failure on the hvsock transport (read/write/connect).
    #[error("gcs i/o: {0}")]
    Io(#[from] std::io::Error),

    /// JSON encode/decode failure on a GCS protocol message payload.
    #[error("gcs json: {0}")]
    Json(#[from] serde_json::Error),

    /// Hyper-V socket (`AF_HYPERV`) setup or address resolution failure.
    #[error("hvsock: {0}")]
    Hvsock(String),

    /// Generic protocol-layer violation — unexpected message type, bad
    /// sequence number, frame length out of range, etc.
    #[error("protocol: {0}")]
    Protocol(String),

    /// Capability/version negotiation with the in-guest GCS failed.
    #[error("negotiation: {0}")]
    Negotiation(String),

    /// An RPC or read/write operation exceeded its deadline.
    #[error("operation timed out")]
    Timeout,

    /// The bridge connection has been closed (cleanly or by the peer).
    #[error("bridge closed")]
    Closed,
}

/// Convenience alias for results returned by the GCS bridge.
pub type GcsResult<T> = Result<T, GcsError>;
