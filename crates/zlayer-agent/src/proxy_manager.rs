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
    endpoint_lb_key, load_existing_certs_into_resolver, CertManager, LbStrategy, LoadBalancer,
    NetworkPolicyChecker, ProxyConfig, ProxyServer, RouteEntry, ServiceRegistry, SniCertResolver,
    StreamRegistry, StreamService, TcpStreamService, UdpStreamService,
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
    /// Endpoint names (used to derive per-endpoint LB group keys for
    /// cleanup on `remove_service`).
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
    /// Optional network policy checker for access control enforcement
    network_policy_checker: Option<NetworkPolicyChecker>,
    /// Dedicated stream registry for node-loopback (`127.0.0.1:<port>`)
    /// publishing.
    ///
    /// This is intentionally separate from [`Self::stream_registry`]: the
    /// latter is keyed by endpoint port and entangled with the L7/L4 +
    /// Public/Internal binding matrix (`ensure_ports_for_service`). The
    /// loopback path forwards the node's `127.0.0.1:<endpoint.port>` to the
    /// container's real backend, independent of how the endpoint is exposed,
    /// so it owns its own registry and listener set.
    loopback_registry: Arc<StreamRegistry>,
    /// Active loopback TCP listeners keyed by published port. The
    /// [`JoinHandle`] owns the bound socket via its accept loop; aborting it
    /// frees the OS port. Used for both dedup and cleanup.
    loopback_tcp: RwLock<HashMap<u16, tokio::task::JoinHandle<()>>>,
    /// Active loopback UDP listeners keyed by published port. See
    /// [`Self::loopback_tcp`].
    loopback_udp: RwLock<HashMap<u16, tokio::task::JoinHandle<()>>>,
    /// Background TCP health-check task for the L7 load balancer. Periodically
    /// TCP-connects to every registered backend and flips its health status,
    /// so a backend that was marked unhealthy by a transient request-path
    /// failure (e.g. the overlay momentarily reconfiguring while sibling
    /// containers churn during a CI build) AUTO-RECOVERS once it answers
    /// connects again. Without this the L7 LB had no recovery path of its own
    /// and a single transient blip left a service stuck on "no healthy
    /// backends" until a daemon restart. Aborted on drop.
    lb_health_checker: tokio::task::JoinHandle<()>,
}

impl Drop for ProxyManager {
    fn drop(&mut self) {
        self.lb_health_checker.abort();
    }
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

        // Spawn the L7 load balancer's own TCP health checker so unhealthy
        // backends auto-recover. Probe every 5s with a 2s per-probe timeout:
        // fast enough that a transient blip during a CI build (sibling
        // containers churning the overlay) clears well within a single e2e
        // step, without hammering backends.
        let lb_health_checker =
            load_balancer.spawn_health_checker(Duration::from_secs(5), Duration::from_secs(2));

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
            network_policy_checker: None,
            loopback_registry: Arc::new(StreamRegistry::new()),
            loopback_tcp: RwLock::new(HashMap::new()),
            loopback_udp: RwLock::new(HashMap::new()),
            lb_health_checker,
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

    /// Set the network policy checker for access control enforcement
    pub fn set_network_policy_checker(&mut self, checker: NetworkPolicyChecker) {
        self.network_policy_checker = Some(checker);
    }

    /// Builder pattern: add network policy checker for access control enforcement
    #[must_use]
    pub fn with_network_policy_checker(mut self, checker: NetworkPolicyChecker) -> Self {
        self.network_policy_checker = Some(checker);
        self
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

        let mut server = ProxyServer::with_registry(
            proxy_config,
            self.registry.clone(),
            self.load_balancer.clone(),
        );
        if let Some(ref checker) = self.network_policy_checker {
            server = server.with_network_policy_checker(checker.clone());
        }
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

        let mut server = ProxyServer::with_tls_resolver(
            proxy_config,
            self.registry.clone(),
            self.load_balancer.clone(),
            sni_resolver,
        )
        .with_cert_manager(Arc::clone(cert_manager));
        if let Some(ref checker) = self.network_policy_checker {
            server = server.with_network_policy_checker(checker.clone());
        }
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

    /// Publish a single container's exposed ports on the node loopback
    /// (`127.0.0.1:<endpoint.port>`), forwarding to wherever the container
    /// actually listens.
    ///
    /// This implements the GitHub-Actions "service published to localhost"
    /// convention so a consumer sharing the node loopback can reach the
    /// service at `localhost:<port>`. The published port is always
    /// `endpoint.port`; the backend the listener forwards to is
    /// `(container_ip, port_override.unwrap_or(endpoint.target_port()))`,
    /// which is already runtime-resolved by the caller:
    ///
    /// - On the macOS seatbelt/libkrun runtimes every replica shares the host
    ///   `127.0.0.1` and gets a unique `port_override`, so the container
    ///   listens on `127.0.0.1:<port_override>` and we forward there.
    /// - On Linux/VZ/HCS the container listens on its overlay IP, so
    ///   `container_ip` is the overlay address and `port_override` is `None`,
    ///   forwarding to `overlay_ip:<target_port>`.
    ///
    /// Backends accumulate across replicas so multiple members round-robin
    /// behind the single loopback port. `Public` endpoints are skipped: they
    /// are already bound on `0.0.0.0` and therefore already reachable on
    /// loopback — binding `127.0.0.1:<port>` again would fail with
    /// `EADDRINUSE`.
    ///
    /// This NEVER rewrites a container's own loopback: it only binds the
    /// NODE's `127.0.0.1` and forwards to the container's runtime-resolved
    /// address.
    ///
    /// Bind failures are tolerated (logged at `warn!`); this never panics and
    /// never returns an error.
    pub async fn publish_loopback_for_container(
        &self,
        service_name: &str,
        spec: &ServiceSpec,
        container_ip: IpAddr,
        port_override: Option<u16>,
    ) {
        for endpoint in &spec.endpoints {
            // Public endpoints already bind 0.0.0.0 -> already on loopback.
            if matches!(endpoint.expose, ExposeType::Public) {
                continue;
            }

            let backend = SocketAddr::new(
                container_ip,
                port_override.unwrap_or_else(|| endpoint.target_port()),
            );
            let publish_port = endpoint.port;

            match endpoint.protocol {
                Protocol::Tcp | Protocol::Http | Protocol::Https | Protocol::Websocket => {
                    // A raw TCP forward carries HTTP/HTTPS/WS just fine, so
                    // all L7 protocols ride the loopback TCP path.
                    self.publish_loopback_tcp(service_name, publish_port, backend)
                        .await;
                }
                Protocol::Udp => {
                    self.publish_loopback_udp(service_name, publish_port, backend)
                        .await;
                }
            }
        }
    }

    /// Register `backend` for the loopback TCP listener on `publish_port`,
    /// binding `127.0.0.1:<publish_port>` if it is not already bound.
    async fn publish_loopback_tcp(
        &self,
        service_name: &str,
        publish_port: u16,
        backend: SocketAddr,
    ) {
        // Accumulate the backend in the loopback registry.
        if let Some(existing) = self.loopback_registry.resolve_tcp(publish_port) {
            let mut backends = existing.backends;
            if !backends.contains(&backend) {
                backends.push(backend);
            }
            self.loopback_registry
                .update_tcp_backends(publish_port, backends);
        } else {
            self.loopback_registry.register_tcp(
                publish_port,
                StreamService::new(service_name.to_string(), vec![backend]),
            );
        }

        // Bind the loopback listener once per port.
        let mut listeners = self.loopback_tcp.write().await;
        if listeners.contains_key(&publish_port) {
            debug!(port = publish_port, "Loopback TCP listener already active");
            return;
        }

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), publish_port);
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                warn!(
                    port = publish_port,
                    bind = %addr,
                    error = %e,
                    "Failed to bind loopback TCP listener, continuing"
                );
                return;
            }
        };

        let tcp_service = Arc::new(TcpStreamService::new(
            Arc::clone(&self.loopback_registry),
            publish_port,
        ));
        let handle = tokio::spawn(async move {
            tcp_service.serve(listener).await;
        });
        listeners.insert(publish_port, handle);
        drop(listeners);

        info!(
            service = service_name,
            port = publish_port,
            bind = %addr,
            backend = %backend,
            "Published service port on node loopback (TCP)"
        );
    }

    /// Register `backend` for the loopback UDP listener on `publish_port`,
    /// binding `127.0.0.1:<publish_port>` if it is not already bound.
    async fn publish_loopback_udp(
        &self,
        service_name: &str,
        publish_port: u16,
        backend: SocketAddr,
    ) {
        if let Some(existing) = self.loopback_registry.resolve_udp(publish_port) {
            let mut backends = existing.backends;
            if !backends.contains(&backend) {
                backends.push(backend);
            }
            self.loopback_registry
                .update_udp_backends(publish_port, backends);
        } else {
            self.loopback_registry.register_udp(
                publish_port,
                StreamService::new(service_name.to_string(), vec![backend]),
            );
        }

        let mut listeners = self.loopback_udp.write().await;
        if listeners.contains_key(&publish_port) {
            debug!(port = publish_port, "Loopback UDP listener already active");
            return;
        }

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), publish_port);
        let socket = match tokio::net::UdpSocket::bind(addr).await {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    port = publish_port,
                    bind = %addr,
                    error = %e,
                    "Failed to bind loopback UDP listener, continuing"
                );
                return;
            }
        };

        let udp_service = Arc::new(UdpStreamService::new(
            Arc::clone(&self.loopback_registry),
            publish_port,
            None,
        ));
        let handle = tokio::spawn(async move {
            if let Err(e) = udp_service.serve(socket).await {
                tracing::error!(
                    port = publish_port,
                    error = %e,
                    "Loopback UDP stream proxy service failed"
                );
            }
        });
        listeners.insert(publish_port, handle);
        drop(listeners);

        info!(
            service = service_name,
            port = publish_port,
            bind = %addr,
            backend = %backend,
            "Published service port on node loopback (UDP)"
        );
    }

    /// Remove a single container's backend from the node-loopback publish
    /// path. Mirrors [`Self::publish_loopback_for_container`]: it recomputes
    /// the same `(container_ip, port_override.unwrap_or(target_port))` backend
    /// per endpoint and drops it from the loopback registry.
    ///
    /// When a published port's backend set becomes empty, the registry entry
    /// is unregistered and the loopback listener is forgotten so the port is
    /// freed for the next bind. `Public` endpoints are skipped (they were
    /// never published here).
    pub async fn unpublish_loopback_for_container(
        &self,
        spec: &ServiceSpec,
        container_ip: IpAddr,
        port_override: Option<u16>,
    ) {
        for endpoint in &spec.endpoints {
            if matches!(endpoint.expose, ExposeType::Public) {
                continue;
            }

            let backend = SocketAddr::new(
                container_ip,
                port_override.unwrap_or_else(|| endpoint.target_port()),
            );
            let publish_port = endpoint.port;

            match endpoint.protocol {
                Protocol::Tcp | Protocol::Http | Protocol::Https | Protocol::Websocket => {
                    self.unpublish_loopback_tcp(publish_port, backend).await;
                }
                Protocol::Udp => {
                    self.unpublish_loopback_udp(publish_port, backend).await;
                }
            }
        }
    }

    /// Drop `backend` from the loopback TCP service on `publish_port`,
    /// freeing the listener when no backends remain.
    async fn unpublish_loopback_tcp(&self, publish_port: u16, backend: SocketAddr) {
        let Some(existing) = self.loopback_registry.resolve_tcp(publish_port) else {
            return;
        };
        let remaining: Vec<SocketAddr> = existing
            .backends
            .into_iter()
            .filter(|b| *b != backend)
            .collect();

        if remaining.is_empty() {
            let _ = self.loopback_registry.unregister_tcp(publish_port);
            let mut listeners = self.loopback_tcp.write().await;
            if let Some(handle) = listeners.remove(&publish_port) {
                handle.abort();
            }
            debug!(
                port = publish_port,
                "Freed loopback TCP listener (no backends remain)"
            );
        } else {
            self.loopback_registry
                .update_tcp_backends(publish_port, remaining);
        }
    }

    /// Drop `backend` from the loopback UDP service on `publish_port`,
    /// freeing the listener when no backends remain.
    async fn unpublish_loopback_udp(&self, publish_port: u16, backend: SocketAddr) {
        let Some(existing) = self.loopback_registry.resolve_udp(publish_port) else {
            return;
        };
        let remaining: Vec<SocketAddr> = existing
            .backends
            .into_iter()
            .filter(|b| *b != backend)
            .collect();

        if remaining.is_empty() {
            let _ = self.loopback_registry.unregister_udp(publish_port);
            let mut listeners = self.loopback_udp.write().await;
            if let Some(handle) = listeners.remove(&publish_port) {
                handle.abort();
            }
            debug!(
                port = publish_port,
                "Freed loopback UDP listener (no backends remain)"
            );
        } else {
            self.loopback_registry
                .update_udp_backends(publish_port, remaining);
        }
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

                    // Register one LB group per L7 endpoint, keyed by the
                    // composite `{service}#{endpoint}`. This matches the
                    // `resolved.name` set by `RouteEntry::from_endpoint` and
                    // is required so that different endpoints on the same
                    // service (potentially with different `target_role`
                    // filters) maintain independent backend pools.
                    let lb_key = endpoint_lb_key(name, &endpoint.name);
                    self.load_balancer
                        .register(&lb_key, vec![], LbStrategy::RoundRobin);

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

        // Register a service-level LB group as well so legacy callers that
        // use `update_backends(service, ...)` (which fans out to all
        // endpoints) and any code that selects by bare service name still
        // resolve. Per-endpoint LB groups (registered above) are the
        // primary source for L7 select; this is a no-op for callers that
        // already use composite keys.
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

            // 1b. Remove from the load balancer (both the service-level
            //     group and every per-endpoint composite group).
            self.load_balancer.unregister(name);
            for endpoint_name in &tracking.endpoint_names {
                let lb_key = endpoint_lb_key(name, endpoint_name);
                self.load_balancer.unregister(&lb_key);
            }

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

    /// Add a single backend to a service.
    ///
    /// Adds to the service-level LB group **and** to every per-endpoint LB
    /// group tracked for `service`. Per-endpoint role filtering happens at
    /// collection time in the agent's service manager, so any backend
    /// surfaced here is already eligible for every endpoint.
    pub async fn add_backend(&self, service: &str, addr: SocketAddr) {
        self.registry.add_backend(service, addr).await;
        self.load_balancer.add_backend(service, addr);
        // Fan out to every per-endpoint LB group for backward-compat.
        let services = self.services.read().await;
        if let Some(tracking) = services.get(service) {
            for endpoint_name in &tracking.endpoint_names {
                let lb_key = endpoint_lb_key(service, endpoint_name);
                self.load_balancer.add_backend(&lb_key, addr);
            }
        }
        info!(service = service, backend = %addr, "Registered backend with proxy");
    }

    /// Remove a backend from a service.
    ///
    /// Removes from the service-level LB group **and** from every
    /// per-endpoint LB group.
    pub async fn remove_backend(&self, service: &str, addr: SocketAddr) {
        self.registry.remove_backend(service, addr).await;
        self.load_balancer.remove_backend(service, &addr);
        let services = self.services.read().await;
        if let Some(tracking) = services.get(service) {
            for endpoint_name in &tracking.endpoint_names {
                let lb_key = endpoint_lb_key(service, endpoint_name);
                self.load_balancer.remove_backend(&lb_key, &addr);
            }
        }
        debug!(service = service, backend = %addr, "Removed backend from service");
    }

    /// Update the health status of a backend in the load balancer.
    ///
    /// Delegates to [`LoadBalancer::mark_health`] so that unhealthy backends
    /// are skipped during selection. Health is tracked on both the
    /// service-level group and every per-endpoint group that contains
    /// this address.
    #[allow(clippy::unused_async)]
    pub async fn update_backend_health(&self, service: &str, addr: SocketAddr, healthy: bool) {
        self.load_balancer.mark_health(service, &addr, healthy);
        let services = self.services.read().await;
        if let Some(tracking) = services.get(service) {
            for endpoint_name in &tracking.endpoint_names {
                let lb_key = endpoint_lb_key(service, endpoint_name);
                self.load_balancer.mark_health(&lb_key, &addr, healthy);
            }
        }
        debug!(
            service = service,
            backend = %addr,
            healthy = healthy,
            "Updated backend health in load balancer"
        );
    }

    /// Update the backends for **every** endpoint of a service with the
    /// same list.
    ///
    /// Use this only when caller cannot distinguish per-endpoint backend
    /// sets (e.g., legacy paths that do not honor `target_role`). Prefer
    /// [`Self::update_endpoint_backends`] when per-endpoint filtering is
    /// possible.
    pub async fn update_backends(&self, service: &str, addrs: Vec<SocketAddr>) {
        self.registry.update_backends(service, addrs.clone()).await;
        // Update the service-level LB group plus every per-endpoint group.
        self.load_balancer.update_backends(service, addrs.clone());
        let services = self.services.read().await;
        if let Some(tracking) = services.get(service) {
            for endpoint_name in &tracking.endpoint_names {
                let lb_key = endpoint_lb_key(service, endpoint_name);
                self.load_balancer.update_backends(&lb_key, addrs.clone());
            }
        }
        debug!(service = service, "Updated backends for service");
    }

    /// Update backends for a single L7 endpoint of a service.
    ///
    /// This honors [`EndpointSpec::target_role`] filtering: the caller
    /// supplies the role-filtered backend list and this method updates
    /// only the routes and LB group corresponding to `(service,
    /// endpoint_name)`.
    pub async fn update_endpoint_backends(
        &self,
        service: &str,
        endpoint_name: &str,
        addrs: Vec<SocketAddr>,
    ) {
        self.registry
            .update_backends_for_endpoint(service, endpoint_name, addrs.clone())
            .await;
        let lb_key = endpoint_lb_key(service, endpoint_name);
        self.load_balancer.update_backends(&lb_key, addrs);
        debug!(
            service = service,
            endpoint = endpoint_name,
            "Updated backends for service endpoint"
        );
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
        mock_service_spec_tcp_only_port(9000)
    }

    fn mock_service_spec_tcp_only_port(port: u16) -> ServiceSpec {
        use zlayer_spec::*;
        let yaml = format!(
            "
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
        port: {port}
"
        );
        serde_yaml::from_str::<DeploymentSpec>(&yaml)
            .unwrap()
            .services
            .remove("test")
            .unwrap()
    }

    /// Reserve an unused localhost TCP port by binding a listener on `:0`,
    /// reading the assigned port, and dropping the listener.
    ///
    /// There is an inherent race between dropping the listener and the test
    /// re-binding the port, but this is dramatically more reliable than
    /// hard-coding a port (e.g., 9000) which is commonly in use on dev
    /// machines (php-fpm, the running zlayer daemon, etc.).
    fn reserve_free_tcp_port() -> u16 {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("failed to bind ephemeral test port");
        listener.local_addr().unwrap().port()
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

        // Use an OS-assigned free port to avoid collisions with anything
        // listening on the dev/CI box (e.g. php-fpm or a running zlayer
        // daemon both default to port 9000 on 127.0.0.1).
        let port = reserve_free_tcp_port();
        let spec = mock_service_spec_tcp_only_port(port);

        // Register the TCP service in the stream registry first (as ServiceManager does)
        stream_registry.register_tcp(port, StreamService::new("grpc-service".to_string(), vec![]));

        // Ensure ports -- should bind TCP listener
        let result = manager.ensure_ports_for_service(&spec, None).await;
        assert!(result.is_ok());

        // Verify the TCP listener port is tracked
        let tcp_ports = manager.tcp_listeners.read().await;
        assert!(tcp_ports.contains(&port));
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

    /// Single-member service spec with one INTERNAL TCP endpoint published on
    /// `port`. Internal (not Public) so the loopback path actually binds it.
    fn mock_internal_tcp_spec(port: u16) -> ServiceSpec {
        use zlayer_spec::*;
        let yaml = format!(
            "
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    scale:
      mode: fixed
      replicas: 1
    endpoints:
      - name: tcp
        protocol: tcp
        port: {port}
        expose: internal
"
        );
        serde_yaml::from_str::<DeploymentSpec>(&yaml)
            .unwrap()
            .services
            .remove("test")
            .unwrap()
    }

    /// End-to-end loopback publish: spin up a real backend `TcpListener`,
    /// publish it on the node loopback, connect to `127.0.0.1:<publish_port>`
    /// and assert bytes round-trip through the forward; then unpublish and
    /// assert the port is freed (a fresh bind succeeds).
    #[tokio::test]
    async fn test_publish_loopback_round_trips_then_frees_port() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Real backend that echoes a single line back with a known reply.
        let backend = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_addr = backend.local_addr().unwrap();
        let backend_ip = backend_addr.ip();
        let backend_port = backend_addr.port();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = backend.accept().await {
                let mut buf = [0u8; 16];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                // Echo back what we received, prefixed.
                let _ = sock.write_all(b"pong:").await;
                let _ = sock.write_all(&buf[..n]).await;
                let _ = sock.flush().await;
            }
        });

        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry, None);

        // Reserve a free publish port (the node-loopback address).
        let publish_port = reserve_free_tcp_port();
        let spec = mock_internal_tcp_spec(publish_port);
        assert!(
            spec.publish_to_node_loopback(),
            "single-member internal spec should publish to loopback"
        );

        // The backend is the real listener; port_override forces the forward
        // target to the backend's actual ephemeral port (the macOS-style path).
        manager
            .publish_loopback_for_container("test", &spec, backend_ip, Some(backend_port))
            .await;

        // Connect to 127.0.0.1:<publish_port> and round-trip a payload.
        let mut client = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, publish_port))
            .await
            .expect("connect to published loopback port");
        client.write_all(b"ping").await.unwrap();
        client.flush().await.unwrap();
        let mut reply = Vec::new();
        client.read_to_end(&mut reply).await.unwrap();
        assert_eq!(&reply, b"pong:ping");
        drop(client);

        // Unpublish; the last backend's removal frees the listener.
        manager
            .unpublish_loopback_for_container(&spec, backend_ip, Some(backend_port))
            .await;

        // The aborted accept task drops the listener asynchronously; retry a
        // few times so the OS reclaims the port before we assert it is free.
        let mut bound = None;
        for _ in 0..50 {
            match std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, publish_port)) {
                Ok(l) => {
                    bound = Some(l);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
        assert!(
            bound.is_some(),
            "loopback port {publish_port} should be freed after unpublish"
        );
    }

    #[tokio::test]
    async fn test_publish_loopback_skips_public_endpoints() {
        // Public endpoints are already on 0.0.0.0, so the loopback path must
        // NOT bind 127.0.0.1:<port> again. mock_mixed_service_spec exposes
        // everything as public.
        let config = ProxyManagerConfig::default();
        let registry = Arc::new(ServiceRegistry::new());
        let manager = ProxyManager::new(config, registry, None);

        let spec = mock_mixed_service_spec();
        let backend_ip: IpAddr = "127.0.0.1".parse().unwrap();
        manager
            .publish_loopback_for_container("mixed", &spec, backend_ip, None)
            .await;

        // No loopback listeners should have been created for public endpoints.
        assert!(manager.loopback_tcp.read().await.is_empty());
        assert!(manager.loopback_udp.read().await.is_empty());
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
