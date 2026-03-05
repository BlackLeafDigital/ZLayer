//! Load balancer for backend selection
//!
//! This module handles **backend selection** (service -> specific backend addr).
//! The [`ServiceRegistry`](crate::routes::ServiceRegistry) handles **routing**
//! (host+path -> service). These are separate concerns.
//!
//! # Strategies
//!
//! - [`LbStrategy::RoundRobin`] — Cycles through healthy backends in order.
//! - [`LbStrategy::LeastConnections`] — Picks the healthy backend with the
//!   fewest active connections (tracked via atomic counters).
//!
//! # Health checking
//!
//! [`LoadBalancer::spawn_health_checker`] launches a background task that
//! periodically TCP-connects to every backend across all groups, updating
//! health status atomically. Concurrency is bounded by a semaphore.

use dashmap::DashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

/// Number of consecutive health-check failures before a backend is marked unhealthy.
const UNHEALTHY_THRESHOLD: u64 = 3;

// ---------------------------------------------------------------------------
// LbStrategy
// ---------------------------------------------------------------------------

/// Load-balancing strategy for a backend group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LbStrategy {
    /// Cycle through healthy backends in registration order.
    RoundRobin,
    /// Pick the healthy backend with the fewest active connections.
    LeastConnections,
}

// ---------------------------------------------------------------------------
// HealthStatus
// ---------------------------------------------------------------------------

/// Whether a backend is considered reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// Per-backend state with atomic connection counting and health tracking.
pub struct Backend {
    /// The network address of this backend.
    pub addr: SocketAddr,
    /// Number of in-flight connections (incremented/decremented atomically).
    active_connections: AtomicU64,
    /// Current health status (behind a std `RwLock` for interior mutability).
    health: std::sync::RwLock<HealthStatus>,
    /// Number of consecutive health-check failures.
    consecutive_failures: AtomicU64,
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backend")
            .field("addr", &self.addr)
            .field(
                "active_connections",
                &self.active_connections.load(Ordering::Relaxed),
            )
            .field("health", &*self.health.read().unwrap())
            .field(
                "consecutive_failures",
                &self.consecutive_failures.load(Ordering::Relaxed),
            )
            .finish()
    }
}

impl Backend {
    /// Create a new backend that starts healthy with zero active connections.
    #[must_use]
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            active_connections: AtomicU64::new(0),
            health: std::sync::RwLock::new(HealthStatus::Healthy),
            consecutive_failures: AtomicU64::new(0),
        }
    }

    /// Increment the active connection count and return an RAII guard that
    /// decrements it on drop.
    pub fn track_connection(self: &Arc<Self>) -> ConnectionGuard {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
        ConnectionGuard {
            backend: Arc::clone(self),
        }
    }

    /// Current number of in-flight connections.
    pub fn active_connections(&self) -> u64 {
        self.active_connections.load(Ordering::Relaxed)
    }

    /// Returns `true` if the backend is currently marked healthy.
    ///
    /// # Panics
    ///
    /// Panics if the health `RwLock` is poisoned.
    pub fn is_healthy(&self) -> bool {
        *self.health.read().unwrap() == HealthStatus::Healthy
    }

    /// Mark this backend as healthy.
    ///
    /// # Panics
    ///
    /// Panics if the health `RwLock` is poisoned.
    pub fn set_healthy(&self) {
        *self.health.write().unwrap() = HealthStatus::Healthy;
    }

    /// Mark this backend as unhealthy.
    ///
    /// # Panics
    ///
    /// Panics if the health `RwLock` is poisoned.
    pub fn set_unhealthy(&self) {
        *self.health.write().unwrap() = HealthStatus::Unhealthy;
    }

    /// Record one consecutive health-check failure.
    pub fn record_failure(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Reset the consecutive failure counter to zero.
    pub fn reset_failures(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    /// Number of consecutive health-check failures.
    pub fn consecutive_failures(&self) -> u64 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// ConnectionGuard
// ---------------------------------------------------------------------------

/// RAII guard that decrements a backend's active connection count on drop.
pub struct ConnectionGuard {
    backend: Arc<Backend>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.backend
            .active_connections
            .fetch_sub(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// BackendGroup
// ---------------------------------------------------------------------------

/// A set of backends for a single service, with a configured selection strategy.
pub struct BackendGroup {
    /// The backends in this group.
    pub backends: Vec<Arc<Backend>>,
    /// The strategy used by [`select`](Self::select).
    pub strategy: LbStrategy,
    /// Monotonically-increasing counter for round-robin.
    rr_counter: AtomicUsize,
}

impl BackendGroup {
    /// Create an empty group with the given strategy.
    #[must_use]
    pub fn new(strategy: LbStrategy) -> Self {
        Self {
            backends: Vec::new(),
            strategy,
            rr_counter: AtomicUsize::new(0),
        }
    }

    /// Select a healthy backend using the configured strategy.
    ///
    /// Returns `None` if every backend is unhealthy (or the group is empty).
    pub fn select(&self) -> Option<Arc<Backend>> {
        if self.backends.is_empty() {
            return None;
        }

        match self.strategy {
            LbStrategy::RoundRobin => self.select_round_robin(),
            LbStrategy::LeastConnections => self.select_least_connections(),
        }
    }

    /// Round-robin: start from the current counter position, try each backend
    /// once, skip unhealthy ones.
    fn select_round_robin(&self) -> Option<Arc<Backend>> {
        let len = self.backends.len();
        let start = self.rr_counter.fetch_add(1, Ordering::Relaxed) % len;

        for i in 0..len {
            let idx = (start + i) % len;
            let backend = &self.backends[idx];
            if backend.is_healthy() {
                return Some(Arc::clone(backend));
            }
        }

        None
    }

    /// Least-connections: find the healthy backend with the fewest active
    /// connections. Ties are broken by position (first wins).
    fn select_least_connections(&self) -> Option<Arc<Backend>> {
        self.backends
            .iter()
            .filter(|b| b.is_healthy())
            .min_by_key(|b| b.active_connections())
            .cloned()
    }

    /// Replace the backend list while preserving state for addresses that
    /// remain unchanged.
    ///
    /// Backends whose address appears in `addrs` keep their existing
    /// connection counts, health status, and failure counters. New addresses
    /// get a fresh `Backend`. Addresses no longer in `addrs` are dropped.
    pub fn update_backends(&mut self, addrs: Vec<SocketAddr>) {
        let mut new_backends = Vec::with_capacity(addrs.len());

        for addr in addrs {
            if let Some(existing) = self.backends.iter().find(|b| b.addr == addr) {
                new_backends.push(Arc::clone(existing));
            } else {
                new_backends.push(Arc::new(Backend::new(addr)));
            }
        }

        self.backends = new_backends;
    }

    /// Add a backend if its address is not already present.
    pub fn add_backend(&mut self, addr: SocketAddr) {
        if !self.backends.iter().any(|b| b.addr == addr) {
            self.backends.push(Arc::new(Backend::new(addr)));
        }
    }

    /// Remove the backend with the given address, if present.
    pub fn remove_backend(&mut self, addr: &SocketAddr) {
        self.backends.retain(|b| b.addr != *addr);
    }
}

// ---------------------------------------------------------------------------
// LoadBalancer
// ---------------------------------------------------------------------------

/// Top-level load balancer that manages backend groups keyed by service name.
pub struct LoadBalancer {
    groups: DashMap<String, BackendGroup>,
}

impl Default for LoadBalancer {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadBalancer {
    /// Create an empty load balancer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            groups: DashMap::new(),
        }
    }

    /// Register (or replace) a backend group for `service`.
    pub fn register(&self, service: &str, addrs: Vec<SocketAddr>, strategy: LbStrategy) {
        let mut group = BackendGroup::new(strategy);
        group.backends = addrs
            .into_iter()
            .map(|a| Arc::new(Backend::new(a)))
            .collect();
        self.groups.insert(service.to_string(), group);
    }

    /// Select a healthy backend for `service`, delegating to the group's
    /// configured strategy.
    #[must_use]
    pub fn select(&self, service: &str) -> Option<Arc<Backend>> {
        self.groups.get(service).and_then(|g| g.select())
    }

    /// Update the backend list for `service`, preserving state for unchanged
    /// addresses.
    pub fn update_backends(&self, service: &str, addrs: Vec<SocketAddr>) {
        if let Some(mut group) = self.groups.get_mut(service) {
            group.update_backends(addrs);
        }
    }

    /// Remove the backend group for `service`.
    pub fn unregister(&self, service: &str) {
        self.groups.remove(service);
    }

    /// Add a single backend to an existing group.
    pub fn add_backend(&self, service: &str, addr: SocketAddr) {
        if let Some(mut group) = self.groups.get_mut(service) {
            group.add_backend(addr);
            debug!(service = service, backend = %addr, total = group.backends.len(), "Added backend to LB group");
        } else {
            warn!(service = service, backend = %addr, "Cannot add backend: LB group not registered");
        }
    }

    /// Remove a single backend from an existing group.
    pub fn remove_backend(&self, service: &str, addr: &SocketAddr) {
        if let Some(mut group) = self.groups.get_mut(service) {
            group.remove_backend(addr);
        }
    }

    /// Return the number of backends registered for `service`, or 0 if
    /// the service is not registered.
    #[must_use]
    pub fn backend_count(&self, service: &str) -> usize {
        self.groups.get(service).map_or(0, |g| g.backends.len())
    }

    /// Return the number of *healthy* backends for `service`, or 0 if the
    /// service is not registered.
    #[must_use]
    pub fn healthy_count(&self, service: &str) -> usize {
        self.groups
            .get(service)
            .map_or(0, |g| g.backends.iter().filter(|b| b.is_healthy()).count())
    }

    /// Update the health status of a specific backend in a service group.
    ///
    /// If `healthy` is `true`, marks the backend healthy and resets its failure
    /// counter. Otherwise marks it unhealthy and records a failure.
    pub fn mark_health(&self, service: &str, addr: &SocketAddr, healthy: bool) {
        if let Some(group) = self.groups.get(service) {
            if let Some(backend) = group.backends.iter().find(|b| b.addr == *addr) {
                if healthy {
                    backend.set_healthy();
                    backend.reset_failures();
                } else {
                    backend.set_unhealthy();
                    backend.record_failure();
                }
            }
        }
    }

    /// Spawn a background health-check task.
    ///
    /// Every `interval` the task TCP-connects to every backend across all
    /// groups with a per-probe `timeout`. Concurrency is bounded to at most
    /// 64 simultaneous probes via a semaphore.
    ///
    /// On success the backend is marked healthy and its failure counter is
    /// reset. On failure it is marked unhealthy and the failure counter is
    /// incremented.
    ///
    /// # Panics
    ///
    /// The spawned task panics if the internal concurrency semaphore is
    /// unexpectedly closed.
    #[must_use]
    pub fn spawn_health_checker(
        self: &Arc<Self>,
        interval: Duration,
        timeout: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let lb = Arc::clone(self);

        tokio::spawn(async move {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(64));

            loop {
                // Collect all backends across all groups.
                let backends: Vec<Arc<Backend>> = lb
                    .groups
                    .iter()
                    .flat_map(|entry| entry.value().backends.clone())
                    .collect();

                debug!(
                    backend_count = backends.len(),
                    "Starting health check sweep"
                );

                let mut handles = Vec::with_capacity(backends.len());

                for backend in backends {
                    let sem = Arc::clone(&semaphore);
                    let probe_timeout = timeout;

                    handles.push(tokio::spawn(async move {
                        let _permit = sem.acquire().await.expect("semaphore closed");
                        let addr = backend.addr;

                        match tokio::time::timeout(
                            probe_timeout,
                            tokio::net::TcpStream::connect(addr),
                        )
                        .await
                        {
                            Ok(Ok(_stream)) => {
                                if !backend.is_healthy() {
                                    debug!(%addr, "Backend recovered");
                                }
                                backend.set_healthy();
                                backend.reset_failures();
                            }
                            Ok(Err(e)) => {
                                backend.record_failure();
                                let failures = backend.consecutive_failures();
                                if failures >= UNHEALTHY_THRESHOLD {
                                    if backend.is_healthy() {
                                        warn!(
                                            %addr,
                                            error = %e,
                                            failures,
                                            "Backend marked unhealthy after consecutive failures"
                                        );
                                    }
                                    backend.set_unhealthy();
                                } else {
                                    debug!(
                                            %addr,
                                            error = %e,
                                            failures,
                                            "Health check failed ({failures}/{UNHEALTHY_THRESHOLD} before unhealthy)"
                                    );
                                }
                            }
                            Err(_elapsed) => {
                                backend.record_failure();
                                let failures = backend.consecutive_failures();
                                if failures >= UNHEALTHY_THRESHOLD {
                                    if backend.is_healthy() {
                                        warn!(
                                            %addr,
                                            failures,
                                            "Backend marked unhealthy after consecutive timeout failures"
                                        );
                                    }
                                    backend.set_unhealthy();
                                } else {
                                    debug!(
                                        %addr,
                                        failures,
                                        "Health check timed out ({failures}/{UNHEALTHY_THRESHOLD} before unhealthy)"
                                    );
                                }
                            }
                        }
                    }));
                }

                // Wait for all probes to finish before sleeping.
                for handle in handles {
                    let _ = handle.await;
                }

                tokio::time::sleep(interval).await;
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(port: u16) -> SocketAddr {
        format!("127.0.0.1:{port}").parse().unwrap()
    }

    #[test]
    fn test_round_robin_selection() {
        let mut group = BackendGroup::new(LbStrategy::RoundRobin);
        group.backends = vec![
            Arc::new(Backend::new(addr(8001))),
            Arc::new(Backend::new(addr(8002))),
            Arc::new(Backend::new(addr(8003))),
        ];

        let a = group.select().unwrap();
        let b = group.select().unwrap();
        let c = group.select().unwrap();
        let d = group.select().unwrap();

        assert_eq!(a.addr, addr(8001));
        assert_eq!(b.addr, addr(8002));
        assert_eq!(c.addr, addr(8003));
        assert_eq!(d.addr, addr(8001)); // wraps around
    }

    #[test]
    fn test_least_connections_selection() {
        let mut group = BackendGroup::new(LbStrategy::LeastConnections);
        let b1 = Arc::new(Backend::new(addr(8001)));
        let b2 = Arc::new(Backend::new(addr(8002)));
        let b3 = Arc::new(Backend::new(addr(8003)));

        // Give b1 a tracked connection so it has 1 active.
        let _guard = b1.track_connection();

        group.backends = vec![b1, Arc::clone(&b2), b3];

        // Should pick b2 or b3 (both have 0 connections); first one wins.
        let selected = group.select().unwrap();
        assert_ne!(selected.addr, addr(8001));
        assert!(selected.addr == addr(8002) || selected.addr == addr(8003));

        // With b2 having a connection too, only b3 has 0.
        let _guard2 = b2.track_connection();
        let selected = group.select().unwrap();
        assert_eq!(selected.addr, addr(8003));
    }

    #[test]
    fn test_unhealthy_backends_skipped() {
        let mut group = BackendGroup::new(LbStrategy::RoundRobin);
        let b1 = Arc::new(Backend::new(addr(8001)));
        let b2 = Arc::new(Backend::new(addr(8002)));
        let b3 = Arc::new(Backend::new(addr(8003)));

        b2.set_unhealthy();

        group.backends = vec![b1, b2, Arc::clone(&b3)];

        // Should cycle between b1 and b3, never returning b2.
        for _ in 0..10 {
            let selected = group.select().unwrap();
            assert_ne!(selected.addr, addr(8002), "Unhealthy backend was selected");
        }
    }

    #[test]
    fn test_connection_guard_decrement() {
        let backend = Arc::new(Backend::new(addr(9000)));
        assert_eq!(backend.active_connections(), 0);

        let guard1 = backend.track_connection();
        assert_eq!(backend.active_connections(), 1);

        let guard2 = backend.track_connection();
        assert_eq!(backend.active_connections(), 2);

        drop(guard1);
        assert_eq!(backend.active_connections(), 1);

        drop(guard2);
        assert_eq!(backend.active_connections(), 0);
    }

    #[test]
    fn test_update_backends_preserves_state() {
        let mut group = BackendGroup::new(LbStrategy::RoundRobin);
        let b1 = Arc::new(Backend::new(addr(8001)));
        let b2 = Arc::new(Backend::new(addr(8002)));

        // Give b1 a tracked connection to create observable state.
        let _guard = b1.track_connection();
        b2.set_unhealthy();

        group.backends = vec![Arc::clone(&b1), Arc::clone(&b2)];

        // Update: keep 8001, drop 8002, add 8003.
        group.update_backends(vec![addr(8001), addr(8003)]);

        assert_eq!(group.backends.len(), 2);

        // The preserved backend for 8001 should still have 1 active connection.
        let preserved = group
            .backends
            .iter()
            .find(|b| b.addr == addr(8001))
            .unwrap();
        assert_eq!(preserved.active_connections(), 1);

        // The new backend for 8003 should start fresh.
        let new_backend = group
            .backends
            .iter()
            .find(|b| b.addr == addr(8003))
            .unwrap();
        assert_eq!(new_backend.active_connections(), 0);
        assert!(new_backend.is_healthy());

        // 8002 should be gone.
        assert!(group.backends.iter().all(|b| b.addr != addr(8002)));
    }

    #[test]
    fn test_all_unhealthy_returns_none() {
        let mut group = BackendGroup::new(LbStrategy::RoundRobin);
        let b1 = Arc::new(Backend::new(addr(8001)));
        let b2 = Arc::new(Backend::new(addr(8002)));

        b1.set_unhealthy();
        b2.set_unhealthy();

        group.backends = vec![b1, b2];

        assert!(group.select().is_none());

        // Same for LeastConnections.
        group.strategy = LbStrategy::LeastConnections;
        assert!(group.select().is_none());
    }

    #[test]
    fn test_register_and_select() {
        let lb = LoadBalancer::new();
        lb.register("web", vec![addr(8080), addr(8081)], LbStrategy::RoundRobin);

        let backend = lb.select("web").unwrap();
        assert!(backend.addr == addr(8080) || backend.addr == addr(8081));

        // Unknown service returns None.
        assert!(lb.select("nonexistent").is_none());
    }

    #[test]
    fn test_add_remove_backend() {
        let lb = LoadBalancer::new();
        lb.register("api", vec![addr(9001)], LbStrategy::RoundRobin);

        // Add a second backend.
        lb.add_backend("api", addr(9002));

        {
            let group = lb.groups.get("api").unwrap();
            assert_eq!(group.backends.len(), 2);
        }

        // Adding the same address again should be a no-op.
        lb.add_backend("api", addr(9002));
        {
            let group = lb.groups.get("api").unwrap();
            assert_eq!(group.backends.len(), 2);
        }

        // Remove the first backend.
        lb.remove_backend("api", &addr(9001));
        {
            let group = lb.groups.get("api").unwrap();
            assert_eq!(group.backends.len(), 1);
            assert_eq!(group.backends[0].addr, addr(9002));
        }
    }

    #[test]
    fn test_unregister() {
        let lb = LoadBalancer::new();
        lb.register("svc", vec![addr(5000)], LbStrategy::RoundRobin);
        assert!(lb.select("svc").is_some());

        lb.unregister("svc");
        assert!(lb.select("svc").is_none());
    }

    #[test]
    fn test_update_backends_via_lb() {
        let lb = LoadBalancer::new();
        lb.register("svc", vec![addr(3000)], LbStrategy::RoundRobin);

        lb.update_backends("svc", vec![addr(3001), addr(3002)]);

        let group = lb.groups.get("svc").unwrap();
        assert_eq!(group.backends.len(), 2);
        assert!(group.backends.iter().any(|b| b.addr == addr(3001)));
        assert!(group.backends.iter().any(|b| b.addr == addr(3002)));
    }

    #[test]
    fn test_empty_group_returns_none() {
        let group = BackendGroup::new(LbStrategy::RoundRobin);
        assert!(group.select().is_none());

        let group_lc = BackendGroup::new(LbStrategy::LeastConnections);
        assert!(group_lc.select().is_none());
    }

    #[test]
    fn test_failure_tracking() {
        let backend = Backend::new(addr(7000));
        assert_eq!(backend.consecutive_failures(), 0);

        backend.record_failure();
        backend.record_failure();
        assert_eq!(backend.consecutive_failures(), 2);

        backend.reset_failures();
        assert_eq!(backend.consecutive_failures(), 0);
    }

    #[test]
    fn test_health_transitions() {
        let backend = Backend::new(addr(7001));
        assert!(backend.is_healthy());

        backend.set_unhealthy();
        assert!(!backend.is_healthy());

        backend.set_healthy();
        assert!(backend.is_healthy());
    }
}
