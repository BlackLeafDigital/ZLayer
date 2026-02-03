//! Local service proxy for tunnel client
//!
//! Handles proxying between the tunnel data channel and local services.
//! When the tunnel client receives an `IncomingConnection` event, it needs to:
//! 1. Connect to the local service (`127.0.0.1:local_port`)
//! 2. Open a data channel back to the server
//! 3. Proxy data bidirectionally between them

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

use crate::{Result, ServiceConfig, TunnelError};

/// Manages proxying connections to local services
pub struct LocalProxy {
    /// Service configurations by `service_id`
    services: DashMap<Uuid, ServiceConfig>,

    /// Active connections (wrapped in Arc for sharing with spawned tasks)
    connections: Arc<DashMap<Uuid, ConnectionHandle>>,

    /// Connection timeout
    connect_timeout: Duration,
}

struct ConnectionHandle {
    /// Abort handle for the proxy task
    abort_handle: tokio::task::AbortHandle,
}

impl LocalProxy {
    /// Create a new local proxy with the specified connection timeout
    #[must_use]
    pub fn new(connect_timeout: Duration) -> Self {
        Self {
            services: DashMap::new(),
            connections: Arc::new(DashMap::new()),
            connect_timeout,
        }
    }

    /// Register a service for proxying
    pub fn register_service(&self, service_id: Uuid, config: ServiceConfig) {
        self.services.insert(service_id, config);
    }

    /// Unregister a service
    pub fn unregister_service(&self, service_id: Uuid) {
        self.services.remove(&service_id);
    }

    /// Handle an incoming connection request
    ///
    /// This connects to the local service and returns a channel for the data stream.
    /// The caller is responsible for connecting the other end (server data channel).
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The `service_id` is not registered
    /// - Connection to the local service times out
    /// - Connection to the local service fails
    pub async fn handle_connection(
        &self,
        service_id: Uuid,
        connection_id: Uuid,
    ) -> Result<TcpStream> {
        let config = self
            .services
            .get(&service_id)
            .ok_or_else(|| TunnelError::registry(format!("Unknown service: {service_id}")))?
            .clone();

        let addr = format!("127.0.0.1:{}", config.local_port);

        let local_stream = tokio::time::timeout(self.connect_timeout, TcpStream::connect(&addr))
            .await
            .map_err(|_| TunnelError::timeout())?
            .map_err(|e| TunnelError::Connection { source: e })?;

        tracing::debug!(
            service_id = %service_id,
            connection_id = %connection_id,
            local_addr = %addr,
            "Connected to local service"
        );

        Ok(local_stream)
    }

    /// Proxy data between two streams bidirectionally
    ///
    /// Returns when either side closes or an error occurs.
    /// Returns the number of bytes (`sent_to_remote`, `received_from_remote`).
    ///
    /// # Errors
    ///
    /// This method does not return errors; it gracefully handles stream
    /// closure and I/O errors by terminating the proxy loop.
    pub async fn proxy_streams(local: TcpStream, remote: TcpStream) -> Result<(u64, u64)> {
        let (mut local_read, mut local_write) = local.into_split();
        let (mut remote_read, mut remote_write) = remote.into_split();

        let local_to_remote = async {
            let mut buf = vec![0u8; 8192];
            let mut total = 0u64;
            loop {
                match local_read.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if remote_write.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                        total += n as u64;
                    }
                }
            }
            let _ = remote_write.shutdown().await;
            total
        };

        let remote_to_local = async {
            let mut buf = vec![0u8; 8192];
            let mut total = 0u64;
            loop {
                match remote_read.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if local_write.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                        total += n as u64;
                    }
                }
            }
            let _ = local_write.shutdown().await;
            total
        };

        let (sent, received) = tokio::join!(local_to_remote, remote_to_local);

        Ok((sent, received))
    }

    /// Start a proxy task for a connection
    ///
    /// Spawns a background task that proxies data between the local and remote streams.
    /// The task is automatically cleaned up when the proxy completes or is cancelled.
    pub fn start_proxy(
        &self,
        connection_id: Uuid,
        local: TcpStream,
        remote: TcpStream,
    ) -> tokio::task::JoinHandle<Result<(u64, u64)>> {
        let connections = Arc::clone(&self.connections);

        let handle = tokio::spawn(async move {
            let result = Self::proxy_streams(local, remote).await;
            connections.remove(&connection_id);
            result
        });

        self.connections.insert(
            connection_id,
            ConnectionHandle {
                abort_handle: handle.abort_handle(),
            },
        );

        handle
    }

    /// Cancel a proxy connection
    pub fn cancel_connection(&self, connection_id: Uuid) {
        if let Some((_, handle)) = self.connections.remove(&connection_id) {
            handle.abort_handle.abort();
        }
    }

    /// Get count of active connections
    #[must_use]
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Get count of registered services
    #[must_use]
    pub fn service_count(&self) -> usize {
        self.services.len()
    }

    /// Clean up all connections
    pub fn shutdown(&self) {
        for item in self.connections.iter() {
            item.abort_handle.abort();
        }
        self.connections.clear();
    }
}

impl Drop for LocalProxy {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_unregister_services() {
        let proxy = LocalProxy::new(Duration::from_secs(5));

        let service_id1 = Uuid::new_v4();
        let service_id2 = Uuid::new_v4();
        let config1 = ServiceConfig::tcp("ssh", 22);
        let config2 = ServiceConfig::tcp("postgres", 5432);

        // Register services
        proxy.register_service(service_id1, config1);
        proxy.register_service(service_id2, config2);
        assert_eq!(proxy.service_count(), 2);

        // Verify services are registered
        assert!(proxy.services.contains_key(&service_id1));
        assert!(proxy.services.contains_key(&service_id2));

        // Unregister one service
        proxy.unregister_service(service_id1);
        assert_eq!(proxy.service_count(), 1);
        assert!(!proxy.services.contains_key(&service_id1));
        assert!(proxy.services.contains_key(&service_id2));

        // Unregister the other
        proxy.unregister_service(service_id2);
        assert_eq!(proxy.service_count(), 0);
    }

    #[test]
    fn test_service_and_connection_counts() {
        let proxy = LocalProxy::new(Duration::from_secs(5));

        assert_eq!(proxy.service_count(), 0);
        assert_eq!(proxy.connection_count(), 0);

        let service_id = Uuid::new_v4();
        proxy.register_service(service_id, ServiceConfig::tcp("test", 8080));
        assert_eq!(proxy.service_count(), 1);
        assert_eq!(proxy.connection_count(), 0);

        proxy.unregister_service(service_id);
        assert_eq!(proxy.service_count(), 0);
    }

    #[test]
    fn test_cancel_connection() {
        let proxy = LocalProxy::new(Duration::from_secs(5));
        let connection_id = Uuid::new_v4();

        // Cancelling a non-existent connection should not panic
        proxy.cancel_connection(connection_id);
        assert_eq!(proxy.connection_count(), 0);
    }

    #[test]
    fn test_shutdown_clears_all() {
        let proxy = LocalProxy::new(Duration::from_secs(5));

        // Register some services
        for _ in 0..5 {
            proxy.register_service(Uuid::new_v4(), ServiceConfig::tcp("test", 8080));
        }
        assert_eq!(proxy.service_count(), 5);

        // Shutdown clears connections (services remain registered)
        proxy.shutdown();
        assert_eq!(proxy.connection_count(), 0);
        // Note: shutdown only clears connections, not services
        assert_eq!(proxy.service_count(), 5);
    }

    #[tokio::test]
    async fn test_handle_connection_unknown_service() {
        let proxy = LocalProxy::new(Duration::from_secs(5));
        let service_id = Uuid::new_v4();
        let connection_id = Uuid::new_v4();

        let result = proxy.handle_connection(service_id, connection_id).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, TunnelError::Registry { .. }));
    }

    #[tokio::test]
    async fn test_handle_connection_timeout() {
        let proxy = LocalProxy::new(Duration::from_millis(100));
        let service_id = Uuid::new_v4();
        let connection_id = Uuid::new_v4();

        // Register a service pointing to a port that's unlikely to be listening
        proxy.register_service(service_id, ServiceConfig::tcp("test", 65432));

        let result = proxy.handle_connection(service_id, connection_id).await;
        assert!(result.is_err());

        // Should be either timeout or connection refused
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            TunnelError::Timeout | TunnelError::Connection { .. }
        ));
    }

    #[test]
    fn test_unregister_nonexistent_service() {
        let proxy = LocalProxy::new(Duration::from_secs(5));
        let service_id = Uuid::new_v4();

        // Should not panic
        proxy.unregister_service(service_id);
        assert_eq!(proxy.service_count(), 0);
    }

    #[test]
    fn test_register_overwrites_existing() {
        let proxy = LocalProxy::new(Duration::from_secs(5));
        let service_id = Uuid::new_v4();

        proxy.register_service(service_id, ServiceConfig::tcp("first", 8080));
        assert_eq!(proxy.service_count(), 1);

        // Register with same ID should overwrite
        proxy.register_service(service_id, ServiceConfig::tcp("second", 9090));
        assert_eq!(proxy.service_count(), 1);

        // Verify the config was updated
        let config = proxy.services.get(&service_id).unwrap();
        assert_eq!(config.name, "second");
        assert_eq!(config.local_port, 9090);
    }
}
