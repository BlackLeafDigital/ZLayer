//! Access session management

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::client::LocalProxy;
use crate::{Result, ServiceProtocol, TunnelError};

/// An access session provides temporary local access to a remote service
#[derive(Debug)]
pub struct AccessSession {
    /// Unique session ID
    pub id: Uuid,

    /// Service/endpoint being accessed
    pub endpoint: String,

    /// Local address (localhost:port)
    pub local_addr: SocketAddr,

    /// Remote address (service address)
    pub remote_addr: String,

    /// Protocol
    pub protocol: ServiceProtocol,

    /// When session was created
    pub created_at: Instant,

    /// When session expires (None = no expiry)
    pub expires_at: Option<Instant>,

    /// Bytes transferred in
    pub bytes_in: AtomicU64,

    /// Bytes transferred out
    pub bytes_out: AtomicU64,

    /// Connection count
    pub connection_count: AtomicU32,
}

impl AccessSession {
    /// Create a new access session
    #[must_use]
    pub fn new(endpoint: String, local_addr: SocketAddr, remote_addr: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            endpoint,
            local_addr,
            remote_addr,
            protocol: ServiceProtocol::Tcp,
            created_at: Instant::now(),
            expires_at: None,
            bytes_in: AtomicU64::new(0),
            bytes_out: AtomicU64::new(0),
            connection_count: AtomicU32::new(0),
        }
    }

    /// Set a TTL for the session
    #[must_use]
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.expires_at = Some(Instant::now() + ttl);
        self
    }

    /// Set the protocol for the session
    #[must_use]
    pub fn with_protocol(mut self, protocol: ServiceProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Check if the session has expired
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|e| Instant::now() > e)
    }

    /// Get the remaining TTL for the session
    #[must_use]
    pub fn remaining_ttl(&self) -> Option<Duration> {
        self.expires_at
            .and_then(|e| e.checked_duration_since(Instant::now()))
    }

    /// Add bytes to the transfer counters
    pub fn add_bytes(&self, bytes_in: u64, bytes_out: u64) {
        self.bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
        self.bytes_out.fetch_add(bytes_out, Ordering::Relaxed);
    }

    /// Increment the connection count
    pub fn increment_connections(&self) {
        self.connection_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement the connection count
    pub fn decrement_connections(&self) {
        self.connection_count.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get the current bytes in count
    #[must_use]
    pub fn get_bytes_in(&self) -> u64 {
        self.bytes_in.load(Ordering::Relaxed)
    }

    /// Get the current bytes out count
    #[must_use]
    pub fn get_bytes_out(&self) -> u64 {
        self.bytes_out.load(Ordering::Relaxed)
    }

    /// Get the current connection count
    #[must_use]
    pub fn get_connection_count(&self) -> u32 {
        self.connection_count.load(Ordering::Relaxed)
    }
}

/// Session info for display
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// Unique session ID
    pub id: Uuid,
    /// Service/endpoint being accessed
    pub endpoint: String,
    /// Local address (localhost:port)
    pub local_addr: SocketAddr,
    /// Remote address (service address)
    pub remote_addr: String,
    /// Protocol
    pub protocol: ServiceProtocol,
    /// When session was created
    pub created_at: Instant,
    /// When session expires (None = no expiry)
    pub expires_at: Option<Instant>,
    /// Remaining TTL
    pub remaining: Option<Duration>,
    /// Bytes transferred in
    pub bytes_in: u64,
    /// Bytes transferred out
    pub bytes_out: u64,
    /// Active connection count
    pub connections: u32,
}

impl From<&AccessSession> for SessionInfo {
    fn from(session: &AccessSession) -> Self {
        Self {
            id: session.id,
            endpoint: session.endpoint.clone(),
            local_addr: session.local_addr,
            remote_addr: session.remote_addr.clone(),
            protocol: session.protocol,
            created_at: session.created_at,
            expires_at: session.expires_at,
            remaining: session.remaining_ttl(),
            bytes_in: session.get_bytes_in(),
            bytes_out: session.get_bytes_out(),
            connections: session.get_connection_count(),
        }
    }
}

/// Manages access sessions
pub struct AccessManager {
    /// Active sessions by ID
    sessions: DashMap<Uuid, Arc<AccessSession>>,

    /// Session shutdown handles
    shutdown_handles: DashMap<Uuid, oneshot::Sender<()>>,

    /// Default TTL for new sessions
    default_ttl: Option<Duration>,
}

impl AccessManager {
    /// Create a new access manager
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            shutdown_handles: DashMap::new(),
            default_ttl: None,
        }
    }

    /// Set a default TTL for new sessions
    #[must_use]
    pub fn with_default_ttl(mut self, ttl: Duration) -> Self {
        self.default_ttl = Some(ttl);
        self
    }

    /// Start a new access session
    ///
    /// Creates a local TCP listener that proxies to the remote service.
    /// Returns the session and the local address to connect to.
    ///
    /// # Errors
    ///
    /// Returns an error if binding to the local port fails.
    pub async fn start_session(
        &self,
        endpoint: String,
        remote_addr: String,
        local_port: Option<u16>,
        ttl: Option<Duration>,
    ) -> Result<Arc<AccessSession>> {
        // Bind to local port (0 = auto-assign)
        let bind_addr = format!("127.0.0.1:{}", local_port.unwrap_or(0));
        let listener = TcpListener::bind(&bind_addr)
            .await
            .map_err(TunnelError::connection)?;

        let local_addr = listener.local_addr().map_err(TunnelError::connection)?;

        let session_ttl = ttl.or(self.default_ttl);
        let mut session = AccessSession::new(endpoint, local_addr, remote_addr.clone());
        if let Some(ttl) = session_ttl {
            session = session.with_ttl(ttl);
        }

        let session = Arc::new(session);
        let session_id = session.id;

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // Spawn listener task
        let session_clone = session.clone();
        let sessions = self.sessions.clone();
        tokio::spawn(async move {
            Self::run_listener(listener, session_clone.clone(), remote_addr, shutdown_rx).await;
            sessions.remove(&session_id);
        });

        self.sessions.insert(session_id, session.clone());
        self.shutdown_handles.insert(session_id, shutdown_tx);

        tracing::info!(
            session_id = %session_id,
            endpoint = %session.endpoint,
            local_addr = %local_addr,
            ttl = ?session_ttl,
            "Access session started"
        );

        Ok(session)
    }

    async fn run_listener(
        listener: TcpListener,
        session: Arc<AccessSession>,
        remote_addr: String,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) {
        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((local_stream, _client_addr)) => {
                            // Check expiry
                            if session.is_expired() {
                                tracing::info!(session_id = %session.id, "Session expired");
                                break;
                            }

                            session.increment_connections();

                            let session_clone = session.clone();
                            let remote = remote_addr.clone();

                            tokio::spawn(async move {
                                // Connect to remote
                                match tokio::net::TcpStream::connect(&remote).await {
                                    Ok(remote_stream) => {
                                        // Proxy streams
                                        let (sent, received) = LocalProxy::proxy_streams(
                                            local_stream,
                                            remote_stream,
                                        )
                                        .await
                                        .unwrap_or((0, 0));

                                        session_clone.add_bytes(received, sent);
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            error = %e,
                                            remote = %remote,
                                            "Failed to connect to remote service"
                                        );
                                    }
                                }

                                session_clone.decrement_connections();
                            });
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Accept error");
                        }
                    }
                }

                _ = &mut shutdown_rx => {
                    tracing::info!(session_id = %session.id, "Session shutdown requested");
                    break;
                }
            }
        }
    }

    /// Stop an access session
    #[must_use]
    pub fn stop_session(&self, session_id: Uuid) -> Option<Arc<AccessSession>> {
        if let Some((_, shutdown_tx)) = self.shutdown_handles.remove(&session_id) {
            let _ = shutdown_tx.send(());
        }
        self.sessions.remove(&session_id).map(|(_, s)| s)
    }

    /// Get a session by ID
    #[must_use]
    pub fn get_session(&self, session_id: Uuid) -> Option<Arc<AccessSession>> {
        self.sessions.get(&session_id).map(|s| s.clone())
    }

    /// List all active sessions
    #[must_use]
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .iter()
            .map(|s| SessionInfo::from(s.value().as_ref()))
            .collect()
    }

    /// Get session count
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Clean up expired sessions
    #[must_use]
    pub fn cleanup_expired(&self) -> usize {
        let expired: Vec<_> = self
            .sessions
            .iter()
            .filter(|s| s.is_expired())
            .map(|s| s.id)
            .collect();

        let count = expired.len();
        for id in expired {
            let _ = self.stop_session(id);
        }
        count
    }

    /// Shutdown all sessions
    pub fn shutdown(&self) {
        let ids: Vec<_> = self.sessions.iter().map(|s| s.id).collect();
        for id in ids {
            let _ = self.stop_session(id);
        }
    }
}

impl Default for AccessManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AccessManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_session_creation() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let session =
            AccessSession::new("test-service".to_string(), addr, "10.0.0.1:80".to_string());

        assert_eq!(session.endpoint, "test-service");
        assert_eq!(session.local_addr, addr);
        assert_eq!(session.remote_addr, "10.0.0.1:80");
        assert_eq!(session.protocol, ServiceProtocol::Tcp);
        assert!(session.expires_at.is_none());
        assert!(!session.is_expired());
    }

    #[test]
    fn test_access_session_with_ttl() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let ttl = Duration::from_secs(3600);
        let session =
            AccessSession::new("test-service".to_string(), addr, "10.0.0.1:80".to_string())
                .with_ttl(ttl);

        assert!(session.expires_at.is_some());
        assert!(!session.is_expired());

        let remaining = session.remaining_ttl();
        assert!(remaining.is_some());
        // Should be close to 3600 seconds (within a second tolerance)
        let remaining_secs = remaining.unwrap().as_secs();
        assert!((3599..=3600).contains(&remaining_secs));
    }

    #[test]
    fn test_access_session_expired() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        // Create a session that expires immediately (0 duration TTL)
        let session =
            AccessSession::new("test-service".to_string(), addr, "10.0.0.1:80".to_string())
                .with_ttl(Duration::ZERO);

        // Should be expired immediately
        assert!(session.is_expired());
        assert!(session.remaining_ttl().is_none());
    }

    #[test]
    fn test_access_session_with_protocol() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let session =
            AccessSession::new("test-service".to_string(), addr, "10.0.0.1:80".to_string())
                .with_protocol(ServiceProtocol::Udp);

        assert_eq!(session.protocol, ServiceProtocol::Udp);
    }

    #[test]
    fn test_access_session_bytes_tracking() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let session =
            AccessSession::new("test-service".to_string(), addr, "10.0.0.1:80".to_string());

        assert_eq!(session.get_bytes_in(), 0);
        assert_eq!(session.get_bytes_out(), 0);

        session.add_bytes(100, 200);
        assert_eq!(session.get_bytes_in(), 100);
        assert_eq!(session.get_bytes_out(), 200);

        session.add_bytes(50, 75);
        assert_eq!(session.get_bytes_in(), 150);
        assert_eq!(session.get_bytes_out(), 275);
    }

    #[test]
    fn test_access_session_connection_tracking() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let session =
            AccessSession::new("test-service".to_string(), addr, "10.0.0.1:80".to_string());

        assert_eq!(session.get_connection_count(), 0);

        session.increment_connections();
        assert_eq!(session.get_connection_count(), 1);

        session.increment_connections();
        session.increment_connections();
        assert_eq!(session.get_connection_count(), 3);

        session.decrement_connections();
        assert_eq!(session.get_connection_count(), 2);
    }

    #[test]
    fn test_session_info_from_access_session() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let ttl = Duration::from_secs(3600);
        let session =
            AccessSession::new("test-service".to_string(), addr, "10.0.0.1:80".to_string())
                .with_ttl(ttl);

        session.add_bytes(100, 200);
        session.increment_connections();

        let info = SessionInfo::from(&session);

        assert_eq!(info.id, session.id);
        assert_eq!(info.endpoint, "test-service");
        assert_eq!(info.local_addr, addr);
        assert_eq!(info.remote_addr, "10.0.0.1:80");
        assert_eq!(info.protocol, ServiceProtocol::Tcp);
        assert!(info.expires_at.is_some());
        assert!(info.remaining.is_some());
        assert_eq!(info.bytes_in, 100);
        assert_eq!(info.bytes_out, 200);
        assert_eq!(info.connections, 1);
    }

    #[test]
    fn test_access_manager_new() {
        let manager = AccessManager::new();
        assert_eq!(manager.session_count(), 0);
        assert!(manager.list_sessions().is_empty());
    }

    #[test]
    fn test_access_manager_with_default_ttl() {
        let manager = AccessManager::new().with_default_ttl(Duration::from_secs(3600));
        assert!(manager.default_ttl.is_some());
        assert_eq!(manager.default_ttl.unwrap(), Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn test_access_manager_start_session() {
        let manager = AccessManager::new();

        let session = manager
            .start_session(
                "test-endpoint".to_string(),
                "127.0.0.1:9999".to_string(),
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(session.endpoint, "test-endpoint");
        assert_eq!(session.remote_addr, "127.0.0.1:9999");
        assert_eq!(manager.session_count(), 1);

        // Stop the session
        let stopped = manager.stop_session(session.id);
        assert!(stopped.is_some());

        // Give the listener task time to clean up
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(manager.session_count(), 0);
    }

    #[tokio::test]
    async fn test_access_manager_start_session_with_port() {
        let manager = AccessManager::new();

        // Use a high port number to avoid conflicts
        let session = manager
            .start_session(
                "test-endpoint".to_string(),
                "127.0.0.1:9999".to_string(),
                Some(0), // Auto-assign port
                None,
            )
            .await
            .unwrap();

        // Verify the session got a local address
        assert_eq!(session.local_addr.ip().to_string(), "127.0.0.1");
        assert!(session.local_addr.port() > 0);

        let _ = manager.stop_session(session.id);
    }

    #[tokio::test]
    async fn test_access_manager_start_session_with_ttl() {
        let manager = AccessManager::new();

        let ttl = Duration::from_secs(3600);
        let session = manager
            .start_session(
                "test-endpoint".to_string(),
                "127.0.0.1:9999".to_string(),
                None,
                Some(ttl),
            )
            .await
            .unwrap();

        assert!(session.expires_at.is_some());
        assert!(!session.is_expired());

        let _ = manager.stop_session(session.id);
    }

    #[tokio::test]
    async fn test_access_manager_get_session() {
        let manager = AccessManager::new();

        let session = manager
            .start_session(
                "test-endpoint".to_string(),
                "127.0.0.1:9999".to_string(),
                None,
                None,
            )
            .await
            .unwrap();

        let session_id = session.id;

        // Get existing session
        let retrieved = manager.get_session(session_id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, session_id);

        // Get non-existent session
        let not_found = manager.get_session(Uuid::new_v4());
        assert!(not_found.is_none());

        let _ = manager.stop_session(session_id);
    }

    #[tokio::test]
    async fn test_access_manager_list_sessions() {
        let manager = AccessManager::new();

        let session1 = manager
            .start_session(
                "endpoint-1".to_string(),
                "127.0.0.1:9991".to_string(),
                None,
                None,
            )
            .await
            .unwrap();

        let session2 = manager
            .start_session(
                "endpoint-2".to_string(),
                "127.0.0.1:9992".to_string(),
                None,
                None,
            )
            .await
            .unwrap();

        let all_sessions = manager.list_sessions();
        assert_eq!(all_sessions.len(), 2);

        let endpoints: Vec<_> = all_sessions.iter().map(|s| s.endpoint.as_str()).collect();
        assert!(endpoints.contains(&"endpoint-1"));
        assert!(endpoints.contains(&"endpoint-2"));

        let _ = manager.stop_session(session1.id);
        let _ = manager.stop_session(session2.id);
    }

    #[tokio::test]
    async fn test_access_manager_cleanup_expired() {
        let manager = AccessManager::new();

        // Create a session that expires immediately
        let session = manager
            .start_session(
                "test-endpoint".to_string(),
                "127.0.0.1:9999".to_string(),
                None,
                Some(Duration::ZERO),
            )
            .await
            .unwrap();

        let session_id = session.id;

        // Session should be expired
        assert!(manager.get_session(session_id).unwrap().is_expired());

        // Cleanup expired sessions
        let cleaned = manager.cleanup_expired();
        assert_eq!(cleaned, 1);

        // Give the listener task time to clean up
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(manager.session_count(), 0);
    }

    #[tokio::test]
    async fn test_access_manager_shutdown() {
        let manager = AccessManager::new();

        let _session1 = manager
            .start_session(
                "endpoint-1".to_string(),
                "127.0.0.1:9991".to_string(),
                None,
                None,
            )
            .await
            .unwrap();

        let _session2 = manager
            .start_session(
                "endpoint-2".to_string(),
                "127.0.0.1:9992".to_string(),
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(manager.session_count(), 2);

        manager.shutdown();

        // Give the listener tasks time to clean up
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(manager.session_count(), 0);
    }

    #[test]
    fn test_access_manager_default() {
        let manager = AccessManager::default();
        assert_eq!(manager.session_count(), 0);
    }

    #[tokio::test]
    async fn test_access_manager_stop_nonexistent_session() {
        let manager = AccessManager::new();

        let result = manager.stop_session(Uuid::new_v4());
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_access_manager_with_default_ttl_applied() {
        let manager = AccessManager::new().with_default_ttl(Duration::from_secs(3600));

        // Start session without explicit TTL - should use default
        let session = manager
            .start_session(
                "test-endpoint".to_string(),
                "127.0.0.1:9999".to_string(),
                None,
                None, // No explicit TTL
            )
            .await
            .unwrap();

        assert!(session.expires_at.is_some());
        assert!(!session.is_expired());

        let _ = manager.stop_session(session.id);
    }

    #[tokio::test]
    async fn test_access_manager_explicit_ttl_overrides_default() {
        let manager = AccessManager::new().with_default_ttl(Duration::from_secs(3600));

        // Start session with explicit TTL that differs from default
        let explicit_ttl = Duration::from_secs(60);
        let session = manager
            .start_session(
                "test-endpoint".to_string(),
                "127.0.0.1:9999".to_string(),
                None,
                Some(explicit_ttl),
            )
            .await
            .unwrap();

        // Remaining TTL should be close to 60 seconds, not 3600
        let remaining = session.remaining_ttl().unwrap();
        assert!(remaining.as_secs() <= 60);

        let _ = manager.stop_session(session.id);
    }
}
