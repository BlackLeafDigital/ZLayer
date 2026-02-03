//! Error types for layer storage

use thiserror::Error;

#[derive(Error, Debug)]
pub enum LayerStorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("S3 error: {0}")]
    S3(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("Layer not found: {0}")]
    NotFound(String),

    #[error("Upload interrupted: {0}")]
    UploadInterrupted(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("WAL parse error: {0}")]
    WalParse(String),

    #[error("Replication error: {0}")]
    Replication(String),

    #[error("Restore failed: {0}")]
    RestoreFailed(String),
}

pub type Result<T> = std::result::Result<T, LayerStorageError>;
