//! Stream service registry for L4 routing
//!
//! Maps listen ports to backend services for TCP and UDP proxying.

use dashmap::DashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A resolved stream service with backend addresses
#[derive(Clone, Debug)]
pub struct StreamService {
    /// Service name (for logging/metrics)
    pub name: String,
    /// Backend addresses for load balancing
    pub backends: Vec<SocketAddr>,
    /// Round-robin index for backend selection
    rr_index: Arc<AtomicUsize>,
}

impl StreamService {
    /// Create a new stream service
    pub fn new(name: String, backends: Vec<SocketAddr>) -> Self {
        Self {
            name,
            backends,
            rr_index: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Select next backend using round-robin
    pub fn select_backend(&self) -> Option<SocketAddr> {
        if self.backends.is_empty() {
            return None;
        }
        let idx = self.rr_index.fetch_add(1, Ordering::Relaxed);
        Some(self.backends[idx % self.backends.len()])
    }

    /// Update backend addresses (for scaling events)
    pub fn update_backends(&mut self, backends: Vec<SocketAddr>) {
        self.backends = backends;
    }

    /// Get current backend count
    pub fn backend_count(&self) -> usize {
        self.backends.len()
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
}
