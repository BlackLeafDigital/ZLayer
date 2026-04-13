//! `ZLayer` API - REST API for container orchestration
//!
//! Provides:
//! - JWT authentication
//! - `RESTful` endpoints for deployments and services
//! - `OpenAPI` documentation with Swagger UI
//! - Rate limiting
//!
//! # Quick Start
//!
//! ```no_run
//! use zlayer_api_zql::{ApiServer, ApiConfig};
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
#[allow(clippy::needless_for_each)]
pub mod openapi;
pub mod ratelimit;
pub mod router;
pub mod server;
pub mod storage;

pub use auth::{create_token, verify_token, AuthState, AuthUser, Claims, OptionalAuthUser};
pub use config::ApiConfig;
pub use error::{ApiError, Result};
pub use openapi::ApiDoc;
pub use ratelimit::{
    create_global_limiter, ip_rate_limit_middleware, rate_limit_middleware, GlobalLimiter,
    IpRateLimiter, RateLimitState,
};
pub use router::{
    build_cluster_routes, build_container_routes, build_cron_routes, build_image_routes,
    build_internal_routes, build_job_routes, build_network_routes, build_proxy_routes,
    build_router, build_router_full, build_router_with_builds, build_router_with_containers,
    build_router_with_deployment_state, build_router_with_internal,
    build_router_with_internal_and_secrets, build_router_with_jobs, build_router_with_secrets,
    build_router_with_services, build_router_with_services_and_secrets, build_router_with_storage,
    build_router_with_tunnels, build_secrets_routes, build_storage_routes, build_tunnel_routes,
    build_volume_routes,
};
pub use server::ApiServer;
#[cfg(unix)]
pub use server::{bind_dual_with_local_auth, serve_bound, BoundListeners};

// Re-export state types for job/cron/build/deployment/service/internal/container endpoints
pub use handlers::build::{BuildManager, BuildState, BuildStateEnum, BuildStatus, TemplateInfo};
pub use handlers::cluster::{
    ClusterApiState, ClusterJoinRequest, ClusterJoinResponse, ClusterNodeSummary, ClusterPeer,
    ForceLeaderRequest, ForceLeaderResponse,
};
pub use handlers::containers::ContainerApiState;
pub use handlers::cron::CronState;
pub use handlers::deployments::{DeploymentState, ServiceHealthInfo};
pub use handlers::internal::{
    InternalAddPeerRequest, InternalAddPeerResponse, InternalScaleRequest, InternalScaleResponse,
    InternalState, INTERNAL_AUTH_HEADER,
};
pub use handlers::jobs::JobState;
pub use handlers::networks::{NetworkApiState, NetworkSummary};
pub use handlers::proxy::ProxyApiState;
pub use handlers::secrets::{CreateSecretRequest, SecretMetadataResponse, SecretsState};
pub use handlers::services::ServiceState;
pub use handlers::storage::{ReplicationInfo, StorageState, StorageStatusResponse};
pub use handlers::tunnels::{
    CreateNodeTunnelRequest, CreateNodeTunnelResponse, CreateTunnelRequest, CreateTunnelResponse,
    TunnelApiState, TunnelStatus, TunnelSummary,
};
pub use handlers::volumes::{DeleteVolumeQuery, VolumeApiState, VolumeSummary};

pub use axum::Extension;

// Re-export storage types
pub use storage::{
    DeploymentStatus, DeploymentStorage, InMemoryStorage, StorageError, StoredDeployment,
    ZqlStorage,
};
