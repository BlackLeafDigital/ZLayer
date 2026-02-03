//! Error types for tunnel operations

use thiserror::Error;

/// Errors that can occur during tunnel operations
#[derive(Debug, Error)]
pub enum TunnelError {
    /// Protocol-level error (invalid message format, decode failure)
    #[error("Protocol error: {message}")]
    Protocol {
        /// Error message describing the protocol violation
        message: String,
    },

    /// Authentication error (invalid token, expired, unauthorized)
    #[error("Authentication error: {reason}")]
    Auth {
        /// Reason for authentication failure
        reason: String,
    },

    /// Connection error (connection refused, timeout, closed)
    #[error("Connection error: {source}")]
    Connection {
        /// Underlying I/O error
        #[from]
        source: std::io::Error,
    },

    /// Registry error (tunnel not found, service not found, already exists)
    #[error("Registry error: {message}")]
    Registry {
        /// Error message describing the registry issue
        message: String,
    },

    /// Configuration error (invalid config, missing required field)
    #[error("Configuration error: {message}")]
    Config {
        /// Error message describing the configuration issue
        message: String,
    },

    /// Operation timed out
    #[error("Operation timed out")]
    Timeout,

    /// Service is shutting down
    #[error("Service is shutting down")]
    Shutdown,
}

impl TunnelError {
    /// Create a new protocol error
    #[must_use]
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol {
            message: message.into(),
        }
    }

    /// Create a new authentication error
    #[must_use]
    pub fn auth(reason: impl Into<String>) -> Self {
        Self::Auth {
            reason: reason.into(),
        }
    }

    /// Create a new registry error
    #[must_use]
    pub fn registry(message: impl Into<String>) -> Self {
        Self::Registry {
            message: message.into(),
        }
    }

    /// Create a new configuration error
    #[must_use]
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config {
            message: message.into(),
        }
    }

    /// Create a new connection error from any error type
    #[must_use]
    pub fn connection<E: std::error::Error>(err: E) -> Self {
        Self::Connection {
            source: std::io::Error::other(err.to_string()),
        }
    }

    /// Create a new connection error with a message
    #[must_use]
    pub fn connection_msg(message: impl Into<String>) -> Self {
        Self::Connection {
            source: std::io::Error::other(message.into()),
        }
    }

    /// Create a timeout error
    #[must_use]
    pub fn timeout() -> Self {
        Self::Timeout
    }
}

/// Result type alias for tunnel operations
pub type Result<T> = std::result::Result<T, TunnelError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = TunnelError::protocol("invalid message type");
        assert_eq!(err.to_string(), "Protocol error: invalid message type");

        let err = TunnelError::auth("token expired");
        assert_eq!(err.to_string(), "Authentication error: token expired");

        let err = TunnelError::registry("tunnel not found");
        assert_eq!(err.to_string(), "Registry error: tunnel not found");

        let err = TunnelError::config("missing server_url");
        assert_eq!(err.to_string(), "Configuration error: missing server_url");

        let err = TunnelError::Timeout;
        assert_eq!(err.to_string(), "Operation timed out");

        let err = TunnelError::Shutdown;
        assert_eq!(err.to_string(), "Service is shutting down");
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let tunnel_err: TunnelError = io_err.into();
        assert!(matches!(tunnel_err, TunnelError::Connection { .. }));
    }
}
