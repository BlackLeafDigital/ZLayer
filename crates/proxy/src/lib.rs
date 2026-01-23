//! ZLayer Reverse Proxy
//!
//! This crate provides a high-performance reverse proxy for routing HTTP/HTTPS
//! traffic to backend services. It supports:
//!
//! - Host and path-based routing
//! - Load balancing (round-robin, least connections)
//! - Health-aware backend selection
//! - HTTP/1.1 and HTTP/2 support
//! - Forwarding headers (X-Forwarded-For, etc.)
//! - Configurable timeouts
//!
//! # Example
//!
//! ```rust,no_run
//! use proxy::{ProxyServer, ProxyServerBuilder, Router, Route, Backend, HealthStatus};
//! use spec::{Protocol, ExposeType};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Create router
//!     let router = Router::new();
//!
//!     // Add a route
//!     router.add_route(Route {
//!         service: "api".to_string(),
//!         endpoint: "http".to_string(),
//!         host: Some("api.example.com".to_string()),
//!         path_prefix: "/".to_string(),
//!         protocol: Protocol::Http,
//!         expose: ExposeType::Public,
//!         strip_prefix: false,
//!     }).await;
//!
//!     // Add backend
//!     let lb = router.get_or_create_lb("api").await;
//!     lb.add_backend(Backend::new("127.0.0.1:8080".parse()?)).await;
//!     lb.update_health("127.0.0.1:8080".parse()?, HealthStatus::Healthy).await;
//!
//!     // Build and run server
//!     let server = ProxyServerBuilder::new()
//!         .http_addr("0.0.0.0:80".parse()?)
//!         .router(router)
//!         .build();
//!
//!     server.run().await?;
//!     Ok(())
//! }
//! ```

pub mod config;
pub mod error;
pub mod lb;
pub mod routing;
pub mod server;
pub mod service;
pub mod tls;
pub mod tunnel;

// Re-export main types
pub use config::{
    HeaderConfig, PoolConfig, ProxyConfig, ServerConfig, TimeoutConfig, TlsConfig, TlsVersion,
};
pub use error::{ProxyError, Result};
pub use lb::{Backend, ConnectionGuard, HealthStatus, LoadBalancer, LoadBalancerAlgorithm};
pub use routing::{Route, RouteMatch, Router};
pub use server::{ProxyServer, ProxyServerBuilder};
pub use service::{empty_body, full_body, BoxBody, ReverseProxyService};
pub use tls::{create_tls_acceptor, TlsServerConfig};
pub use tunnel::{
    is_upgrade_request, is_upgrade_response, is_websocket_upgrade, proxy_tunnel, proxy_upgrade,
};
