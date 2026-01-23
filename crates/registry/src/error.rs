//! Registry error types

use oci_distribution::errors::OciDistributionError;
use thiserror::Error;

/// Registry-specific errors
#[derive(Debug, Error)]
pub enum RegistryError {
    /// OCI distribution error
    #[error("OCI distribution error: {0}")]
    Oci(#[from] OciDistributionError),

    /// Cache error
    #[error("cache error: {0}")]
    Cache(#[from] CacheError),

    /// Image not found
    #[error("image {image} not found in registry {registry}")]
    NotFound { registry: String, image: String },

    /// Authentication failed
    #[error("authentication failed for registry {registry}: {reason}")]
    AuthFailed { registry: String, reason: String },

    /// Pull cancelled
    #[error("pull cancelled for image {image}")]
    PullCancelled { image: String },
}

/// Cache-specific errors
#[derive(Debug, Error)]
pub enum CacheError {
    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Database error
    #[error("database error: {0}")]
    Database(String),

    /// Blob not found in cache
    #[error("blob {digest} not found in cache")]
    NotFound { digest: String },

    /// Cache corrupted
    #[error("cache corrupted: {0}")]
    Corrupted(String),

    /// Invalid digest format
    #[error("invalid digest format: {0}")]
    InvalidDigest(String),
}

pub type Result<T, E = RegistryError> = std::result::Result<T, E>;
