//! API server

use std::net::SocketAddr;
use std::path::Path;
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;
use tracing::info;

use axum::Router;

use crate::config::ApiConfig;
use crate::router::build_router;

/// API server
pub struct ApiServer {
    config: ApiConfig,
}

impl ApiServer {
    /// Create a new API server
    pub fn new(config: ApiConfig) -> Self {
        Self { config }
    }

    /// Get the bind address
    pub fn bind_addr(&self) -> SocketAddr {
        self.config.bind
    }

    /// Run the server
    pub async fn run(self) -> anyhow::Result<()> {
        let addr = self.config.bind;
        let router = build_router(&self.config);

        info!(
            bind = %addr,
            swagger = self.config.swagger_enabled,
            rate_limit = self.config.rate_limit.enabled,
            "Starting API server"
        );

        let listener = TcpListener::bind(addr).await?;

        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;

        Ok(())
    }

    /// Run the server with graceful shutdown
    pub async fn run_with_shutdown(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<()> {
        let addr = self.config.bind;
        let router = build_router(&self.config);

        info!(
            bind = %addr,
            swagger = self.config.swagger_enabled,
            "Starting API server with graceful shutdown"
        );

        let listener = TcpListener::bind(addr).await?;

        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await?;

        info!("API server shut down");
        Ok(())
    }

    /// Run the server on both TCP and Unix socket simultaneously with graceful shutdown.
    ///
    /// This serves the same router on a TCP listener (for external/network access) and
    /// a Unix domain socket (for local CLI-to-daemon IPC) concurrently. Both listeners
    /// are shut down gracefully when the provided shutdown future completes.
    ///
    /// The TCP listener preserves `ConnectInfo<SocketAddr>` for handlers that need
    /// client address information. The Unix listener uses plain `into_make_service()`
    /// since Unix sockets don't have TCP socket addresses.
    ///
    /// # Arguments
    /// * `tcp_addr` - TCP address to bind (e.g. `0.0.0.0:3669`)
    /// * `unix_path` - Path for the Unix domain socket file (e.g. `/run/zlayer/api.sock`)
    /// * `router` - Pre-built axum Router to serve on both listeners
    /// * `shutdown` - Future that completes when shutdown is requested
    #[cfg(unix)]
    pub async fn run_dual(
        tcp_addr: SocketAddr,
        unix_path: impl AsRef<Path>,
        router: Router,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let unix_path = unix_path.as_ref().to_path_buf();

        // Clean up stale socket file if it exists
        let _ = std::fs::remove_file(&unix_path);

        // Ensure parent directory exists
        if let Some(parent) = unix_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Bind TCP listener
        let tcp_listener = TcpListener::bind(tcp_addr).await?;
        let tcp_local_addr = tcp_listener.local_addr()?;

        // Bind Unix listener
        let unix_listener = UnixListener::bind(&unix_path)?;

        // Set Unix socket permissions to 0o660 (owner + group read/write)
        std::fs::set_permissions(&unix_path, std::fs::Permissions::from_mode(0o660))?;

        info!(
            tcp = %tcp_local_addr,
            unix = %unix_path.display(),
            "Starting API server on TCP and Unix socket"
        );

        // Use a watch channel to fan out the shutdown signal to both servers
        let (shutdown_tx, mut shutdown_rx_tcp) = tokio::sync::watch::channel(false);
        let mut shutdown_rx_unix = shutdown_tx.subscribe();

        // Spawn a task that awaits the caller's shutdown future, then signals both servers
        tokio::spawn(async move {
            shutdown.await;
            let _ = shutdown_tx.send(true);
        });

        // Clone the router so each server gets its own copy
        let tcp_router = router.clone();
        let unix_router = router.clone();

        // TCP server with ConnectInfo<SocketAddr> for existing handlers
        let tcp_server = axum::serve(
            tcp_listener,
            tcp_router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx_tcp.wait_for(|&v| v).await;
        });

        // Unix socket server without ConnectInfo (no TCP addresses on UDS)
        let unix_server = axum::serve(unix_listener, unix_router.into_make_service())
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx_unix.wait_for(|&v| v).await;
            });

        // Run both servers concurrently, stop when either errors or both finish
        let result = tokio::try_join!(
            async { tcp_server.await.map_err(anyhow::Error::from) },
            async { unix_server.await.map_err(anyhow::Error::from) },
        );

        // Clean up the Unix socket file
        let _ = std::fs::remove_file(&unix_path);

        info!("API server (dual) shut down");

        result.map(|_| ())
    }

    /// Run dual TCP + Unix socket server with automatic admin auth on the Unix socket.
    ///
    /// This variant creates a long-lived admin JWT and injects it into every
    /// Unix socket request that lacks an `Authorization` header, providing
    /// transparent authentication for local CLI-to-daemon communication.
    ///
    /// # Arguments
    /// * `tcp_addr` - TCP address to bind
    /// * `unix_path` - Path for the Unix domain socket file
    /// * `router` - Pre-built axum Router
    /// * `jwt_secret` - JWT secret used to mint the local admin token
    /// * `shutdown` - Future that completes when shutdown is requested
    #[cfg(unix)]
    pub async fn run_dual_with_local_auth(
        tcp_addr: SocketAddr,
        unix_path: impl AsRef<Path>,
        router: Router,
        jwt_secret: &str,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<()> {
        use axum::http::header::AUTHORIZATION;
        use axum::middleware;
        use std::os::unix::fs::PermissionsExt;

        let unix_path = unix_path.as_ref().to_path_buf();

        // Clean up stale socket file if it exists
        let _ = std::fs::remove_file(&unix_path);

        // Ensure parent directory exists
        if let Some(parent) = unix_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Mint a long-lived (24h) admin token for local socket connections
        let local_token = crate::auth::create_token(
            jwt_secret,
            "local-admin",
            std::time::Duration::from_secs(86400),
            vec!["admin".to_string()],
        )
        .map_err(|e| anyhow::anyhow!("Failed to mint local admin token: {}", e))?;

        let bearer = format!("Bearer {}", local_token);

        // Bind TCP listener
        let tcp_listener = TcpListener::bind(tcp_addr).await?;
        let tcp_local_addr = tcp_listener.local_addr()?;

        // Bind Unix listener
        let unix_listener = UnixListener::bind(&unix_path)?;

        // Set Unix socket permissions to 0o660 (owner + group read/write)
        std::fs::set_permissions(&unix_path, std::fs::Permissions::from_mode(0o660))?;

        info!(
            tcp = %tcp_local_addr,
            unix = %unix_path.display(),
            "Starting API server on TCP and Unix socket (with local auth bypass)"
        );

        // Use a watch channel to fan out the shutdown signal to both servers
        let (shutdown_tx, mut shutdown_rx_tcp) = tokio::sync::watch::channel(false);
        let mut shutdown_rx_unix = shutdown_tx.subscribe();

        // Spawn a task that awaits the caller's shutdown future, then signals both servers
        tokio::spawn(async move {
            shutdown.await;
            let _ = shutdown_tx.send(true);
        });

        // Clone the router so each server gets its own copy
        let tcp_router = router.clone();

        // Add local auth injection middleware to the Unix router
        let unix_router = router.layer(middleware::from_fn(
            move |mut request: axum::http::Request<axum::body::Body>, next: middleware::Next| {
                let bearer_clone = bearer.clone();
                async move {
                    // Only inject auth if the request doesn't already have one
                    if !request.headers().contains_key(AUTHORIZATION) {
                        request.headers_mut().insert(
                            AUTHORIZATION,
                            bearer_clone.parse().expect("valid header value"),
                        );
                    }
                    next.run(request).await
                }
            },
        ));

        // TCP server with ConnectInfo<SocketAddr> for existing handlers
        let tcp_server = axum::serve(
            tcp_listener,
            tcp_router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx_tcp.wait_for(|&v| v).await;
        });

        // Unix socket server without ConnectInfo (no TCP addresses on UDS)
        let unix_server = axum::serve(unix_listener, unix_router.into_make_service())
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx_unix.wait_for(|&v| v).await;
            });

        // Run both servers concurrently, stop when either errors or both finish
        let result = tokio::try_join!(
            async { tcp_server.await.map_err(anyhow::Error::from) },
            async { unix_server.await.map_err(anyhow::Error::from) },
        );

        // Clean up the Unix socket file
        let _ = std::fs::remove_file(&unix_path);

        info!("API server (dual) shut down");

        result.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_creation() {
        let config = ApiConfig::default();
        let server = ApiServer::new(config);
        assert_eq!(server.bind_addr(), "0.0.0.0:3669".parse().unwrap());
    }

    #[test]
    fn test_server_custom_bind() {
        let config = ApiConfig {
            bind: "127.0.0.1:9090".parse().unwrap(),
            ..Default::default()
        };
        let server = ApiServer::new(config);
        assert_eq!(server.bind_addr(), "127.0.0.1:9090".parse().unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_run_dual_starts_and_shuts_down() {
        use axum::routing::get;
        use std::time::Duration;

        let tmp_dir = tempfile::tempdir().unwrap();
        let sock_path = tmp_dir.path().join("test.sock");

        let router = Router::new().route("/health", get(|| async { "ok" }));

        // Use port 0 so the OS picks a free port
        let tcp_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

        // Signal shutdown after a brief delay
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let shutdown = async move {
            let _ = rx.await;
        };

        let sock_path_clone = sock_path.clone();
        let handle = tokio::spawn(async move {
            ApiServer::run_dual(tcp_addr, sock_path_clone, router, shutdown).await
        });

        // Give the servers a moment to bind
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify the Unix socket file was created
        assert!(sock_path.exists(), "Unix socket file should exist");

        // Verify Unix socket permissions are 0o660
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&sock_path).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o660,
                "Socket permissions should be 0o660, got {:o}",
                mode
            );
        }

        // Signal shutdown
        let _ = tx.send(());

        // Wait for clean shutdown
        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("Server should shut down within 5 seconds")
            .expect("Server task should not panic");

        assert!(
            result.is_ok(),
            "run_dual should return Ok: {:?}",
            result.err()
        );

        // Socket file should be cleaned up
        assert!(
            !sock_path.exists(),
            "Unix socket file should be removed after shutdown"
        );
    }
}
