//! API server

use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::info;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_creation() {
        let config = ApiConfig::default();
        let server = ApiServer::new(config);
        assert_eq!(server.bind_addr(), "0.0.0.0:8080".parse().unwrap());
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
}
