//! Tunnel registry for managing active tunnels and their services
//!
//! The [`TunnelRegistry`] is the central component for tracking all active tunnels,
//! their services, and port assignments on the tunnel server.

use dashmap::DashMap;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{Result, ServiceProtocol, TunnelError};

// =============================================================================
// Control Messages
// =============================================================================

/// Control messages sent to tunnel clients via the control channel
#[derive(Debug, Clone)]
pub enum ControlMessage {
    /// New incoming connection for a service
    Connect {
        /// Service ID receiving the connection
        service_id: Uuid,
        /// Unique identifier for this connection
        connection_id: Uuid,
        /// Remote client address
        client_addr: SocketAddr,
    },
    /// Heartbeat request from server
    Heartbeat {
        /// Unix timestamp in milliseconds
        timestamp: u64,
    },
    /// Tunnel is being disconnected by the server
    Disconnect {
        /// Reason for disconnection
        reason: String,
    },
}

// =============================================================================
// Tunnel Service
// =============================================================================

/// A service exposed through a tunnel
#[derive(Debug)]
pub struct TunnelService {
    /// Unique service identifier
    pub id: Uuid,
    /// Service name (human-readable)
    pub name: String,
    /// Protocol type (TCP/UDP)
    pub protocol: ServiceProtocol,
    /// Local port on the tunnel client
    pub local_port: u16,
    /// Requested remote port (0 = auto-assign)
    pub remote_port: u16,
    /// Actually assigned port on the server
    pub assigned_port: Option<u16>,
    /// Number of active connections through this service
    pub active_connections: AtomicUsize,
}

impl TunnelService {
    /// Create a new tunnel service
    #[must_use]
    pub fn new(name: String, protocol: ServiceProtocol, local_port: u16, remote_port: u16) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            protocol,
            local_port,
            remote_port,
            assigned_port: None,
            active_connections: AtomicUsize::new(0),
        }
    }

    /// Get the current number of active connections
    #[must_use]
    pub fn connection_count(&self) -> usize {
        self.active_connections.load(Ordering::Relaxed)
    }

    /// Increment the active connection count
    pub fn increment_connections(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement the active connection count
    pub fn decrement_connections(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Clone for TunnelService {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            name: self.name.clone(),
            protocol: self.protocol,
            local_port: self.local_port,
            remote_port: self.remote_port,
            assigned_port: self.assigned_port,
            active_connections: AtomicUsize::new(self.active_connections.load(Ordering::Relaxed)),
        }
    }
}

// =============================================================================
// Tunnel
// =============================================================================

/// A registered tunnel with its services
#[derive(Debug)]
pub struct Tunnel {
    /// Unique tunnel identifier
    pub id: Uuid,
    /// Hashed authentication token for validation
    pub token_hash: String,
    /// Human-readable name (optional)
    pub name: Option<String>,
    /// Services registered on this tunnel
    pub services: DashMap<Uuid, TunnelService>,
    /// Channel to send control messages to the tunnel client
    pub control_tx: mpsc::Sender<ControlMessage>,
    /// When the tunnel was established
    pub connected_at: Instant,
    /// Last activity timestamp (updated on any tunnel activity)
    pub last_activity: Mutex<Instant>,
    /// Client's reported address
    pub client_addr: Option<SocketAddr>,
}

impl Tunnel {
    /// Create a new tunnel
    #[must_use]
    pub fn new(
        token_hash: String,
        name: Option<String>,
        control_tx: mpsc::Sender<ControlMessage>,
        client_addr: Option<SocketAddr>,
    ) -> Self {
        let now = Instant::now();
        Self {
            id: Uuid::new_v4(),
            token_hash,
            name,
            services: DashMap::new(),
            control_tx,
            connected_at: now,
            last_activity: Mutex::new(now),
            client_addr,
        }
    }

    /// Update the last activity timestamp
    pub fn touch(&self) {
        *self.last_activity.lock() = Instant::now();
    }

    /// Get the last activity timestamp
    #[must_use]
    pub fn last_activity(&self) -> Instant {
        *self.last_activity.lock()
    }

    /// Get the number of services registered on this tunnel
    #[must_use]
    pub fn service_count(&self) -> usize {
        self.services.len()
    }

    /// Check if the tunnel has been idle for longer than the specified duration
    #[must_use]
    pub fn is_idle_for(&self, duration: Duration) -> bool {
        self.last_activity().elapsed() > duration
    }

    /// Send a control message to the tunnel client
    ///
    /// # Errors
    ///
    /// Returns an error if the control channel is closed.
    pub async fn send_control(&self, message: ControlMessage) -> Result<()> {
        self.control_tx
            .send(message)
            .await
            .map_err(|_| TunnelError::registry("control channel closed"))
    }

    /// Try to send a control message without blocking
    ///
    /// # Errors
    ///
    /// Returns an error if the channel is full or closed.
    pub fn try_send_control(&self, message: ControlMessage) -> Result<()> {
        self.control_tx
            .try_send(message)
            .map_err(|e| TunnelError::registry(format!("failed to send control message: {e}")))
    }
}

// =============================================================================
// Port Pool
// =============================================================================

/// Port pool for automatic port assignment
#[derive(Debug)]
struct PortPool {
    /// Start of the port range (inclusive)
    range_start: u16,
    /// End of the port range (inclusive)
    range_end: u16,
    /// Currently used ports
    used_ports: HashSet<u16>,
}

impl PortPool {
    /// Create a new port pool with the given range
    fn new(start: u16, end: u16) -> Self {
        Self {
            range_start: start,
            range_end: end,
            used_ports: HashSet::new(),
        }
    }

    /// Allocate the next available port
    fn allocate(&mut self) -> Option<u16> {
        for port in self.range_start..=self.range_end {
            if !self.used_ports.contains(&port) {
                self.used_ports.insert(port);
                return Some(port);
            }
        }
        None
    }

    /// Try to allocate a specific port
    fn allocate_specific(&mut self, port: u16) -> bool {
        if port < self.range_start || port > self.range_end {
            return false;
        }
        if self.used_ports.contains(&port) {
            return false;
        }
        self.used_ports.insert(port);
        true
    }

    /// Release a port back to the pool
    fn release(&mut self, port: u16) {
        self.used_ports.remove(&port);
    }

    /// Check if a port is available
    fn is_available(&self, port: u16) -> bool {
        port >= self.range_start && port <= self.range_end && !self.used_ports.contains(&port)
    }

    /// Get the number of available ports
    fn available_count(&self) -> usize {
        let total = (self.range_end - self.range_start + 1) as usize;
        total.saturating_sub(self.used_ports.len())
    }
}

// =============================================================================
// Tunnel Info
// =============================================================================

/// Summary information about a tunnel (for listing)
#[derive(Debug, Clone)]
pub struct TunnelInfo {
    /// Unique tunnel identifier
    pub id: Uuid,
    /// Human-readable name (optional)
    pub name: Option<String>,
    /// When the tunnel was established
    pub connected_at: Instant,
    /// Last activity timestamp
    pub last_activity: Instant,
    /// Number of services registered
    pub service_count: usize,
    /// Client's reported address
    pub client_addr: Option<SocketAddr>,
}

impl From<&Tunnel> for TunnelInfo {
    fn from(tunnel: &Tunnel) -> Self {
        Self {
            id: tunnel.id,
            name: tunnel.name.clone(),
            connected_at: tunnel.connected_at,
            last_activity: tunnel.last_activity(),
            service_count: tunnel.service_count(),
            client_addr: tunnel.client_addr,
        }
    }
}

// =============================================================================
// Tunnel Registry
// =============================================================================

/// Registry for managing active tunnels and their services
///
/// The registry provides thread-safe access to tunnel and service information,
/// with support for:
/// - Tunnel registration and lookup
/// - Service registration with automatic or specific port assignment
/// - Port-based service routing for incoming connections
/// - Activity tracking and stale tunnel cleanup
#[derive(Debug)]
pub struct TunnelRegistry {
    /// Active tunnels indexed by ID
    tunnels: DashMap<Uuid, Arc<Tunnel>>,
    /// Tunnel lookup by token hash
    tunnels_by_token: DashMap<String, Uuid>,
    /// Service lookup by assigned port (port -> (`tunnel_id`, `service_id`))
    services_by_port: DashMap<u16, (Uuid, Uuid)>,
    /// Port pool for automatic assignment
    port_pool: Mutex<PortPool>,
}

impl TunnelRegistry {
    /// Create a new registry with the given port range for auto-assignment
    ///
    /// # Arguments
    ///
    /// * `port_range` - Tuple of (start, end) ports for auto-assignment (inclusive)
    #[must_use]
    pub fn new(port_range: (u16, u16)) -> Self {
        Self {
            tunnels: DashMap::new(),
            tunnels_by_token: DashMap::new(),
            services_by_port: DashMap::new(),
            port_pool: Mutex::new(PortPool::new(port_range.0, port_range.1)),
        }
    }

    /// Register a new tunnel
    ///
    /// # Arguments
    ///
    /// * `token_hash` - Hashed authentication token
    /// * `name` - Optional human-readable name
    /// * `control_tx` - Channel for sending control messages to the client
    /// * `client_addr` - Optional client address
    ///
    /// # Errors
    ///
    /// Returns an error if a tunnel with the same token hash already exists.
    pub fn register_tunnel(
        &self,
        token_hash: String,
        name: Option<String>,
        control_tx: mpsc::Sender<ControlMessage>,
        client_addr: Option<SocketAddr>,
    ) -> Result<Arc<Tunnel>> {
        // Check if token already registered
        if self.tunnels_by_token.contains_key(&token_hash) {
            return Err(TunnelError::registry("token already registered"));
        }

        let tunnel = Arc::new(Tunnel::new(
            token_hash.clone(),
            name,
            control_tx,
            client_addr,
        ));

        // Insert into both maps
        self.tunnels_by_token.insert(token_hash, tunnel.id);
        self.tunnels.insert(tunnel.id, Arc::clone(&tunnel));

        tracing::info!(
            tunnel_id = %tunnel.id,
            client_addr = ?client_addr,
            "tunnel registered"
        );

        Ok(tunnel)
    }

    /// Unregister a tunnel and clean up all its services
    ///
    /// Returns the removed tunnel if it existed.
    pub fn unregister_tunnel(&self, tunnel_id: Uuid) -> Option<Arc<Tunnel>> {
        let tunnel = self.tunnels.remove(&tunnel_id).map(|(_, t)| t)?;

        // Remove from token lookup
        self.tunnels_by_token.remove(&tunnel.token_hash);

        // Clean up all services and release their ports
        let mut port_pool = self.port_pool.lock();
        for entry in &tunnel.services {
            let service = entry.value();
            if let Some(port) = service.assigned_port {
                self.services_by_port.remove(&port);
                port_pool.release(port);
            }
        }

        tracing::info!(
            tunnel_id = %tunnel.id,
            services_removed = tunnel.services.len(),
            "tunnel unregistered"
        );

        Some(tunnel)
    }

    /// Get a tunnel by its ID
    #[must_use]
    pub fn get_tunnel(&self, tunnel_id: Uuid) -> Option<Arc<Tunnel>> {
        self.tunnels.get(&tunnel_id).map(|entry| Arc::clone(&entry))
    }

    /// Get a tunnel by its token hash
    #[must_use]
    pub fn get_tunnel_by_token(&self, token_hash: &str) -> Option<Arc<Tunnel>> {
        let tunnel_id = self.tunnels_by_token.get(token_hash)?;
        self.get_tunnel(*tunnel_id)
    }

    /// Check if a token is already registered
    #[must_use]
    pub fn token_exists(&self, token_hash: &str) -> bool {
        self.tunnels_by_token.contains_key(token_hash)
    }

    /// Add a service to a tunnel
    ///
    /// # Arguments
    ///
    /// * `tunnel_id` - ID of the tunnel to add the service to
    /// * `name` - Service name
    /// * `protocol` - Service protocol (TCP/UDP)
    /// * `local_port` - Local port on the tunnel client
    /// * `remote_port` - Requested remote port (0 = auto-assign)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The tunnel doesn't exist
    /// - The requested port is already in use
    /// - No ports are available for auto-assignment
    pub fn add_service(
        &self,
        tunnel_id: Uuid,
        name: &str,
        protocol: ServiceProtocol,
        local_port: u16,
        remote_port: u16,
    ) -> Result<TunnelService> {
        let tunnel = self
            .get_tunnel(tunnel_id)
            .ok_or_else(|| TunnelError::registry("tunnel not found"))?;

        let mut service = TunnelService::new(name.to_string(), protocol, local_port, remote_port);

        // Allocate port
        let assigned_port = {
            let mut port_pool = self.port_pool.lock();

            if remote_port == 0 {
                // Auto-assign
                port_pool
                    .allocate()
                    .ok_or_else(|| TunnelError::registry("no ports available"))?
            } else {
                // Try to allocate specific port
                if !port_pool.allocate_specific(remote_port) {
                    return Err(TunnelError::registry(format!(
                        "port {remote_port} is not available"
                    )));
                }
                remote_port
            }
        };

        service.assigned_port = Some(assigned_port);

        // Register in lookup maps
        self.services_by_port
            .insert(assigned_port, (tunnel_id, service.id));
        tunnel.services.insert(service.id, service.clone());

        tracing::info!(
            tunnel_id = %tunnel_id,
            service_id = %service.id,
            service_name = %name,
            assigned_port = assigned_port,
            "service registered"
        );

        Ok(service)
    }

    /// Remove a service from a tunnel
    ///
    /// # Errors
    ///
    /// Returns an error if the tunnel or service doesn't exist.
    pub fn remove_service(&self, tunnel_id: Uuid, service_id: Uuid) -> Result<TunnelService> {
        let tunnel = self
            .get_tunnel(tunnel_id)
            .ok_or_else(|| TunnelError::registry("tunnel not found"))?;

        let (_, service) = tunnel
            .services
            .remove(&service_id)
            .ok_or_else(|| TunnelError::registry("service not found"))?;

        // Release the port
        if let Some(port) = service.assigned_port {
            self.services_by_port.remove(&port);
            self.port_pool.lock().release(port);
        }

        tracing::info!(
            tunnel_id = %tunnel_id,
            service_id = %service_id,
            service_name = %service.name,
            "service unregistered"
        );

        Ok(service)
    }

    /// Get a service by its assigned port
    ///
    /// Returns the tunnel and service if found.
    #[must_use]
    pub fn get_service_by_port(&self, port: u16) -> Option<(Arc<Tunnel>, TunnelService)> {
        let (tunnel_id, service_id) = *self.services_by_port.get(&port)?;
        let tunnel = self.get_tunnel(tunnel_id)?;
        let service = tunnel.services.get(&service_id)?.clone();
        Some((tunnel, service))
    }

    /// List all active tunnels
    #[must_use]
    pub fn list_tunnels(&self) -> Vec<TunnelInfo> {
        self.tunnels
            .iter()
            .map(|entry| TunnelInfo::from(entry.value().as_ref()))
            .collect()
    }

    /// Get the number of active tunnels
    #[must_use]
    pub fn tunnel_count(&self) -> usize {
        self.tunnels.len()
    }

    /// Get the total number of services across all tunnels
    #[must_use]
    pub fn service_count(&self) -> usize {
        self.tunnels
            .iter()
            .map(|entry| entry.value().service_count())
            .sum()
    }

    /// Update the last activity timestamp for a tunnel
    pub fn touch_tunnel(&self, tunnel_id: Uuid) {
        if let Some(tunnel) = self.get_tunnel(tunnel_id) {
            tunnel.touch();
        }
    }

    /// Clean up stale tunnels that have been idle for longer than the specified duration
    ///
    /// Returns the IDs of tunnels that were removed.
    pub fn cleanup_stale(&self, max_idle: Duration) -> Vec<Uuid> {
        let stale_ids: Vec<Uuid> = self
            .tunnels
            .iter()
            .filter(|entry| entry.value().is_idle_for(max_idle))
            .map(|entry| *entry.key())
            .collect();

        for tunnel_id in &stale_ids {
            if let Some(tunnel) = self.unregister_tunnel(*tunnel_id) {
                // Try to notify the client
                let _ = tunnel.try_send_control(ControlMessage::Disconnect {
                    reason: "idle timeout".to_string(),
                });

                tracing::info!(
                    tunnel_id = %tunnel_id,
                    "removed stale tunnel"
                );
            }
        }

        stale_ids
    }

    /// Get the number of available ports in the pool
    #[must_use]
    pub fn available_ports(&self) -> usize {
        self.port_pool.lock().available_count()
    }

    /// Check if a specific port is available
    #[must_use]
    pub fn is_port_available(&self, port: u16) -> bool {
        self.port_pool.lock().is_available(port)
    }
}

impl Default for TunnelRegistry {
    fn default() -> Self {
        Self::new((30000, 31000))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn create_test_registry() -> TunnelRegistry {
        TunnelRegistry::new((30000, 30100))
    }

    fn create_test_tunnel(registry: &TunnelRegistry, token: &str) -> Arc<Tunnel> {
        let (tx, _rx) = mpsc::channel(16);
        registry
            .register_tunnel(
                token.to_string(),
                Some("test-tunnel".to_string()),
                tx,
                Some("127.0.0.1:12345".parse().unwrap()),
            )
            .unwrap()
    }

    #[test]
    fn test_register_tunnel() {
        let registry = create_test_registry();
        let (tx, _rx) = mpsc::channel(16);

        let tunnel = registry
            .register_tunnel(
                "token123".to_string(),
                Some("my-tunnel".to_string()),
                tx,
                Some("192.168.1.100:54321".parse().unwrap()),
            )
            .unwrap();

        assert_eq!(tunnel.token_hash, "token123");
        assert_eq!(tunnel.name, Some("my-tunnel".to_string()));
        assert_eq!(
            tunnel.client_addr,
            Some("192.168.1.100:54321".parse().unwrap())
        );
        assert_eq!(registry.tunnel_count(), 1);
    }

    #[test]
    fn test_register_duplicate_token() {
        let registry = create_test_registry();
        let (tx1, _rx1) = mpsc::channel(16);
        let (tx2, _rx2) = mpsc::channel(16);

        registry
            .register_tunnel("same-token".to_string(), None, tx1, None)
            .unwrap();

        let result = registry.register_tunnel("same-token".to_string(), None, tx2, None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("already registered"));
    }

    #[test]
    fn test_unregister_tunnel() {
        let registry = create_test_registry();
        let tunnel = create_test_tunnel(&registry, "token123");
        let tunnel_id = tunnel.id;

        // Add a service
        registry
            .add_service(tunnel_id, "ssh", ServiceProtocol::Tcp, 22, 0)
            .unwrap();

        assert_eq!(registry.tunnel_count(), 1);
        assert_eq!(registry.service_count(), 1);

        // Unregister
        let removed = registry.unregister_tunnel(tunnel_id);
        assert!(removed.is_some());
        assert_eq!(registry.tunnel_count(), 0);
        assert_eq!(registry.service_count(), 0);

        // Verify tunnel lookup fails
        assert!(registry.get_tunnel(tunnel_id).is_none());
        assert!(registry.get_tunnel_by_token("token123").is_none());
    }

    #[test]
    fn test_get_tunnel_by_id() {
        let registry = create_test_registry();
        let tunnel = create_test_tunnel(&registry, "token123");

        let found = registry.get_tunnel(tunnel.id);
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, tunnel.id);

        // Non-existent tunnel
        assert!(registry.get_tunnel(Uuid::new_v4()).is_none());
    }

    #[test]
    fn test_get_tunnel_by_token() {
        let registry = create_test_registry();
        let tunnel = create_test_tunnel(&registry, "my-secret-token");

        let found = registry.get_tunnel_by_token("my-secret-token");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, tunnel.id);

        // Non-existent token
        assert!(registry.get_tunnel_by_token("wrong-token").is_none());
    }

    #[test]
    fn test_token_exists() {
        let registry = create_test_registry();
        create_test_tunnel(&registry, "token123");

        assert!(registry.token_exists("token123"));
        assert!(!registry.token_exists("other-token"));
    }

    #[test]
    fn test_add_service_auto_assign() {
        let registry = create_test_registry();
        let tunnel = create_test_tunnel(&registry, "token123");

        let service = registry
            .add_service(
                tunnel.id,
                "ssh",
                ServiceProtocol::Tcp,
                22,
                0, // Auto-assign
            )
            .unwrap();

        assert_eq!(service.name, "ssh");
        assert_eq!(service.protocol, ServiceProtocol::Tcp);
        assert_eq!(service.local_port, 22);
        assert_eq!(service.remote_port, 0);
        assert_eq!(service.assigned_port, Some(30000)); // First available port
    }

    #[test]
    fn test_add_service_specific_port() {
        let registry = create_test_registry();
        let tunnel = create_test_tunnel(&registry, "token123");

        let service = registry
            .add_service(
                tunnel.id,
                "ssh",
                ServiceProtocol::Tcp,
                22,
                30050, // Specific port
            )
            .unwrap();

        assert_eq!(service.assigned_port, Some(30050));
    }

    #[test]
    fn test_add_service_port_conflict() {
        let registry = create_test_registry();
        let tunnel = create_test_tunnel(&registry, "token123");

        // First service gets port 30050
        registry
            .add_service(tunnel.id, "ssh", ServiceProtocol::Tcp, 22, 30050)
            .unwrap();

        // Second service trying to get same port fails
        let result = registry.add_service(tunnel.id, "ssh2", ServiceProtocol::Tcp, 2222, 30050);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not available"));
    }

    #[test]
    fn test_add_service_tunnel_not_found() {
        let registry = create_test_registry();

        let result = registry.add_service(
            Uuid::new_v4(), // Non-existent tunnel
            "ssh",
            ServiceProtocol::Tcp,
            22,
            0,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_remove_service() {
        let registry = create_test_registry();
        let tunnel = create_test_tunnel(&registry, "token123");

        let service = registry
            .add_service(tunnel.id, "ssh", ServiceProtocol::Tcp, 22, 30050)
            .unwrap();

        assert_eq!(registry.service_count(), 1);
        assert!(!registry.is_port_available(30050));

        // Remove the service
        let removed = registry.remove_service(tunnel.id, service.id).unwrap();
        assert_eq!(removed.name, "ssh");
        assert_eq!(registry.service_count(), 0);
        assert!(registry.is_port_available(30050)); // Port released
    }

    #[test]
    fn test_remove_service_not_found() {
        let registry = create_test_registry();
        let tunnel = create_test_tunnel(&registry, "token123");

        let result = registry.remove_service(tunnel.id, Uuid::new_v4());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_get_service_by_port() {
        let registry = create_test_registry();
        let tunnel = create_test_tunnel(&registry, "token123");

        let service = registry
            .add_service(tunnel.id, "ssh", ServiceProtocol::Tcp, 22, 30050)
            .unwrap();

        let (found_tunnel, found_service) = registry.get_service_by_port(30050).unwrap();
        assert_eq!(found_tunnel.id, tunnel.id);
        assert_eq!(found_service.id, service.id);

        // Non-existent port
        assert!(registry.get_service_by_port(30051).is_none());
    }

    #[test]
    fn test_multiple_tunnels_multiple_services() {
        let registry = create_test_registry();

        let tunnel1 = create_test_tunnel(&registry, "token1");
        let tunnel2 = create_test_tunnel(&registry, "token2");

        // Add services to tunnel1
        registry
            .add_service(tunnel1.id, "ssh", ServiceProtocol::Tcp, 22, 0)
            .unwrap();
        registry
            .add_service(tunnel1.id, "postgres", ServiceProtocol::Tcp, 5432, 0)
            .unwrap();

        // Add services to tunnel2
        registry
            .add_service(tunnel2.id, "game", ServiceProtocol::Udp, 27015, 0)
            .unwrap();

        assert_eq!(registry.tunnel_count(), 2);
        assert_eq!(registry.service_count(), 3);

        // Verify services are on correct tunnels
        let t1 = registry.get_tunnel(tunnel1.id).unwrap();
        let t2 = registry.get_tunnel(tunnel2.id).unwrap();
        assert_eq!(t1.service_count(), 2);
        assert_eq!(t2.service_count(), 1);
    }

    #[test]
    fn test_list_tunnels() {
        let registry = create_test_registry();

        create_test_tunnel(&registry, "token1");
        create_test_tunnel(&registry, "token2");
        create_test_tunnel(&registry, "token3");

        let tunnels = registry.list_tunnels();
        assert_eq!(tunnels.len(), 3);

        // Verify all tunnels have correct info
        for info in &tunnels {
            assert!(info.name.is_some());
            assert_eq!(info.service_count, 0);
        }
    }

    #[test]
    fn test_touch_and_activity_tracking() {
        let registry = create_test_registry();
        let tunnel = create_test_tunnel(&registry, "token123");

        let initial_activity = tunnel.last_activity();

        // Small sleep to ensure time difference
        std::thread::sleep(Duration::from_millis(10));

        registry.touch_tunnel(tunnel.id);

        let updated_activity = tunnel.last_activity();
        assert!(updated_activity > initial_activity);
    }

    #[test]
    fn test_cleanup_stale() {
        let registry = create_test_registry();

        let tunnel1 = create_test_tunnel(&registry, "token1");
        let tunnel2 = create_test_tunnel(&registry, "token2");

        // Make tunnel1 stale by not touching it
        // Touch tunnel2 to keep it active
        registry.touch_tunnel(tunnel2.id);

        // Small sleep
        std::thread::sleep(Duration::from_millis(50));

        // Only tunnel1 should be cleaned up if we use a very small idle threshold
        // But since both were created almost simultaneously, let's just verify the mechanism
        let stale = registry.cleanup_stale(Duration::from_secs(3600)); // 1 hour
        assert!(stale.is_empty()); // Nothing should be stale yet

        // Verify both tunnels still exist
        assert!(registry.get_tunnel(tunnel1.id).is_some());
        assert!(registry.get_tunnel(tunnel2.id).is_some());
    }

    #[test]
    fn test_port_pool_allocation() {
        let registry = TunnelRegistry::new((30000, 30002)); // Only 3 ports
        let tunnel = create_test_tunnel(&registry, "token123");

        // Allocate all ports
        let s1 = registry
            .add_service(tunnel.id, "s1", ServiceProtocol::Tcp, 1, 0)
            .unwrap();
        let s2 = registry
            .add_service(tunnel.id, "s2", ServiceProtocol::Tcp, 2, 0)
            .unwrap();
        let s3 = registry
            .add_service(tunnel.id, "s3", ServiceProtocol::Tcp, 3, 0)
            .unwrap();

        assert_eq!(s1.assigned_port, Some(30000));
        assert_eq!(s2.assigned_port, Some(30001));
        assert_eq!(s3.assigned_port, Some(30002));

        // Fourth allocation should fail
        let result = registry.add_service(tunnel.id, "s4", ServiceProtocol::Tcp, 4, 0);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no ports available"));

        // Release a port and try again
        registry.remove_service(tunnel.id, s2.id).unwrap();

        let s4 = registry
            .add_service(tunnel.id, "s4", ServiceProtocol::Tcp, 4, 0)
            .unwrap();
        assert_eq!(s4.assigned_port, Some(30001)); // Reused port
    }

    #[test]
    fn test_port_pool_out_of_range() {
        let registry = TunnelRegistry::new((30000, 30010));
        let tunnel = create_test_tunnel(&registry, "token123");

        // Try to allocate port outside range
        let result = registry.add_service(tunnel.id, "s1", ServiceProtocol::Tcp, 1, 25000);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not available"));
    }

    #[test]
    fn test_available_ports() {
        let registry = TunnelRegistry::new((30000, 30009)); // 10 ports
        let tunnel = create_test_tunnel(&registry, "token123");

        assert_eq!(registry.available_ports(), 10);

        registry
            .add_service(tunnel.id, "s1", ServiceProtocol::Tcp, 1, 0)
            .unwrap();
        assert_eq!(registry.available_ports(), 9);

        registry
            .add_service(tunnel.id, "s2", ServiceProtocol::Tcp, 2, 30005)
            .unwrap();
        assert_eq!(registry.available_ports(), 8);
    }

    #[test]
    fn test_is_port_available() {
        let registry = TunnelRegistry::new((30000, 30010));
        let tunnel = create_test_tunnel(&registry, "token123");

        assert!(registry.is_port_available(30005));
        assert!(!registry.is_port_available(29999)); // Out of range
        assert!(!registry.is_port_available(30011)); // Out of range

        registry
            .add_service(tunnel.id, "s1", ServiceProtocol::Tcp, 1, 30005)
            .unwrap();
        assert!(!registry.is_port_available(30005)); // Now in use
    }

    #[test]
    fn test_tunnel_service_connection_count() {
        let service = TunnelService::new("test".to_string(), ServiceProtocol::Tcp, 22, 2222);

        assert_eq!(service.connection_count(), 0);

        service.increment_connections();
        service.increment_connections();
        assert_eq!(service.connection_count(), 2);

        service.decrement_connections();
        assert_eq!(service.connection_count(), 1);
    }

    #[test]
    fn test_tunnel_service_clone() {
        let service = TunnelService::new("test".to_string(), ServiceProtocol::Tcp, 22, 2222);
        service.increment_connections();
        service.increment_connections();

        let cloned = service.clone();
        assert_eq!(cloned.id, service.id);
        assert_eq!(cloned.name, service.name);
        assert_eq!(cloned.connection_count(), 2);
    }

    #[test]
    fn test_tunnel_is_idle_for() {
        let (tx, _rx) = mpsc::channel(16);
        let tunnel = Tunnel::new("token".to_string(), None, tx, None);

        // Just created, should not be idle
        assert!(!tunnel.is_idle_for(Duration::from_secs(1)));

        // Wait a bit
        std::thread::sleep(Duration::from_millis(50));

        // Should be idle for very short duration
        assert!(tunnel.is_idle_for(Duration::from_millis(10)));

        // Touch and verify it's not idle anymore
        tunnel.touch();
        assert!(!tunnel.is_idle_for(Duration::from_millis(10)));
    }

    #[test]
    fn test_tunnel_info_from() {
        let (tx, _rx) = mpsc::channel(16);
        let tunnel = Tunnel::new(
            "token".to_string(),
            Some("my-tunnel".to_string()),
            tx,
            Some("192.168.1.1:1234".parse().unwrap()),
        );

        let info = TunnelInfo::from(&tunnel);
        assert_eq!(info.id, tunnel.id);
        assert_eq!(info.name, Some("my-tunnel".to_string()));
        assert_eq!(info.service_count, 0);
        assert_eq!(info.client_addr, Some("192.168.1.1:1234".parse().unwrap()));
    }

    #[test]
    fn test_default_registry() {
        let registry = TunnelRegistry::default();
        assert_eq!(registry.tunnel_count(), 0);
        // Default port range is 30000-31000, so 1001 ports
        assert_eq!(registry.available_ports(), 1001);
    }

    #[tokio::test]
    async fn test_tunnel_send_control() {
        let (tx, mut rx) = mpsc::channel(16);
        let tunnel = Tunnel::new("token".to_string(), None, tx, None);

        // Send a control message
        tunnel
            .send_control(ControlMessage::Heartbeat { timestamp: 12345 })
            .await
            .unwrap();

        // Receive and verify
        let msg = rx.recv().await.unwrap();
        match msg {
            ControlMessage::Heartbeat { timestamp } => assert_eq!(timestamp, 12345),
            _ => panic!("unexpected message"),
        }
    }

    #[test]
    fn test_tunnel_try_send_control() {
        let (tx, _rx) = mpsc::channel(16);
        let tunnel = Tunnel::new("token".to_string(), None, tx, None);

        // Try send should succeed when channel has capacity
        let result = tunnel.try_send_control(ControlMessage::Heartbeat { timestamp: 12345 });
        assert!(result.is_ok());
    }

    #[test]
    fn test_tunnel_try_send_control_full_channel() {
        let (tx, _rx) = mpsc::channel(1); // Capacity of 1
        let tunnel = Tunnel::new("token".to_string(), None, tx, None);

        // Fill the channel
        tunnel
            .try_send_control(ControlMessage::Heartbeat { timestamp: 1 })
            .unwrap();

        // Next send should fail (channel full)
        let result = tunnel.try_send_control(ControlMessage::Heartbeat { timestamp: 2 });
        assert!(result.is_err());
    }
}
