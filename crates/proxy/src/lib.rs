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
}

impl Default for PingoraProxyConfig {
    fn default() -> Self {
        Self {
            http_addr: "0.0.0.0:80".to_string(),
            https_addr: "0.0.0.0:443".to_string(),
            acme_email: None,
            cert_storage_path: "/var/lib/zlayer/certs".to_string(),
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
}

/// Start the Pingora-based proxy server
///
/// This function initializes and starts the high-performance Pingora proxy.
/// It creates the necessary infrastructure including:
/// - Certificate manager for TLS
/// - ZLayerProxy with the service registry
/// - HTTP listener on the configured address
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

    // TODO: Add TLS listener for HTTPS once certificates are configured
    // This requires setting up TLS acceptor with certificates from CertManager
    // For now, only HTTP is enabled

    // Add the service to the server
    server.add_service(proxy_service);

    tracing::info!(
        http_addr = %config.http_addr,
        https_addr = %config.https_addr,
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
    }

    #[test]
    fn test_pingora_proxy_config_custom() {
        let config = PingoraProxyConfig {
            http_addr: "127.0.0.1:8080".to_string(),
            https_addr: "127.0.0.1:8443".to_string(),
            acme_email: Some("admin@example.com".to_string()),
            cert_storage_path: "/tmp/certs".to_string(),
        };
        assert_eq!(config.http_addr, "127.0.0.1:8080");
        assert_eq!(config.https_addr, "127.0.0.1:8443");
        assert_eq!(config.acme_email, Some("admin@example.com".to_string()));
        assert_eq!(config.cert_storage_path, "/tmp/certs");
    }
}
