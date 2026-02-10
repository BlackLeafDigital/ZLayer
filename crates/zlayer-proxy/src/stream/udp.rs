//! UDP stream proxy service
//!
//! Custom UDP proxy implementation with session tracking.
//! Each client gets a dedicated session that maps to a backend.

use dashmap::DashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use super::config::DEFAULT_UDP_SESSION_TIMEOUT;
use super::registry::StreamRegistry;

/// UDP session state
struct UdpSession {
    /// Backend address for this session
    backend: SocketAddr,
    /// Socket to communicate with backend (bound to ephemeral port)
    backend_socket: Arc<UdpSocket>,
    /// Last activity timestamp
    last_activity: Instant,
}

/// UDP stream proxy service
///
/// Listens on a port and proxies UDP datagrams to registered backends.
/// Maintains session state to route responses back to the correct client.
pub struct UdpStreamService {
    registry: Arc<StreamRegistry>,
    listen_port: u16,
    session_timeout: Duration,
}

impl UdpStreamService {
    /// Create a new UDP stream service
    pub fn new(
        registry: Arc<StreamRegistry>,
        listen_port: u16,
        session_timeout: Option<Duration>,
    ) -> Self {
        Self {
            registry,
            listen_port,
            session_timeout: session_timeout.unwrap_or(DEFAULT_UDP_SESSION_TIMEOUT),
        }
    }

    /// Get the listen port
    pub fn port(&self) -> u16 {
        self.listen_port
    }

    /// Get the session timeout
    pub fn session_timeout(&self) -> Duration {
        self.session_timeout
    }

    /// Get a reference to the registry
    pub fn registry(&self) -> &Arc<StreamRegistry> {
        &self.registry
    }

    /// Run the UDP proxy service by binding its own socket.
    ///
    /// This method runs indefinitely, proxying UDP datagrams between
    /// clients and backends. Each client address gets its own session.
    pub async fn run(self: Arc<Self>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Bind to listen port
        let listen_addr = format!("0.0.0.0:{}", self.listen_port);
        let socket = UdpSocket::bind(&listen_addr).await?;

        tracing::info!(port = self.listen_port, "UDP stream proxy listening");

        self.serve(socket).await
    }

    /// Run the UDP proxy service on an externally-provided socket.
    ///
    /// This is the non-self-binding entry point, used by `ProxyManager` to serve
    /// UDP endpoints when the caller has already bound the socket.
    ///
    /// Runs indefinitely, proxying UDP datagrams between clients and backends.
    /// Each client address gets its own session with a dedicated backend socket.
    pub async fn serve(
        self: Arc<Self>,
        socket: UdpSocket,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let socket = Arc::new(socket);

        tracing::info!(
            port = self.listen_port,
            "UDP stream proxy serving (standalone)"
        );

        // Session tracking: client_addr -> session
        let sessions: Arc<DashMap<SocketAddr, UdpSession>> = Arc::new(DashMap::new());

        // Channel for backend responses to be sent back to clients
        let (response_tx, mut response_rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(4096);

        // Spawn response sender task
        let socket_for_responses = socket.clone();
        tokio::spawn(async move {
            while let Some((data, client_addr)) = response_rx.recv().await {
                if let Err(e) = socket_for_responses.send_to(&data, client_addr).await {
                    tracing::debug!(
                        error = %e,
                        client = %client_addr,
                        "Failed to send UDP response to client"
                    );
                }
            }
        });

        // Spawn session cleanup task
        let sessions_for_cleanup = sessions.clone();
        let timeout = self.session_timeout;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                let now = Instant::now();
                let before = sessions_for_cleanup.len();
                sessions_for_cleanup
                    .retain(|_, session| now.duration_since(session.last_activity) < timeout);
                let after = sessions_for_cleanup.len();
                if before != after {
                    tracing::debug!(
                        removed = before - after,
                        remaining = after,
                        "Cleaned up expired UDP sessions"
                    );
                }
            }
        });

        // Main receive loop
        let mut buf = vec![0u8; 65535];
        loop {
            let (len, client_addr) = socket.recv_from(&mut buf).await?;
            let data = buf[..len].to_vec();

            // Get or create session for this client
            let session_backend = if let Some(mut existing) = sessions.get_mut(&client_addr) {
                existing.last_activity = Instant::now();
                existing.backend
            } else {
                // Create new session
                let service = match self.registry.resolve_udp(self.listen_port) {
                    Some(s) => s,
                    None => {
                        tracing::warn!(
                            port = self.listen_port,
                            client = %client_addr,
                            "No service registered for UDP port"
                        );
                        continue;
                    }
                };

                let backend = match service.select_backend() {
                    Some(b) => b,
                    None => {
                        tracing::warn!(
                            port = self.listen_port,
                            service = %service.name,
                            client = %client_addr,
                            "No backends available for UDP service"
                        );
                        continue;
                    }
                };

                // Create dedicated socket for this session's backend communication
                let backend_socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
                backend_socket.connect(&backend).await?;

                tracing::debug!(
                    port = self.listen_port,
                    service = %service.name,
                    client = %client_addr,
                    backend = %backend,
                    "Created new UDP session"
                );

                // Spawn task to receive responses from backend
                let backend_socket_recv = backend_socket.clone();
                let response_tx = response_tx.clone();
                let client = client_addr;
                let sessions_ref = sessions.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65535];
                    loop {
                        match backend_socket_recv.recv(&mut buf).await {
                            Ok(len) => {
                                // Update session activity
                                if let Some(mut s) = sessions_ref.get_mut(&client) {
                                    s.last_activity = Instant::now();
                                }
                                // Send response back to client
                                if response_tx
                                    .send((buf[..len].to_vec(), client))
                                    .await
                                    .is_err()
                                {
                                    break; // Channel closed
                                }
                            }
                            Err(e) => {
                                tracing::debug!(
                                    error = %e,
                                    client = %client,
                                    "Backend socket receive error"
                                );
                                break;
                            }
                        }
                    }
                });

                let session = UdpSession {
                    backend,
                    backend_socket,
                    last_activity: Instant::now(),
                };
                sessions.insert(client_addr, session);
                backend
            };

            // Forward packet to backend
            if let Some(s) = sessions.get(&client_addr) {
                if let Err(e) = s.backend_socket.send(&data).await {
                    tracing::debug!(
                        error = %e,
                        client = %client_addr,
                        backend = %session_backend,
                        "Failed to forward UDP packet to backend"
                    );
                }
            }
        }
    }
}
