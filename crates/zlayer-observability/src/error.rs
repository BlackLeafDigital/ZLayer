//! Error types for observability

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ObservabilityError {
    #[error("Failed to initialize logging: {0}")]
    LoggingInit(String),

    #[error("Failed to initialize tracing: {0}")]
    TracingInit(String),

    #[error("Failed to initialize metrics: {0}")]
    MetricsInit(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ObservabilityError>;
