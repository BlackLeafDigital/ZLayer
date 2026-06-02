//! Error type for the overlayd transport, server, and client.

use thiserror::Error;

/// Maximum accepted IPC frame size (8 MiB). A status snapshot of a large
/// cluster is the biggest legitimate frame; anything past this is treated as a
/// framing desync / hostile peer and rejected rather than allocated.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Errors raised by the overlayd IPC layer and server.
#[derive(Debug, Error)]
pub enum OverlaydError {
    /// Underlying socket / pipe I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON (de)serialization of a frame failed.
    #[error("frame codec error: {0}")]
    Codec(#[from] serde_json::Error),
    /// A frame exceeded [`MAX_FRAME_BYTES`].
    #[error("frame too large: {0} bytes (max {MAX_FRAME_BYTES})")]
    FrameTooLarge(usize),
    /// The peer closed the connection.
    #[error("connection closed by peer")]
    Closed,
    /// The overlay engine reported a failure (wraps the human-readable reason
    /// that crosses the wire in `OverlaydResponse::Err`).
    #[error("overlay engine error: {0}")]
    Overlay(String),
    /// Any other error with a message.
    #[error("{0}")]
    Other(String),
}

/// Convenience result alias for the overlayd crate.
pub type Result<T> = std::result::Result<T, OverlaydError>;
