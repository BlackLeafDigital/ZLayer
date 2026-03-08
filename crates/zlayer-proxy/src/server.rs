//! HTTP server implementation
//!
//! This module provides the HTTP/HTTPS server for the proxy.
//! Uses `ServiceRegistry` for route resolution instead of the legacy `Router`.

use crate::acme::CertManager;
use crate::config::ProxyConfig;
use crate::error::{ProxyError, Result};
use crate::lb::LoadBalancer;
use crate::routes::ServiceRegistry;
use crate::service::ReverseProxyService;
use crate::sni_resolver::SniCertResolver;
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
    /// Service registry for route resolution
    registry: Arc<ServiceRegistry>,
    /// Load balancer for backend selection
    load_balancer: Arc<LoadBalancer>,
    /// Shutdown signal sender
    shutdown_tx: watch::Sender<bool>,
    /// Shutdown signal receiver
    shutdown_rx: watch::Receiver<bool>,
    /// TLS acceptor for HTTPS connections
    tls_acceptor: Option<TlsAcceptor>,
    /// Certificate manager for ACME challenge responses
    cert_manager: Option<Arc<CertManager>>,
}

impl ProxyServer {
    /// Create a new proxy server
    pub fn new(
        config: ProxyConfig,
        registry: Arc<ServiceRegistry>,
        load_balancer: Arc<LoadBalancer>,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        Self {
            config: Arc::new(config),
            registry,
            load_balancer,
            shutdown_tx,
            shutdown_rx,
            tls_acceptor: None,
            cert_manager: None,
        }
    }

    /// Create a proxy server with an existing registry (alias for `new`)
    pub fn with_registry(
        config: ProxyConfig,
        registry: Arc<ServiceRegistry>,
        load_balancer: Arc<LoadBalancer>,
    ) -> Self {
        Self::new(config, registry, load_balancer)
    }

    /// Create a proxy server with TLS via SNI resolver
    pub fn with_tls_resolver(
        config: ProxyConfig,
        registry: Arc<ServiceRegistry>,
        load_balancer: Arc<LoadBalancer>,
        resolver: Arc<SniCertResolver>,
    ) -> Self {
        let tls_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(resolver);
        let acceptor = TlsAcceptor::from(Arc::new(tls_config));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        Self {
            config: Arc::new(config),
            registry,
            load_balancer,
            shutdown_tx,
            shutdown_rx,
            tls_acceptor: Some(acceptor),
            cert_manager: None,
        }
    }

    /// Set the certificate manager for ACME challenge interception
    #[must_use]
    pub fn with_cert_manager(mut self, cm: Arc<CertManager>) -> Self {
        self.cert_manager = Some(cm);
        self
    }

    /// Check if TLS is enabled
    #[must_use]
    pub fn has_tls(&self) -> bool {
        self.tls_acceptor.is_some()
    }

    /// Get the TLS acceptor if configured
    #[must_use]
    pub fn tls_acceptor(&self) -> Option<&TlsAcceptor> {
        self.tls_acceptor.as_ref()
    }

    /// Get the service registry
    #[must_use]
    pub fn registry(&self) -> Arc<ServiceRegistry> {
        self.registry.clone()
    }

    /// Get the configuration
    #[must_use]
    pub fn config(&self) -> Arc<ProxyConfig> {
        self.config.clone()
    }

    /// Signal the server to shut down
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Run the HTTP server
    ///
    /// # Errors
    ///
    /// Returns an error if binding to the configured HTTP address fails
    /// or if the accept loop encounters a fatal error.
    pub async fn run(&self) -> Result<()> {
        let addr = self.config.server.http_addr;
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| ProxyError::BindFailed {
                addr,
                reason: e.to_string(),
            })?;

        info!(addr = %addr, "HTTP proxy server listening");

        self.accept_loop(listener).await
    }

    /// Run the server on a specific address
    ///
    /// # Errors
    ///
    /// Returns an error if binding to the given address fails
    /// or if the accept loop encounters a fatal error.
    pub async fn run_on(&self, addr: SocketAddr) -> Result<()> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| ProxyError::BindFailed {
                addr,
                reason: e.to_string(),
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
                            let registry = self.registry.clone();
                            let load_balancer = self.load_balancer.clone();
                            let config = self.config.clone();
                            let cert_manager = self.cert_manager.clone();

                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_connection(
                                    stream,
                                    remote_addr,
                                    registry,
                                    load_balancer,
                                    config,
                                    cert_manager,
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
        registry: Arc<ServiceRegistry>,
        load_balancer: Arc<LoadBalancer>,
        config: Arc<ProxyConfig>,
        cert_manager: Option<Arc<CertManager>>,
    ) -> Result<()> {
        let io = TokioIo::new(stream);

        let mut service =
            ReverseProxyService::new(registry, load_balancer, config).with_remote_addr(remote_addr);
        if let Some(cm) = cert_manager {
            service = service.with_cert_manager(cm);
        }

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
    /// This requires TLS to be configured when creating the `ProxyServer`.
    ///
    /// # Errors
    ///
    /// Returns an error if TLS is not configured, if binding to the
    /// configured HTTPS address fails, or if the accept loop encounters a fatal error.
    pub async fn run_https(&self) -> Result<()> {
        let acceptor = self
            .tls_acceptor
            .as_ref()
            .ok_or_else(|| ProxyError::Config("TLS not configured".to_string()))?;

        let addr = self.config.server.https_addr;
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| ProxyError::BindFailed {
                addr,
                reason: e.to_string(),
            })?;

        info!(addr = %addr, "HTTPS proxy server listening");

        self.accept_loop_tls(listener, acceptor.clone()).await
    }

    /// Run the HTTPS server on a specific address
    ///
    /// # Errors
    ///
    /// Returns an error if TLS is not configured, if binding to the
    /// given address fails, or if the accept loop encounters a fatal error.
    pub async fn run_https_on(&self, addr: SocketAddr) -> Result<()> {
        let acceptor = self
            .tls_acceptor
            .as_ref()
            .ok_or_else(|| ProxyError::Config("TLS not configured".to_string()))?;

        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| ProxyError::BindFailed {
                addr,
                reason: e.to_string(),
            })?;

        info!(addr = %addr, "HTTPS proxy server listening");

        self.accept_loop_tls(listener, acceptor.clone()).await
    }

    /// Run both HTTP and HTTPS servers concurrently
    ///
    /// This requires TLS to be configured when creating the `ProxyServer`.
    ///
    /// # Errors
    ///
    /// Returns an error if TLS is not configured, if binding to either
    /// the HTTP or HTTPS address fails, or if either accept loop encounters
    /// a fatal error.
    #[allow(clippy::similar_names)]
    pub async fn run_both(&self) -> Result<()> {
        let http_addr = self.config.server.http_addr;
        let https_addr = self.config.server.https_addr;

        let acceptor = self
            .tls_acceptor
            .as_ref()
            .ok_or_else(|| ProxyError::Config("TLS not configured".to_string()))?;

        let http_listener =
            TcpListener::bind(http_addr)
                .await
                .map_err(|e| ProxyError::BindFailed {
                    addr: http_addr,
                    reason: e.to_string(),
                })?;

        let https_listener =
            TcpListener::bind(https_addr)
                .await
                .map_err(|e| ProxyError::BindFailed {
                    addr: https_addr,
                    reason: e.to_string(),
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
                            let registry = self.registry.clone();
                            let load_balancer = self.load_balancer.clone();
                            let config = self.config.clone();
                            let acceptor = acceptor.clone();
                            let cert_manager = self.cert_manager.clone();

                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_tls_connection(
                                    stream,
                                    remote_addr,
                                    registry,
                                    load_balancer,
                                    config,
                                    acceptor,
                                    cert_manager,
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
        registry: Arc<ServiceRegistry>,
        load_balancer: Arc<LoadBalancer>,
        config: Arc<ProxyConfig>,
        acceptor: TlsAcceptor,
        cert_manager: Option<Arc<CertManager>>,
    ) -> Result<()> {
        // Perform TLS handshake
        let tls_stream = acceptor
            .accept(stream)
            .await
            .map_err(|e| ProxyError::Tls(format!("TLS handshake failed: {e}")))?;

        let io = TokioIo::new(tls_stream);

        let mut service = ReverseProxyService::new(registry, load_balancer, config)
            .with_remote_addr(remote_addr)
            .with_tls(true);
        if let Some(cm) = cert_manager {
            service = service.with_cert_manager(cm);
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lb::LoadBalancer;
    use crate::routes::{ResolvedService, RouteEntry};
    use zlayer_spec::{ExposeType, Protocol};

    /// Helper to build a minimal `RouteEntry` for tests.
    fn make_entry(
        service: &str,
        host: Option<&str>,
        path: &str,
        backends: Vec<SocketAddr>,
    ) -> RouteEntry {
        RouteEntry {
            service_name: service.to_string(),
            endpoint_name: "http".to_string(),
            host: host.map(std::string::ToString::to_string),
            path_prefix: path.to_string(),
            resolved: ResolvedService {
                name: service.to_string(),
                backends,
                use_tls: false,
                sni_hostname: String::new(),
                expose: ExposeType::Public,
                protocol: Protocol::Http,
                strip_prefix: false,
                path_prefix: path.to_string(),
                target_port: 8080,
            },
        }
    }

    #[tokio::test]
    async fn test_server_shutdown() {
        let registry = Arc::new(ServiceRegistry::new());
        let lb = Arc::new(LoadBalancer::new());
        let server = ProxyServer::new(ProxyConfig::default(), registry, lb);

        // Create a separate handle for shutdown
        let shutdown_tx = server.shutdown_tx.clone();

        // Signal shutdown immediately
        let _ = shutdown_tx.send(true);

        // Server should exit gracefully
        // (In a real test, we'd spawn the server and verify it stops)
    }

    #[tokio::test]
    async fn test_registry_integration() {
        let registry = Arc::new(ServiceRegistry::new());

        // Add a route
        registry
            .register(make_entry(
                "test-service",
                None,
                "/api",
                vec!["127.0.0.1:8081".parse().unwrap()],
            ))
            .await;

        let lb = Arc::new(LoadBalancer::new());
        let server = ProxyServer::new(ProxyConfig::default(), registry, lb);

        // Verify registry is accessible
        let reg = server.registry();
        assert_eq!(reg.route_count().await, 1);
    }
}
