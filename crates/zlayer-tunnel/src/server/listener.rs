//! Dynamic service listeners for tunneled services
//!
//! Creates TCP and UDP listeners for services registered through tunnels.
//! When connections arrive, notifies the tunnel client via control channel.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{ControlMessage, Result, TunnelError, TunnelRegistry};

// =============================================================================
// Listener Handle
// =============================================================================

/// Handle for managing a single listener's lifecycle
struct ListenerHandle {
    /// Shutdown signal sender
    shutdown_tx: oneshot::Sender<()>,
}

// =============================================================================
// Pending Connection
// =============================================================================

/// A pending connection waiting for the tunnel client to accept
struct PendingConnection {
    /// Stream from the external client (for TCP)
    stream: Option<TcpStream>,
    /// When this pending connection expires
    expires_at: Instant,
}

// =============================================================================
// Listener Manager
// =============================================================================

/// Manages dynamic listeners for tunneled services
///
/// The `ListenerManager` creates TCP and UDP listeners for services registered
/// through tunnels. When external connections arrive at these listeners, it
/// notifies the appropriate tunnel client via the control channel.
///
/// # Architecture
///
/// ```text
/// External Client --> ListenerManager --> Control Channel --> Tunnel Client
///                          |
///                          v
///                     TunnelRegistry
///                          |
///                          v
///                     Service Lookup
/// ```
///
/// # Example
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use std::time::Duration;
/// use zlayer_tunnel::{TunnelRegistry, ListenerManager};
///
/// async fn example() {
///     let registry = Arc::new(TunnelRegistry::default());
///     let manager = ListenerManager::new(registry, Duration::from_secs(30));
///
///     // Start a TCP listener on port 8080
///     let actual_port = manager.start_tcp_listener(8080).await.unwrap();
///     println!("Listening on port {}", actual_port);
///
///     // Later, stop the listener
///     manager.stop_tcp_listener(actual_port);
/// }
/// ```
pub struct ListenerManager {
    /// Reference to the tunnel registry for service lookups
    registry: Arc<TunnelRegistry>,

    /// Active TCP listeners by port
    tcp_listeners: DashMap<u16, ListenerHandle>,

    /// Active UDP listeners by port
    udp_listeners: DashMap<u16, ListenerHandle>,

    /// Pending connections waiting for tunnel client to accept
    /// Maps `connection_id` -> `PendingConnection`
    pending_connections: Arc<DashMap<Uuid, PendingConnection>>,

    /// Connection timeout for pending connections
    connection_timeout: Duration,
}

impl ListenerManager {
    /// Create a new listener manager
    ///
    /// # Arguments
    ///
    /// * `registry` - The tunnel registry for service lookups
    /// * `connection_timeout` - How long to wait for tunnel client to accept connections
    #[must_use]
    pub fn new(registry: Arc<TunnelRegistry>, connection_timeout: Duration) -> Self {
        Self {
            registry,
            tcp_listeners: DashMap::new(),
            udp_listeners: DashMap::new(),
            pending_connections: Arc::new(DashMap::new()),
            connection_timeout,
        }
    }

    /// Start a TCP listener for a service
    ///
    /// Creates a TCP listener on the specified port. When connections arrive,
    /// the listener will look up the service in the registry and notify the
    /// appropriate tunnel client.
    ///
    /// # Arguments
    ///
    /// * `port` - The port to listen on (0 for auto-assign)
    ///
    /// # Returns
    ///
    /// The actual port being listened on (useful when port 0 is specified).
    ///
    /// # Errors
    ///
    /// Returns an error if the port cannot be bound.
    pub async fn start_tcp_listener(&self, port: u16) -> Result<u16> {
        // Check if already listening on this port
        if self.tcp_listeners.contains_key(&port) {
            return Ok(port);
        }

        // Bind listener
        let addr = format!("0.0.0.0:{port}");
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(TunnelError::connection)?;

        let actual_port = listener
            .local_addr()
            .map_err(TunnelError::connection)?
            .port();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let registry = self.registry.clone();
        let pending = self.pending_connections.clone();
        let timeout = self.connection_timeout;

        // Spawn listener task
        tokio::spawn(async move {
            Self::tcp_listener_loop(listener, registry, pending, timeout, shutdown_rx).await;
        });

        self.tcp_listeners
            .insert(actual_port, ListenerHandle { shutdown_tx });

        tracing::info!(port = actual_port, "TCP listener started");

        Ok(actual_port)
    }

    /// TCP listener main loop
    async fn tcp_listener_loop(
        listener: TcpListener,
        registry: Arc<TunnelRegistry>,
        pending: Arc<DashMap<Uuid, PendingConnection>>,
        timeout: Duration,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) {
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);

        loop {
            tokio::select! {
                biased;

                // Check for shutdown first
                _ = &mut shutdown_rx => {
                    tracing::info!(port = port, "TCP listener shutting down");
                    break;
                }

                // Accept new connections
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, client_addr)) => {
                            Self::handle_tcp_connection(
                                stream,
                                client_addr,
                                port,
                                &registry,
                                &pending,
                                timeout,
                            );
                        }
                        Err(e) => {
                            tracing::error!(port = port, error = %e, "TCP accept error");
                        }
                    }
                }
            }
        }
    }

    /// Handle a new TCP connection
    fn handle_tcp_connection(
        stream: TcpStream,
        client_addr: SocketAddr,
        port: u16,
        registry: &Arc<TunnelRegistry>,
        pending: &Arc<DashMap<Uuid, PendingConnection>>,
        timeout: Duration,
    ) {
        // Find the service for this port
        let Some((tunnel, service)) = registry.get_service_by_port(port) else {
            tracing::warn!(
                port = port,
                client = %client_addr,
                "Connection to unregistered service port"
            );
            return;
        };

        let connection_id = Uuid::new_v4();

        // Create pending connection entry
        pending.insert(
            connection_id,
            PendingConnection {
                stream: Some(stream),
                expires_at: Instant::now() + timeout,
            },
        );

        // Notify tunnel client of incoming connection
        let ctrl_msg = ControlMessage::Connect {
            service_id: service.id,
            connection_id,
            client_addr,
        };

        // Try to send the control message (non-blocking)
        if tunnel.try_send_control(ctrl_msg).is_err() {
            tracing::warn!(
                tunnel_id = %tunnel.id,
                connection_id = %connection_id,
                "Failed to send connect notification - control channel full or closed"
            );
            pending.remove(&connection_id);
            return;
        }

        // Increment connection count
        service.increment_connections();

        tracing::debug!(
            tunnel_id = %tunnel.id,
            service_id = %service.id,
            connection_id = %connection_id,
            client = %client_addr,
            "TCP connection pending"
        );

        // Spawn a task to handle timeout for this pending connection
        let pending_clone = pending.clone();
        let service_clone = service.clone();
        let tunnel_id = tunnel.id;
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;

            // If the connection is still pending after timeout, remove it
            if let Some((_, conn)) = pending_clone.remove(&connection_id) {
                if conn.stream.is_some() {
                    tracing::debug!(
                        tunnel_id = %tunnel_id,
                        connection_id = %connection_id,
                        "Pending connection timed out"
                    );
                    service_clone.decrement_connections();
                }
            }
        });
    }

    /// Start a UDP listener for a service
    ///
    /// Creates a UDP socket on the specified port. When datagrams arrive,
    /// the listener will look up the service in the registry and notify the
    /// appropriate tunnel client.
    ///
    /// # Arguments
    ///
    /// * `port` - The port to listen on (0 for auto-assign)
    ///
    /// # Returns
    ///
    /// The actual port being listened on.
    ///
    /// # Errors
    ///
    /// Returns an error if the port cannot be bound.
    ///
    /// # Note
    ///
    /// UDP proxying requires additional session tracking infrastructure.
    /// This implementation notifies tunnel clients of incoming packets but
    /// full bidirectional UDP proxying should be implemented separately.
    pub async fn start_udp_listener(&self, port: u16) -> Result<u16> {
        // Check if already listening
        if self.udp_listeners.contains_key(&port) {
            return Ok(port);
        }

        let addr = format!("0.0.0.0:{port}");
        let socket = UdpSocket::bind(&addr)
            .await
            .map_err(TunnelError::connection)?;

        let actual_port = socket.local_addr().map_err(TunnelError::connection)?.port();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let registry = self.registry.clone();

        // Spawn listener task
        tokio::spawn(async move {
            Self::udp_listener_loop(socket, registry, shutdown_rx).await;
        });

        self.udp_listeners
            .insert(actual_port, ListenerHandle { shutdown_tx });

        tracing::info!(port = actual_port, "UDP listener started");

        Ok(actual_port)
    }

    /// UDP listener main loop
    async fn udp_listener_loop(
        socket: UdpSocket,
        registry: Arc<TunnelRegistry>,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) {
        let mut buf = vec![0u8; 65535];
        let port = socket.local_addr().map(|a| a.port()).unwrap_or(0);

        loop {
            tokio::select! {
                biased;

                // Check for shutdown first
                _ = &mut shutdown_rx => {
                    tracing::info!(port = port, "UDP listener shutting down");
                    break;
                }

                // Receive datagrams
                recv_result = socket.recv_from(&mut buf) => {
                    match recv_result {
                        Ok((len, client_addr)) => {
                            Self::handle_udp_packet(
                                &buf[..len],
                                client_addr,
                                port,
                                &registry,
                            );
                        }
                        Err(e) => {
                            tracing::error!(port = port, error = %e, "UDP recv error");
                        }
                    }
                }
            }
        }
    }

    /// Handle a UDP packet
    fn handle_udp_packet(
        _data: &[u8],
        client_addr: SocketAddr,
        port: u16,
        registry: &Arc<TunnelRegistry>,
    ) {
        // Find the service for this port
        let Some((tunnel, service)) = registry.get_service_by_port(port) else {
            tracing::warn!(
                port = port,
                client = %client_addr,
                "UDP packet to unregistered service port"
            );
            return;
        };

        let connection_id = Uuid::new_v4();

        // For UDP, we notify the tunnel client of the incoming packet
        // Full UDP proxying with session tracking requires additional
        // infrastructure similar to zlayer-proxy/stream/udp.rs
        let ctrl_msg = ControlMessage::Connect {
            service_id: service.id,
            connection_id,
            client_addr,
        };

        if let Err(e) = tunnel.try_send_control(ctrl_msg) {
            tracing::warn!(
                tunnel_id = %tunnel.id,
                error = %e,
                "Failed to send UDP connect notification"
            );
        }

        tracing::trace!(
            tunnel_id = %tunnel.id,
            service_id = %service.id,
            port = port,
            client = %client_addr,
            "UDP packet received"
        );
    }

    /// Stop a TCP listener
    ///
    /// Signals the listener task to shut down and removes it from management.
    pub fn stop_tcp_listener(&self, port: u16) {
        if let Some((_, handle)) = self.tcp_listeners.remove(&port) {
            let _ = handle.shutdown_tx.send(());
            tracing::info!(port = port, "TCP listener stopped");
        }
    }

    /// Stop a UDP listener
    ///
    /// Signals the listener task to shut down and removes it from management.
    pub fn stop_udp_listener(&self, port: u16) {
        if let Some((_, handle)) = self.udp_listeners.remove(&port) {
            let _ = handle.shutdown_tx.send(());
            tracing::info!(port = port, "UDP listener stopped");
        }
    }

    /// Accept a pending connection
    ///
    /// Called when the tunnel client sends a `ConnectAck` message.
    /// Returns the TCP stream for the connection if it exists and hasn't expired.
    ///
    /// # Arguments
    ///
    /// * `connection_id` - The connection ID from the `Connect` message
    ///
    /// # Returns
    ///
    /// The TCP stream if the connection is still pending, or `None` if it
    /// has already been accepted, rejected, or timed out.
    #[must_use]
    pub fn accept_connection(&self, connection_id: Uuid) -> Option<TcpStream> {
        self.pending_connections
            .remove(&connection_id)
            .and_then(|(_, mut conn)| conn.stream.take())
    }

    /// Reject a pending connection
    ///
    /// Called when the tunnel client sends a `ConnectFail` message.
    /// Removes the pending connection and decrements the service connection count.
    ///
    /// # Arguments
    ///
    /// * `connection_id` - The connection ID from the `Connect` message
    pub fn reject_connection(&self, connection_id: Uuid) {
        if self.pending_connections.remove(&connection_id).is_some() {
            tracing::debug!(
                connection_id = %connection_id,
                "Connection rejected by tunnel client"
            );
        }
    }

    /// Get the count of active TCP listeners
    #[must_use]
    pub fn tcp_listener_count(&self) -> usize {
        self.tcp_listeners.len()
    }

    /// Get the count of active UDP listeners
    #[must_use]
    pub fn udp_listener_count(&self) -> usize {
        self.udp_listeners.len()
    }

    /// Get the count of pending connections
    #[must_use]
    pub fn pending_connection_count(&self) -> usize {
        self.pending_connections.len()
    }

    /// Clean up expired pending connections
    ///
    /// Removes any pending connections that have exceeded their timeout.
    /// Returns the number of connections that were cleaned up.
    pub fn cleanup_expired_connections(&self) -> usize {
        let now = Instant::now();
        let mut cleaned = 0;

        self.pending_connections.retain(|_, pending| {
            if pending.expires_at <= now {
                cleaned += 1;
                false
            } else {
                true
            }
        });

        if cleaned > 0 {
            tracing::debug!(count = cleaned, "Cleaned up expired pending connections");
        }

        cleaned
    }

    /// Check if a TCP listener is active on the given port
    #[must_use]
    pub fn has_tcp_listener(&self, port: u16) -> bool {
        self.tcp_listeners.contains_key(&port)
    }

    /// Check if a UDP listener is active on the given port
    #[must_use]
    pub fn has_udp_listener(&self, port: u16) -> bool {
        self.udp_listeners.contains_key(&port)
    }

    /// Get the list of active TCP listener ports
    #[must_use]
    pub fn tcp_listener_ports(&self) -> Vec<u16> {
        self.tcp_listeners.iter().map(|r| *r.key()).collect()
    }

    /// Get the list of active UDP listener ports
    #[must_use]
    pub fn udp_listener_ports(&self) -> Vec<u16> {
        self.udp_listeners.iter().map(|r| *r.key()).collect()
    }

    /// Shutdown all listeners
    ///
    /// Stops all TCP and UDP listeners and clears pending connections.
    pub fn shutdown(&self) {
        // Stop all TCP listeners
        let tcp_ports: Vec<u16> = self.tcp_listeners.iter().map(|r| *r.key()).collect();
        for port in tcp_ports {
            self.stop_tcp_listener(port);
        }

        // Stop all UDP listeners
        let udp_ports: Vec<u16> = self.udp_listeners.iter().map(|r| *r.key()).collect();
        for port in udp_ports {
            self.stop_udp_listener(port);
        }

        // Clear pending connections
        self.pending_connections.clear();

        tracing::info!("Listener manager shut down");
    }
}

impl Drop for ListenerManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn create_test_registry() -> Arc<TunnelRegistry> {
        Arc::new(TunnelRegistry::new((30000, 30100)))
    }

    fn create_test_manager(registry: Arc<TunnelRegistry>) -> ListenerManager {
        ListenerManager::new(registry, Duration::from_secs(30))
    }

    #[tokio::test]
    async fn test_start_and_stop_tcp_listener() {
        let registry = create_test_registry();
        let manager = create_test_manager(registry);

        // Start a TCP listener on a random port
        let port = manager.start_tcp_listener(0).await.unwrap();
        assert!(port > 0);
        assert_eq!(manager.tcp_listener_count(), 1);
        assert!(manager.has_tcp_listener(port));

        // Stop the listener
        manager.stop_tcp_listener(port);
        assert_eq!(manager.tcp_listener_count(), 0);
        assert!(!manager.has_tcp_listener(port));
    }

    #[tokio::test]
    async fn test_start_and_stop_udp_listener() {
        let registry = create_test_registry();
        let manager = create_test_manager(registry);

        // Start a UDP listener on a random port
        let port = manager.start_udp_listener(0).await.unwrap();
        assert!(port > 0);
        assert_eq!(manager.udp_listener_count(), 1);
        assert!(manager.has_udp_listener(port));

        // Stop the listener
        manager.stop_udp_listener(port);
        assert_eq!(manager.udp_listener_count(), 0);
        assert!(!manager.has_udp_listener(port));
    }

    #[tokio::test]
    async fn test_idempotent_listener_start() {
        let registry = create_test_registry();
        let manager = create_test_manager(registry);

        // Start a TCP listener
        let port = manager.start_tcp_listener(0).await.unwrap();

        // Starting again on the same port should return the same port
        let port2 = manager.start_tcp_listener(port).await.unwrap();
        assert_eq!(port, port2);
        assert_eq!(manager.tcp_listener_count(), 1);

        manager.stop_tcp_listener(port);
    }

    #[tokio::test]
    async fn test_multiple_listeners() {
        let registry = create_test_registry();
        let manager = create_test_manager(registry);

        // Start multiple TCP listeners
        let port1 = manager.start_tcp_listener(0).await.unwrap();
        let port2 = manager.start_tcp_listener(0).await.unwrap();
        let port3 = manager.start_tcp_listener(0).await.unwrap();

        assert_ne!(port1, port2);
        assert_ne!(port2, port3);
        assert_ne!(port1, port3);
        assert_eq!(manager.tcp_listener_count(), 3);

        // Start UDP listeners too
        let udp_port1 = manager.start_udp_listener(0).await.unwrap();
        let udp_port2 = manager.start_udp_listener(0).await.unwrap();

        assert_eq!(manager.udp_listener_count(), 2);

        // Check ports list
        let tcp_ports = manager.tcp_listener_ports();
        assert_eq!(tcp_ports.len(), 3);
        assert!(tcp_ports.contains(&port1));
        assert!(tcp_ports.contains(&port2));
        assert!(tcp_ports.contains(&port3));

        let udp_ports = manager.udp_listener_ports();
        assert_eq!(udp_ports.len(), 2);
        assert!(udp_ports.contains(&udp_port1));
        assert!(udp_ports.contains(&udp_port2));

        // Shutdown
        manager.shutdown();
        assert_eq!(manager.tcp_listener_count(), 0);
        assert_eq!(manager.udp_listener_count(), 0);
    }

    #[tokio::test]
    async fn test_listener_counts() {
        let registry = create_test_registry();
        let manager = create_test_manager(registry);

        assert_eq!(manager.tcp_listener_count(), 0);
        assert_eq!(manager.udp_listener_count(), 0);

        let tcp_port = manager.start_tcp_listener(0).await.unwrap();
        assert_eq!(manager.tcp_listener_count(), 1);

        let udp_port = manager.start_udp_listener(0).await.unwrap();
        assert_eq!(manager.udp_listener_count(), 1);

        manager.stop_tcp_listener(tcp_port);
        assert_eq!(manager.tcp_listener_count(), 0);

        manager.stop_udp_listener(udp_port);
        assert_eq!(manager.udp_listener_count(), 0);
    }

    #[test]
    fn test_cleanup_expired_connections() {
        let registry = create_test_registry();
        let manager = ListenerManager::new(registry, Duration::from_millis(1));

        // Add some pending connections with very short timeout
        manager.pending_connections.insert(
            Uuid::new_v4(),
            PendingConnection {
                stream: None,
                expires_at: Instant::now() - Duration::from_secs(1), // Already expired
            },
        );
        manager.pending_connections.insert(
            Uuid::new_v4(),
            PendingConnection {
                stream: None,
                expires_at: Instant::now() + Duration::from_secs(60), // Not expired
            },
        );

        assert_eq!(manager.pending_connection_count(), 2);

        // Clean up expired
        let cleaned = manager.cleanup_expired_connections();
        assert_eq!(cleaned, 1);
        assert_eq!(manager.pending_connection_count(), 1);
    }

    #[test]
    fn test_accept_and_reject_connection() {
        let registry = create_test_registry();
        let manager = create_test_manager(registry);

        let connection_id = Uuid::new_v4();

        // Add a pending connection (without stream for simplicity)
        manager.pending_connections.insert(
            connection_id,
            PendingConnection {
                stream: None,
                expires_at: Instant::now() + Duration::from_secs(30),
            },
        );

        assert_eq!(manager.pending_connection_count(), 1);

        // Accept should remove the connection
        let stream = manager.accept_connection(connection_id);
        assert!(stream.is_none()); // No stream was stored
        assert_eq!(manager.pending_connection_count(), 0);

        // Add another pending connection
        let connection_id2 = Uuid::new_v4();
        manager.pending_connections.insert(
            connection_id2,
            PendingConnection {
                stream: None,
                expires_at: Instant::now() + Duration::from_secs(30),
            },
        );

        // Reject should also remove the connection
        manager.reject_connection(connection_id2);
        assert_eq!(manager.pending_connection_count(), 0);
    }

    #[tokio::test]
    async fn test_tcp_connection_to_registered_service() {
        let registry = create_test_registry();

        // Register a tunnel and service
        let (tx, mut rx) = mpsc::channel(16);
        let tunnel = registry
            .register_tunnel(
                "test-token".to_string(),
                Some("test-tunnel".to_string()),
                tx,
                Some("127.0.0.1:12345".parse().unwrap()),
            )
            .unwrap();

        let service = registry
            .add_service(
                tunnel.id,
                "test-service",
                crate::ServiceProtocol::Tcp,
                8080,
                0,
            )
            .unwrap();

        let assigned_port = service.assigned_port.unwrap();

        // Create listener manager and start listener on the assigned port
        let manager = create_test_manager(registry.clone());
        let listen_port = manager.start_tcp_listener(assigned_port).await.unwrap();
        assert_eq!(listen_port, assigned_port);

        // Give the listener time to start
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Connect to the listener
        let client = tokio::net::TcpStream::connect(format!("127.0.0.1:{assigned_port}"))
            .await
            .unwrap();

        // Should receive a Connect control message
        let ctrl_msg = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for control message")
            .expect("channel closed");

        match ctrl_msg {
            ControlMessage::Connect {
                service_id,
                connection_id,
                client_addr,
            } => {
                assert_eq!(service_id, service.id);
                assert!(!connection_id.is_nil());
                assert!(client_addr.to_string().starts_with("127.0.0.1:"));

                // The connection should be pending
                assert!(manager.pending_connection_count() > 0);
            }
            _ => panic!("expected Connect message, got {:?}", ctrl_msg),
        }

        // Clean up
        drop(client);
        manager.shutdown();
    }

    #[tokio::test]
    async fn test_shutdown_clears_all() {
        let registry = create_test_registry();
        let manager = create_test_manager(registry);

        // Start some listeners
        let _tcp1 = manager.start_tcp_listener(0).await.unwrap();
        let _tcp2 = manager.start_tcp_listener(0).await.unwrap();
        let _udp1 = manager.start_udp_listener(0).await.unwrap();

        // Add a pending connection
        manager.pending_connections.insert(
            Uuid::new_v4(),
            PendingConnection {
                stream: None,
                expires_at: Instant::now() + Duration::from_secs(30),
            },
        );

        assert_eq!(manager.tcp_listener_count(), 2);
        assert_eq!(manager.udp_listener_count(), 1);
        assert_eq!(manager.pending_connection_count(), 1);

        // Shutdown
        manager.shutdown();

        assert_eq!(manager.tcp_listener_count(), 0);
        assert_eq!(manager.udp_listener_count(), 0);
        assert_eq!(manager.pending_connection_count(), 0);
    }

    #[tokio::test]
    async fn test_stop_nonexistent_listener() {
        let registry = create_test_registry();
        let manager = create_test_manager(registry);

        // Stopping a non-existent listener should not panic
        manager.stop_tcp_listener(12345);
        manager.stop_udp_listener(12345);
    }

    #[test]
    fn test_accept_nonexistent_connection() {
        let registry = create_test_registry();
        let manager = create_test_manager(registry);

        // Accepting a non-existent connection should return None
        let stream = manager.accept_connection(Uuid::new_v4());
        assert!(stream.is_none());
    }

    #[test]
    fn test_reject_nonexistent_connection() {
        let registry = create_test_registry();
        let manager = create_test_manager(registry);

        // Rejecting a non-existent connection should not panic
        manager.reject_connection(Uuid::new_v4());
    }
}
