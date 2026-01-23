//! Proxy configuration types
//!
//! This module defines configuration types for the proxy server.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Main proxy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Server configuration
    #[serde(default)]
    pub server: ServerConfig,

    /// TLS configuration (optional)
    #[serde(default)]
    pub tls: Option<TlsConfig>,

    /// Connection pool settings
    #[serde(default)]
    pub pool: PoolConfig,

    /// Timeouts
    #[serde(default)]
    pub timeouts: TimeoutConfig,

    /// Header configuration
    #[serde(default)]
    pub headers: HeaderConfig,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            tls: None,
            pool: PoolConfig::default(),
            timeouts: TimeoutConfig::default(),
            headers: HeaderConfig::default(),
        }
    }
}

/// Server bind configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// HTTP bind address
    #[serde(default = "default_http_addr")]
    pub http_addr: SocketAddr,

    /// HTTPS bind address (if TLS enabled)
    #[serde(default = "default_https_addr")]
    pub https_addr: SocketAddr,

    /// Enable HTTP/2
    #[serde(default = "default_http2_enabled")]
    pub http2_enabled: bool,

    /// Maximum concurrent connections
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
}

fn default_http_addr() -> SocketAddr {
    "0.0.0.0:80".parse().unwrap()
}

fn default_https_addr() -> SocketAddr {
    "0.0.0.0:443".parse().unwrap()
}

fn default_http2_enabled() -> bool {
    true
}

fn default_max_connections() -> usize {
    10000
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            http_addr: default_http_addr(),
            https_addr: default_https_addr(),
            http2_enabled: default_http2_enabled(),
            max_connections: default_max_connections(),
        }
    }
}

/// TLS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Path to certificate file (PEM)
    pub cert_path: PathBuf,

    /// Path to private key file (PEM)
    pub key_path: PathBuf,

    /// Minimum TLS version
    #[serde(default = "default_min_tls_version")]
    pub min_version: TlsVersion,

    /// Enable ALPN for HTTP/2
    #[serde(default = "default_true")]
    pub alpn_h2: bool,
}

fn default_min_tls_version() -> TlsVersion {
    TlsVersion::Tls12
}

fn default_true() -> bool {
    true
}

/// TLS version
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TlsVersion {
    #[serde(rename = "1.2")]
    Tls12,
    #[serde(rename = "1.3")]
    Tls13,
}

/// Connection pool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    /// Maximum idle connections per backend
    #[serde(default = "default_max_idle")]
    pub max_idle_per_backend: usize,

    /// Idle connection timeout
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout: Duration,
}

fn default_max_idle() -> usize {
    32
}

fn default_idle_timeout() -> Duration {
    Duration::from_secs(90)
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_idle_per_backend: default_max_idle(),
            idle_timeout: default_idle_timeout(),
        }
    }
}

/// Timeout configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutConfig {
    /// Connection timeout
    #[serde(default = "default_connect_timeout")]
    pub connect: Duration,

    /// Request timeout (total)
    #[serde(default = "default_request_timeout")]
    pub request: Duration,

    /// Read timeout
    #[serde(default = "default_read_timeout")]
    pub read: Duration,

    /// Write timeout
    #[serde(default = "default_write_timeout")]
    pub write: Duration,
}

fn default_connect_timeout() -> Duration {
    Duration::from_secs(5)
}

fn default_request_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_read_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_write_timeout() -> Duration {
    Duration::from_secs(30)
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            connect: default_connect_timeout(),
            request: default_request_timeout(),
            read: default_read_timeout(),
            write: default_write_timeout(),
        }
    }
}

/// Header configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderConfig {
    /// Add X-Forwarded-For header
    #[serde(default = "default_true")]
    pub x_forwarded_for: bool,

    /// Add X-Forwarded-Proto header
    #[serde(default = "default_true")]
    pub x_forwarded_proto: bool,

    /// Add X-Forwarded-Host header
    #[serde(default = "default_true")]
    pub x_forwarded_host: bool,

    /// Add X-Real-IP header
    #[serde(default = "default_true")]
    pub x_real_ip: bool,

    /// Add Via header
    #[serde(default = "default_true")]
    pub via: bool,

    /// Server name for Via header
    #[serde(default = "default_server_name")]
    pub server_name: String,
}

fn default_server_name() -> String {
    "zlayer-proxy".to_string()
}

impl Default for HeaderConfig {
    fn default() -> Self {
        Self {
            x_forwarded_for: true,
            x_forwarded_proto: true,
            x_forwarded_host: true,
            x_real_ip: true,
            via: true,
            server_name: default_server_name(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ProxyConfig::default();
        assert_eq!(
            config.server.http_addr,
            "0.0.0.0:80".parse::<SocketAddr>().unwrap()
        );
        assert!(config.server.http2_enabled);
        assert!(config.tls.is_none());
        assert_eq!(config.timeouts.connect, Duration::from_secs(5));
    }

    #[test]
    fn test_config_serialization() {
        let config = ProxyConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ProxyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.server.http_addr, config.server.http_addr);
    }
}
