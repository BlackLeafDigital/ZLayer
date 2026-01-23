//! Init action error types

use std::time::Duration;
use thiserror::Error;

/// Init action errors
#[derive(Debug, Error)]
pub enum InitError {
    /// TCP connection failed
    #[error("TCP connection to {host}:{port} failed: {reason}")]
    TcpFailed { host: String, port: u16, reason: String },

    /// HTTP request failed
    #[error("HTTP request to {url} failed: {reason}")]
    HttpFailed { url: String, reason: String },

    /// Command execution failed
    #[error("Command '{command}' failed with exit code {code}")]
    CommandFailed {
        command: String,
        code: i32,
        stdout: String,
        stderr: String,
    },

    /// Timeout exceeded
    #[error("Timeout exceeded: {timeout:?}")]
    Timeout { timeout: Duration },

    /// Action not found
    #[error("Unknown init action: {0}")]
    UnknownAction(String),

    /// Invalid parameters
    #[error("Invalid parameters for action '{action}': {reason}")]
    InvalidParams { action: String, reason: String },
}

pub type Result<T, E = InitError> = std::result::Result<T, E>;
