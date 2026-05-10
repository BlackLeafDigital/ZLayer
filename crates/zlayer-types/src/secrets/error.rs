//! Error type for secrets operations.
//!
//! Lifted into `zlayer-types` so cross-crate consumers (`zlayer-api`,
//! `zlayer-agent`, the CLI) can name secrets errors without depending on
//! `zlayer-secrets`. The `zlayer-secrets` crate re-exports these for
//! backward compatibility.

use thiserror::Error;

/// Error type for secrets operations.
#[derive(Error, Debug)]
pub enum SecretsError {
    #[error("Secret not found: {name}")]
    NotFound { name: String },

    #[error("Access denied to secret: {name}")]
    AccessDenied { name: String },

    #[error("Encryption error: {0}")]
    Encryption(String),

    #[error("Decryption error: {0}")]
    Decryption(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Invalid secret name: {name}")]
    InvalidName { name: String },

    #[error("Secret already exists: {name}")]
    AlreadyExists { name: String },

    #[error("Provider error: {0}")]
    Provider(String),
}

/// Convenience `Result` alias parameterised over [`SecretsError`].
pub type Result<T> = std::result::Result<T, SecretsError>;
