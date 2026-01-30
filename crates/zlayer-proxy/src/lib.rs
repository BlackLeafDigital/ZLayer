//! ZLayer Reverse Proxy
//!
//! This crate provides a high-performance reverse proxy for routing HTTP/HTTPS
//! traffic to backend services. It supports:
//!
//! - Host and path-based routing
//! - Load balancing (round-robin, least connections)
//! - Health-aware backend selection
//! - HTTP/1.1 and HTTP/2 support
//! - Forwarding headers (X-Forwarded-For, etc.)
//! - Configurable timeouts
//!
//! # Architecture
//!
//! The proxy crate contains two implementations:
//!
//! 1. **Legacy (hyper-based)**: Original implementation using hyper directly
//! 2. **Pingora-based**: New high-performance implementation using Cloudflare's Pingora
//!
//! The Pingora implementation provides:
//! - 70% less CPU usage vs nginx/hyper
//! - 67% less memory usage
//! - 99.92% connection reuse (vs 87.1%)
//! - Improved p95 TTFB by ~80ms
//!
//! # Example (Pingora)
//!
//! ```rust,ignore
//! use proxy::{PingoraProxyConfig, ServiceRegistry, start_proxy};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     // Create service registry
//!     let registry = Arc::new(ServiceRegistry::new());
//!
//!     // Register services
//!     registry.register("api.example.com", None, proxy::ResolvedService {
//!         name: "api".to_string(),
//!         backends: vec!["127.0.0.1:8080".parse()?],
//!         use_tls: false,
//!         sni_hostname: String::new(),
//!     });
//!
//!     // Start proxy
//!     let config = PingoraProxyConfig::default();
//!     start_proxy(config, registry)?;
//!
//!     Ok(())
//! }
//! ```

// Original hyper-based modules
pub mod config;
pub mod error;
pub mod lb;
pub mod routing;
pub mod server;
pub mod service;
pub mod tls;
pub mod tunnel;

// New Pingora-based modules
pub mod acme;
pub mod proxy;
pub mod routes;
pub mod sni_resolver;

use std::path::PathBuf;
use std::sync::Arc;

// Re-export main types from original modules
pub use config::{
    HeaderConfig, PoolConfig, ProxyConfig, ServerConfig, TimeoutConfig, TlsConfig, TlsVersion,
};
pub use error::{ProxyError, Result};
pub use lb::{Backend, ConnectionGuard, HealthStatus, LoadBalancer, LoadBalancerAlgorithm};
pub use routing::{Route, RouteMatch, Router};
pub use server::{ProxyServer, ProxyServerBuilder};
pub use service::{empty_body, full_body, BoxBody, ReverseProxyService};
pub use tls::{create_tls_acceptor, TlsServerConfig};
pub use tunnel::{
    is_upgrade_request, is_upgrade_response, is_websocket_upgrade, proxy_tunnel, proxy_upgrade,
};

// Re-export Pingora-based types
pub use acme::CertManager;
pub use proxy::{ZLayerCtx, ZLayerProxy};
pub use routes::{ResolvedService, ServiceRegistry};
pub use sni_resolver::SniCertResolver;

// ============================================================================
// Pingora Server Bootstrap
// ============================================================================

/// Configuration for the Pingora-based proxy server
///
/// This configuration struct controls the behavior of the high-performance
/// Pingora proxy implementation.
#[derive(Debug, Clone)]
pub struct PingoraProxyConfig {
    /// HTTP listener address (default: "0.0.0.0:80")
    pub http_addr: String,
    /// HTTPS listener address (default: "0.0.0.0:443")
    pub https_addr: String,
    /// Optional ACME email for Let's Encrypt certificate provisioning
    pub acme_email: Option<String>,
    /// Path to store TLS certificates (default: "/var/lib/zlayer/certs")
    pub cert_storage_path: String,
    /// Enable automatic ACME certificate provisioning
    pub acme_enabled: bool,
    /// Use Let's Encrypt staging environment (for testing)
    pub acme_staging: bool,
    /// Custom ACME directory URL (for non-LE CAs like ZeroSSL)
    pub acme_directory_url: Option<String>,
    /// Domains to auto-provision certificates for on startup
    pub auto_provision_domains: Vec<String>,
}

impl Default for PingoraProxyConfig {
    fn default() -> Self {
        Self {
            http_addr: "0.0.0.0:80".to_string(),
            https_addr: "0.0.0.0:443".to_string(),
            acme_email: None,
            cert_storage_path: "/var/lib/zlayer/certs".to_string(),
            acme_enabled: false,
            acme_staging: false,
            acme_directory_url: None,
            auto_provision_domains: vec![],
        }
    }
}

impl PingoraProxyConfig {
    /// Get the ACME directory URL based on configuration
    ///
    /// Returns the custom directory URL if set, otherwise returns the appropriate
    /// Let's Encrypt URL based on whether staging mode is enabled.
    pub fn acme_directory(&self) -> &str {
        match &self.acme_directory_url {
            Some(url) => url.as_str(),
            None if self.acme_staging => "https://acme-staging-v02.api.letsencrypt.org/directory",
            None => "https://acme-v02.api.letsencrypt.org/directory",
        }
    }
}

/// Error type for proxy startup failures
#[derive(Debug, thiserror::Error)]
pub enum ProxyStartError {
    /// Failed to create certificate manager
    #[error("Certificate manager error: {0}")]
    CertManager(String),
    /// Failed to create Pingora server
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
                    let key_path = storage_path.join(format!("{}.key", stem));

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
        match sni_resolver
            .load_cert(&cert_info.domain, &cert_pem, &key_pem)
            .await
        {
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

/// Start the Pingora-based proxy server
///
/// This function initializes and starts the high-performance Pingora proxy.
/// It creates the necessary infrastructure including:
/// - Certificate manager for TLS
/// - SNI resolver for dynamic certificate selection
/// - ZLayerProxy with the service registry
/// - HTTP listener on the configured address
/// - HTTPS listener (if certificates are available)
///
/// # Arguments
///
/// * `config` - Proxy configuration
/// * `service_registry` - Service registry for route resolution
///
/// # Returns
///
/// This function does not return under normal operation - it runs the server
/// forever. It returns an error only if initialization fails.
///
/// # Example
///
/// ```rust,ignore
/// use proxy::{PingoraProxyConfig, ServiceRegistry, start_proxy};
/// use std::sync::Arc;
///
/// let registry = Arc::new(ServiceRegistry::new());
/// let config = PingoraProxyConfig::default();
/// start_proxy(config, registry)?;
/// ```
///
/// # Note
///
/// This function blocks the current thread. If you need to run the proxy
/// alongside other tasks, spawn it in a separate thread or use `start_proxy_async`.
pub fn start_proxy(
    config: PingoraProxyConfig,
    service_registry: Arc<ServiceRegistry>,
) -> std::result::Result<(), ProxyStartError> {
    // We need to create the CertManager in sync context, then run the server
    // Since CertManager::new is async, we use a runtime for initialization

    // Create a minimal runtime just for initialization
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| ProxyStartError::ServerCreation(format!("Failed to create runtime: {}", e)))?;

    let cert_manager = rt.block_on(async {
        CertManager::new(config.cert_storage_path.clone(), config.acme_email.clone())
            .await
            .map_err(|e| ProxyStartError::CertManager(e.to_string()))
    })?;

    let cert_manager = Arc::new(cert_manager);

    // Create SNI resolver for dynamic certificate selection
    let sni_resolver = Arc::new(SniCertResolver::new());

    // Pre-load existing certificates into SNI resolver
    let loaded_count = rt.block_on(async {
        load_existing_certs_into_resolver(&cert_manager, &sni_resolver).await
    })?;

    tracing::info!(
        count = loaded_count,
        "Pre-loaded certificates into SNI resolver"
    );

    // Start automatic certificate renewal task if ACME is enabled
    if config.acme_enabled {
        let _renewal_handle = cert_manager
            .clone()
            .start_renewal_task(sni_resolver.clone());
        tracing::info!("Started automatic certificate renewal task");
    }

    // Discover certificates for TLS listener
    // Pingora's TLS requires file paths, so we use the first available certificate
    let storage_path = PathBuf::from(&config.cert_storage_path);
    let discovered_certs = discover_certificates(&storage_path);

    // Create Pingora server
    // Server::new takes Option<Opt> - None uses defaults
    let mut server = pingora_core::server::Server::new(None)
        .map_err(|e| ProxyStartError::ServerCreation(format!("Failed to create server: {}", e)))?;

    // Bootstrap the server (prepares for graceful upgrade, loads FDs, etc.)
    server.bootstrap();

    // Create the ZLayerProxy instance
    let zlayer_proxy = ZLayerProxy::new(service_registry, cert_manager);

    // Create the HTTP proxy service using Pingora's http_proxy_service
    let mut proxy_service = pingora_proxy::http_proxy_service(&server.configuration, zlayer_proxy);

    // Add TCP listener for HTTP
    proxy_service.add_tcp(&config.http_addr);

    // Add TLS listener for HTTPS if we have certificates
    // Note: Pingora's rustls TLS implementation uses a single certificate per listener.
    // For full SNI support with dynamic certificate selection, we would need to either:
    // 1. Modify Pingora to support custom ResolvesServerCert
    // 2. Use a terminating proxy in front that handles SNI
    // For now, we use the first discovered certificate as the default.
    let https_enabled = if let Some(first_cert) = discovered_certs.first() {
        let cert_path = first_cert.cert_path.to_string_lossy().to_string();
        let key_path = first_cert.key_path.to_string_lossy().to_string();

        tracing::info!(
            domain = %first_cert.domain,
            cert_path = %cert_path,
            "Using certificate for HTTPS listener"
        );

        // Add TLS listener with the first certificate
        // Note: Pingora's add_tls returns a Result
        match proxy_service.add_tls(&config.https_addr, &cert_path, &key_path) {
            Ok(()) => {
                tracing::info!(
                    https_addr = %config.https_addr,
                    "HTTPS listener enabled"
                );
                true
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to add HTTPS listener, continuing with HTTP only"
                );
                false
            }
        }
    } else {
        tracing::info!(
            storage_path = %config.cert_storage_path,
            "No certificates found, HTTPS listener not enabled. \
             Add certificates to the storage path and restart to enable HTTPS."
        );
        false
    };

    // Add the service to the server
    server.add_service(proxy_service);

    tracing::info!(
        http_addr = %config.http_addr,
        https_addr = %config.https_addr,
        https_enabled = https_enabled,
        cert_count = loaded_count,
        "Starting ZLayer Pingora proxy"
    );

    // Run the server forever (this blocks)
    // Note: run_forever() calls std::process::exit(0) at the end
    server.run_forever();
}

/// Start the Pingora-based proxy server asynchronously
///
/// This is an async wrapper around `start_proxy` that spawns the proxy
/// in a blocking task, allowing it to run alongside other async operations.
///
/// # Arguments
///
/// * `config` - Proxy configuration
/// * `service_registry` - Service registry for route resolution
///
/// # Returns
///
/// A `JoinHandle` that can be used to wait for the proxy to complete
/// (though under normal operation it runs forever).
///
/// # Example
///
/// ```rust,ignore
/// use proxy::{PingoraProxyConfig, ServiceRegistry, start_proxy_async};
/// use std::sync::Arc;
///
/// #[tokio::main]
/// async fn main() {
///     let registry = Arc::new(ServiceRegistry::new());
///     let config = PingoraProxyConfig::default();
///
///     let proxy_handle = start_proxy_async(config, registry);
///
///     // Do other async work...
///
///     // Wait for proxy (normally runs forever)
///     proxy_handle.await.unwrap();
/// }
/// ```
pub fn start_proxy_async(
    config: PingoraProxyConfig,
    service_registry: Arc<ServiceRegistry>,
) -> tokio::task::JoinHandle<std::result::Result<(), ProxyStartError>> {
    tokio::task::spawn_blocking(move || start_proxy(config, service_registry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pingora_proxy_config_default() {
        let config = PingoraProxyConfig::default();
        assert_eq!(config.http_addr, "0.0.0.0:80");
        assert_eq!(config.https_addr, "0.0.0.0:443");
        assert!(config.acme_email.is_none());
        assert_eq!(config.cert_storage_path, "/var/lib/zlayer/certs");
        assert!(!config.acme_enabled);
        assert!(!config.acme_staging);
        assert!(config.acme_directory_url.is_none());
        assert!(config.auto_provision_domains.is_empty());
    }

    #[test]
    fn test_pingora_proxy_config_custom() {
        let config = PingoraProxyConfig {
            http_addr: "127.0.0.1:8080".to_string(),
            https_addr: "127.0.0.1:8443".to_string(),
            acme_email: Some("admin@example.com".to_string()),
            cert_storage_path: "/tmp/certs".to_string(),
            acme_enabled: true,
            acme_staging: true,
            acme_directory_url: None,
            auto_provision_domains: vec!["example.com".to_string(), "api.example.com".to_string()],
        };
        assert_eq!(config.http_addr, "127.0.0.1:8080");
        assert_eq!(config.https_addr, "127.0.0.1:8443");
        assert_eq!(config.acme_email, Some("admin@example.com".to_string()));
        assert_eq!(config.cert_storage_path, "/tmp/certs");
        assert!(config.acme_enabled);
        assert!(config.acme_staging);
        assert!(config.acme_directory_url.is_none());
        assert_eq!(config.auto_provision_domains.len(), 2);
    }

    #[test]
    fn test_acme_directory_production() {
        let config = PingoraProxyConfig {
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
        let config = PingoraProxyConfig {
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
        let config = PingoraProxyConfig {
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
