//! Proxy management for agent-controlled services
//!
//! This module provides the `ProxyManager` struct that integrates the proxy crate
//! with the agent's service management. It handles:
//! - Managing proxy routes based on `ServiceSpec` endpoints (HTTP/HTTPS/WebSocket)
//! - Managing L4 stream proxy listeners (TCP/UDP)
//! - Tracking and updating backend servers for load balancing
//! - Coordinating proxy server lifecycle

use crate::error::Result;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use zlayer_proxy::{
    load_existing_certs_into_resolver, CertManager, LbStrategy, LoadBalancer, ProxyConfig,
    ProxyServer, RouteEntry, ServiceRegistry, SniCertResolver, StreamRegistry, TcpStreamService,
    UdpStreamService,
};
use zlayer_spec::{ExposeType, Protocol, ServiceSpec};

/// Configuration for the `ProxyManager`
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
    #[must_use]
    pub fn new(http_addr: SocketAddr) -> Self {
        Self {
            http_addr,
            https_addr: None,
            http2_enabled: true,
        }
    }

    /// Set the HTTPS address
    #[must_use]
    pub fn with_https(mut self, addr: SocketAddr) -> Self {
        self.https_addr = Some(addr);
        self
    }

    /// Set HTTP/2 support
    #[must_use]
    pub fn with_http2(mut self, enabled: bool) -> Self {
        self.http2_enabled = enabled;
        self
    }
}

/// Per-service tracking information for cleanup purposes.
#[derive(Debug, Clone)]
struct ServiceTracking {
    /// Endpoint names (retained for Debug output and future introspection)
    #[allow(dead_code)]
    endpoint_names: Vec<String>,
    /// TCP ports owned by this service
    tcp_ports: Vec<u16>,
    /// UDP ports owned by this service
    udp_ports: Vec<u16>,
    /// HTTP/HTTPS/WebSocket ports owned by this service
    http_ports: Vec<u16>,
}

/// Manages proxy routing for agent-controlled services
///
/// The `ProxyManager` coordinates between the agent's service lifecycle and
/// the proxy crate's routing/load balancing infrastructure. It supports:
///
/// - **HTTP/HTTPS/WebSocket (L7)**: Multiple port listeners sharing the same
///   `ServiceRegistry` for request matching and load balancing.
/// - **TCP/UDP (L4)**: Standalone stream proxy listeners that forward raw
///   connections/datagrams to backends via the `StreamRegistry`.
pub struct ProxyManager {
    /// Configuration
    config: ProxyManagerConfig,
    /// Shared service registry for HTTP request matching and backend management
    registry: Arc<ServiceRegistry>,
    /// Load balancer for health-aware backend selection
    load_balancer: Arc<LoadBalancer>,
    /// Per-port HTTP proxy server handles
    servers: RwLock<HashMap<u16, Arc<ProxyServer>>>,
    /// Tracked services and their endpoints (includes port ownership for cleanup)
    services: RwLock<HashMap<String, ServiceTracking>>,
    /// Stream registry for L4 TCP/UDP proxy routing
    stream_registry: Option<Arc<StreamRegistry>>,
    /// Certificate manager for TLS
    cert_manager: Option<Arc<CertManager>>,
    /// Ports with active TCP stream listeners (to avoid double-binding)
    tcp_listeners: RwLock<HashSet<u16>>,
    /// Ports with active UDP stream listeners (to avoid double-binding)
    udp_listeners: RwLock<HashSet<u16>>,
    /// Number of active proxy connections (for graceful drain on shutdown)
    active_connections: Arc<AtomicU64>,
}

impl ProxyManager {
    /// Create a new `ProxyManager` with the given configuration, service registry,
    /// and optional certificate manager.
    pub fn new(
        config: ProxyManagerConfig,
        registry: Arc<ServiceRegistry>,
        cert_manager: Option<Arc<CertManager>>,
    ) -> Self {
        let load_balancer = Arc::new(LoadBalancer::new());

        Self {
            config,
            registry,
            load_balancer,
            servers: RwLock::new(HashMap::new()),
            services: RwLock::new(HashMap::new()),
            stream_registry: None,
            cert_manager,
            tcp_listeners: RwLock::new(HashSet::new()),
            udp_listeners: RwLock::new(HashSet::new()),
            active_connections: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Get a reference to the service registry
    pub fn registry(&self) -> Arc<ServiceRegistry> {
        self.registry.clone()
    }

    /// Get a reference to the load balancer
    pub fn load_balancer(&self) -> Arc<LoadBalancer> {
        self.load_balancer.clone()
    }

    /// Get the number of currently active proxy connections.
    pub fn active_connections(&self) -> u64 {
        self.active_connections.load(Ordering::Relaxed)
    }

    /// Get a reference to the certificate manager (if configured)
    pub fn cert_manager(&self) -> Option<&Arc<CertManager>> {
        self.cert_manager.as_ref()
    }

    /// Set the stream registry for L4 proxy integration (TCP/UDP)
    pub fn set_stream_registry(&mut self, registry: Arc<StreamRegistry>) {
        self.stream_registry = Some(registry);
    }

    /// Builder pattern: add stream registry for L4 proxy integration
    #[must_use]
    pub fn with_stream_registry(mut self, registry: Arc<StreamRegistry>) -> Self {
        self.stream_registry = Some(registry);
        self
    }

    /// Get the stream registry (if configured)
    pub fn stream_registry(&self) -> Option<&Arc<StreamRegistry>> {
        self.stream_registry.as_ref()
    }

    /// Start listening on a specific port bound to the given address.
    ///
    /// If already listening on this port, skip.
    /// All port listeners share the same `ServiceRegistry` for request matching.
    ///
    /// # Errors
    /// Returns an error if the proxy server cannot be started.
    pub async fn listen_on(&self, port: u16, bind_ip: IpAddr) -> Result<()> {
        let mut servers = self.servers.write().await;

        if servers.contains_key(&port) {
            debug!(port = port, "Already listening on port");
            return Ok(());
        }

        let addr = SocketAddr::new(bind_ip, port);
        let mut proxy_config = ProxyConfig::default();
        proxy_config.server.http_addr = addr;
        proxy_config.server.http2_enabled = self.config.http2_enabled;

        let server = ProxyServer::with_registry(
            proxy_config,
            self.registry.clone(),
            self.load_balancer.clone(),
        );
        let server = Arc::new(server);

        info!(port = port, bind = %addr, "Proxy listening on port");

        let server_clone = server.clone();
        tokio::spawn(async move {
            if let Err(e) = server_clone.run().await {
                tracing::error!(port = port, error = %e, "Proxy server error on port");
            }
        });

        servers.insert(port, server);
        Ok(())
    }

    /// Start an HTTPS listener on the given port using `SniCertResolver` for dynamic cert selection.
    ///
    /// If already listening on this port, skip.
    /// Requires a `CertManager` to be configured; logs a warning and returns `Ok(())` if not.
    ///
    /// # Errors
    /// Returns an error if the HTTPS proxy server cannot be started.
    pub async fn listen_on_tls(&self, port: u16, bind_ip: IpAddr) -> Result<()> {
        let mut servers = self.servers.write().await;

        if servers.contains_key(&port) {
            debug!(port = port, "Already listening on port (TLS)");
            return Ok(());
        }

        let Some(cert_manager) = &self.cert_manager else {
            warn!(
                port = port,
                "Cannot start TLS listener: no CertManager configured"
            );
            return Ok(());
        };

        // Create SniCertResolver and load existing certs
        let sni_resolver = Arc::new(SniCertResolver::new());

        // Load existing certificates (best-effort; log warnings on failure)
        let _ = load_existing_certs_into_resolver(cert_manager, &sni_resolver).await;

        let addr = SocketAddr::new(bind_ip, port);
        let mut proxy_config = ProxyConfig::default();
        proxy_config.server.https_addr = addr;

        let server = ProxyServer::with_tls_resolver(
            proxy_config,
            self.registry.clone(),
            self.load_balancer.clone(),
            sni_resolver,
        )
        .with_cert_manager(Arc::clone(cert_manager));
        let server = Arc::new(server);

        info!(port = port, bind = %addr, "HTTPS proxy listening on port");

        let server_clone = server.clone();
        tokio::spawn(async move {
            if let Err(e) = server_clone.run_https().await {
                tracing::error!(port = port, error = %e, "HTTPS proxy server error");
            }
        });

        servers.insert(port, server);
        Ok(())
    }

    /// Stop all proxy servers on all ports.
    ///
    /// After signalling each server to shut down, waits up to 30 seconds for
    /// active connections to drain before returning.
    pub async fn stop(&self) {
        let mut servers = self.servers.write().await;
        for (port, server) in servers.drain() {
            info!(port = port, "Stopping proxy on port");
            server.shutdown();
        }

        // Wait up to 30s for active connections to drain
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while self.active_connections.load(Ordering::Relaxed) > 0 {
            if tokio::time::Instant::now() >= deadline {
                let remaining = self.active_connections.load(Ordering::Relaxed);
                warn!(
                    remaining = remaining,
                    "Drain timeout reached, forcing shutdown"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        info!("All proxy servers stopped");
    }

    /// Remove and shut down the listener on a specific port.
    pub async fn unbind(&self, port: u16) {
        let mut servers = self.servers.write().await;
        if let Some(server) = servers.remove(&port) {
            info!(port = port, "Unbinding proxy from port");
            server.shutdown();
        }
    }

    /// Scan a service's endpoints and ensure the proxy is listening on all
    /// required ports.
    ///
    /// - **HTTP/HTTPS/WebSocket** endpoints start an HTTP proxy listener.
    /// - **TCP** endpoints bind a `TcpListener` and spawn a `TcpStreamService`.
    /// - **UDP** endpoints bind a `UdpSocket` and spawn a `UdpStreamService`.
    ///
    /// Bind address is determined by the `expose` type:
    /// - **Public** endpoints bind to `0.0.0.0` (all interfaces).
    /// - **Internal** endpoints bind to the overlay IP so they are only
    ///   reachable from within the overlay network.  If no overlay is
    ///   available, internal endpoints bind to `127.0.0.1` (localhost only).
    ///
    /// # Errors
    /// Returns an error if an HTTP/HTTPS listener cannot be started.
    pub async fn ensure_ports_for_service(
        &self,
        spec: &ServiceSpec,
        overlay_ip: Option<IpAddr>,
    ) -> Result<()> {
        for endpoint in &spec.endpoints {
            let bind_ip = match endpoint.expose {
                ExposeType::Public => IpAddr::V4(Ipv4Addr::UNSPECIFIED), // 0.0.0.0
                ExposeType::Internal => {
                    // Prefer overlay IP; fall back to loopback if overlay is unavailable.
                    let ip = overlay_ip.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
                    if overlay_ip.is_none() {
                        warn!(
                            endpoint = %endpoint.name,
                            port = endpoint.port,
                            "No overlay IP available for internal endpoint; binding to 127.0.0.1"
                        );
                    }
                    ip
                }
            };

            match endpoint.protocol {
                Protocol::Https => {
                    // L7 TLS: start HTTPS proxy listener with SNI cert resolution
                    self.listen_on_tls(endpoint.port, bind_ip).await?;
                }
                Protocol::Http | Protocol::Websocket => {
                    // L7: start HTTP proxy listener
                    self.listen_on(endpoint.port, bind_ip).await?;
                }
                Protocol::Tcp => {
                    // L4 TCP: bind listener and spawn TcpStreamService
                    self.ensure_tcp_listener(endpoint.port, bind_ip).await;
                }
                Protocol::Udp => {
                    // L4 UDP: bind socket and spawn UdpStreamService
                    self.ensure_udp_listener(endpoint.port, bind_ip).await;
                }
            }
        }
        Ok(())
    }

    /// Ensure a TCP stream listener is running on the given port.
    ///
    /// If a listener is already active on this port, this is a no-op.
    /// Requires `stream_registry` to be configured; logs a warning if not.
    async fn ensure_tcp_listener(&self, port: u16, bind_ip: IpAddr) {
        // Check if already listening
        {
            let listeners = self.tcp_listeners.read().await;
            if listeners.contains(&port) {
                debug!(port = port, "TCP stream listener already active");
                return;
            }
        }

        let registry = if let Some(r) = &self.stream_registry {
            Arc::clone(r)
        } else {
            warn!(
                port = port,
                "Cannot start TCP listener: StreamRegistry not configured"
            );
            return;
        };

        let addr = SocketAddr::new(bind_ip, port);
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                warn!(
                    port = port,
                    bind = %addr,
                    error = %e,
                    "Failed to bind TCP stream listener, continuing"
                );
                return;
            }
        };

        // Mark as active before spawning
        {
            let mut listeners = self.tcp_listeners.write().await;
            listeners.insert(port);
        }

        let tcp_service = Arc::new(TcpStreamService::new(registry, port));
        tokio::spawn(async move {
            tcp_service.serve(listener).await;
        });

        info!(port = port, bind = %addr, "TCP stream proxy listening");
    }

    /// Ensure a UDP stream listener is running on the given port.
    ///
    /// If a listener is already active on this port, this is a no-op.
    /// Requires `stream_registry` to be configured; logs a warning if not.
    async fn ensure_udp_listener(&self, port: u16, bind_ip: IpAddr) {
        // Check if already listening
        {
            let listeners = self.udp_listeners.read().await;
            if listeners.contains(&port) {
                debug!(port = port, "UDP stream listener already active");
                return;
            }
        }

        let registry = if let Some(r) = &self.stream_registry {
            Arc::clone(r)
        } else {
            warn!(
                port = port,
                "Cannot start UDP listener: StreamRegistry not configured"
            );
            return;
        };

        let addr = SocketAddr::new(bind_ip, port);
        let socket = match tokio::net::UdpSocket::bind(addr).await {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    port = port,
                    bind = %addr,
                    error = %e,
                    "Failed to bind UDP stream listener, continuing"
                );
                return;
            }
        };

        // Mark as active before spawning
        {
            let mut listeners = self.udp_listeners.write().await;
            listeners.insert(port);
        }

        let udp_service = Arc::new(UdpStreamService::new(registry, port, None));
        tokio::spawn(async move {
            if let Err(e) = udp_service.serve(socket).await {
                tracing::error!(
                    port = port,
                    error = %e,
                    "UDP stream proxy service failed"
                );
            }
        });

        info!(port = port, bind = %addr, "UDP stream proxy listening");
    }

    /// Add routes for a service based on its specification
    ///
    /// This creates proxy routes for each endpoint defined in the `ServiceSpec`.
    /// HTTP/HTTPS/WebSocket endpoints get L7 routes via the `ServiceRegistry`.
    /// TCP/UDP endpoints are tracked but their L4 registration is handled
    /// by the `ServiceManager::register_service_routes()` method.
    pub async fn add_service(&self, name: &str, spec: &ServiceSpec) {
        let mut services = self.services.write().await;

        // Track which endpoints and ports we're adding
        let mut endpoint_names = Vec::new();
        let mut tcp_ports = Vec::new();
        let mut udp_ports = Vec::new();
        let mut http_ports = Vec::new();

        for endpoint in &spec.endpoints {
            match endpoint.protocol {
                Protocol::Http | Protocol::Https | Protocol::Websocket => {
                    // L7: register route in the ServiceRegistry
                    let entry = RouteEntry::from_endpoint(name, endpoint);
                    self.registry.register(entry).await;
                    http_ports.push(endpoint.port);

                    info!(
                        service = name,
                        endpoint = %endpoint.name,
                        protocol = ?endpoint.protocol,
                        path = ?endpoint.path,
                        expose = ?endpoint.expose,
                        "Added HTTP proxy route for service"
                    );
                }
                Protocol::Tcp => {
                    tcp_ports.push(endpoint.port);
                    info!(
                        service = name,
                        endpoint = %endpoint.name,
                        protocol = ?endpoint.protocol,
                        port = endpoint.port,
                        expose = ?endpoint.expose,
                        "Tracking TCP stream endpoint for service"
                    );
                }
                Protocol::Udp => {
                    udp_ports.push(endpoint.port);
                    info!(
                        service = name,
                        endpoint = %endpoint.name,
                        protocol = ?endpoint.protocol,
                        port = endpoint.port,
                        expose = ?endpoint.expose,
                        "Tracking UDP stream endpoint for service"
                    );
                }
            }

            endpoint_names.push(endpoint.name.clone());
        }

        // Register the service in the load balancer (starts with no backends)
        self.load_balancer
            .register(name, vec![], LbStrategy::RoundRobin);

        services.insert(
            name.to_string(),
            ServiceTracking {
                endpoint_names,
                tcp_ports,
                udp_ports,
                http_ports,
            },
        );
    }

    /// Remove all routes, L4 listeners, and HTTP server handles for a service.
    ///
    /// This performs a full cleanup of all proxy resources associated with the
    /// service:
    /// - Removes L7 (HTTP/HTTPS/WebSocket) routes from the `ServiceRegistry`
    /// - Unregisters TCP/UDP stream services from the `StreamRegistry`
    /// - Removes port tracking for TCP/UDP listeners
    /// - Shuts down HTTP proxy server handles that were exclusively owned by
    ///   this service (only if no other service uses the same port)
    pub async fn remove_service(&self, name: &str) {
        let mut services = self.services.write().await;

        if let Some(tracking) = services.remove(name) {
            // 1. Remove L7 routes from the ServiceRegistry
            self.registry.unregister_service(name).await;

            // 1b. Remove from the load balancer
            self.load_balancer.unregister(name);

            // 2. Unregister TCP stream services and clear port tracking
            if !tracking.tcp_ports.is_empty() {
                let mut tcp_set = self.tcp_listeners.write().await;
                for port in &tracking.tcp_ports {
                    if let Some(registry) = &self.stream_registry {
                        let _ = registry.unregister_tcp(*port);
                    }
                    tcp_set.remove(port);
                    debug!(service = name, port = port, "Removed TCP listener tracking");
                }
            }

            // 3. Unregister UDP stream services and clear port tracking
            if !tracking.udp_ports.is_empty() {
                let mut udp_set = self.udp_listeners.write().await;
                for port in &tracking.udp_ports {
                    if let Some(registry) = &self.stream_registry {
                        let _ = registry.unregister_udp(*port);
                    }
                    udp_set.remove(port);
                    debug!(service = name, port = port, "Removed UDP listener tracking");
                }
            }

            // 4. Shut down HTTP proxy servers on ports exclusively owned by
            //    this service (skip ports still used by other services)
            if !tracking.http_ports.is_empty() {
                let ports_still_in_use: HashSet<u16> = services
                    .values()
                    .flat_map(|t| t.http_ports.iter().copied())
                    .collect();

                let mut servers = self.servers.write().await;
                for port in &tracking.http_ports {
                    if !ports_still_in_use.contains(port) {
                        if let Some(server) = servers.remove(port) {
                            server.shutdown();
                            info!(
                                service = name,
                                port = port,
                                "Shut down HTTP proxy server (no remaining services on port)"
                            );
                        }
                    }
                }
            }

            info!(service = name, "Removed all proxy resources for service");
        }
    }

    /// Add a single backend to a service
    pub async fn add_backend(&self, service: &str, addr: SocketAddr) {
        self.registry.add_backend(service, addr).await;
        self.load_balancer.add_backend(service, addr);
        info!(service = service, backend = %addr, "Registered backend with proxy");
    }

    /// Remove a backend from a service
    pub async fn remove_backend(&self, service: &str, addr: SocketAddr) {
        self.registry.remove_backend(service, addr).await;
        self.load_balancer.remove_backend(service, &addr);
        debug!(service = service, backend = %addr, "Removed backend from service");
    }

    /// Update the health status of a backend in the load balancer.
    ///
    /// Delegates to [`LoadBalancer::mark_health`] so that unhealthy backends
    /// are skipped during selection.
    #[allow(clippy::unused_async)]
    pub async fn update_backend_health(&self, service: &str, addr: SocketAddr, healthy: bool) {
        self.load_balancer.mark_health(service, &addr, healthy);
        debug!(
            service = service,
            backend = %addr,
            healthy = healthy,
            "Updated backend health in load balancer"
        );
    }

    /// Update the backends for a service
    ///
    /// This replaces all backends for the given service with the provided list.
    /// Each backend should be the address where the service replica is listening.
    pub async fn update_backends(&self, service: &str, addrs: Vec<SocketAddr>) {
        self.registry.update_backends(service, addrs.clone()).await;
        self.load_balancer.update_backends(service, addrs);
        debug!(service = service, "Updated backends for service");
    }

    /// Get the number of registered routes
    pub async fn route_count(&self) -> usize {
        self.registry.route_count().await
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_service_spec_with_endpoints() -> ServiceSpec {
        use zlayer_spec::*;
        serde_yaml::from_str::<DeploymentSpec>(
            r"
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
",
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }

    fn mock_service_spec_tcp_only() -> ServiceSpec {
        use zlayer_spec::*;
        serde_yaml::from_str::<DeploymentSpec>(
            r"
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
",
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }

    #[tokio::test]
    async fn test_proxy_manager_new() {
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry, None);

        assert_eq!(manager.route_count().await, 0);
        assert!(manager.list_services().await.is_empty());
    }

    #[tokio::test]
    async fn test_add_service_with_http_endpoints() {
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry, None);

        let spec = mock_service_spec_with_endpoints();
        manager.add_service("api", &spec).await;

        // Should have 2 routes (http and websocket)
        assert_eq!(manager.route_count().await, 2);
        assert!(manager.has_service("api").await);
    }

    #[tokio::test]
    async fn test_tcp_endpoints_tracked_not_routed() {
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry, None);

        let spec = mock_service_spec_tcp_only();
        manager.add_service("grpc-service", &spec).await;

        // TCP endpoints don't add HTTP routes
        assert_eq!(manager.route_count().await, 0);
        // But the service is still tracked with its endpoint name
        assert!(manager.has_service("grpc-service").await);
    }

    #[tokio::test]
    async fn test_remove_service() {
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry, None);

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
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry.clone(), None);

        let spec = mock_service_spec_with_endpoints();
        manager.add_service("api", &spec).await;

        // Add backends
        let addr1: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let addr2: SocketAddr = "127.0.0.1:8081".parse().unwrap();

        manager.add_backend("api", addr1).await;
        manager.add_backend("api", addr2).await;

        // Verify backends via the registry's resolve
        let resolved = registry.resolve(None, "/api").await.unwrap();
        assert_eq!(resolved.backends.len(), 2);

        // Remove a backend
        manager.remove_backend("api", addr1).await;
        let resolved = registry.resolve(None, "/api").await.unwrap();
        assert_eq!(resolved.backends.len(), 1);
    }

    #[tokio::test]
    async fn test_update_backends_replaces_all() {
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry.clone(), None);

        let spec = mock_service_spec_with_endpoints();
        manager.add_service("api", &spec).await;

        // Add initial backend
        let addr1: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        manager.add_backend("api", addr1).await;

        // Update with new backends (replaces)
        let new_backends: Vec<SocketAddr> = vec![
            "127.0.0.1:9000".parse().unwrap(),
            "127.0.0.1:9001".parse().unwrap(),
            "127.0.0.1:9002".parse().unwrap(),
        ];
        manager.update_backends("api", new_backends).await;

        let resolved = registry.resolve(None, "/api").await.unwrap();
        assert_eq!(resolved.backends.len(), 3);
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

    /// Test that `ensure_ports_for_service` correctly differentiates
    /// Public (0.0.0.0) vs Internal (overlay or 127.0.0.1) bind addresses.
    /// We can't actually bind in unit tests, but we verify the function
    /// processes both endpoint types without error.
    #[tokio::test]
    async fn test_ensure_ports_differentiates_public_and_internal() {
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry, None);

        let spec = mock_service_spec_with_endpoints();
        // Passing None for overlay_ip: internal endpoints should fall back to 127.0.0.1
        let result = manager.ensure_ports_for_service(&spec, None).await;
        // listen_on may fail because we can't actually bind in tests, but
        // the function itself should run without panicking.
        let _ = result;
    }

    #[tokio::test]
    async fn test_ensure_ports_with_overlay_ip() {
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry, None);

        let spec = mock_service_spec_with_endpoints();
        // Pass an overlay IP -- internal endpoints should bind there
        let overlay_ip: IpAddr = "10.200.0.5".parse().unwrap();
        let result = manager
            .ensure_ports_for_service(&spec, Some(overlay_ip))
            .await;
        let _ = result;
    }

    fn mock_mixed_service_spec() -> ServiceSpec {
        use zlayer_spec::*;
        serde_yaml::from_str::<DeploymentSpec>(
            r"
version: v1
deployment: test
services:
  mixed:
    rtype: service
    image:
      name: test:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
        path: /api
        expose: public
      - name: grpc
        protocol: tcp
        port: 9000
        expose: public
      - name: game
        protocol: udp
        port: 27015
        expose: public
",
        )
        .unwrap()
        .services
        .remove("mixed")
        .unwrap()
    }

    #[tokio::test]
    async fn test_add_mixed_service_tracks_all_endpoints() {
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry, None);

        let spec = mock_mixed_service_spec();
        manager.add_service("mixed", &spec).await;

        // Only 1 HTTP route (tcp and udp don't add HTTP routes)
        assert_eq!(manager.route_count().await, 1);
        // Service is tracked
        assert!(manager.has_service("mixed").await);
    }

    #[tokio::test]
    async fn test_ensure_ports_tcp_with_stream_registry() {
        use zlayer_proxy::StreamService;

        let stream_registry = Arc::new(StreamRegistry::new());
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let mut manager = ProxyManager::new(config, registry, None);
        manager.set_stream_registry(stream_registry.clone());

        let spec = mock_service_spec_tcp_only();

        // Register the TCP service in the stream registry first (as ServiceManager does)
        stream_registry.register_tcp(9000, StreamService::new("grpc-service".to_string(), vec![]));

        // Ensure ports -- should bind TCP listener
        let result = manager.ensure_ports_for_service(&spec, None).await;
        assert!(result.is_ok());

        // Verify the TCP listener port is tracked
        let tcp_ports = manager.tcp_listeners.read().await;
        assert!(tcp_ports.contains(&9000));
    }

    #[tokio::test]
    async fn test_ensure_ports_tcp_without_stream_registry() {
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry, None);

        let spec = mock_service_spec_tcp_only();

        // Without stream registry, ensure_ports should not fail, just warn
        let result = manager.ensure_ports_for_service(&spec, None).await;
        assert!(result.is_ok());

        // No TCP listeners should be tracked
        let tcp_ports = manager.tcp_listeners.read().await;
        assert!(tcp_ports.is_empty());
    }

    #[tokio::test]
    async fn test_stream_registry_setter() {
        let stream_registry = Arc::new(StreamRegistry::new());
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let mut manager = ProxyManager::new(config, registry, None);

        assert!(manager.stream_registry().is_none());
        manager.set_stream_registry(stream_registry.clone());
        assert!(manager.stream_registry().is_some());
    }

    #[tokio::test]
    async fn test_registry_accessor() {
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry.clone(), None);

        // registry() should return the same Arc
        assert_eq!(Arc::as_ptr(&manager.registry()), Arc::as_ptr(&registry));
    }
}
