//! Scheduler error types

use thiserror::Error;

/// Errors that can occur in the scheduler crate
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// Metrics collection failed
    #[error("Metrics collection failed: {0}")]
    MetricsCollection(String),

    /// Autoscaling operation failed
    #[error("Autoscaling error: {0}")]
    Autoscaling(String),

    /// Raft consensus error
    #[error("Raft consensus error: {0}")]
    Raft(String),

    /// Service not found in the scheduler
    #[error("Service not found: {0}")]
    ServiceNotFound(String),

    /// Invalid configuration provided
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    /// Cooldown period is active, cannot scale
    #[error("Cooldown period active for service: {0}")]
    CooldownActive(String),

    /// Scale bounds (min/max) exceeded
    #[error("Scale bounds exceeded: {0}")]
    ScaleBoundsExceeded(String),

    /// Storage operation failed
    #[error("Storage error: {0}")]
    Storage(String),

    /// Network operation failed
    #[error("Network error: {0}")]
    Network(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Errors specific to Raft network operations
#[derive(Debug, Error)]
pub enum RaftNetworkError {
    /// Request timeout
    #[error("Network request timed out")]
    Timeout,

    /// Target node unreachable
    #[error("Target node unreachable: {0}")]
    Unreachable(String),

    /// Serialization/deserialization failed
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Invalid or unexpected response
    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    /// HTTP client error
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

/// Result type alias for scheduler operations
pub type Result<T> = std::result::Result<T, SchedulerError>;
