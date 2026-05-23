//! API router construction

use axum::{
    middleware,
    routing::{delete, get, post, put, MethodFilter},
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

/// One-time startup audit logged when an `AuthState` is built. The
/// `jwt_secret_fp` is the first 8 hex chars of `SHA-256(secret)` — enough to
/// detect a per-restart secret rotation across two boots without ever
/// logging the secret itself. If this fingerprint differs across daemon
/// restarts, every previously-issued session cookie becomes signature
/// invalid (look for `variant = "InvalidSignature"` in `verify_token` logs).
fn log_auth_state_audit(state: &AuthState) {
    use secrecy::ExposeSecret;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(state.jwt_secret.expose_secret().as_bytes());
    let fp = hex::encode(hasher.finalize());
    tracing::warn!(
        user_store_configured = state.user_store.is_some(),
        jwt_secret_fp = %&fp[..8],
        cookie_secure = state.cookie_secure,
        "api: AuthState startup audit",
    );
}
use crate::handlers;
use crate::handlers::audit::AuditState;
use crate::handlers::cluster::ClusterApiState;
use crate::handlers::container_networks::BridgeNetworkApiState;
use crate::handlers::containers::ContainerApiState;
use crate::handlers::credentials::CredentialState;
use crate::handlers::cron::CronState;
use crate::handlers::deployments::DeploymentState;
use crate::handlers::environments::{EnvironmentsRouterState, EnvironmentsState};
use crate::handlers::groups::GroupsState;
use crate::handlers::internal::InternalState;
use crate::handlers::jobs::JobState;
use crate::handlers::networks::NetworkApiState;
use crate::handlers::nodes::NodeApiState;
use crate::handlers::notifiers::NotifiersState;
use crate::handlers::overlay::OverlayApiState;
use crate::handlers::permissions::PermissionsState;
use crate::handlers::projects::ProjectState;
use crate::handlers::proxy::ProxyApiState;
use crate::handlers::secrets::SecretsState;
use crate::handlers::services::ServiceState;
use crate::handlers::storage::StorageState;
use crate::handlers::syncs::SyncState;
use crate::handlers::tasks::TasksState;
use crate::handlers::tunnels::TunnelApiState;
use crate::handlers::variables::VariableState;
use crate::handlers::volumes::VolumeApiState;
use crate::handlers::webhooks::WebhookState;
use crate::handlers::workflows::WorkflowsState;
use crate::middleware::csrf::csrf_middleware;
use crate::openapi::ApiDoc;
use crate::ratelimit::{rate_limit_middleware, IpRateLimiter, RateLimitState};
use crate::storage::{DeploymentStorage, EnvironmentStorage, InMemoryStorage};

use crate::handlers::build::{build_routes, BuildState};
use zlayer_agent::{CronScheduler, JobExecutor, ServiceManager};
use zlayer_secrets::SecretsStore;

/// Build the full auth-routes sub-router (unauthenticated endpoints: token,
/// bootstrap, login, logout, plus authenticated me/csrf).
///
/// Note: all routes share the same `AuthState`. `me` and `csrf` require an
/// authenticated actor (enforced by the `AuthActor` extractor, which accepts
/// either a Bearer token or a session cookie); the others are intentionally
/// unauthenticated so browser clients can log in / bootstrap without an
/// existing session.
fn build_auth_routes(auth_state: AuthState) -> Router {
    use axum::extract::State;
    use axum_extra::extract::cookie::CookieJar;

    // `logout` and `csrf` are sync functions in the auth handler module, so
    // wrap them in async adapters to satisfy axum's `Handler` trait (which
    // requires handlers to return a `Future`).
    async fn logout_adapter(jar: CookieJar) -> impl axum::response::IntoResponse {
        handlers::auth::logout(jar)
    }
    async fn csrf_adapter(
        actor: crate::handlers::users::AuthActor,
        state: State<AuthState>,
        jar: CookieJar,
    ) -> impl axum::response::IntoResponse {
        handlers::auth::csrf(actor, state, jar)
    }

    Router::new()
        .route("/token", post(handlers::auth::get_token))
        .route("/bootstrap", post(handlers::auth::bootstrap))
        .route("/login", post(handlers::auth::login))
        .route("/logout", post(logout_adapter))
        .route("/me", get(handlers::auth::me))
        .route("/csrf", get(csrf_adapter))
        .route("/oidc/providers", get(handlers::oidc::list_providers))
        .route("/oidc/{provider}/start", get(handlers::oidc::start))
        .route("/oidc/{provider}/callback", get(handlers::oidc::callback))
        .with_state(auth_state)
}

/// Build the users-CRUD sub-router.
///
/// All routes require authentication (Bearer or session cookie) and most
/// require the `admin` role — those checks happen inside the handlers via
/// the `AuthActor` extractor. The router itself does not layer extra auth
/// middleware; the shared `Extension(auth_state)` at the top-level router
/// provides `AuthState` to the extractors.
fn build_users_routes(auth_state: AuthState) -> Router {
    Router::new()
        .route("/", get(handlers::users::list_users))
        .route("/", post(handlers::users::create_user))
        .route("/{id}", get(handlers::users::get_user))
        .route("/{id}", axum::routing::patch(handlers::users::update_user))
        .route("/{id}", delete(handlers::users::delete_user))
        .route("/{id}/password", post(handlers::users::set_password))
        .with_state(auth_state)
}

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
        credential_store: config.credential_store.clone(),
        user_store: config.user_store.clone(),
        identity: config.identity.clone(),
        oidc_clients: config.oidc_clients.clone(),
        oidc_state: config.oidc_state.clone(),
        cookie_secure: false,
    };
    log_auth_state_audit(&auth_state);

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

    // Auth + users routes
    let auth_routes = build_auth_routes(auth_state.clone());
    let users_routes = build_users_routes(auth_state.clone());

    // Deployment CRUD routes (use DeploymentState)
    let deployment_crud_routes = Router::new()
        .route("/", get(handlers::deployments::list_deployments))
        .route("/", post(handlers::deployments::create_deployment))
        .route("/{name}", get(handlers::deployments::get_deployment))
        .route("/{name}", delete(handlers::deployments::delete_deployment))
        .route(
            "/{name}/spec",
            get(handlers::deployments::get_deployment_spec),
        )
        .route(
            "/{name}/events",
            get(handlers::deployments::stream_deployment_events),
        )
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
        .route(
            "/{deployment}/services/{service}/containers",
            get(handlers::services::list_containers),
        )
        .route(
            "/{deployment}/services/{service}/exec",
            post(handlers::services::exec_in_service),
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
        .nest("/api/v1/users", users_routes)
        .nest("/api/v1/deployments", deployments_api)
        .nest("/api/v1/daemon", build_daemon_routes())
        .layer(Extension(auth_state))
        .layer(Extension(rate_limit_state))
        .layer(Extension(ip_limiter))
        .layer(middleware::from_fn(rate_limit_middleware))
        .layer(middleware::from_fn(csrf_middleware))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    // Add Swagger UI if enabled
    if config.swagger_enabled {
        router = router
            .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()));
    }

    router
}

/// Build routes for daemon-level introspection.
///
/// Currently exposes only the process-wide capability survey at
/// `GET /capabilities`. The handler is state-free — the survey is memoised
/// in a process-wide `OnceLock` inside
/// [`zlayer_agent::capability::DaemonCapabilities`].
pub fn build_daemon_routes() -> Router<()> {
    Router::new().route(
        "/capabilities",
        get(handlers::daemon::get_daemon_capabilities),
    )
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
/// This extends the basic router with a `ServiceManager` for service scaling operations.
/// The service endpoints (`/deployments/{name}/services/...`) will use the provided
/// `ServiceManager` to perform actual container lifecycle operations.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `service_manager` - `ServiceManager` for container lifecycle operations
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
/// let router = build_router_with_services(&config, storage, service_manager, None);
/// # Ok(())
/// # }
/// ```
pub fn build_router_with_services(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    service_manager: Arc<RwLock<ServiceManager>>,
    local_node_id: Option<String>,
) -> Router {
    // Auth state
    let auth_state = AuthState {
        jwt_secret: config.jwt_secret.clone(),
        credential_store: config.credential_store.clone(),
        user_store: config.user_store.clone(),
        identity: config.identity.clone(),
        oidc_clients: config.oidc_clients.clone(),
        oidc_state: config.oidc_state.clone(),
        cookie_secure: false,
    };
    log_auth_state_audit(&auth_state);

    // Deployment state (for deployment CRUD operations)
    let deployment_state = DeploymentState::new(storage.clone());

    // Service state (for service scaling operations)
    let service_state = ServiceState::new(service_manager, storage, local_node_id);

    // Rate limiting
    let rate_limit_state = RateLimitState::new(&config.rate_limit);
    let ip_limiter = Arc::new(IpRateLimiter::new(config.rate_limit.clone()));

    // CORS layer
    let cors = build_cors_layer(config);

    // Health routes (no auth required)
    let health_routes = Router::new()
        .route("/live", get(handlers::health::liveness))
        .route("/ready", get(handlers::health::readiness));

    // Auth + users routes
    let auth_routes = build_auth_routes(auth_state.clone());
    let users_routes = build_users_routes(auth_state.clone());

    // Deployment CRUD routes (use DeploymentState)
    let deployment_crud_routes = Router::new()
        .route("/", get(handlers::deployments::list_deployments))
        .route("/", post(handlers::deployments::create_deployment))
        .route("/{name}", get(handlers::deployments::get_deployment))
        .route("/{name}", delete(handlers::deployments::delete_deployment))
        .route(
            "/{name}/spec",
            get(handlers::deployments::get_deployment_spec),
        )
        .route(
            "/{name}/events",
            get(handlers::deployments::stream_deployment_events),
        )
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
        .route(
            "/{deployment}/services/{service}/containers",
            get(handlers::services::list_containers),
        )
        .route(
            "/{deployment}/services/{service}/exec",
            post(handlers::services::exec_in_service),
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
        .nest("/api/v1/users", users_routes)
        .nest("/api/v1/deployments", api_v1)
        .nest("/api/v1/daemon", build_daemon_routes())
        .layer(Extension(auth_state))
        .layer(Extension(rate_limit_state))
        .layer(Extension(ip_limiter))
        .layer(middleware::from_fn(rate_limit_middleware))
        .layer(middleware::from_fn(csrf_middleware))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    // Add Swagger UI if enabled
    if config.swagger_enabled {
        router = router
            .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()));
    }

    router
}

/// Build the API router with services, using a pre-built `DeploymentState`.
///
/// This variant allows the caller to provide a `DeploymentState` with orchestration
/// handles (service manager, overlay, proxy) already wired in, enabling the
/// `create_deployment` handler to perform actual container orchestration.
///
/// # Arguments
/// * `config` - API configuration
/// * `deployment_state` - Pre-built deployment state (may include orchestration handles)
/// * `service_manager` - `ServiceManager` for service scaling operations
/// * `storage` - Storage backend (for `ServiceState`)
pub fn build_router_with_deployment_state(
    config: &ApiConfig,
    deployment_state: DeploymentState,
    service_manager: Arc<RwLock<ServiceManager>>,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    local_node_id: Option<String>,
) -> Router {
    // Auth state
    let auth_state = AuthState {
        jwt_secret: config.jwt_secret.clone(),
        credential_store: config.credential_store.clone(),
        user_store: config.user_store.clone(),
        identity: config.identity.clone(),
        oidc_clients: config.oidc_clients.clone(),
        oidc_state: config.oidc_state.clone(),
        cookie_secure: false,
    };
    log_auth_state_audit(&auth_state);

    // Service state (for service scaling operations)
    let service_state = ServiceState::new(service_manager, storage, local_node_id);

    // Rate limiting
    let rate_limit_state = RateLimitState::new(&config.rate_limit);
    let ip_limiter = Arc::new(IpRateLimiter::new(config.rate_limit.clone()));

    // CORS layer
    let cors = build_cors_layer(config);

    // Health routes (no auth required)
    let health_routes = Router::new()
        .route("/live", get(handlers::health::liveness))
        .route("/ready", get(handlers::health::readiness));

    // Auth + users routes
    let auth_routes = build_auth_routes(auth_state.clone());
    let users_routes = build_users_routes(auth_state.clone());

    // Deployment CRUD routes (use pre-built DeploymentState with orchestration)
    let deployment_crud_routes = Router::new()
        .route("/", get(handlers::deployments::list_deployments))
        .route("/", post(handlers::deployments::create_deployment))
        .route("/{name}", get(handlers::deployments::get_deployment))
        .route("/{name}", delete(handlers::deployments::delete_deployment))
        .route(
            "/{name}/spec",
            get(handlers::deployments::get_deployment_spec),
        )
        .route(
            "/{name}/events",
            get(handlers::deployments::stream_deployment_events),
        )
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
        .route(
            "/{deployment}/services/{service}/containers",
            get(handlers::services::list_containers),
        )
        .route(
            "/{deployment}/services/{service}/exec",
            post(handlers::services::exec_in_service),
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
        .nest("/api/v1/users", users_routes)
        .nest("/api/v1/deployments", api_v1)
        .nest("/api/v1/daemon", build_daemon_routes())
        .layer(Extension(auth_state))
        .layer(Extension(rate_limit_state))
        .layer(Extension(ip_limiter))
        .layer(middleware::from_fn(rate_limit_middleware))
        .layer(middleware::from_fn(csrf_middleware))
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
        .route("/add-peer", post(handlers::internal::add_peer_internal))
        // `zlayer node upgrade` — the leader hits `upgrade/start` on each
        // follower in turn and polls `upgrade/{id}` for progress.
        .route(
            "/upgrade/start",
            post(handlers::internal::internal_upgrade_start),
        )
        .route(
            "/upgrade/{upgrade_id}",
            get(handlers::internal::internal_upgrade_status),
        )
        // Pre-self-upgrade nudge: the leader picks a healthy follower
        // and POSTs here so that follower campaigns immediately
        // instead of waiting for heartbeat-loss after the leader exits.
        .route(
            "/raft/trigger-elect",
            post(handlers::internal::internal_raft_trigger_elect),
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
/// * `service_manager` - `ServiceManager` for container lifecycle operations
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
///     None,
/// );
/// # Ok(())
/// # }
/// ```
pub fn build_router_with_internal(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    service_manager: Arc<RwLock<ServiceManager>>,
    internal_token: String,
    local_node_id: Option<String>,
) -> Router {
    // Start with the services router
    let base_router =
        build_router_with_services(config, storage, service_manager.clone(), local_node_id);

    // Create internal state
    let internal_state = InternalState::new(service_manager, internal_token);

    // Build internal routes
    let internal_routes = build_internal_routes(internal_state);

    // Merge internal routes
    base_router.nest("/api/v1/internal", internal_routes)
}

/// Build routes for image management.
///
/// Creates the routes for listing, removing, and pruning images. These
/// routes require authentication and dispatch into the runtime's image
/// management methods (`list_images`, `remove_image`, `prune_images`).
///
/// # Arguments
/// * `runtime` - The container runtime (Youki, Docker, WASM, etc.)
///
/// # Returns
/// A `Router` with `/images`, `/images/{image}`, and `/system/prune` routes.
///
/// Convenience constructor: builds an `ImageState` with a fresh, unattached
/// event bus. Callers that want image lifecycle events on the daemon-wide
/// `/api/v1/events` stream should call [`build_image_routes_with_state`]
/// instead and pass an `ImageState` whose `event_bus` is shared with the
/// container/network/volume states.
pub fn build_image_routes(
    runtime: Arc<dyn zlayer_agent::runtime::Runtime + Send + Sync>,
) -> Router<()> {
    use crate::handlers::images::ImageState;
    build_image_routes_with_state(ImageState::new(runtime))
}

/// Build image-management routes with an explicit [`ImageState`].
///
/// Used by the daemon to wire the shared [`crate::DaemonEventBus`] so
/// image lifecycle events fan out on the same broadcast channel as
/// container, network, and volume events.
pub fn build_image_routes_with_state(state: crate::handlers::images::ImageState) -> Router<()> {
    use crate::handlers::images::image_routes;
    image_routes().with_state(state)
}

/// Build routes for secrets management
///
/// Creates the routes for CRUD operations on secrets. These routes require
/// authentication; mutating endpoints require the `admin` role.
///
/// Routes:
/// - `POST   /`             — create or update a secret
/// - `GET    /`             — list secrets in a scope
/// - `POST   /bulk-import`  — admin-only dotenv bulk import
/// - `GET    /{name}`       — get metadata (admin `?reveal=true` returns value)
/// - `DELETE /{name}`       — delete a secret
///
/// # Arguments
/// * `secrets_state` - State containing the secrets store and (optionally)
///   the environment store for env-aware scope resolution.
///
/// # Returns
/// A Router with the secrets endpoints
pub fn build_secrets_routes(secrets_state: SecretsState) -> Router<()> {
    Router::new()
        .route("/", post(handlers::secrets::create_secret))
        .route("/", get(handlers::secrets::list_secrets))
        .route("/bulk-import", post(handlers::secrets::bulk_import_secrets))
        .route("/reveal-all", get(handlers::secrets::reveal_all_secrets))
        .route("/{name}/rotate", post(handlers::secrets::rotate_secret))
        .route("/{name}", get(handlers::secrets::get_secret_metadata))
        .route("/{name}", delete(handlers::secrets::delete_secret))
        .with_state(secrets_state)
}

/// Build routes for environment CRUD.
///
/// Routes:
/// - `GET    /`        — list environments (`?project=` filters)
/// - `POST   /`        — create (admin-only)
/// - `GET    /{id}`    — fetch one
/// - `PATCH  /{id}`    — rename / re-describe (admin-only)
/// - `DELETE /{id}`    — delete (admin-only, refuses if secrets remain)
///
/// All routes require authentication (Bearer or session cookie) via the
/// `AuthActor` extractor; admin enforcement happens in the handlers.
///
/// # Arguments
/// * `env_state` - Environment storage state.
/// * `secrets_state` - Secrets state, used by the cascade-safety check on
///   delete to count remaining secrets in the env's scope.
///
/// # Returns
/// A Router with the environment endpoints
pub fn build_environment_routes(
    env_state: EnvironmentsState,
    secrets_state: SecretsState,
) -> Router<()> {
    let state = EnvironmentsRouterState::new(env_state, secrets_state);
    Router::new()
        .route("/", get(handlers::environments::list_environments))
        .route("/", post(handlers::environments::create_environment))
        .route("/{id}", get(handlers::environments::get_environment))
        .route(
            "/{id}",
            axum::routing::patch(handlers::environments::update_environment),
        )
        .route("/{id}", delete(handlers::environments::delete_environment))
        .with_state(state)
}

/// Build routes for variable CRUD.
///
/// Routes:
/// - `GET    /`     -- list variables (filtered by scope)
/// - `POST   /`     -- create (admin-only)
/// - `GET    /{id}` -- fetch one
/// - `PATCH  /{id}` -- update (admin-only)
/// - `DELETE /{id}` -- delete (admin-only)
///
/// All routes require authentication (Bearer or session cookie) via the
/// `AuthActor` extractor; admin enforcement happens in the handlers.
///
/// # Arguments
/// * `variable_state` - Variable storage state.
///
/// # Returns
/// A Router with the variable endpoints
pub fn build_variable_routes(variable_state: VariableState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::variables::list_variables))
        .route("/", post(handlers::variables::create_variable))
        .route("/{id}", get(handlers::variables::get_variable))
        .route(
            "/{id}",
            axum::routing::patch(handlers::variables::update_variable),
        )
        .route("/{id}", delete(handlers::variables::delete_variable))
        .with_state(variable_state)
}

/// Build routes for task CRUD and execution.
///
/// Routes:
/// - `GET    /`            -- list tasks (filtered by `project_id`)
/// - `POST   /`            -- create (admin-only)
/// - `GET    /{id}`         -- fetch one
/// - `DELETE /{id}`         -- delete (admin-only)
/// - `POST   /{id}/run`     -- execute synchronously (admin-only)
/// - `GET    /{id}/runs`    -- list past runs
///
/// All routes require authentication (Bearer or session cookie) via the
/// `AuthActor` extractor; admin enforcement happens in the handlers.
///
/// # Arguments
/// * `tasks_state` - Task storage state.
///
/// # Returns
/// A Router with the task endpoints
pub fn build_task_routes(tasks_state: TasksState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::tasks::list_tasks))
        .route("/", post(handlers::tasks::create_task))
        .route("/{id}", get(handlers::tasks::get_task))
        .route("/{id}", delete(handlers::tasks::delete_task))
        .route("/{id}/run", post(handlers::tasks::run_task))
        .route("/{id}/runs", get(handlers::tasks::list_task_runs))
        .with_state(tasks_state)
}

/// Build routes for project CRUD and deployment linking.
///
/// Routes:
/// - `GET    /`                    -- list all projects
/// - `POST   /`                    -- create (admin-only)
/// - `GET    /{id}`                -- fetch one
/// - `PATCH  /{id}`                -- update (admin-only)
/// - `DELETE /{id}`                -- delete with cascade (admin-only)
/// - `GET    /{id}/deployments`    -- list linked deployments
/// - `POST   /{id}/deployments`    -- link a deployment
/// - `DELETE /{id}/deployments/{name}` -- unlink a deployment
/// - `POST   /{id}/pull`           -- clone / fast-forward the project repo (admin-only)
///
/// All routes require authentication via the `AuthActor` extractor; admin
/// enforcement happens in the handlers.
///
/// # Arguments
/// * `project_state` - Project storage state.
///
/// # Returns
/// A Router with the project endpoints
pub fn build_project_routes(project_state: ProjectState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::projects::list_projects))
        .route("/", post(handlers::projects::create_project))
        .route("/{id}", get(handlers::projects::get_project))
        .route(
            "/{id}",
            axum::routing::patch(handlers::projects::update_project),
        )
        .route("/{id}", delete(handlers::projects::delete_project))
        .route(
            "/{id}/deployments",
            get(handlers::projects::list_project_deployments),
        )
        .route(
            "/{id}/deployments",
            post(handlers::projects::link_project_deployment),
        )
        .route(
            "/{id}/deployments/{name}",
            delete(handlers::projects::unlink_project_deployment),
        )
        .route("/{id}/pull", post(handlers::projects::pull_project))
        .with_state(project_state)
}

/// Build routes for webhook management on projects.
///
/// Routes:
/// - `GET    /{id}/webhook`         -- get webhook URL + secret (generates on first call)
/// - `POST   /{id}/webhook/rotate`  -- rotate webhook secret (admin-only)
///
/// These routes require authentication via the `AuthActor` extractor.
/// Mount under `/api/v1/projects` alongside (merged with) the project
/// CRUD routes.
///
/// # Arguments
/// * `webhook_state` - Webhook state including project store + secrets.
///
/// # Returns
/// A Router with the webhook management endpoints
pub fn build_project_webhook_routes(webhook_state: WebhookState) -> Router<()> {
    Router::new()
        .route("/{id}/webhook", get(handlers::webhooks::get_webhook_info))
        .route(
            "/{id}/webhook/rotate",
            post(handlers::webhooks::rotate_webhook_secret),
        )
        .with_state(webhook_state)
}

/// Build the public (unauthenticated) webhook receiver route.
///
/// Routes:
/// - `POST /{provider}/{project_id}` -- HMAC-verified push handler
///
/// This must be mounted at `/webhooks` **outside** the auth-gated API
/// namespace.
///
/// # Arguments
/// * `webhook_state` - Webhook state including project store + secrets.
///
/// # Returns
/// A Router with the webhook receiver endpoint
pub fn build_webhook_receiver_routes(webhook_state: WebhookState) -> Router<()> {
    Router::new()
        .route(
            "/{provider}/{project_id}",
            post(handlers::webhooks::receive_webhook),
        )
        .with_state(webhook_state)
}

/// Build routes for credential management (registry + git).
///
/// Routes:
/// - `GET    /registry`       -- list registry credentials
/// - `POST   /registry`       -- create registry credential (admin-only)
/// - `DELETE /registry/{id}`  -- delete registry credential (admin-only)
/// - `GET    /git`            -- list git credentials
/// - `POST   /git`            -- create git credential (admin-only)
/// - `DELETE /git/{id}`       -- delete git credential (admin-only)
///
/// All routes require authentication via the `AuthActor` extractor; admin
/// enforcement happens in the handlers.
///
/// # Arguments
/// * `credential_state` - Credential storage state.
///
/// # Returns
/// A Router with the credential endpoints
pub fn build_credential_routes(credential_state: CredentialState) -> Router<()> {
    Router::new()
        .route(
            "/registry",
            get(handlers::credentials::list_registry_credentials),
        )
        .route(
            "/registry",
            post(handlers::credentials::create_registry_credential),
        )
        .route(
            "/registry/{id}",
            delete(handlers::credentials::delete_registry_credential),
        )
        .route("/git", get(handlers::credentials::list_git_credentials))
        .route("/git", post(handlers::credentials::create_git_credential))
        .route(
            "/git/{id}",
            delete(handlers::credentials::delete_git_credential),
        )
        .with_state(credential_state)
}

/// Build routes for network management
///
/// Creates the routes for CRUD operations on network access-control groups.
/// These routes require authentication; mutating endpoints require the
/// `operator` role.
///
/// # Arguments
/// * `network_state` - State containing the in-memory network store
///
/// # Returns
/// A Router with the network endpoints
pub fn build_network_routes(network_state: NetworkApiState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::networks::list_networks))
        .route("/", post(handlers::networks::create_network))
        .route("/{name}", get(handlers::networks::get_network))
        .route("/{name}", put(handlers::networks::update_network))
        .route("/{name}", delete(handlers::networks::delete_network))
        .with_state(network_state)
}

/// Build the API router with secrets and environments management.
///
/// Wires the secrets store with an environment store so the secrets handler
/// can resolve `?environment={id}` to a scoped namespace, and mounts the
/// environment CRUD routes under `/api/v1/environments`.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `secrets_store` - Secrets store for CRUD operations
/// * `env_store` - Environment storage backend (pass an
///   [`InMemoryEnvironmentStore`] for legacy callers that have not wired
///   persistent storage yet).
///
/// # Example
///
/// ```no_run
/// use zlayer_api::{ApiConfig, build_router_with_secrets};
/// use zlayer_api::storage::{InMemoryEnvironmentStore, InMemoryStorage};
/// use zlayer_secrets::PersistentSecretsStore;
/// use std::sync::Arc;
///
/// # async fn example() -> anyhow::Result<()> {
/// let config = ApiConfig::default();
/// let storage = Arc::new(InMemoryStorage::new());
/// let env_store = Arc::new(InMemoryEnvironmentStore::new());
/// // let secrets_store = Arc::new(PersistentSecretsStore::open(...)?);
/// // let router = build_router_with_secrets(&config, storage, secrets_store, env_store);
/// # Ok(())
/// # }
/// ```
pub fn build_router_with_secrets(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    secrets_store: Arc<dyn SecretsStore + Send + Sync>,
    env_store: Arc<dyn EnvironmentStorage>,
) -> Router {
    let base_router = build_router_with_storage(config, storage);

    let secrets_state = SecretsState::with_environments(secrets_store, env_store.clone());
    let env_state = EnvironmentsState::new(env_store);

    let secrets_routes = build_secrets_routes(secrets_state.clone());
    let env_routes = build_environment_routes(env_state, secrets_state);

    base_router
        .nest("/api/v1/secrets", secrets_routes)
        .nest("/api/v1/environments", env_routes)
}

/// Build the API router with services, secrets, and environment management.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `service_manager` - `ServiceManager` for container lifecycle operations
/// * `secrets_store` - Secrets store for CRUD operations
/// * `env_store` - Environment storage backend
pub fn build_router_with_services_and_secrets(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    service_manager: Arc<RwLock<ServiceManager>>,
    secrets_store: Arc<dyn SecretsStore + Send + Sync>,
    env_store: Arc<dyn EnvironmentStorage>,
    local_node_id: Option<String>,
) -> Router {
    let base_router = build_router_with_services(config, storage, service_manager, local_node_id);

    let secrets_state = SecretsState::with_environments(secrets_store, env_store.clone());
    let env_state = EnvironmentsState::new(env_store);

    let secrets_routes = build_secrets_routes(secrets_state.clone());
    let env_routes = build_environment_routes(env_state, secrets_state);

    base_router
        .nest("/api/v1/secrets", secrets_routes)
        .nest("/api/v1/environments", env_routes)
}

/// Build the API router with internal, secrets, and environment management.
///
/// Includes all features: services, internal scheduler endpoints, secrets,
/// and environments.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `service_manager` - `ServiceManager` for container lifecycle operations
/// * `internal_token` - Shared secret for authenticating internal API calls
/// * `secrets_store` - Secrets store for CRUD operations
/// * `env_store` - Environment storage backend
pub fn build_router_with_internal_and_secrets(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    service_manager: Arc<RwLock<ServiceManager>>,
    internal_token: String,
    secrets_store: Arc<dyn SecretsStore + Send + Sync>,
    env_store: Arc<dyn EnvironmentStorage>,
    local_node_id: Option<String>,
) -> Router {
    let base_router = build_router_with_internal(
        config,
        storage,
        service_manager,
        internal_token,
        local_node_id,
    );

    let secrets_state = SecretsState::with_environments(secrets_store, env_store.clone());
    let env_state = EnvironmentsState::new(env_store);

    let secrets_routes = build_secrets_routes(secrets_state.clone());
    let env_routes = build_environment_routes(env_state, secrets_state);

    base_router
        .nest("/api/v1/secrets", secrets_routes)
        .nest("/api/v1/environments", env_routes)
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
/// These endpoints provide information about the encrypted overlay network,
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
        .route("/nat/status", get(handlers::overlay::get_nat_status))
        .with_state(overlay_state)
}

/// Build routes for proxy status
///
/// Creates read-only routes for inspecting the reverse proxy's state:
/// routes, backends, TLS certificates, and L4 stream proxies.
///
/// # Arguments
/// * `proxy_state` - State containing optional references to proxy subsystems
pub fn build_proxy_routes(proxy_state: ProxyApiState) -> Router<()> {
    Router::new()
        .route("/routes", get(handlers::proxy::list_routes))
        .route("/backends", get(handlers::proxy::list_backends))
        .route("/tls", get(handlers::proxy::list_tls))
        .route("/streams", get(handlers::proxy::list_streams))
        .with_state(proxy_state)
}

/// Build the API router with nodes and overlay capabilities
///
/// This extends the services router with endpoints for node management
/// and overlay network status.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `service_manager` - `ServiceManager` for container lifecycle operations
/// * `node_state` - State for node management endpoints
/// * `overlay_state` - State for overlay network endpoints
pub fn build_router_with_nodes_and_overlay(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    service_manager: Arc<RwLock<ServiceManager>>,
    node_state: NodeApiState,
    overlay_state: OverlayApiState,
    local_node_id: Option<String>,
) -> Router {
    // Start with the services router
    let base_router = build_router_with_services(config, storage, service_manager, local_node_id);

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
        .route(
            "/access/sessions",
            post(handlers::tunnels::create_access_session),
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

/// Build routes for cluster management (join, node listing).
///
/// Creates the routes for cluster join and node listing operations.
/// The join endpoint does NOT require JWT auth (it uses token-based auth).
///
/// # Arguments
/// * `cluster_state` - State containing the Raft coordinator
///
/// # Returns
/// A Router with the cluster endpoints
pub fn build_cluster_routes(cluster_state: ClusterApiState) -> Router<()> {
    Router::new()
        .route("/join", post(handlers::cluster::cluster_join))
        .route(
            "/signing-pubkey",
            get(handlers::cluster::cluster_signing_pubkey),
        )
        .route(
            "/signing-pubkeys",
            get(handlers::cluster::cluster_signing_pubkeys),
        )
        .route(
            "/trust-bundle",
            get(handlers::cluster::cluster_trust_bundle),
        )
        .route(
            "/trust-imports",
            post(handlers::cluster::cluster_import_trust_bundle),
        )
        .route(
            "/trust-bundles",
            get(handlers::cluster::cluster_list_trust_bundles),
        )
        .route(
            "/trust-imports/{cluster_domain}",
            delete(handlers::cluster::cluster_remove_trust_bundle),
        )
        .route(
            "/rotate-signing-key",
            post(handlers::cluster::cluster_rotate_signing_key),
        )
        .route(
            "/revoke-token",
            post(handlers::cluster::cluster_revoke_token),
        )
        .route(
            "/revocations",
            get(handlers::cluster::cluster_list_revocations),
        )
        .route(
            "/jwt-algorithm",
            post(handlers::cluster::cluster_set_jwt_algorithm),
        )
        .route("/jwt-status", get(handlers::cluster::cluster_jwt_status))
        .route(
            "/wipe-join-secret",
            post(handlers::cluster::cluster_wipe_join_secret),
        )
        .route("/nodes", get(handlers::cluster::cluster_list_nodes))
        .route("/workers", get(handlers::cluster::cluster_list_workers))
        .route(
            "/gossip/peers",
            get(handlers::cluster::cluster_list_gossip_peers),
        )
        .route("/heartbeat", post(handlers::cluster::cluster_heartbeat))
        .route(
            "/force-leader",
            post(handlers::cluster::cluster_force_leader),
        )
        // Rolling daemon-binary upgrade entry point. Followers respond
        // with 421 + X-Leader-Addr so the CLI can redirect to the leader.
        .route("/upgrade", post(handlers::cluster::cluster_upgrade))
        // Leader self-upgrade. Re-enters internal/upgrade/start over
        // loopback so the daemon exits 75 and is respawned by the
        // supervisor on the new binary.
        .route(
            "/upgrade-self",
            post(handlers::cluster::cluster_upgrade_self),
        )
        .route(
            "/nodes/{id}",
            delete(handlers::cluster::cluster_remove_node),
        )
        .route(
            "/nodes/{id}/mode",
            put(handlers::cluster::cluster_set_node_mode),
        )
        .route(
            "/nodes/{id}/drain",
            put(handlers::cluster::cluster_drain_node),
        )
        .route(
            "/nodes/{id}/undrain",
            put(handlers::cluster::cluster_undrain_node),
        )
        .with_state(cluster_state)
}

/// Build routes for storage replication status
///
/// Creates the route for querying storage replication status.
/// This route requires authentication but no specific role (read-only).
///
/// # Arguments
/// * `storage_state` - State containing the optional `SQLite` replicator
///
/// # Returns
/// A Router with the storage status endpoint
pub fn build_storage_routes(storage_state: StorageState) -> Router<()> {
    Router::new()
        .route("/status", get(handlers::storage::get_storage_status))
        .with_state(storage_state)
}

/// Build routes for volume management
///
/// Creates the routes for listing and deleting named volumes.
/// These routes require authentication; delete requires the `operator` role.
///
/// # Arguments
/// * `volume_state` - State containing the storage manager and volume directory
///
/// # Returns
/// A Router with the volume endpoints
pub fn build_volume_routes(volume_state: VolumeApiState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::volumes::list_volumes))
        .route("/", post(handlers::volumes::create_volume))
        .route("/{name}", get(handlers::volumes::get_volume))
        .route("/{name}", delete(handlers::volumes::delete_volume))
        .with_state(volume_state)
}

/// Build routes for raw container lifecycle management
///
/// Creates the routes for direct container creation, management, and inspection.
/// These routes are intended for CI runners and tooling that need container
/// access independent of the deployment/service abstraction.
///
/// # Arguments
/// * `container_state` - State containing the container runtime
///
/// # Returns
/// A Router with the container endpoints
pub fn build_container_routes(container_state: ContainerApiState) -> Router<()> {
    Router::new()
        .route("/", post(handlers::containers::create_container))
        .route("/", get(handlers::containers::list_containers))
        .route("/{id}", get(handlers::containers::get_container))
        .route("/{id}", delete(handlers::containers::delete_container))
        .route("/{id}/logs", get(handlers::containers::get_container_logs))
        // Native exec (Task 4.1.5): create an exec instance, return its id.
        // The buffered/SSE [`exec_in_container`] handler is still defined for
        // tests and the Docker compat shim's translation layer; the active
        // wire shape on this path is now the create-exec one.
        .route("/{id}/exec", post(handlers::exec_instances::create_exec))
        .route("/{id}/resize", post(handlers::containers::resize_container))
        .route("/{id}/wait", get(handlers::containers::wait_container))
        .route(
            "/{id}/wait",
            post(handlers::containers::wait_container_post),
        )
        .route("/{id}/rename", post(handlers::containers::rename_container))
        .route("/{id}/update", post(handlers::containers::update_container))
        .route(
            "/{id}/stats",
            get(handlers::containers::get_container_stats),
        )
        .route("/{id}/stop", post(handlers::containers::stop_container))
        .route("/{id}/start", post(handlers::containers::start_container))
        .route(
            "/{id}/restart",
            post(handlers::containers::restart_container),
        )
        .route("/{id}/kill", post(handlers::containers::kill_container))
        .route("/{id}/pause", post(handlers::containers::pause_container))
        .route(
            "/{id}/unpause",
            post(handlers::containers::unpause_container),
        )
        .route("/{id}/top", get(handlers::containers::top_container))
        .route(
            "/{id}/changes",
            get(handlers::containers::changes_container),
        )
        .route("/{id}/port", get(handlers::containers::port_container))
        // `docker cp` archive endpoints (GET / PUT / HEAD on the same path).
        .route(
            "/{id}/archive",
            get(handlers::containers::archive_get)
                .put(handlers::containers::archive_put)
                .on(MethodFilter::HEAD, handlers::containers::archive_head),
        )
        .route("/prune", post(handlers::containers::prune_containers))
        .with_state(container_state)
}

/// Build the daemon's exec-management routes mounted at `/api/v1/exec`.
///
/// Task 4.1.5: native exec API. Returns the per-exec endpoints
/// (`/{id}/start`, `/{id}/resize`, `/{id}/json`) keyed off the exec ids
/// minted by [`handlers::exec_instances::create_exec`]. Shares
/// [`ContainerApiState`] with the container routes so the same
/// [`crate::handlers::exec_instances::ExecInstances`] backs both.
pub fn build_exec_routes(container_state: ContainerApiState) -> Router<()> {
    // `start` is registered as GET because axum's `WebSocketUpgrade`
    // extractor enforces `Method::GET` on HTTP/1.1 (RFC 6455 §4.1). The
    // task spec described it as `POST` to mirror Docker's hijacked-stream
    // shape, but Docker's `/exec/{id}/start` is *not* a real WebSocket —
    // it's a raw HTTP/1.1 connection-hijack. Real RFC-6455 WebSocket
    // upgrades require GET, and the daemon's native exec API uses real
    // WebSockets.
    Router::new()
        .route("/{id}/start", get(handlers::exec_instances::start_exec))
        .route("/{id}/resize", post(handlers::exec_instances::resize_exec))
        .route("/{id}/json", get(handlers::exec_instances::inspect_exec))
        .with_state(container_state)
}

/// Build the daemon-wide container event stream route.
///
/// Mounts `GET /` (to be nested at `/api/v1/events`) as an SSE stream of
/// container lifecycle events. The event bus lives on `ContainerApiState`,
/// so lifecycle handlers and the event stream share the same broadcast
/// channel when both are constructed from the same state instance.
///
/// # Arguments
/// * `container_state` - Same state used by `/api/v1/containers`, so
///   start/die/etc. events published by lifecycle handlers are visible on
///   the event stream.
pub fn build_event_routes(container_state: ContainerApiState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::events::stream_events))
        .with_state(container_state)
}

/// Build routes for container bridge/overlay network management.
///
/// Creates the routes for CRUD operations on user-defined bridge or overlay
/// networks plus connect/disconnect of containers. These routes require
/// authentication; mutating endpoints require the `operator` role.
///
/// # Arguments
/// * `state` - [`BridgeNetworkApiState`] (registry + optional runtime).
///
/// # Returns
/// A Router with the container-network endpoints
pub fn build_container_network_routes(state: BridgeNetworkApiState) -> Router<()> {
    Router::new()
        .route(
            "/",
            post(handlers::container_networks::create_container_network),
        )
        .route(
            "/",
            get(handlers::container_networks::list_container_networks),
        )
        .route(
            "/{id_or_name}",
            get(handlers::container_networks::get_container_network),
        )
        .route(
            "/{id_or_name}",
            delete(handlers::container_networks::delete_container_network),
        )
        .route(
            "/{id_or_name}/connect",
            post(handlers::container_networks::connect_container_network),
        )
        .route(
            "/{id_or_name}/disconnect",
            post(handlers::container_networks::disconnect_container_network),
        )
        .with_state(state)
}

pub fn build_job_routes(job_state: JobState) -> Router<()> {
    Router::new()
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
        .with_state(job_state)
}

pub fn build_cron_routes(cron_state: CronState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::cron::list_cron_jobs))
        .route("/{name}", get(handlers::cron::get_cron_job))
        .route("/{name}/trigger", post(handlers::cron::trigger_cron_job))
        .route("/{name}/enable", put(handlers::cron::enable_cron_job))
        .route("/{name}/disable", put(handlers::cron::disable_cron_job))
        .with_state(cron_state)
}

/// Build the API router with raw container management capabilities
///
/// This extends the services router with endpoints for direct container
/// lifecycle management, independent of deployments/services.
///
/// # Arguments
/// * `config` - API configuration
/// * `storage` - Deployment storage backend
/// * `service_manager` - `ServiceManager` for service scaling operations
/// * `runtime` - Container runtime for direct container management
pub fn build_router_with_containers(
    config: &ApiConfig,
    storage: Arc<dyn DeploymentStorage + Send + Sync>,
    service_manager: Arc<RwLock<ServiceManager>>,
    runtime: Arc<dyn zlayer_agent::Runtime + Send + Sync>,
    local_node_id: Option<String>,
) -> Router {
    // Start with the services router
    let base_router = build_router_with_services(config, storage, service_manager, local_node_id);

    // Create container state (carries the shared event bus)
    let container_state = ContainerApiState::new(runtime);

    // Build container + event + exec routes from the same state so all three
    // sides see the same event bus and `ExecInstances`. The state type is
    // `Clone`, so this is cheap.
    let container_routes = build_container_routes(container_state.clone());
    let event_routes = build_event_routes(container_state.clone());
    let exec_routes = build_exec_routes(container_state);

    base_router
        .nest("/api/v1/containers", container_routes)
        .nest("/api/v1/events", event_routes)
        .nest("/api/v1/exec", exec_routes)
}

/// Build the sync routes sub-router.
///
/// # Arguments
/// * `sync_state` - State containing the sync store and clone root
///
/// # Returns
/// A Router with the sync endpoints mounted at `/api/v1/syncs`
pub fn build_sync_routes(sync_state: SyncState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::syncs::list_syncs))
        .route("/", post(handlers::syncs::create_sync))
        .route("/{id}/diff", get(handlers::syncs::diff_sync))
        .route("/{id}/apply", post(handlers::syncs::apply_sync))
        .route("/{id}", delete(handlers::syncs::delete_sync))
        .with_state(sync_state)
}

/// Build routes for workflow CRUD and execution.
///
/// Routes:
/// - `GET    /`            -- list workflows
/// - `POST   /`            -- create (admin-only)
/// - `GET    /{id}`         -- fetch one
/// - `DELETE /{id}`         -- delete (admin-only)
/// - `POST   /{id}/run`     -- execute sequentially (admin-only)
/// - `GET    /{id}/runs`    -- list past runs
///
/// All routes require authentication (Bearer or session cookie) via the
/// `AuthActor` extractor; admin enforcement happens in the handlers.
///
/// # Arguments
/// * `workflows_state` - Workflow storage state (includes task store for execution).
///
/// # Returns
/// A Router with the workflow endpoints
pub fn build_workflow_routes(workflows_state: WorkflowsState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::workflows::list_workflows))
        .route("/", post(handlers::workflows::create_workflow))
        .route("/{id}", get(handlers::workflows::get_workflow))
        .route("/{id}", delete(handlers::workflows::delete_workflow))
        .route("/{id}/run", post(handlers::workflows::run_workflow))
        .route("/{id}/runs", get(handlers::workflows::list_workflow_runs))
        .with_state(workflows_state)
}

/// Build routes for notifier CRUD and test-notification.
///
/// Routes:
/// - `GET    /`             -- list notifiers
/// - `POST   /`             -- create (admin-only)
/// - `GET    /{id}`          -- fetch one
/// - `PATCH  /{id}`          -- update (admin-only)
/// - `DELETE /{id}`          -- delete (admin-only)
/// - `POST   /{id}/test`     -- send test notification (admin-only)
///
/// All routes require authentication (Bearer or session cookie) via the
/// `AuthActor` extractor; admin enforcement happens in the handlers.
///
/// # Arguments
/// * `notifiers_state` - Notifier storage state (includes HTTP client).
///
/// # Returns
/// A Router with the notifier endpoints
pub fn build_notifier_routes(notifiers_state: NotifiersState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::notifiers::list_notifiers))
        .route("/", post(handlers::notifiers::create_notifier))
        .route("/{id}", get(handlers::notifiers::get_notifier))
        .route(
            "/{id}",
            axum::routing::patch(handlers::notifiers::update_notifier),
        )
        .route("/{id}", delete(handlers::notifiers::delete_notifier))
        .route("/{id}/test", post(handlers::notifiers::test_notifier))
        .with_state(notifiers_state)
}

/// Build routes for group CRUD and membership management.
///
/// Routes:
/// - `GET    /`                         -- list groups
/// - `POST   /`                         -- create (admin-only)
/// - `GET    /{id}`                      -- fetch one
/// - `PATCH  /{id}`                      -- update name/description (admin-only)
/// - `DELETE /{id}`                      -- delete (admin-only)
/// - `POST   /{id}/members`              -- add member (admin-only)
/// - `DELETE /{id}/members/{user_id}`    -- remove member (admin-only)
///
/// All routes require authentication via the `AuthActor` extractor; admin
/// enforcement happens in the handlers.
///
/// # Arguments
/// * `groups_state` - Group storage state.
///
/// # Returns
/// A Router with the group endpoints
pub fn build_group_routes(groups_state: GroupsState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::groups::list_groups))
        .route("/", post(handlers::groups::create_group))
        .route("/{id}", get(handlers::groups::get_group))
        .route(
            "/{id}",
            axum::routing::patch(handlers::groups::update_group),
        )
        .route("/{id}", delete(handlers::groups::delete_group))
        .route("/{id}/members", post(handlers::groups::add_member))
        .route(
            "/{id}/members/{user_id}",
            delete(handlers::groups::remove_member),
        )
        .with_state(groups_state)
}

/// Build routes for permission grant/revoke/listing.
///
/// Routes:
/// - `GET    /`              -- list permissions for a subject (user or group)
/// - `GET    /by-resource`   -- list permissions granted on a specific resource
/// - `POST   /`              -- grant (admin-only)
/// - `DELETE /{id}`          -- revoke (admin-only)
///
/// All routes require authentication via the `AuthActor` extractor; admin
/// enforcement happens in the handlers.
///
/// # Arguments
/// * `permissions_state` - Permission storage state (includes group store
///   for validating group grants).
///
/// # Returns
/// A Router with the permission endpoints
pub fn build_permission_routes(permissions_state: PermissionsState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::permissions::list_permissions))
        .route(
            "/by-resource",
            get(handlers::permissions::list_permissions_by_resource),
        )
        .route("/", post(handlers::permissions::grant_permission))
        .route("/{id}", delete(handlers::permissions::revoke_permission))
        .with_state(permissions_state)
}

/// Build routes for the audit log query endpoint.
///
/// Routes:
/// - `GET /` -- list audit entries (admin-only, supports filters)
///
/// # Arguments
/// * `audit_state` - Audit storage state.
///
/// # Returns
/// A Router with the audit endpoint
pub fn build_audit_routes(audit_state: AuditState) -> Router<()> {
    Router::new()
        .route("/", get(handlers::audit::list_audit))
        .with_state(audit_state)
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
#[allow(deprecated)]
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

        let _router =
            build_router_with_internal(&config, storage, service_manager, internal_token, None);
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
            None,
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

    #[test]
    fn test_build_storage_routes() {
        let storage_state = StorageState::new();
        let _routes = build_storage_routes(storage_state);
        // Routes build without error
    }

    #[test]
    fn test_build_storage_routes_default() {
        let storage_state = StorageState::default();
        let _routes = build_storage_routes(storage_state);
        // Routes build without error (disabled replicator)
    }

    #[test]
    fn test_build_container_routes() {
        let runtime: Arc<dyn zlayer_agent::Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let container_state = ContainerApiState::new(runtime);
        let _routes = build_container_routes(container_state);
        // Routes build without error
    }

    #[test]
    fn test_build_router_with_containers() {
        let config = ApiConfig::default();
        let storage: Arc<dyn DeploymentStorage + Send + Sync> = Arc::new(InMemoryStorage::new());
        let runtime: Arc<dyn zlayer_agent::Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let service_manager = Arc::new(RwLock::new(ServiceManager::new(runtime.clone())));

        let _router =
            build_router_with_containers(&config, storage, service_manager, runtime, None);
        // Router builds without error
    }
}
