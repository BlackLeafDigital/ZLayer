//! Error types for overlay network operations

use std::net::Ipv4Addr;
use thiserror::Error;

/// Errors that can occur during overlay network operations
#[derive(Debug, Error)]
pub enum OverlayError {
    /// Overlay transport not available
    #[error("Overlay transport not available: {0}")]
    TransportNotAvailable(String),

    /// Transport command execution failed
    #[error("Transport command failed: {0}")]
    TransportCommand(String),

    /// Boringtun device error
    #[error("Boringtun device error: {0}")]
    BoringtunDevice(String),

    /// Invalid CIDR notation
    #[error("Invalid CIDR notation: {0}")]
    InvalidCidr(String),

    /// No available IPs in the configured CIDR range
    #[error("No available IP addresses in CIDR range")]
    NoAvailableIps,

    /// IP address already allocated
    #[error("IP address {0} is already allocated")]
    IpAlreadyAllocated(Ipv4Addr),

    /// IP address not in CIDR range
    #[error("IP address {0} is not within CIDR range {1}")]
    IpNotInRange(Ipv4Addr, String),

    /// Interface already exists
    #[error("Overlay interface '{0}' already exists")]
    InterfaceExists(String),

    /// Interface not found
    #[error("Overlay interface '{0}' not found")]
    InterfaceNotFound(String),

    /// Peer not found
    #[error("Peer with public key '{0}' not found")]
    PeerNotFound(String),

    /// Peer is unreachable
    #[error("Peer at {ip} is unreachable: {reason}")]
    PeerUnreachable { ip: Ipv4Addr, reason: String },

    /// Configuration file error
    #[error("Configuration error: {0}")]
    Config(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Already initialized
    #[error("Overlay network already initialized at {0}")]
    AlreadyInitialized(String),

    /// Not initialized
    #[error("Overlay network not initialized. Run init_leader or join first")]
    NotInitialized,

    /// Permission denied
    #[error("Permission denied: {0}. This operation typically requires root privileges")]
    PermissionDenied(String),

    /// Timeout waiting for operation
    #[error("Operation timed out: {0}")]
    Timeout(String),

    /// Invalid key format
    #[error("Invalid key format: {0}")]
    InvalidKey(String),

    /// Network configuration error
    #[error("Network configuration error: {0}")]
    NetworkConfig(String),

    /// DNS error
    #[error("DNS error: {0}")]
    Dns(String),
}

/// Result type alias for overlay operations
pub type Result<T> = std::result::Result<T, OverlayError>;
