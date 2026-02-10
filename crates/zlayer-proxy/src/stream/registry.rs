//! Stream service registry for L4 routing
//!
//! Maps listen ports to backend services for TCP and UDP proxying.
//! Includes health-aware backend selection: unhealthy backends are
//! skipped during round-robin selection, with a fallback to any
//! backend if all are marked unhealthy.

use dashmap::DashMap;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Health state of a stream backend
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendHealth {
    /// Backend is reachable and accepting connections
    Healthy,
    /// Backend failed the last health probe
    Unhealthy,
    /// Health has not yet been determined (treated as healthy)
    Unknown,
}

impl BackendHealth {
    /// Returns `true` if the backend should be considered usable.
    pub fn is_usable(self) -> bool {
        matches!(self, BackendHealth::Healthy | BackendHealth::Unknown)
    }
}

/// A resolved stream service with backend addresses and health state
#[derive(Clone, Debug)]
pub struct StreamService {
    /// Service name (for logging/metrics)
    pub name: String,
    /// Backend addresses for load balancing
    pub backends: Vec<SocketAddr>,
    /// Per-backend health state
    health: Arc<RwLock<HashMap<SocketAddr, BackendHealth>>>,
    /// Round-robin index for backend selection
    rr_index: Arc<AtomicUsize>,
}

impl StreamService {
    /// Create a new stream service
    pub fn new(name: String, backends: Vec<SocketAddr>) -> Self {
        let health: HashMap<SocketAddr, BackendHealth> = backends
            .iter()
            .map(|addr| (*addr, BackendHealth::Unknown))
            .collect();
        Self {
            name,
            backends,
            health: Arc::new(RwLock::new(health)),
            rr_index: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Select next backend using round-robin, skipping unhealthy backends.
    ///
    /// Tries up to `backends.len()` candidates. If all backends are unhealthy,
    /// falls back to returning *any* backend (better than nothing).
    pub fn select_backend(&self) -> Option<SocketAddr> {
        if self.backends.is_empty() {
            return None;
        }

        let len = self.backends.len();
        let start = self.rr_index.fetch_add(1, Ordering::Relaxed);

        // Try to read health state without blocking; if the lock is held,
        // just fall through to simple round-robin.
        let health_guard = self.health.try_read();

        if let Ok(health) = health_guard {
            // First pass: find a healthy backend
            for i in 0..len {
                let idx = (start + i) % len;
                let addr = self.backends[idx];
                let status = health.get(&addr).copied().unwrap_or(BackendHealth::Unknown);
                if status.is_usable() {
                    return Some(addr);
                }
            }
        }

        // Fallback: all unhealthy or lock contention — use simple round-robin
        Some(self.backends[start % len])
    }

    /// Update backend addresses (for scaling events).
    ///
    /// New backends start with `Unknown` health; removed backends are pruned
    /// from the health map.
    pub fn update_backends(&mut self, backends: Vec<SocketAddr>) {
        // We need to block here since this is called from a &mut self context
        // (inside DashMap::get_mut), so we can use blocking write.
        let mut health = self
            .health
            .try_write()
            .unwrap_or_else(|_| {
                // In the extremely unlikely case of write contention, just proceed
                // with a fresh health map.
                tracing::warn!(service = %self.name, "Health map write contention during backend update");
                // This should never actually happen since update_backends holds &mut self
                unreachable!("update_backends requires exclusive access")
            });

        // Add new backends with Unknown health
        for addr in &backends {
            health.entry(*addr).or_insert(BackendHealth::Unknown);
        }

        // Remove backends that are no longer present
        let backend_set: std::collections::HashSet<SocketAddr> = backends.iter().copied().collect();
        health.retain(|addr, _| backend_set.contains(addr));

        self.backends = backends;
    }

    /// Set the health status of a specific backend
    pub async fn set_backend_health(&self, addr: SocketAddr, status: BackendHealth) {
        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(&addr) {
            *h = status;
        }
    }

    /// Get the health status of a specific backend
    pub async fn get_backend_health(&self, addr: SocketAddr) -> BackendHealth {
        let health = self.health.read().await;
        health.get(&addr).copied().unwrap_or(BackendHealth::Unknown)
    }

    /// Get current backend count
    pub fn backend_count(&self) -> usize {
        self.backends.len()
    }

    /// Get count of healthy (usable) backends
    pub async fn healthy_count(&self) -> usize {
        let health = self.health.read().await;
        self.backends
            .iter()
            .filter(|addr| {
                health
                    .get(addr)
                    .copied()
                    .unwrap_or(BackendHealth::Unknown)
                    .is_usable()
            })
            .count()
    }
}

/// Registry for L4 stream services
///
/// Maps listen ports to services for both TCP and UDP protocols.
#[derive(Default)]
pub struct StreamRegistry {
    /// TCP services by listen port
    tcp_services: DashMap<u16, StreamService>,
    /// UDP services by listen port
    udp_services: DashMap<u16, StreamService>,
}

impl StreamRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a TCP service for a port
    pub fn register_tcp(&self, port: u16, service: StreamService) {
        tracing::debug!(
            port = port,
            service = %service.name,
            backends = service.backend_count(),
            "Registered TCP stream service"
        );
        self.tcp_services.insert(port, service);
    }

    /// Register a UDP service for a port
    pub fn register_udp(&self, port: u16, service: StreamService) {
        tracing::debug!(
            port = port,
            service = %service.name,
            backends = service.backend_count(),
            "Registered UDP stream service"
        );
        self.udp_services.insert(port, service);
    }

    /// Resolve TCP service for a port
    pub fn resolve_tcp(&self, port: u16) -> Option<StreamService> {
        self.tcp_services.get(&port).map(|s| s.clone())
    }

    /// Resolve UDP service for a port
    pub fn resolve_udp(&self, port: u16) -> Option<StreamService> {
        self.udp_services.get(&port).map(|s| s.clone())
    }

    /// Update backends for a TCP service
    pub fn update_tcp_backends(&self, port: u16, backends: Vec<SocketAddr>) {
        if let Some(mut service) = self.tcp_services.get_mut(&port) {
            tracing::debug!(
                port = port,
                service = %service.name,
                old_count = service.backend_count(),
                new_count = backends.len(),
                "Updating TCP backends"
            );
            service.update_backends(backends);
        }
    }

    /// Update backends for a UDP service
    pub fn update_udp_backends(&self, port: u16, backends: Vec<SocketAddr>) {
        if let Some(mut service) = self.udp_services.get_mut(&port) {
            tracing::debug!(
                port = port,
                service = %service.name,
                old_count = service.backend_count(),
                new_count = backends.len(),
                "Updating UDP backends"
            );
            service.update_backends(backends);
        }
    }

    /// Remove a TCP service
    pub fn unregister_tcp(&self, port: u16) -> Option<StreamService> {
        self.tcp_services.remove(&port).map(|(_, s)| s)
    }

    /// Remove a UDP service
    pub fn unregister_udp(&self, port: u16) -> Option<StreamService> {
        self.udp_services.remove(&port).map(|(_, s)| s)
    }

    /// Get count of registered TCP services
    pub fn tcp_count(&self) -> usize {
        self.tcp_services.len()
    }

    /// Get count of registered UDP services
    pub fn udp_count(&self) -> usize {
        self.udp_services.len()
    }

    /// List all registered TCP ports
    pub fn tcp_ports(&self) -> Vec<u16> {
        self.tcp_services.iter().map(|e| *e.key()).collect()
    }

    /// List all registered UDP ports
    pub fn udp_ports(&self) -> Vec<u16> {
        self.udp_services.iter().map(|e| *e.key()).collect()
    }

    /// Spawn a background health checker that periodically probes all
    /// registered TCP backends with a connect-only health check.
    ///
    /// UDP backends are not probed (there is no reliable connectionless
    /// health check). They remain `Unknown` and are always considered usable.
    ///
    /// The task runs every `interval` and uses `timeout` for each probe.
    /// Returns a `JoinHandle` that can be used to cancel the checker.
    pub fn spawn_health_checker(
        self: &Arc<Self>,
        interval: Duration,
        timeout: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let registry = Arc::clone(self);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // Skip the first immediate tick
            ticker.tick().await;

            loop {
                ticker.tick().await;

                // Iterate all TCP services and probe each backend
                for entry in registry.tcp_services.iter() {
                    let service = entry.value().clone();
                    let backends = service.backends.clone();

                    for addr in backends {
                        let svc = service.clone();
                        let probe_timeout = timeout;

                        // Probe each backend concurrently
                        tokio::spawn(async move {
                            let result = tokio::time::timeout(
                                probe_timeout,
                                tokio::net::TcpStream::connect(addr),
                            )
                            .await;

                            let health = match result {
                                Ok(Ok(_stream)) => BackendHealth::Healthy,
                                Ok(Err(e)) => {
                                    tracing::debug!(
                                        service = %svc.name,
                                        backend = %addr,
                                        error = %e,
                                        "TCP health check failed (connect error)"
                                    );
                                    BackendHealth::Unhealthy
                                }
                                Err(_) => {
                                    tracing::debug!(
                                        service = %svc.name,
                                        backend = %addr,
                                        "TCP health check failed (timeout)"
                                    );
                                    BackendHealth::Unhealthy
                                }
                            };

                            svc.set_backend_health(addr, health).await;
                        });
                    }
                }
            }
        })
    }
}
