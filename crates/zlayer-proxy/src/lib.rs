//! `ZLayer` Reverse Proxy
//!
//! This crate provides a high-performance reverse proxy for routing HTTP/HTTPS
//! traffic to backend services. It supports:
//!
//! - Host and path-based routing via `ServiceRegistry`
//! - Round-robin backend selection
//! - Health-aware backend selection for L4 streams
//! - HTTP/1.1 support with upgrade (WebSocket) pass-through
//! - Forwarding headers (X-Forwarded-For, etc.)
//! - TLS termination with dynamic SNI certificate selection
//! - ACME (Let's Encrypt) automatic certificate provisioning
//! - L4 TCP/UDP stream proxying
//!
//! # Example
//!
//! ```rust,ignore
//! use zlayer_proxy::{ProxyConfig, ProxyServer, ServiceRegistry, RouteEntry};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     let registry = Arc::new(ServiceRegistry::new());
//!
//!     // Register HTTP services
//!     registry.register(RouteEntry { /* ... */ }).await;
//!
//!     // Start proxy server
//!     let lb = Arc::new(LoadBalancer::new());
//!     let server = ProxyServer::new(ProxyConfig::default(), registry, lb);
//!     server.run().await?;
//!
//!     Ok(())
//! }
//! ```

// Core modules
pub mod config;
pub mod error;
pub mod server;
pub mod service;
pub mod tls;
pub mod tunnel;

// Load balancing and service routing
pub mod acme;
pub mod lb;
pub mod routes;
pub mod sni_resolver;
pub mod stream;

use std::path::PathBuf;
use std::time::Duration;

// Re-export main types
pub use config::{
    HeaderConfig, PoolConfig, ProxyConfig, ServerConfig, TimeoutConfig, TlsConfig, TlsVersion,
};
pub use error::{ProxyError, Result};
pub use server::ProxyServer;
pub use service::{empty_body, full_body, BoxBody, ReverseProxyService};
pub use tls::{create_tls_acceptor, TlsServerConfig};
pub use tunnel::{
    is_upgrade_request, is_upgrade_response, is_websocket_upgrade, proxy_tunnel, proxy_upgrade,
};

// Re-export load balancer types
pub use lb::{Backend, BackendGroup, ConnectionGuard, HealthStatus, LbStrategy, LoadBalancer};

// Re-export service routing types
pub use acme::CertManager;
pub use routes::{ResolvedService, RouteEntry, ServiceRegistry};
pub use sni_resolver::SniCertResolver;

// Re-export stream (L4) proxy types
pub use stream::{
    BackendHealth as StreamBackendHealth, StreamRegistry, StreamService, TcpListenerConfig,
    TcpStreamService, UdpListenerConfig, UdpStreamService, DEFAULT_UDP_SESSION_TIMEOUT,
};

// ============================================================================
// Proxy Configuration & Certificate Utilities
// ============================================================================

/// Default UDP session timeout for stream proxying
fn default_udp_session_timeout() -> Duration {
    stream::DEFAULT_UDP_SESSION_TIMEOUT
}

/// Configuration for the `ZLayer` proxy server
///
/// This configuration struct controls the behavior of the proxy,
/// including listener addresses, ACME/TLS settings, and L4 stream config.
#[derive(Debug, Clone)]
pub struct ZLayerProxyConfig {
    /// HTTP listener address (default: "0.0.0.0:80")
    pub http_addr: String,
    /// HTTPS listener address (default: "0.0.0.0:443")
    pub https_addr: String,
    /// Optional ACME email for Let's Encrypt certificate provisioning
    pub acme_email: Option<String>,
    /// Path to store TLS certificates (default: `zlayer_paths::ZLayerDirs::system_default().certs()`)
    pub cert_storage_path: String,
    /// Enable automatic ACME certificate provisioning
    pub acme_enabled: bool,
    /// Use Let's Encrypt staging environment (for testing)
    pub acme_staging: bool,
    /// Custom ACME directory URL (for non-LE CAs like `ZeroSSL`)
    pub acme_directory_url: Option<String>,
    /// Domains to auto-provision certificates for on startup
    pub auto_provision_domains: Vec<String>,
    /// TCP stream proxy listeners for L4 proxying
    pub tcp: Vec<stream::TcpListenerConfig>,
    /// UDP stream proxy listeners for L4 proxying
    pub udp: Vec<stream::UdpListenerConfig>,
    /// Default UDP session timeout (default: 60 seconds)
    pub udp_session_timeout: Duration,
}

impl Default for ZLayerProxyConfig {
    fn default() -> Self {
        Self {
            http_addr: "0.0.0.0:80".to_string(),
            https_addr: "0.0.0.0:443".to_string(),
            acme_email: None,
            cert_storage_path: zlayer_paths::ZLayerDirs::system_default()
                .certs()
                .to_string_lossy()
                .into_owned(),
            acme_enabled: false,
            acme_staging: false,
            acme_directory_url: None,
            auto_provision_domains: vec![],
            tcp: vec![],
            udp: vec![],
            udp_session_timeout: default_udp_session_timeout(),
        }
    }
}

impl ZLayerProxyConfig {
    /// Get the ACME directory URL based on configuration
    ///
    /// Returns the custom directory URL if set, otherwise returns the appropriate
    /// Let's Encrypt URL based on whether staging mode is enabled.
    #[must_use]
    pub fn acme_directory(&self) -> &str {
        match &self.acme_directory_url {
            Some(url) => url.as_str(),
            None if self.acme_staging => "https://acme-staging-v02.api.letsencrypt.org/directory",
            None => "https://acme-v02.api.letsencrypt.org/directory",
        }
    }
}

/// Backwards-compatible alias for `ZLayerProxyConfig`.
pub type PingoraProxyConfig = ZLayerProxyConfig;

/// Error type for proxy startup failures
#[derive(Debug, thiserror::Error)]
pub enum ProxyStartError {
    /// Failed to create certificate manager
    #[error("Certificate manager error: {0}")]
    CertManager(String),
    /// Failed to create server
    #[error("Server creation error: {0}")]
    ServerCreation(String),
    /// Failed to set up TLS
    #[error("TLS setup error: {0}")]
    TlsSetup(String),
    /// IO error (e.g., reading certificate files)
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Information about a discovered certificate on disk
#[derive(Debug, Clone)]
pub struct DiscoveredCert {
    /// Domain name (extracted from filename)
    pub domain: String,
    /// Path to the certificate file
    pub cert_path: PathBuf,
    /// Path to the private key file
    pub key_path: PathBuf,
}

/// Find all certificates in the storage directory
///
/// Scans the given directory for `.crt` files and their corresponding `.key` files.
/// Returns a list of discovered certificates.
///
/// # Arguments
///
/// * `storage_path` - Path to the certificate storage directory
///
/// # Returns
///
/// Vector of `DiscoveredCert` structs for each valid cert/key pair found
pub fn discover_certificates(storage_path: &PathBuf) -> Vec<DiscoveredCert> {
    let mut certs = Vec::new();

    // Read the directory
    let entries = match std::fs::read_dir(storage_path) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                path = %storage_path.display(),
                error = %e,
                "Failed to read certificate storage directory"
            );
            return certs;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Look for .crt files
        if let Some(extension) = path.extension() {
            if extension == "crt" {
                // Extract domain from filename (e.g., "example.com.crt" -> "example.com")
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    let key_path = storage_path.join(format!("{stem}.key"));

                    // Check if corresponding key file exists
                    if key_path.exists() {
                        tracing::debug!(
                            domain = %stem,
                            cert_path = %path.display(),
                            key_path = %key_path.display(),
                            "Discovered certificate"
                        );
                        certs.push(DiscoveredCert {
                            domain: stem.to_string(),
                            cert_path: path.clone(),
                            key_path,
                        });
                    } else {
                        tracing::warn!(
                            domain = %stem,
                            cert_path = %path.display(),
                            "Certificate found but missing corresponding key file"
                        );
                    }
                }
            }
        }
    }

    certs
}

/// Load existing certificates into the SNI resolver
///
/// # Arguments
///
/// * `cert_manager` - The certificate manager to load certificates from
/// * `sni_resolver` - The SNI resolver to populate with certificates
///
/// # Returns
///
/// The number of certificates loaded
///
/// # Errors
///
/// Returns an error if reading certificate files from disk fails.
pub async fn load_existing_certs_into_resolver(
    cert_manager: &CertManager,
    sni_resolver: &SniCertResolver,
) -> std::result::Result<usize, ProxyStartError> {
    let storage_path = cert_manager.storage_path().clone();
    let discovered = discover_certificates(&storage_path);
    let mut loaded = 0;

    for cert_info in discovered {
        // Read certificate and key files
        let cert_pem = match tokio::fs::read_to_string(&cert_info.cert_path).await {
            Ok(content) => content,
            Err(e) => {
                tracing::warn!(
                    domain = %cert_info.domain,
                    path = %cert_info.cert_path.display(),
                    error = %e,
                    "Failed to read certificate file"
                );
                continue;
            }
        };

        let key_pem = match tokio::fs::read_to_string(&cert_info.key_path).await {
            Ok(content) => content,
            Err(e) => {
                tracing::warn!(
                    domain = %cert_info.domain,
                    path = %cert_info.key_path.display(),
                    error = %e,
                    "Failed to read key file"
                );
                continue;
            }
        };

        // Load into SNI resolver
        match sni_resolver.load_cert(&cert_info.domain, &cert_pem, &key_pem) {
            Ok(()) => {
                tracing::info!(
                    domain = %cert_info.domain,
                    "Loaded certificate into SNI resolver"
                );
                loaded += 1;
            }
            Err(e) => {
                tracing::warn!(
                    domain = %cert_info.domain,
                    error = %e,
                    "Failed to load certificate into SNI resolver"
                );
            }
        }
    }

    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_config_default() {
        let config = ZLayerProxyConfig::default();
        assert_eq!(config.http_addr, "0.0.0.0:80");
        assert_eq!(config.https_addr, "0.0.0.0:443");
        assert!(config.acme_email.is_none());
        assert_eq!(
            config.cert_storage_path,
            zlayer_paths::ZLayerDirs::system_default()
                .certs()
                .to_string_lossy()
        );
        assert!(!config.acme_enabled);
        assert!(!config.acme_staging);
        assert!(config.acme_directory_url.is_none());
        assert!(config.auto_provision_domains.is_empty());
        assert!(config.tcp.is_empty());
        assert!(config.udp.is_empty());
        assert_eq!(config.udp_session_timeout, DEFAULT_UDP_SESSION_TIMEOUT);
    }

    #[test]
    fn test_proxy_config_custom() {
        let config = ZLayerProxyConfig {
            http_addr: "127.0.0.1:8080".to_string(),
            https_addr: "127.0.0.1:8443".to_string(),
            acme_email: Some("admin@example.com".to_string()),
            cert_storage_path: "/tmp/certs".to_string(),
            acme_enabled: true,
            acme_staging: true,
            acme_directory_url: None,
            auto_provision_domains: vec!["example.com".to_string(), "api.example.com".to_string()],
            tcp: vec![TcpListenerConfig {
                port: 5432,
                protocol_hint: Some("postgresql".to_string()),
                tls: false,
                proxy_protocol: false,
            }],
            udp: vec![UdpListenerConfig {
                port: 27015,
                protocol_hint: Some("source-engine".to_string()),
                session_timeout: Some(Duration::from_secs(120)),
            }],
            udp_session_timeout: Duration::from_secs(90),
        };
        assert_eq!(config.http_addr, "127.0.0.1:8080");
        assert_eq!(config.https_addr, "127.0.0.1:8443");
        assert_eq!(config.acme_email, Some("admin@example.com".to_string()));
        assert_eq!(config.cert_storage_path, "/tmp/certs");
        assert!(config.acme_enabled);
        assert!(config.acme_staging);
        assert!(config.acme_directory_url.is_none());
        assert_eq!(config.auto_provision_domains.len(), 2);
        assert_eq!(config.tcp.len(), 1);
        assert_eq!(config.tcp[0].port, 5432);
        assert_eq!(config.udp.len(), 1);
        assert_eq!(config.udp[0].port, 27015);
        assert_eq!(config.udp_session_timeout, Duration::from_secs(90));
    }

    #[test]
    fn test_pingora_proxy_config_alias() {
        // Ensure the backwards-compatible alias works
        let _config: PingoraProxyConfig = ZLayerProxyConfig::default();
    }

    #[test]
    fn test_acme_directory_production() {
        let config = ZLayerProxyConfig {
            acme_staging: false,
            acme_directory_url: None,
            ..Default::default()
        };
        assert_eq!(
            config.acme_directory(),
            "https://acme-v02.api.letsencrypt.org/directory"
        );
    }

    #[test]
    fn test_acme_directory_staging() {
        let config = ZLayerProxyConfig {
            acme_staging: true,
            acme_directory_url: None,
            ..Default::default()
        };
        assert_eq!(
            config.acme_directory(),
            "https://acme-staging-v02.api.letsencrypt.org/directory"
        );
    }

    #[test]
    fn test_acme_directory_custom() {
        let custom_url = "https://acme.zerossl.com/v2/DV90";
        let config = ZLayerProxyConfig {
            acme_staging: true, // Should be ignored when custom URL is set
            acme_directory_url: Some(custom_url.to_string()),
            ..Default::default()
        };
        assert_eq!(config.acme_directory(), custom_url);
    }

    #[test]
    fn test_discover_certificates_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let certs = discover_certificates(&dir.path().to_path_buf());
        assert!(certs.is_empty());
    }

    #[test]
    fn test_discover_certificates_with_certs() {
        let dir = tempfile::tempdir().unwrap();

        // Create a valid cert/key pair
        std::fs::write(dir.path().join("example.com.crt"), "cert content").unwrap();
        std::fs::write(dir.path().join("example.com.key"), "key content").unwrap();

        // Create another cert/key pair
        std::fs::write(dir.path().join("api.example.com.crt"), "cert content 2").unwrap();
        std::fs::write(dir.path().join("api.example.com.key"), "key content 2").unwrap();

        let certs = discover_certificates(&dir.path().to_path_buf());
        assert_eq!(certs.len(), 2);

        let domains: Vec<&str> = certs.iter().map(|c| c.domain.as_str()).collect();
        assert!(domains.contains(&"example.com"));
        assert!(domains.contains(&"api.example.com"));
    }

    #[test]
    fn test_discover_certificates_missing_key() {
        let dir = tempfile::tempdir().unwrap();

        // Create a cert without corresponding key
        std::fs::write(dir.path().join("orphan.com.crt"), "cert content").unwrap();

        let certs = discover_certificates(&dir.path().to_path_buf());
        assert!(certs.is_empty()); // Should not find any since key is missing
    }

    #[test]
    fn test_discover_certificates_nonexistent_dir() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        let certs = discover_certificates(&path);
        assert!(certs.is_empty());
    }

    #[test]
    fn test_discovered_cert_paths() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(dir.path().join("test.example.com.crt"), "cert").unwrap();
        std::fs::write(dir.path().join("test.example.com.key"), "key").unwrap();

        let certs = discover_certificates(&dir.path().to_path_buf());
        assert_eq!(certs.len(), 1);

        let cert = &certs[0];
        assert_eq!(cert.domain, "test.example.com");
        assert!(cert.cert_path.ends_with("test.example.com.crt"));
        assert!(cert.key_path.ends_with("test.example.com.key"));
    }
}
