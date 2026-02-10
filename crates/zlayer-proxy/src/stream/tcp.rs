//! TCP stream proxy service
//!
//! Implements raw TCP proxying using Pingora's ServerApp trait and a standalone
//! `serve()` method for use outside of Pingora's server infrastructure.
//! Provides bidirectional tunneling between clients and backends.

use async_trait::async_trait;
use pingora_core::apps::ServerApp;
use pingora_core::protocols::Stream;
use pingora_core::server::ShutdownWatch;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

/// Event type for tracking bidirectional copy operations
enum DuplexEvent {
    /// Data received from downstream (client)
    DownstreamRead(usize),
    /// Data received from upstream (backend)
    UpstreamRead(usize),
}

impl TcpStreamService {
    /// Create a new TCP stream service
    pub fn new(registry: Arc<StreamRegistry>, listen_port: u16) -> Self {
        Self {
            registry,
            listen_port,
        }
    }

    /// Get the listen port
    pub fn port(&self) -> u16 {
        self.listen_port
    }

    /// Get a reference to the registry
    pub fn registry(&self) -> &Arc<StreamRegistry> {
        &self.registry
    }

    /// Run a standalone TCP accept loop on the given listener.
    ///
    /// For each accepted connection, resolves a backend from the registry and
    /// spawns a task to perform bidirectional tunneling. This method runs
    /// indefinitely until the listener encounters a fatal error.
    ///
    /// This is the non-Pingora entry point, used by `ProxyManager` to serve
    /// TCP endpoints without requiring the full Pingora server infrastructure.
    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        tracing::info!(
            port = self.listen_port,
            "TCP stream proxy listening (standalone)"
        );

        loop {
            let (client_stream, client_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    // Transient errors (too many open files, etc.) — log and retry
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
        let service = match self.registry.resolve_tcp(self.listen_port) {
            Some(s) => s,
            None => {
                tracing::warn!(
                    port = self.listen_port,
                    client = %client_addr,
                    "No service registered for TCP port"
                );
                return;
            }
        };

        // Select backend using round-robin
        let backend = match service.select_backend() {
            Some(b) => b,
            None => {
                tracing::warn!(
                    port = self.listen_port,
                    service = %service.name,
                    client = %client_addr,
                    "No backends available for TCP service"
                );
                return;
            }
        };

        tracing::debug!(
            port = self.listen_port,
            service = %service.name,
            client = %client_addr,
            backend = %backend,
            "Proxying TCP connection (standalone)"
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

    /// Bidirectional data copy between two raw TcpStreams.
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

    /// Perform bidirectional data copy between downstream and upstream
    ///
    /// Uses fixed-size buffers and select! to handle both directions concurrently.
    /// Continues until one side closes or an error occurs.
    async fn duplex(&self, mut downstream: Stream, mut upstream: tokio::net::TcpStream) {
        // Fixed-size buffers for bidirectional copy
        // Using 8KB which is a good balance between memory and performance
        let mut upstream_buf = [0u8; 8192];
        let mut downstream_buf = [0u8; 8192];

        loop {
            let downstream_read = downstream.read(&mut upstream_buf);
            let upstream_read = upstream.read(&mut downstream_buf);

            let event: DuplexEvent;
            tokio::select! {
                result = downstream_read => {
                    match result {
                        Ok(n) => event = DuplexEvent::DownstreamRead(n),
                        Err(e) => {
                            tracing::debug!(error = %e, "downstream read error");
                            return;
                        }
                    }
                }
                result = upstream_read => {
                    match result {
                        Ok(n) => event = DuplexEvent::UpstreamRead(n),
                        Err(e) => {
                            tracing::debug!(error = %e, "upstream read error");
                            return;
                        }
                    }
                }
            }

            match event {
                DuplexEvent::DownstreamRead(0) => {
                    tracing::debug!("downstream connection closed");
                    return;
                }
                DuplexEvent::UpstreamRead(0) => {
                    tracing::debug!("upstream connection closed");
                    return;
                }
                DuplexEvent::DownstreamRead(n) => {
                    if let Err(e) = upstream.write_all(&upstream_buf[..n]).await {
                        tracing::debug!(error = %e, "upstream write error");
                        return;
                    }
                    if let Err(e) = upstream.flush().await {
                        tracing::debug!(error = %e, "upstream flush error");
                        return;
                    }
                }
                DuplexEvent::UpstreamRead(n) => {
                    if let Err(e) = downstream.write_all(&downstream_buf[..n]).await {
                        tracing::debug!(error = %e, "downstream write error");
                        return;
                    }
                    if let Err(e) = downstream.flush().await {
                        tracing::debug!(error = %e, "downstream flush error");
                        return;
                    }
                }
            }
        }
    }
}

#[async_trait]
impl ServerApp for TcpStreamService {
    async fn process_new(
        self: &Arc<Self>,
        io: Stream,
        _shutdown: &ShutdownWatch,
    ) -> Option<Stream> {
        // Resolve service for this port
        let service = match self.registry.resolve_tcp(self.listen_port) {
            Some(s) => s,
            None => {
                tracing::warn!(
                    port = self.listen_port,
                    "No service registered for TCP port"
                );
                return None;
            }
        };

        // Select backend using round-robin
        let backend: SocketAddr = match service.select_backend() {
            Some(b) => b,
            None => {
                tracing::warn!(
                    port = self.listen_port,
                    service = %service.name,
                    "No backends available for TCP service"
                );
                return None;
            }
        };

        tracing::debug!(
            port = self.listen_port,
            service = %service.name,
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
                    "Failed to connect to backend"
                );
                return None;
            }
        };

        // Perform bidirectional tunneling
        self.duplex(io, upstream).await;

        // Don't reuse the stream - TCP proxy connections are one-shot
        None
    }
}
