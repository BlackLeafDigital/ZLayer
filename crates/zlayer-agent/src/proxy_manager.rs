//! Proxy management for agent-controlled services
//!
//! This module provides the `ProxyManager` struct that integrates the proxy crate
//! with the agent's service management. It handles:
//! - Managing proxy routes based on ServiceSpec endpoints
//! - Tracking and updating backend servers for load balancing
//! - Coordinating proxy server lifecycle

use crate::error::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};
use zlayer_proxy::{Backend, HealthStatus, ProxyConfig, ProxyServer, Route, Router};
use zlayer_spec::{ExposeType, Protocol, ServiceSpec};

/// Configuration for the ProxyManager
#[derive(Debug, Clone)]
pub struct ProxyManagerConfig {
    /// HTTP bind address
    pub http_addr: SocketAddr,
    /// HTTPS bind address (optional)
    pub https_addr: Option<SocketAddr>,
    /// Whether to enable HTTP/2
    pub http2_enabled: bool,
}

impl Default for ProxyManagerConfig {
    fn default() -> Self {
        Self {
            http_addr: "0.0.0.0:80".parse().unwrap(),
            https_addr: None,
            http2_enabled: true,
        }
    }
}

impl ProxyManagerConfig {
    /// Create a new configuration with the specified HTTP address
    pub fn new(http_addr: SocketAddr) -> Self {
        Self {
            http_addr,
            https_addr: None,
            http2_enabled: true,
        }
    }

    /// Set the HTTPS address
    pub fn with_https(mut self, addr: SocketAddr) -> Self {
        self.https_addr = Some(addr);
        self
    }

    /// Set HTTP/2 support
    pub fn with_http2(mut self, enabled: bool) -> Self {
        self.http2_enabled = enabled;
        self
    }
}

/// Manages proxy routing for agent-controlled services
///
/// The `ProxyManager` coordinates between the agent's service lifecycle and
/// the proxy crate's routing/load balancing infrastructure. It supports listening
/// on multiple ports simultaneously, with all port listeners sharing the same
/// router for request matching and load balancing.
pub struct ProxyManager {
    /// Configuration
    config: ProxyManagerConfig,
    /// Shared router for request matching
    router: Arc<Router>,
    /// Per-port proxy server handles
    servers: RwLock<HashMap<u16, Arc<ProxyServer>>>,
    /// Tracked services and their endpoints
    services: RwLock<HashMap<String, Vec<String>>>,
}

impl ProxyManager {
    /// Create a new ProxyManager with the given configuration
    pub fn new(config: ProxyManagerConfig) -> Self {
        Self {
            config,
            router: Arc::new(Router::new()),
            servers: RwLock::new(HashMap::new()),
            services: RwLock::new(HashMap::new()),
        }
    }

    /// Get a reference to the router
    pub fn router(&self) -> Arc<Router> {
        self.router.clone()
    }

    /// Start listening on a specific port. If already listening on this port, skip.
    /// All port listeners share the same Router for request matching.
    pub async fn listen_on(&self, port: u16) -> Result<()> {
        let mut servers = self.servers.write().await;

        if servers.contains_key(&port) {
            debug!(port = port, "Already listening on port");
            return Ok(());
        }

        let addr: SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();
        let mut proxy_config = ProxyConfig::default();
        proxy_config.server.http_addr = addr;
        proxy_config.server.http2_enabled = self.config.http2_enabled;

        let server = ProxyServer::with_router(proxy_config, self.router.clone());
        let server = Arc::new(server);

        info!(port = port, "Proxy listening on port");

        let server_clone = server.clone();
        tokio::spawn(async move {
            if let Err(e) = server_clone.run().await {
                tracing::error!(port = port, error = %e, "Proxy server error on port");
            }
        });

        servers.insert(port, server);
        Ok(())
    }

    /// Stop all proxy servers on all ports
    pub async fn stop(&self) {
        let mut servers = self.servers.write().await;
        for (port, server) in servers.drain() {
            info!(port = port, "Stopping proxy on port");
            server.shutdown();
        }
    }

    /// Scan a service's endpoints and ensure the proxy is listening on all
    /// ports that have `expose: public` or `expose: internal`.
    pub async fn ensure_ports_for_service(&self, spec: &ServiceSpec) -> Result<()> {
        for endpoint in &spec.endpoints {
            if endpoint.expose == ExposeType::Public || endpoint.expose == ExposeType::Internal {
                self.listen_on(endpoint.port).await?;
            }
        }
        Ok(())
    }

    /// Add routes for a service based on its specification
    ///
    /// This creates proxy routes for each endpoint defined in the ServiceSpec.
    /// HTTP/HTTPS/WebSocket endpoints will be routed based on path prefix.
    pub async fn add_service(&self, name: &str, spec: &ServiceSpec) {
        let mut services = self.services.write().await;

        // Track which endpoints we're adding
        let mut endpoint_names = Vec::new();

        for endpoint in &spec.endpoints {
            // Only add HTTP-compatible endpoints
            if !is_http_compatible(endpoint.protocol) {
                debug!(
                    service = name,
                    endpoint = %endpoint.name,
                    protocol = ?endpoint.protocol,
                    "Skipping non-HTTP endpoint"
                );
                continue;
            }

            let route = Route::from_endpoint(name, endpoint);

            self.router.add_route(route).await;
            endpoint_names.push(endpoint.name.clone());

            info!(
                service = name,
                endpoint = %endpoint.name,
                path = ?endpoint.path,
                expose = ?endpoint.expose,
                "Added proxy route for service"
            );
        }

        // Ensure load balancer exists for this service
        if !endpoint_names.is_empty() {
            let _ = self.router.get_or_create_lb(name).await;
        }

        services.insert(name.to_string(), endpoint_names);
    }

    /// Remove all routes for a service
    pub async fn remove_service(&self, name: &str) {
        let mut services = self.services.write().await;

        if services.remove(name).is_some() {
            self.router.remove_service_routes(name).await;
            info!(service = name, "Removed proxy routes for service");
        }
    }

    /// Update the backends for a service
    ///
    /// This replaces all backends for the given service with the provided list.
    /// Each backend should be the address where the service replica is listening.
    pub async fn update_backends(&self, service: &str, backends: Vec<Backend>) {
        let lb = self.router.get_or_create_lb(service).await;

        debug!(
            service = service,
            count = backends.len(),
            "Updating backends for service"
        );

        lb.set_backends(backends).await;
    }

    /// Add a single backend to a service
    pub async fn add_backend(&self, service: &str, addr: SocketAddr) {
        let lb = self.router.get_or_create_lb(service).await;

        let backend = Backend::new(addr);
        lb.add_backend(backend).await;

        debug!(
            service = service,
            backend = %addr,
            "Added backend to service"
        );
    }

    /// Remove a backend from a service
    pub async fn remove_backend(&self, service: &str, addr: SocketAddr) {
        if let Some(lb) = self.router.get_lb(service).await {
            lb.remove_backend(addr).await;

            debug!(
                service = service,
                backend = %addr,
                "Removed backend from service"
            );
        }
    }

    /// Update the health status of a backend
    pub async fn update_backend_health(&self, service: &str, addr: SocketAddr, healthy: bool) {
        if let Some(lb) = self.router.get_lb(service).await {
            let status = if healthy {
                HealthStatus::Healthy
            } else {
                HealthStatus::Unhealthy
            };

            lb.update_health(addr, status).await;

            debug!(
                service = service,
                backend = %addr,
                healthy = healthy,
                "Updated backend health"
            );
        }
    }

    /// Get the number of registered routes
    pub async fn route_count(&self) -> usize {
        self.router.route_count().await
    }

    /// Get the list of registered service names
    pub async fn list_services(&self) -> Vec<String> {
        self.services.read().await.keys().cloned().collect()
    }

    /// Check if a service has any registered endpoints
    pub async fn has_service(&self, name: &str) -> bool {
        self.services.read().await.contains_key(name)
    }
}

/// Check if a protocol is HTTP-compatible (can be proxied over HTTP)
fn is_http_compatible(protocol: Protocol) -> bool {
    matches!(
        protocol,
        Protocol::Http | Protocol::Https | Protocol::Websocket
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_service_spec_with_endpoints() -> ServiceSpec {
        use zlayer_spec::*;
        serde_yaml::from_str::<DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
        path: /api
        expose: public
      - name: websocket
        protocol: websocket
        port: 8081
        path: /ws
        expose: internal
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }

    fn mock_service_spec_tcp_only() -> ServiceSpec {
        use zlayer_spec::*;
        serde_yaml::from_str::<DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    endpoints:
      - name: grpc
        protocol: tcp
        port: 9000
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }

    #[tokio::test]
    async fn test_proxy_manager_new() {
        let config = ProxyManagerConfig::default();
        let manager = ProxyManager::new(config);

        assert_eq!(manager.route_count().await, 0);
        assert!(manager.list_services().await.is_empty());
    }

    #[tokio::test]
    async fn test_add_service_with_http_endpoints() {
        let config = ProxyManagerConfig::default();
        let manager = ProxyManager::new(config);

        let spec = mock_service_spec_with_endpoints();
        manager.add_service("api", &spec).await;

        // Should have 2 routes (http and websocket)
        assert_eq!(manager.route_count().await, 2);
        assert!(manager.has_service("api").await);
    }

    #[tokio::test]
    async fn test_skip_tcp_endpoints() {
        let config = ProxyManagerConfig::default();
        let manager = ProxyManager::new(config);

        let spec = mock_service_spec_tcp_only();
        manager.add_service("grpc-service", &spec).await;

        // TCP endpoints are skipped
        assert_eq!(manager.route_count().await, 0);
        // Service is still tracked (even with no routes)
        assert!(manager.has_service("grpc-service").await);
    }

    #[tokio::test]
    async fn test_remove_service() {
        let config = ProxyManagerConfig::default();
        let manager = ProxyManager::new(config);

        let spec = mock_service_spec_with_endpoints();
        manager.add_service("api", &spec).await;
        assert_eq!(manager.route_count().await, 2);

        manager.remove_service("api").await;
        assert_eq!(manager.route_count().await, 0);
        assert!(!manager.has_service("api").await);
    }

    #[tokio::test]
    async fn test_backend_management() {
        let config = ProxyManagerConfig::default();
        let manager = ProxyManager::new(config);

        let spec = mock_service_spec_with_endpoints();
        manager.add_service("api", &spec).await;

        // Add backends
        let addr1: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let addr2: SocketAddr = "127.0.0.1:8081".parse().unwrap();

        manager.add_backend("api", addr1).await;
        manager.add_backend("api", addr2).await;

        let lb = manager.router.get_lb("api").await.unwrap();
        assert_eq!(lb.backend_count().await, 2);

        // Remove a backend
        manager.remove_backend("api", addr1).await;
        assert_eq!(lb.backend_count().await, 1);
    }

    #[tokio::test]
    async fn test_update_backends_replaces_all() {
        let config = ProxyManagerConfig::default();
        let manager = ProxyManager::new(config);

        let spec = mock_service_spec_with_endpoints();
        manager.add_service("api", &spec).await;

        // Add initial backend
        let addr1: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        manager.add_backend("api", addr1).await;

        // Update with new backends (replaces)
        let new_backends = vec![
            Backend::new("127.0.0.1:9000".parse().unwrap()),
            Backend::new("127.0.0.1:9001".parse().unwrap()),
            Backend::new("127.0.0.1:9002".parse().unwrap()),
        ];
        manager.update_backends("api", new_backends).await;

        let lb = manager.router.get_lb("api").await.unwrap();
        assert_eq!(lb.backend_count().await, 3);
    }

    #[tokio::test]
    async fn test_health_update() {
        let config = ProxyManagerConfig::default();
        let manager = ProxyManager::new(config);

        let spec = mock_service_spec_with_endpoints();
        manager.add_service("api", &spec).await;

        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        manager.add_backend("api", addr).await;

        // Mark as unhealthy
        manager.update_backend_health("api", addr, false).await;

        let lb = manager.router.get_lb("api").await.unwrap();
        // healthy_count only returns backends with Healthy or Unknown status
        // After setting unhealthy, it should be 0
        assert_eq!(lb.healthy_count().await, 0);

        // Mark as healthy
        manager.update_backend_health("api", addr, true).await;
        assert_eq!(lb.healthy_count().await, 1);
    }

    #[tokio::test]
    async fn test_config_builder() {
        let config = ProxyManagerConfig::new("0.0.0.0:8080".parse().unwrap())
            .with_https("0.0.0.0:8443".parse().unwrap())
            .with_http2(false);

        assert_eq!(
            config.http_addr,
            "0.0.0.0:8080".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            config.https_addr,
            Some("0.0.0.0:8443".parse::<SocketAddr>().unwrap())
        );
        assert!(!config.http2_enabled);
    }
}
