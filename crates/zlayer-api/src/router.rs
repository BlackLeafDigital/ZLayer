//! API router construction

use axum::{
    middleware,
    routing::{delete, get, post, put},
    Extension, Router,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::auth::AuthState;
use crate::config::ApiConfig;
use crate::handlers;
use crate::handlers::cron::CronState;
use crate::handlers::deployments::DeploymentState;
use crate::handlers::internal::InternalState;
use crate::handlers::jobs::JobState;
use crate::handlers::nodes::NodeApiState;
use crate::handlers::overlay::OverlayApiState;
use crate::handlers::secrets::SecretsState;
use crate::handlers::services::ServiceState;
use crate::handlers::tunnels::TunnelApiState;
use crate::openapi::ApiDoc;
use crate::ratelimit::{rate_limit_middleware, IpRateLimiter, RateLimitState};
use crate::storage::{DeploymentStorage, InMemoryStorage};

use crate::handlers::build::{build_routes, BuildState};
use zlayer_agent::{CronScheduler, JobExecutor, ServiceManager};
use zlayer_secrets::SecretsStore;

/// Build the API router with default in-memory storage
pub fn build_router(config: &ApiConfig) -> Router {
    let storage: Arc<dyn DeploymentStorage + Send + Sync> = Arc::new(InMemoryStorage::new());
    build_router_with_storage(config, storage)
}

/// Build the API router with a custom storage backend
pub fn build_router_with_storage(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
) -> Router {
    // Auth state
    let auth_state = AuthState {
        jwt_secret: config.jwt_secret.clone(),
    };

    // Deployment state (for CRUD operations)
    let deployment_state = DeploymentState::new(storage.clone());

    // Service state (read-only mode - no service manager)
    let service_state = ServiceState::read_only(storage);

    // Rate limiting
    let rate_limit_state = RateLimitState::new(&config.rate_limit);
    let ip_limiter = Arc::new(IpRateLimiter::new(config.rate_limit.clone()));

    // CORS layer
    let cors = build_cors_layer(config);

    // Health routes (no auth required)
    let health_routes = Router::new()
        .route("/live", get(handlers::health::liveness))
        .route("/ready", get(handlers::health::readiness));

    // Auth routes (no auth required for token endpoint)
    let auth_routes = Router::new()
        .route("/token", post(handlers::auth::get_token))
        .with_state(auth_state.clone());

    // Deployment CRUD routes (use DeploymentState)
    let deployment_crud_routes = Router::new()
        .route("/", get(handlers::deployments::list_deployments))
        .route("/", post(handlers::deployments::create_deployment))
        .route("/{name}", get(handlers::deployments::get_deployment))
        .route("/{name}", delete(handlers::deployments::delete_deployment))
        .with_state(deployment_state);

    // Service routes (use ServiceState - read-only in this router)
    let service_routes = Router::new()
        .route(
            "/{deployment}/services",
            get(handlers::services::list_services),
        )
        .route(
            "/{deployment}/services/{service}",
            get(handlers::services::get_service),
        )
        .route(
            "/{deployment}/services/{service}/scale",
            post(handlers::services::scale_service),
        )
        .route(
            "/{deployment}/services/{service}/logs",
            get(handlers::services::get_service_logs),
        )
        .with_state(service_state);

    // API v1 routes - nest deployment and service routes under /deployments
    let deployments_api = Router::new()
        .merge(deployment_crud_routes)
        .merge(service_routes);

    // Main router
    let mut router = Router::new()
        .nest("/health", health_routes)
        .nest("/auth", auth_routes)
        .nest("/api/v1/deployments", deployments_api)
        .layer(Extension(auth_state))
        .layer(Extension(rate_limit_state))
        .layer(Extension(ip_limiter))
        .layer(middleware::from_fn(rate_limit_middleware))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    // Add Swagger UI if enabled
    if config.swagger_enabled {
        router = router
            .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()));
    }

    router
}

/// Build the API router with job and cron execution capabilities
///
/// This extends the basic router with endpoints for triggering jobs and managing cron schedules.
///
/// # Arguments
/// * `config` - API configuration
/// * `job_executor` - Job executor for running jobs
/// * `cron_scheduler` - Cron scheduler for managing scheduled jobs
pub fn build_router_with_jobs(
    config: &ApiConfig,
    job_executor: Arc<JobExecutor>,
    cron_scheduler: Arc<CronScheduler>,
) -> Router {
    // Start with the basic router
    let base_router = build_router(config);

    // Job state
    let job_state = JobState {
        executor: job_executor,
    };

    // Cron state
    let cron_state = CronState {
        scheduler: cron_scheduler,
    };

    // Job routes
    let job_routes = Router::new()
        .route("/{name}/trigger", post(handlers::jobs::trigger_job))
        .route(
            "/{execution_id}/status",
            get(handlers::jobs::get_execution_status),
        )
        .route(
            "/{name}/executions",
            get(handlers::jobs::list_job_executions),
        )
        .route(
            "/{execution_id}/cancel",
            post(handlers::jobs::cancel_execution),
        )
        .with_state(job_state);

    // Cron routes
    let cron_routes = Router::new()
        .route("/", get(handlers::cron::list_cron_jobs))
        .route("/{name}", get(handlers::cron::get_cron_job))
        .route("/{name}/trigger", post(handlers::cron::trigger_cron_job))
        .route("/{name}/enable", put(handlers::cron::enable_cron_job))
        .route("/{name}/disable", put(handlers::cron::disable_cron_job))
        .with_state(cron_state);

    // Merge job and cron routes into API v1
    base_router
        .nest("/api/v1/jobs", job_routes)
        .nest("/api/v1/cron", cron_routes)
}

/// Build the API router with build capabilities
///
/// This extends the basic router with endpoints for building container images.
///
/// # Arguments
/// * `config` - API configuration
/// * `build_dir` - Directory for storing build contexts and logs
pub fn build_router_with_builds(config: &ApiConfig, build_dir: std::path::PathBuf) -> Router {
    let base_router = build_router(config);

    let build_state = BuildState::new(build_dir);

    // Build routes
    let build_api_routes = build_routes().with_state(build_state);

    base_router.nest("/api/v1", build_api_routes)
}

/// Build the API router with all features (jobs, cron, and builds)
///
/// This creates a full-featured API router with all available capabilities.
///
/// # Arguments
/// * `config` - API configuration
/// * `job_executor` - Job executor for running jobs
/// * `cron_scheduler` - Cron scheduler for managing scheduled jobs
/// * `build_dir` - Directory for storing build contexts and logs
pub fn build_router_full(
    config: &ApiConfig,
    job_executor: Arc<JobExecutor>,
    cron_scheduler: Arc<CronScheduler>,
    build_dir: std::path::PathBuf,
) -> Router {
    // Start with jobs and cron router
    let base_router = build_router_with_jobs(config, job_executor, cron_scheduler);

    let build_state = BuildState::new(build_dir);

    // Build routes
    let build_api_routes = build_routes().with_state(build_state);

    base_router.nest("/api/v1", build_api_routes)
}

/// Build the API router with service management capabilities
///
/// This extends the basic router with a ServiceManager for service scaling operations.
/// The service endpoints (`/deployments/{name}/services/...`) will use the provided
/// ServiceManager to perform actual container lifecycle operations.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `service_manager` - ServiceManager for container lifecycle operations
///
/// # Example
///
/// ```no_run
/// use zlayer_api::{ApiConfig, build_router_with_services};
/// use zlayer_api::storage::InMemoryStorage;
/// use zlayer_agent::{ServiceManager, MockRuntime};
/// use std::sync::Arc;
/// use tokio::sync::RwLock;
///
/// # async fn example() -> anyhow::Result<()> {
/// let config = ApiConfig::default();
/// let storage = Arc::new(InMemoryStorage::new());
/// let runtime = Arc::new(MockRuntime::new());
/// let service_manager = Arc::new(RwLock::new(ServiceManager::new(runtime)));
///
/// let router = build_router_with_services(&config, storage, service_manager);
/// # Ok(())
/// # }
/// ```
pub fn build_router_with_services(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    service_manager: Arc<RwLock<ServiceManager>>,
) -> Router {
    // Auth state
    let auth_state = AuthState {
        jwt_secret: config.jwt_secret.clone(),
    };

    // Deployment state (for deployment CRUD operations)
    let deployment_state = DeploymentState::new(storage.clone());

    // Service state (for service scaling operations)
    let service_state = ServiceState::new(service_manager, storage);

    // Rate limiting
    let rate_limit_state = RateLimitState::new(&config.rate_limit);
    let ip_limiter = Arc::new(IpRateLimiter::new(config.rate_limit.clone()));

    // CORS layer
    let cors = build_cors_layer(config);

    // Health routes (no auth required)
    let health_routes = Router::new()
        .route("/live", get(handlers::health::liveness))
        .route("/ready", get(handlers::health::readiness));

    // Auth routes (no auth required for token endpoint)
    let auth_routes = Router::new()
        .route("/token", post(handlers::auth::get_token))
        .with_state(auth_state.clone());

    // Deployment CRUD routes (use DeploymentState)
    let deployment_crud_routes = Router::new()
        .route("/", get(handlers::deployments::list_deployments))
        .route("/", post(handlers::deployments::create_deployment))
        .route("/{name}", get(handlers::deployments::get_deployment))
        .route("/{name}", delete(handlers::deployments::delete_deployment))
        .with_state(deployment_state);

    // Service routes (use ServiceState for scaling operations)
    let service_routes = Router::new()
        .route(
            "/{deployment}/services",
            get(handlers::services::list_services),
        )
        .route(
            "/{deployment}/services/{service}",
            get(handlers::services::get_service),
        )
        .route(
            "/{deployment}/services/{service}/scale",
            post(handlers::services::scale_service),
        )
        .route(
            "/{deployment}/services/{service}/logs",
            get(handlers::services::get_service_logs),
        )
        .with_state(service_state);

    // API v1 routes - merge deployment and service routes
    let api_v1 = Router::new()
        .merge(deployment_crud_routes)
        .merge(service_routes);

    // Main router
    let mut router = Router::new()
        .nest("/health", health_routes)
        .nest("/auth", auth_routes)
        .nest("/api/v1/deployments", api_v1)
        .layer(Extension(auth_state))
        .layer(Extension(rate_limit_state))
        .layer(Extension(ip_limiter))
        .layer(middleware::from_fn(rate_limit_middleware))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    // Add Swagger UI if enabled
    if config.swagger_enabled {
        router = router
            .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()));
    }

    router
}

/// Build the internal routes for scheduler-to-agent communication
///
/// These routes use a shared secret for authentication (via X-ZLayer-Internal-Token header)
/// rather than JWT tokens, making them suitable for internal service-to-service calls.
///
/// # Arguments
/// * `internal_state` - State containing the service manager and internal token
///
/// # Returns
/// A Router with the internal endpoints mounted at /api/v1/internal
pub fn build_internal_routes(internal_state: InternalState) -> Router {
    Router::new()
        .route("/scale", post(handlers::internal::scale_service_internal))
        .route(
            "/replicas/{service}",
            get(handlers::internal::get_replicas_internal),
        )
        .layer(Extension(internal_state.clone()))
        .with_state(internal_state)
}

/// Build the API router with internal scheduler endpoints
///
/// This extends the service-enabled router with internal endpoints for
/// scheduler-to-agent communication. These endpoints use a shared secret
/// for authentication rather than JWT tokens.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `service_manager` - ServiceManager for container lifecycle operations
/// * `internal_token` - Shared secret for authenticating internal API calls
///
/// # Example
///
/// ```no_run
/// use zlayer_api::{ApiConfig, build_router_with_internal};
/// use zlayer_api::storage::InMemoryStorage;
/// use zlayer_agent::{ServiceManager, MockRuntime};
/// use std::sync::Arc;
/// use tokio::sync::RwLock;
///
/// # async fn example() -> anyhow::Result<()> {
/// let config = ApiConfig::default();
/// let storage = Arc::new(InMemoryStorage::new());
/// let runtime = Arc::new(MockRuntime::new());
/// let service_manager = Arc::new(RwLock::new(ServiceManager::new(runtime)));
/// let internal_token = "my-secret-token".to_string();
///
/// let router = build_router_with_internal(
///     &config,
///     storage,
///     service_manager,
///     internal_token,
/// );
/// # Ok(())
/// # }
/// ```
pub fn build_router_with_internal(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    service_manager: Arc<RwLock<ServiceManager>>,
    internal_token: String,
) -> Router {
    // Start with the services router
    let base_router = build_router_with_services(config, storage, service_manager.clone());

    // Create internal state
    let internal_state = InternalState::new(service_manager, internal_token);

    // Build internal routes
    let internal_routes = build_internal_routes(internal_state);

    // Merge internal routes
    base_router.nest("/api/v1/internal", internal_routes)
}

/// Build routes for secrets management
///
/// Creates the routes for CRUD operations on secrets. These routes require
/// authentication and use the user's ID as the scope for secrets.
///
/// # Arguments
/// * `secrets_state` - State containing the secrets store
///
/// # Returns
/// A Router with the secrets endpoints
pub fn build_secrets_routes(secrets_state: SecretsState) -> Router<()> {
    Router::new()
        .route("/", post(handlers::secrets::create_secret))
        .route("/", get(handlers::secrets::list_secrets))
        .route("/{name}", get(handlers::secrets::get_secret_metadata))
        .route("/{name}", delete(handlers::secrets::delete_secret))
        .with_state(secrets_state)
}

/// Build the API router with secrets management capabilities
///
/// This extends the basic router with endpoints for secrets management.
/// Secrets are scoped to the authenticated user's ID.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `secrets_store` - Secrets store for CRUD operations
///
/// # Example
///
/// ```no_run
/// use zlayer_api::{ApiConfig, build_router_with_secrets};
/// use zlayer_api::storage::InMemoryStorage;
/// use zlayer_secrets::PersistentSecretsStore;
/// use std::sync::Arc;
///
/// # async fn example() -> anyhow::Result<()> {
/// let config = ApiConfig::default();
/// let storage = Arc::new(InMemoryStorage::new());
/// // let secrets_store = Arc::new(PersistentSecretsStore::open(...)?);
/// // let router = build_router_with_secrets(&config, storage, secrets_store);
/// # Ok(())
/// # }
/// ```
pub fn build_router_with_secrets(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    secrets_store: Arc<dyn SecretsStore + Send + Sync>,
) -> Router {
    // Start with the basic router with storage
    let base_router = build_router_with_storage(config, storage);

    // Create secrets state
    let secrets_state = SecretsState::new(secrets_store);

    // Build secrets routes
    let secrets_routes = build_secrets_routes(secrets_state);

    // Merge secrets routes into API v1
    base_router.nest("/api/v1/secrets", secrets_routes)
}

/// Build the API router with services and secrets capabilities
///
/// This extends the services router with endpoints for secrets management.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `service_manager` - ServiceManager for container lifecycle operations
/// * `secrets_store` - Secrets store for CRUD operations
pub fn build_router_with_services_and_secrets(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    service_manager: Arc<RwLock<ServiceManager>>,
    secrets_store: Arc<dyn SecretsStore + Send + Sync>,
) -> Router {
    // Start with the services router
    let base_router = build_router_with_services(config, storage, service_manager);

    // Create secrets state
    let secrets_state = SecretsState::new(secrets_store);

    // Build secrets routes
    let secrets_routes = build_secrets_routes(secrets_state);

    // Merge secrets routes into API v1
    base_router.nest("/api/v1/secrets", secrets_routes)
}

/// Build the API router with internal and secrets capabilities
///
/// This extends the internal router with endpoints for secrets management.
/// Includes all features: services, internal scheduler endpoints, and secrets.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `service_manager` - ServiceManager for container lifecycle operations
/// * `internal_token` - Shared secret for authenticating internal API calls
/// * `secrets_store` - Secrets store for CRUD operations
pub fn build_router_with_internal_and_secrets(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    service_manager: Arc<RwLock<ServiceManager>>,
    internal_token: String,
    secrets_store: Arc<dyn SecretsStore + Send + Sync>,
) -> Router {
    // Start with the internal router
    let base_router = build_router_with_internal(config, storage, service_manager, internal_token);

    // Create secrets state
    let secrets_state = SecretsState::new(secrets_store);

    // Build secrets routes
    let secrets_routes = build_secrets_routes(secrets_state);

    // Merge secrets routes into API v1
    base_router.nest("/api/v1/secrets", secrets_routes)
}

/// Build routes for node management
///
/// Creates the routes for listing and managing cluster nodes.
/// These routes require authentication.
///
/// # Arguments
/// * `node_state` - State containing node management references
///
/// # Returns
/// A Router with the node endpoints
pub fn build_node_routes(node_state: NodeApiState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::nodes::list_nodes))
        .route("/{id}", get(handlers::nodes::get_node))
        .route("/{id}/labels", post(handlers::nodes::update_node_labels))
        .route("/join-token", post(handlers::nodes::generate_join_token))
        .with_state(node_state)
}

/// Build routes for overlay network status
///
/// Creates the routes for querying overlay network status.
/// These endpoints provide information about the WireGuard overlay,
/// peer health, IP allocation, and DNS service.
///
/// # Arguments
/// * `overlay_state` - State containing overlay network references
///
/// # Returns
/// A Router with the overlay endpoints
pub fn build_overlay_routes(overlay_state: OverlayApiState) -> Router<()> {
    Router::new()
        .route("/status", get(handlers::overlay::get_overlay_status))
        .route("/peers", get(handlers::overlay::get_overlay_peers))
        .route("/ip-alloc", get(handlers::overlay::get_ip_allocation))
        .route("/dns", get(handlers::overlay::get_dns_status))
        .with_state(overlay_state)
}

/// Build the API router with nodes and overlay capabilities
///
/// This extends the services router with endpoints for node management
/// and overlay network status.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `service_manager` - ServiceManager for container lifecycle operations
/// * `node_state` - State for node management endpoints
/// * `overlay_state` - State for overlay network endpoints
pub fn build_router_with_nodes_and_overlay(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    service_manager: Arc<RwLock<ServiceManager>>,
    node_state: NodeApiState,
    overlay_state: OverlayApiState,
) -> Router {
    // Start with the services router
    let base_router = build_router_with_services(config, storage, service_manager);

    // Build node and overlay routes
    let node_routes = build_node_routes(node_state);
    let overlay_routes = build_overlay_routes(overlay_state);

    // Merge routes into API v1
    base_router
        .nest("/api/v1/nodes", node_routes)
        .nest("/api/v1/overlay", overlay_routes)
}

/// Build routes for tunnel management
///
/// Creates the routes for creating, listing, and revoking tunnels.
/// These routes require authentication.
///
/// # Arguments
/// * `tunnel_state` - State containing tunnel storage
///
/// # Returns
/// A Router with the tunnel endpoints
pub fn build_tunnel_routes(tunnel_state: TunnelApiState) -> Router<()> {
    Router::new()
        .route("/", post(handlers::tunnels::create_tunnel))
        .route("/", get(handlers::tunnels::list_tunnels))
        .route("/{id}", delete(handlers::tunnels::revoke_tunnel))
        .route("/{id}/status", get(handlers::tunnels::get_tunnel_status))
        .route("/node", post(handlers::tunnels::create_node_tunnel))
        .route(
            "/node/{name}",
            delete(handlers::tunnels::remove_node_tunnel),
        )
        .with_state(tunnel_state)
}

/// Build the API router with tunnel capabilities
///
/// This extends the basic router with endpoints for tunnel management.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `tunnel_state` - State for tunnel management endpoints
pub fn build_router_with_tunnels(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    tunnel_state: TunnelApiState,
) -> Router {
    // Start with the basic router with storage
    let base_router = build_router_with_storage(config, storage);

    // Build tunnel routes
    let tunnel_routes = build_tunnel_routes(tunnel_state);

    // Merge tunnel routes into API v1
    base_router.nest("/api/v1/tunnels", tunnel_routes)
}

fn build_cors_layer(config: &ApiConfig) -> CorsLayer {
    let cors = CorsLayer::new().max_age(std::time::Duration::from_secs(config.cors.max_age));

    let cors = if config.cors.allowed_origins.is_empty() {
        cors.allow_origin(Any)
    } else {
        // Parse origins
        let origins: Vec<_> = config
            .cors
            .allowed_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        cors.allow_origin(origins)
    };

    let cors = if config.cors.allow_credentials {
        cors.allow_credentials(true)
    } else {
        cors
    };

    cors.allow_methods(Any).allow_headers(Any)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_agent::MockRuntime;

    #[test]
    fn test_build_router() {
        let config = ApiConfig::default();
        let _router = build_router(&config);
        // Router builds without error
    }

    #[test]
    fn test_build_router_without_swagger() {
        let config = ApiConfig {
            swagger_enabled: false,
            ..Default::default()
        };
        let _router = build_router(&config);
    }

    #[test]
    fn test_build_cors_layer_default() {
        let config = ApiConfig::default();
        let _cors = build_cors_layer(&config);
    }

    #[test]
    fn test_build_cors_layer_with_origins() {
        let mut config = ApiConfig::default();
        config.cors.allowed_origins = vec![
            "http://localhost:3000".to_string(),
            "https://example.com".to_string(),
        ];
        let _cors = build_cors_layer(&config);
    }

    #[test]
    fn test_build_cors_layer_with_credentials() {
        let mut config = ApiConfig::default();
        config.cors.allow_credentials = true;
        let _cors = build_cors_layer(&config);
    }

    #[test]
    fn test_build_router_with_internal() {
        let config = ApiConfig::default();
        let storage: Arc<dyn DeploymentStorage + Send + Sync> = Arc::new(InMemoryStorage::new());
        let runtime: Arc<dyn zlayer_agent::Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let service_manager = Arc::new(RwLock::new(ServiceManager::new(runtime)));
        let internal_token = "test-secret-token".to_string();

        let _router = build_router_with_internal(&config, storage, service_manager, internal_token);
        // Router builds without error
    }

    #[test]
    fn test_build_internal_routes() {
        let runtime: Arc<dyn zlayer_agent::Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let service_manager = Arc::new(RwLock::new(ServiceManager::new(runtime)));
        let internal_state = InternalState::new(service_manager, "test-secret-token".to_string());

        let _routes = build_internal_routes(internal_state);
        // Routes build without error
    }

    #[test]
    fn test_build_node_routes() {
        let node_state = NodeApiState::new();
        let _routes = build_node_routes(node_state);
        // Routes build without error
    }

    #[test]
    fn test_build_overlay_routes() {
        let overlay_state = OverlayApiState::new();
        let _routes = build_overlay_routes(overlay_state);
        // Routes build without error
    }

    #[test]
    fn test_build_router_with_nodes_and_overlay() {
        let config = ApiConfig::default();
        let storage: Arc<dyn DeploymentStorage + Send + Sync> = Arc::new(InMemoryStorage::new());
        let runtime: Arc<dyn zlayer_agent::Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let service_manager = Arc::new(RwLock::new(ServiceManager::new(runtime)));
        let node_state = NodeApiState::new();
        let overlay_state = OverlayApiState::new();

        let _router = build_router_with_nodes_and_overlay(
            &config,
            storage,
            service_manager,
            node_state,
            overlay_state,
        );
        // Router builds without error
    }

    #[test]
    fn test_build_tunnel_routes() {
        let tunnel_state = TunnelApiState::new();
        let _routes = build_tunnel_routes(tunnel_state);
        // Routes build without error
    }

    #[test]
    fn test_build_router_with_tunnels() {
        let config = ApiConfig::default();
        let storage: Arc<dyn DeploymentStorage + Send + Sync> = Arc::new(InMemoryStorage::new());
        let tunnel_state = TunnelApiState::new();

        let _router = build_router_with_tunnels(&config, storage, tunnel_state);
        // Router builds without error
    }
}
