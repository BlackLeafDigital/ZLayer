//! Load balancing implementation
//!
//! This module provides load balancing for distributing requests across backends.

use crate::error::{ProxyError, Result};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Health status of a backend
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Backend is healthy and can receive traffic
    Healthy,
    /// Backend is unhealthy and should not receive traffic
    Unhealthy,
    /// Backend health is unknown (not yet checked)
    Unknown,
}

/// A backend server
#[derive(Debug, Clone)]
pub struct Backend {
    /// Backend address
    pub addr: SocketAddr,
    /// Current health status
    pub health: HealthStatus,
    /// Weight for weighted load balancing (higher = more traffic)
    pub weight: u32,
    /// Active connections count
    pub active_connections: Arc<AtomicUsize>,
}

impl Backend {
    /// Create a new backend with default settings
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            health: HealthStatus::Unknown,
            weight: 1,
            active_connections: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Create a new backend with a specific weight
    pub fn with_weight(addr: SocketAddr, weight: u32) -> Self {
        Self {
            addr,
            health: HealthStatus::Unknown,
            weight,
            active_connections: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Check if the backend is available for traffic
    pub fn is_available(&self) -> bool {
        matches!(self.health, HealthStatus::Healthy | HealthStatus::Unknown)
    }

    /// Increment active connection count
    pub fn inc_connections(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement active connection count
    pub fn dec_connections(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get current active connection count
    pub fn connection_count(&self) -> usize {
        self.active_connections.load(Ordering::Relaxed)
    }
}

/// Load balancing algorithm
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LoadBalancerAlgorithm {
    /// Round-robin selection
    #[default]
    RoundRobin,
    /// Least connections
    LeastConnections,
    /// Random selection
    Random,
}


/// Load balancer for distributing requests across backends
#[derive(Debug)]
pub struct LoadBalancer {
    /// Service name (for error messages)
    service: String,
    /// Available backends
    backends: RwLock<Vec<Backend>>,
    /// Current index for round-robin
    current_index: AtomicUsize,
    /// Load balancing algorithm
    algorithm: LoadBalancerAlgorithm,
}

impl LoadBalancer {
    /// Create a new load balancer
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            backends: RwLock::new(Vec::new()),
            current_index: AtomicUsize::new(0),
            algorithm: LoadBalancerAlgorithm::default(),
        }
    }

    /// Create a new load balancer with a specific algorithm
    pub fn with_algorithm(service: impl Into<String>, algorithm: LoadBalancerAlgorithm) -> Self {
        Self {
            service: service.into(),
            backends: RwLock::new(Vec::new()),
            current_index: AtomicUsize::new(0),
            algorithm,
        }
    }

    /// Add a backend to the load balancer
    pub async fn add_backend(&self, backend: Backend) {
        let mut backends = self.backends.write().await;
        // Check if backend already exists
        if !backends.iter().any(|b| b.addr == backend.addr) {
            backends.push(backend);
        }
    }

    /// Remove a backend from the load balancer
    pub async fn remove_backend(&self, addr: SocketAddr) {
        let mut backends = self.backends.write().await;
        backends.retain(|b| b.addr != addr);
    }

    /// Update the health status of a backend
    pub async fn update_health(&self, addr: SocketAddr, health: HealthStatus) {
        let mut backends = self.backends.write().await;
        if let Some(backend) = backends.iter_mut().find(|b| b.addr == addr) {
            backend.health = health;
        }
    }

    /// Set all backends at once (replaces existing)
    pub async fn set_backends(&self, backends: Vec<Backend>) {
        let mut current = self.backends.write().await;
        *current = backends;
    }

    /// Get the number of backends
    pub async fn backend_count(&self) -> usize {
        self.backends.read().await.len()
    }

    /// Get the number of healthy backends
    pub async fn healthy_count(&self) -> usize {
        self.backends
            .read()
            .await
            .iter()
            .filter(|b| b.is_available())
            .count()
    }

    /// Select the next backend for a request
    pub async fn select(&self) -> Result<Backend> {
        let backends = self.backends.read().await;
        let available: Vec<_> = backends.iter().filter(|b| b.is_available()).collect();

        if available.is_empty() {
            return Err(ProxyError::NoHealthyBackends {
                service: self.service.clone(),
            });
        }

        let selected = match self.algorithm {
            LoadBalancerAlgorithm::RoundRobin => self.select_round_robin(&available),
            LoadBalancerAlgorithm::LeastConnections => self.select_least_connections(&available),
            LoadBalancerAlgorithm::Random => self.select_random(&available),
        };

        Ok(selected.clone())
    }

    fn select_round_robin<'a>(&self, available: &[&'a Backend]) -> &'a Backend {
        let index = self.current_index.fetch_add(1, Ordering::Relaxed) % available.len();
        available[index]
    }

    fn select_least_connections<'a>(&self, available: &[&'a Backend]) -> &'a Backend {
        available
            .iter()
            .min_by_key(|b| b.connection_count())
            .unwrap()
    }

    fn select_random<'a>(&self, available: &[&'a Backend]) -> &'a Backend {
        // Simple random using current time nanos
        let index = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as usize)
            .unwrap_or(0)
            % available.len();
        available[index]
    }
}

/// RAII guard for tracking backend connections
pub struct ConnectionGuard {
    backend: Backend,
}

impl ConnectionGuard {
    /// Create a new connection guard
    pub fn new(backend: Backend) -> Self {
        backend.inc_connections();
        Self { backend }
    }

    /// Get the backend address
    pub fn addr(&self) -> SocketAddr {
        self.backend.addr
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.backend.dec_connections();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_load_balancer_round_robin() {
        let lb = LoadBalancer::new("test-service");

        lb.add_backend(Backend::new("127.0.0.1:8081".parse().unwrap()))
            .await;
        lb.add_backend(Backend::new("127.0.0.1:8082".parse().unwrap()))
            .await;
        lb.add_backend(Backend::new("127.0.0.1:8083".parse().unwrap()))
            .await;

        // Mark all as healthy
        lb.update_health("127.0.0.1:8081".parse().unwrap(), HealthStatus::Healthy)
            .await;
        lb.update_health("127.0.0.1:8082".parse().unwrap(), HealthStatus::Healthy)
            .await;
        lb.update_health("127.0.0.1:8083".parse().unwrap(), HealthStatus::Healthy)
            .await;

        let b1 = lb.select().await.unwrap();
        let b2 = lb.select().await.unwrap();
        let b3 = lb.select().await.unwrap();
        let b4 = lb.select().await.unwrap();

        // Should cycle through backends
        assert_ne!(b1.addr, b2.addr);
        assert_ne!(b2.addr, b3.addr);
        assert_eq!(b1.addr, b4.addr); // Wraps around
    }

    #[tokio::test]
    async fn test_load_balancer_no_healthy_backends() {
        let lb = LoadBalancer::new("test-service");

        lb.add_backend(Backend::new("127.0.0.1:8081".parse().unwrap()))
            .await;
        lb.update_health("127.0.0.1:8081".parse().unwrap(), HealthStatus::Unhealthy)
            .await;

        let result = lb.select().await;
        assert!(matches!(result, Err(ProxyError::NoHealthyBackends { .. })));
    }

    #[tokio::test]
    async fn test_connection_guard() {
        let backend = Backend::new("127.0.0.1:8080".parse().unwrap());
        assert_eq!(backend.connection_count(), 0);

        {
            let _guard = ConnectionGuard::new(backend.clone());
            assert_eq!(backend.connection_count(), 1);

            let _guard2 = ConnectionGuard::new(backend.clone());
            assert_eq!(backend.connection_count(), 2);
        }

        // Guards dropped
        assert_eq!(backend.connection_count(), 0);
    }

    #[tokio::test]
    async fn test_least_connections() {
        let lb =
            LoadBalancer::with_algorithm("test-service", LoadBalancerAlgorithm::LeastConnections);

        let b1 = Backend::new("127.0.0.1:8081".parse().unwrap());
        let b2 = Backend::new("127.0.0.1:8082".parse().unwrap());

        // Simulate b1 having more connections
        b1.inc_connections();
        b1.inc_connections();

        lb.add_backend(b1).await;
        lb.add_backend(b2).await;

        lb.update_health("127.0.0.1:8081".parse().unwrap(), HealthStatus::Healthy)
            .await;
        lb.update_health("127.0.0.1:8082".parse().unwrap(), HealthStatus::Healthy)
            .await;

        // Should select b2 (fewer connections)
        let selected = lb.select().await.unwrap();
        assert_eq!(
            selected.addr,
            "127.0.0.1:8082".parse::<SocketAddr>().unwrap()
        );
    }
}
