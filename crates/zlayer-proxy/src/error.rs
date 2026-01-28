//! Proxy error types
//!
//! This module defines all error types for the proxy crate.

use std::net::SocketAddr;
use thiserror::Error;

/// Errors that can occur in the proxy
#[derive(Debug, Error)]
pub enum ProxyError {
    /// Failed to bind to address
    #[error("Failed to bind to {addr}: {reason}")]
    BindFailed { addr: SocketAddr, reason: String },

    /// Failed to connect to backend
    #[error("Failed to connect to backend {backend}: {reason}")]
    BackendConnectionFailed { backend: SocketAddr, reason: String },

    /// No healthy backends available
    #[error("No healthy backends available for service '{service}'")]
    NoHealthyBackends { service: String },

    /// Backend request failed
    #[error("Backend request failed: {0}")]
    BackendRequestFailed(String),

    /// Route not found
    #[error("No route found for host '{host}' path '{path}'")]
    RouteNotFound { host: String, path: String },

    /// Invalid request
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// TLS error
    #[error("TLS error: {0}")]
    Tls(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Hyper error
    #[error("HTTP error: {0}")]
    Hyper(#[from] hyper::Error),

    /// Timeout
    #[error("Request timeout after {0:?}")]
    Timeout(std::time::Duration),

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Result type alias for proxy operations
pub type Result<T, E = ProxyError> = std::result::Result<T, E>;

impl ProxyError {
    /// Returns the HTTP status code for this error
    pub fn status_code(&self) -> http::StatusCode {
        match self {
            ProxyError::RouteNotFound { .. } => http::StatusCode::NOT_FOUND,
            ProxyError::NoHealthyBackends { .. } => http::StatusCode::SERVICE_UNAVAILABLE,
            ProxyError::BackendConnectionFailed { .. } => http::StatusCode::BAD_GATEWAY,
            ProxyError::BackendRequestFailed(_) => http::StatusCode::BAD_GATEWAY,
            ProxyError::InvalidRequest(_) => http::StatusCode::BAD_REQUEST,
            ProxyError::Timeout(_) => http::StatusCode::GATEWAY_TIMEOUT,
            _ => http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_status_codes() {
        let err = ProxyError::RouteNotFound {
            host: "example.com".to_string(),
            path: "/api".to_string(),
        };
        assert_eq!(err.status_code(), http::StatusCode::NOT_FOUND);

        let err = ProxyError::NoHealthyBackends {
            service: "api".to_string(),
        };
        assert_eq!(err.status_code(), http::StatusCode::SERVICE_UNAVAILABLE);

        let err = ProxyError::BackendConnectionFailed {
            backend: "127.0.0.1:8080".parse().unwrap(),
            reason: "connection refused".to_string(),
        };
        assert_eq!(err.status_code(), http::StatusCode::BAD_GATEWAY);
    }
}
