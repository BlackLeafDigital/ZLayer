//! TCP stream proxy service
//!
//! Implements raw TCP proxying with a standalone `serve()` method.
//! Provides bidirectional tunneling between clients and backends.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

use super::registry::StreamRegistry;

/// TCP stream proxy service
///
/// Listens on a port and proxies TCP connections to registered backends
/// using round-robin load balancing.
pub struct TcpStreamService {
    registry: Arc<StreamRegistry>,
    listen_port: u16,
}

impl TcpStreamService {
    /// Create a new TCP stream service
    #[must_use]
    pub fn new(registry: Arc<StreamRegistry>, listen_port: u16) -> Self {
        Self {
            registry,
            listen_port,
        }
    }

    /// Get the listen port
    #[must_use]
    pub fn port(&self) -> u16 {
        self.listen_port
    }

    /// Get a reference to the registry
    #[must_use]
    pub fn registry(&self) -> &Arc<StreamRegistry> {
        &self.registry
    }

    /// Run a standalone TCP accept loop on the given listener.
    ///
    /// For each accepted connection, resolves a backend from the registry and
    /// spawns a task to perform bidirectional tunneling. This method runs
    /// indefinitely until the listener encounters a fatal error.
    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        tracing::info!(port = self.listen_port, "TCP stream proxy listening");

        loop {
            let (client_stream, client_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    // Transient errors (too many open files, etc.) -- log and retry
                    tracing::warn!(
                        port = self.listen_port,
                        error = %e,
                        "TCP accept error, retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
            };

            let svc = Arc::clone(&self);
            tokio::spawn(async move {
                svc.handle_raw_connection(client_stream, client_addr).await;
            });
        }
    }

    /// Handle a single raw TCP connection (resolve backend, tunnel).
    async fn handle_raw_connection(
        &self,
        client_stream: tokio::net::TcpStream,
        client_addr: SocketAddr,
    ) {
        // Resolve service for this port
        let Some(service) = self.registry.resolve_tcp(self.listen_port) else {
            tracing::warn!(
                port = self.listen_port,
                client = %client_addr,
                "No service registered for TCP port"
            );
            return;
        };

        // Select backend using round-robin
        let Some(backend) = service.select_backend() else {
            tracing::warn!(
                port = self.listen_port,
                service = %service.name,
                client = %client_addr,
                "No backends available for TCP service"
            );
            return;
        };

        tracing::debug!(
            port = self.listen_port,
            service = %service.name,
            client = %client_addr,
            backend = %backend,
            "Proxying TCP connection"
        );

        // Connect to the upstream backend
        let upstream = match tokio::net::TcpStream::connect(backend).await {
            Ok(stream) => stream,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    backend = %backend,
                    service = %service.name,
                    client = %client_addr,
                    "Failed to connect to TCP backend"
                );
                return;
            }
        };

        // Perform bidirectional tunneling using raw TcpStreams
        Self::duplex_raw(client_stream, upstream).await;
    }

    /// Bidirectional data copy between two raw `TcpStreams`.
    ///
    /// Uses `tokio::io::copy_bidirectional` for efficient zero-copy-capable
    /// proxying when the OS supports it (e.g. splice on Linux).
    async fn duplex_raw(
        mut downstream: tokio::net::TcpStream,
        mut upstream: tokio::net::TcpStream,
    ) {
        match tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await {
            Ok((down_to_up, up_to_down)) => {
                tracing::debug!(
                    down_to_up = down_to_up,
                    up_to_down = up_to_down,
                    "TCP tunnel closed"
                );
            }
            Err(e) => {
                tracing::debug!(error = %e, "TCP tunnel error");
            }
        }
    }
}
