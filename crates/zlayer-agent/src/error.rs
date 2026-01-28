//! Agent-specific errors

use std::time::Duration;
use thiserror::Error;

/// Agent runtime errors
#[derive(Debug, Error)]
pub enum AgentError {
    /// Container not found
    #[error("Container '{container}' not found: {reason}")]
    NotFound { container: String, reason: String },

    /// Failed to pull image
    #[error("Failed to pull image '{image}': {reason}")]
    PullFailed { image: String, reason: String },

    /// Failed to create container
    #[error("Failed to create container '{id}': {reason}")]
    CreateFailed { id: String, reason: String },

    /// Failed to start container
    #[error("Failed to start container '{id}': {reason}")]
    StartFailed { id: String, reason: String },

    /// Container exited unexpectedly
    #[error("Container '{id}' exited unexpectedly with code {code}")]
    UnexpectedExit { id: String, code: i32 },

    /// Health check failed
    #[error("Health check failed for '{id}': {reason}")]
    HealthCheckFailed { id: String, reason: String },

    /// Init action failed
    #[error("Init action failed for '{id}': {reason}")]
    InitActionFailed { id: String, reason: String },

    /// Timeout
    #[error("Timeout after {timeout:?}")]
    Timeout { timeout: Duration },

    /// Dependency timeout - service waiting for dependency condition
    #[error("Dependency timeout: '{service}' waiting for '{dependency}' ({condition}) after {timeout:?}")]
    DependencyTimeout {
        service: String,
        dependency: String,
        condition: String,
        timeout: Duration,
    },

    /// Invalid spec
    #[error("Invalid spec: {0}")]
    InvalidSpec(String),

    /// Network setup or operation failed
    #[error("Network error: {0}")]
    Network(String),

    /// Configuration error (missing or invalid configuration)
    #[error("Configuration error: {0}")]
    Configuration(String),

    /// Internal runtime error
    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T, E = AgentError> = std::result::Result<T, E>;
