//! Core error types for ZLayer
//!
//! This module defines the global error hierarchy used across all ZLayer crates.

use std::path::PathBuf;
use thiserror::Error;

/// Global ZLayer error type
#[derive(Debug, Error)]
pub enum ZLayerError {
    /// Spec-related errors
    #[error("spec error: {0}")]
    Spec(#[from] zlayer_spec::SpecError),

    /// Container runtime errors
    #[error("container error: {0}")]
    Container(#[from] ContainerError),

    /// Network-related errors
    #[error("network error: {0}")]
    Network(#[from] NetworkError),

    /// Runtime/agent errors
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),

    /// Configuration errors
    #[error("config error: {0}")]
    Config(#[from] ConfigError),

    /// Registry/OCI errors
    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),

    /// IO errors with context
    #[error("IO error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Container runtime errors
#[derive(Debug, Error)]
pub enum ContainerError {
    /// Failed to pull image
    #[error("failed to pull image {image}: {reason}")]
    PullFailed { image: String, reason: String },

    /// Failed to create container
    #[error("failed to create container: {0}")]
    CreateFailed(String),

    /// Failed to start container
    #[error("failed to start container {id}: {reason}")]
    StartFailed { id: String, reason: String },

    /// Container exited unexpectedly
    #[error("container {id} exited with code {code}")]
    Exited { id: String, code: i32 },

    /// Container not found
    #[error("container {id} not found")]
    NotFound { id: String },

    /// Health check failed
    #[error("health check failed for {id}: {reason}")]
    HealthCheckFailed { id: String, reason: String },

    /// Init action failed
    #[error("init action {action} failed for {id}: {reason}")]
    InitActionFailed {
        id: String,
        action: String,
        reason: String,
    },
}

/// Network-related errors
#[derive(Debug, Error)]
pub enum NetworkError {
    /// Failed to create WireGuard interface
    #[error("failed to create WireGuard interface {name}: {reason}")]
    WireGuardCreateFailed { name: String, reason: String },

    /// Failed to configure WireGuard peer
    #[error("failed to configure WireGuard peer: {0}")]
    WireGuardPeerFailed(String),

    /// DNS resolution failed
    #[error("DNS resolution failed for {name}: {reason}")]
    DnsFailed { name: String, reason: String },

    /// Connection timeout
    #[error("connection timeout to {address}")]
    ConnectionTimeout { address: String },

    /// Failed to bind port
    #[error("failed to bind port {port}: {reason}")]
    BindFailed { port: u16, reason: String },
}

/// Runtime/agent errors
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// Failed to join deployment
    #[error("failed to join deployment {name}: {reason}")]
    JoinFailed { name: String, reason: String },

    /// Scheduler error
    #[error("scheduler error: {0}")]
    Scheduler(String),

    /// Service discovery error
    #[error("service discovery error: {0}")]
    ServiceDiscovery(String),

    /// Autoscaling error
    #[error("autoscaling error for {service}: {reason}")]
    AutoscalingFailed { service: String, reason: String },

    /// Leadership lost
    #[error("leadership lost for deployment {name}")]
    LeadershipLost { name: String },
}

/// Configuration errors
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Missing required configuration
    #[error("missing required configuration: {0}")]
    Missing(String),

    /// Invalid configuration value
    #[error("invalid configuration for {key}: {reason}")]
    Invalid { key: String, reason: String },

    /// Failed to load configuration file
    #[error("failed to load config from {path}: {reason}")]
    LoadFailed { path: PathBuf, reason: String },

    /// Generic configuration error
    #[error("{0}")]
    Other(String),
}

impl ConfigError {
    /// Create a generic configuration error
    pub fn other(msg: impl Into<String>) -> ZLayerError {
        ZLayerError::Config(ConfigError::Other(msg.into()))
    }
}

/// Registry/OCI errors
#[derive(Debug, Error)]
pub enum RegistryError {
    /// Failed to pull manifest
    #[error("failed to pull manifest for {image}: {reason}")]
    ManifestFailed { image: String, reason: String },

    /// Failed to pull blob
    #[error("failed to pull blob {digest}: {reason}")]
    BlobFailed { digest: String, reason: String },

    /// Authentication failed
    #[error("authentication failed for registry {registry}")]
    AuthFailed { registry: String },

    /// Image not found
    #[error("image {image} not found")]
    NotFound { image: String },
}

/// Result type alias for ZLayer operations
pub type Result<T, E = ZLayerError> = std::result::Result<T, E>;

/// Convenience alias for core Error type
pub use ZLayerError as Error;

impl Error {
    /// Create a configuration error
    pub fn config(msg: impl Into<String>) -> Self {
        ZLayerError::Config(ConfigError::Other(msg.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = ContainerError::NotFound {
            id: "test-id".to_string(),
        };
        assert!(err.to_string().contains("test-id"));
    }
}
