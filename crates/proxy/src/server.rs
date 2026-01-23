//! HTTP server implementation
//!
//! This module provides the HTTP/HTTPS server for the proxy.

use crate::config::ProxyConfig;
use crate::error::{ProxyError, Result};
use crate::routing::Router;
use crate::service::ReverseProxyService;
use crate::tls::{create_tls_acceptor, TlsServerConfig};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, error, info, warn};

/// The proxy server
pub struct ProxyServer {
    /// Server configuration
    config: Arc<ProxyConfig>,
    /// Router for request matching
    router: Arc<Router>,
    /// Shutdown signal sender
    shutdown_tx: watch::Sender<bool>,
    /// Shutdown signal receiver
    shutdown_rx: watch::Receiver<bool>,
    /// TLS acceptor for HTTPS connections
    tls_acceptor: Option<TlsAcceptor>,
}

impl ProxyServer {
    /// Create a new proxy server
    pub fn new(config: ProxyConfig, router: Router) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        Self {
            config: Arc::new(config),
            router: Arc::new(router),
            shutdown_tx,
            shutdown_rx,
            tls_acceptor: None,
        }
    }

    /// Create a proxy server with an existing router
    pub fn with_router(config: ProxyConfig, router: Arc<Router>) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        Self {
            config: Arc::new(config),
            router,
            shutdown_tx,
            shutdown_rx,
            tls_acceptor: None,
        }
    }

    /// Create a proxy server with TLS support
    pub fn with_tls(config: ProxyConfig, router: Router, tls_acceptor: TlsAcceptor) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        Self {
            config: Arc::new(config),
            router: Arc::new(router),
            shutdown_tx,
            shutdown_rx,
            tls_acceptor: Some(tls_acceptor),
        }
    }

    /// Check if TLS is enabled
    pub fn has_tls(&self) -> bool {
        self.tls_acceptor.is_some()
    }

    /// Get the TLS acceptor if configured
    pub fn tls_acceptor(&self) -> Option<&TlsAcceptor> {
        self.tls_acceptor.as_ref()
    }

    /// Get the router
    pub fn router(&self) -> Arc<Router> {
        self.router.clone()
    }

    /// Get the configuration
    pub fn config(&self) -> Arc<ProxyConfig> {
        self.config.clone()
    }

    /// Signal the server to shut down
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Run the HTTP server
    pub async fn run(&self) -> Result<()> {
        let addr = self.config.server.http_addr;
        let listener = TcpListener::bind(addr).await.map_err(|e| {
            ProxyError::BindFailed {
                addr,
                reason: e.to_string(),
            }
        })?;

        info!(addr = %addr, "HTTP proxy server listening");

        self.accept_loop(listener).await
    }

    /// Run the server on a specific address
    pub async fn run_on(&self, addr: SocketAddr) -> Result<()> {
        let listener = TcpListener::bind(addr).await.map_err(|e| {
            ProxyError::BindFailed {
                addr,
                reason: e.to_string(),
            }
        })?;

        info!(addr = %addr, "HTTP proxy server listening");

        self.accept_loop(listener).await
    }

    async fn accept_loop(&self, listener: TcpListener) -> Result<()> {
        let mut shutdown_rx = self.shutdown_rx.clone();

        loop {
            tokio::select! {
                // Check for shutdown signal
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Shutting down proxy server");
                        break;
                    }
                }

                // Accept new connections
                result = listener.accept() => {
                    match result {
                        Ok((stream, remote_addr)) => {
                            let router = self.router.clone();
                            let config = self.config.clone();

                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_connection(
                                    stream,
                                    remote_addr,
                                    router,
                                    config,
                                ).await {
                                    debug!(
                                        error = %e,
                                        remote_addr = %remote_addr,
                                        "Connection error"
                                    );
                                }
                            });
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to accept connection");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_connection(
        stream: tokio::net::TcpStream,
        remote_addr: SocketAddr,
        router: Arc<Router>,
        config: Arc<ProxyConfig>,
    ) -> Result<()> {
        let io = TokioIo::new(stream);

        let service = ReverseProxyService::new(router, config).with_remote_addr(remote_addr);

        let service = service_fn(move |req: Request<Incoming>| {
            let svc = service.clone();
            async move {
                match svc.proxy_request(req).await {
                    Ok(response) => Ok::<_, hyper::Error>(response),
                    Err(e) => {
                        error!(error = %e, "Proxy error");
                        Ok(ReverseProxyService::error_response(&e))
                    }
                }
            }
        });

        http1::Builder::new()
            .preserve_header_case(true)
            .title_case_headers(false)
            .serve_connection(io, service)
            .with_upgrades()
            .await
            .map_err(ProxyError::Hyper)?;

        Ok(())
    }

    /// Run the HTTPS server
    ///
    /// This requires TLS to be configured when creating the ProxyServer.
    pub async fn run_https(&self) -> Result<()> {
        let acceptor = self.tls_acceptor.as_ref().ok_or_else(|| {
            ProxyError::Config("TLS not configured".to_string())
        })?;

        let addr = self.config.server.https_addr;
        let listener = TcpListener::bind(addr).await.map_err(|e| {
            ProxyError::BindFailed {
                addr,
                reason: e.to_string(),
            }
        })?;

        info!(addr = %addr, "HTTPS proxy server listening");

        self.accept_loop_tls(listener, acceptor.clone()).await
    }

    /// Run the HTTPS server on a specific address
    pub async fn run_https_on(&self, addr: SocketAddr) -> Result<()> {
        let acceptor = self.tls_acceptor.as_ref().ok_or_else(|| {
            ProxyError::Config("TLS not configured".to_string())
        })?;

        let listener = TcpListener::bind(addr).await.map_err(|e| {
            ProxyError::BindFailed {
                addr,
                reason: e.to_string(),
            }
        })?;

        info!(addr = %addr, "HTTPS proxy server listening");

        self.accept_loop_tls(listener, acceptor.clone()).await
    }

    /// Run both HTTP and HTTPS servers concurrently
    ///
    /// This requires TLS to be configured when creating the ProxyServer.
    pub async fn run_both(&self) -> Result<()> {
        let http_addr = self.config.server.http_addr;
        let https_addr = self.config.server.https_addr;

        let acceptor = self.tls_acceptor.as_ref().ok_or_else(|| {
            ProxyError::Config("TLS not configured".to_string())
        })?;

        let http_listener = TcpListener::bind(http_addr).await.map_err(|e| {
            ProxyError::BindFailed {
                addr: http_addr,
                reason: e.to_string(),
            }
        })?;

        let https_listener = TcpListener::bind(https_addr).await.map_err(|e| {
            ProxyError::BindFailed {
                addr: https_addr,
                reason: e.to_string(),
            }
        })?;

        info!(http = %http_addr, https = %https_addr, "Proxy server listening");

        // Run both accept loops concurrently
        let http_future = self.accept_loop(http_listener);
        let https_future = self.accept_loop_tls(https_listener, acceptor.clone());

        tokio::select! {
            result = http_future => result,
            result = https_future => result,
        }
    }

    async fn accept_loop_tls(&self, listener: TcpListener, acceptor: TlsAcceptor) -> Result<()> {
        let mut shutdown_rx = self.shutdown_rx.clone();

        loop {
            tokio::select! {
                // Check for shutdown signal
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Shutting down HTTPS proxy server");
                        break;
                    }
                }

                // Accept new connections
                result = listener.accept() => {
                    match result {
                        Ok((stream, remote_addr)) => {
                            let router = self.router.clone();
                            let config = self.config.clone();
                            let acceptor = acceptor.clone();

                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_tls_connection(
                                    stream,
                                    remote_addr,
                                    router,
                                    config,
                                    acceptor,
                                ).await {
                                    debug!(
                                        error = %e,
                                        remote_addr = %remote_addr,
                                        "TLS connection error"
                                    );
                                }
                            });
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to accept TLS connection");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_tls_connection(
        stream: tokio::net::TcpStream,
        remote_addr: SocketAddr,
        router: Arc<Router>,
        config: Arc<ProxyConfig>,
        acceptor: TlsAcceptor,
    ) -> Result<()> {
        // Perform TLS handshake
        let tls_stream = acceptor.accept(stream).await.map_err(|e| {
            ProxyError::Tls(format!("TLS handshake failed: {}", e))
        })?;

        let io = TokioIo::new(tls_stream);

        let service = ReverseProxyService::new(router, config)
            .with_remote_addr(remote_addr)
            .with_tls(true);

        let service = service_fn(move |req: Request<Incoming>| {
            let svc = service.clone();
            async move {
                match svc.proxy_request(req).await {
                    Ok(response) => Ok::<_, hyper::Error>(response),
                    Err(e) => {
                        error!(error = %e, "Proxy error");
                        Ok(ReverseProxyService::error_response(&e))
                    }
                }
            }
        });

        http1::Builder::new()
            .preserve_header_case(true)
            .title_case_headers(false)
            .serve_connection(io, service)
            .with_upgrades()
            .await
            .map_err(ProxyError::Hyper)?;

        Ok(())
    }
}

/// Builder for ProxyServer
pub struct ProxyServerBuilder {
    config: ProxyConfig,
    router: Option<Router>,
    tls_config: Option<TlsServerConfig>,
}

impl ProxyServerBuilder {
    /// Create a new builder with default configuration
    pub fn new() -> Self {
        Self {
            config: ProxyConfig::default(),
            router: None,
            tls_config: None,
        }
    }

    /// Set the configuration
    pub fn config(mut self, config: ProxyConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the router
    pub fn router(mut self, router: Router) -> Self {
        self.router = Some(router);
        self
    }

    /// Set the HTTP bind address
    pub fn http_addr(mut self, addr: SocketAddr) -> Self {
        self.config.server.http_addr = addr;
        self
    }

    /// Set the HTTPS bind address
    pub fn https_addr(mut self, addr: SocketAddr) -> Self {
        self.config.server.https_addr = addr;
        self
    }

    /// Enable or disable HTTP/2
    pub fn http2_enabled(mut self, enabled: bool) -> Self {
        self.config.server.http2_enabled = enabled;
        self
    }

    /// Set TLS configuration
    pub fn tls(mut self, config: TlsServerConfig) -> Self {
        self.tls_config = Some(config);
        self
    }

    /// Set TLS configuration from certificate and key paths
    pub fn tls_files(mut self, cert_path: impl Into<String>, key_path: impl Into<String>) -> Self {
        self.tls_config = Some(TlsServerConfig::new(cert_path, key_path));
        self
    }

    /// Build the ProxyServer
    pub fn build(self) -> ProxyServer {
        let router = self.router.unwrap_or_default();
        ProxyServer::new(self.config, router)
    }

    /// Build the ProxyServer with TLS
    ///
    /// Returns an error if TLS configuration is invalid.
    pub fn build_with_tls(self) -> Result<ProxyServer> {
        let router = self.router.unwrap_or_default();

        let tls_acceptor = if let Some(tls_config) = self.tls_config {
            Some(create_tls_acceptor(&tls_config)?)
        } else if let Some(ref tls) = self.config.tls {
            let tls_config = TlsServerConfig::from_config(tls);
            Some(create_tls_acceptor(&tls_config)?)
        } else {
            return Err(ProxyError::Config("TLS configuration required".to_string()));
        };

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        Ok(ProxyServer {
            config: Arc::new(self.config),
            router: Arc::new(router),
            shutdown_tx,
            shutdown_rx,
            tls_acceptor,
        })
    }
}

impl Default for ProxyServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lb::{Backend, HealthStatus};
    use crate::routing::Route;
    use spec::{ExposeType, Protocol};

    #[tokio::test]
    async fn test_server_builder() {
        let server = ProxyServerBuilder::new()
            .http_addr("127.0.0.1:8080".parse().unwrap())
            .http2_enabled(true)
            .build();

        assert_eq!(
            server.config.server.http_addr,
            "127.0.0.1:8080".parse::<SocketAddr>().unwrap()
        );
        assert!(server.config.server.http2_enabled);
    }

    #[tokio::test]
    async fn test_server_shutdown() {
        let server = ProxyServerBuilder::new()
            .http_addr("127.0.0.1:0".parse().unwrap())
            .build();

        // Create a separate handle for shutdown
        let shutdown_tx = server.shutdown_tx.clone();

        // Signal shutdown immediately
        let _ = shutdown_tx.send(true);

        // Server should exit gracefully
        // (In a real test, we'd spawn the server and verify it stops)
    }

    #[tokio::test]
    async fn test_router_integration() {
        let router = Router::new();

        // Add a route
        router
            .add_route(Route {
                service: "test-service".to_string(),
                endpoint: "http".to_string(),
                host: None,
                path_prefix: "/api".to_string(),
                protocol: Protocol::Http,
                expose: ExposeType::Public,
                strip_prefix: false,
            })
            .await;

        // Add a backend
        let lb = router.get_or_create_lb("test-service").await;
        lb.add_backend(Backend::new("127.0.0.1:8081".parse().unwrap()))
            .await;
        lb.update_health(
            "127.0.0.1:8081".parse().unwrap(),
            HealthStatus::Healthy,
        )
        .await;

        let server = ProxyServerBuilder::new().router(router).build();

        // Verify router is accessible
        let router = server.router();
        assert_eq!(router.route_count().await, 1);
    }
}
