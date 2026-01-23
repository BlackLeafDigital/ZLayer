//! ZLayer API - REST API for container orchestration
//!
//! Provides:
//! - JWT authentication
//! - RESTful endpoints for deployments and services
//! - OpenAPI documentation with Swagger UI
//! - Rate limiting
//!
//! # Quick Start
//!
//! ```no_run
//! use api::{ApiServer, ApiConfig};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = ApiConfig::default();
//!     let server = ApiServer::new(config);
//!     server.run().await
//! }
//! ```

pub mod auth;
pub mod config;
pub mod error;
pub mod handlers;
pub mod openapi;
pub mod ratelimit;
pub mod router;
pub mod server;

pub use auth::{create_token, verify_token, AuthState, AuthUser, Claims, OptionalAuthUser};
pub use config::ApiConfig;
pub use error::{ApiError, Result};
pub use openapi::ApiDoc;
pub use ratelimit::{
    create_global_limiter, ip_rate_limit_middleware, rate_limit_middleware, GlobalLimiter,
    IpRateLimiter, RateLimitState,
};
pub use router::build_router;
pub use server::ApiServer;
