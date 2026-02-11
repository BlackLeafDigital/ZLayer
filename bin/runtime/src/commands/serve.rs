use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::daemon::{init_daemon, restore_deployments, DaemonConfig};
use zlayer_api::handlers::build::build_routes;
use zlayer_api::handlers::cluster::ClusterApiState;
use zlayer_api::handlers::nodes::NodeApiState;
use zlayer_api::handlers::overlay::OverlayApiState;
use zlayer_api::router::{
    build_cluster_routes, build_node_routes, build_overlay_routes, build_tunnel_routes,
};
use zlayer_api::{ApiConfig, ApiServer, BuildState, TunnelApiState};
use zlayer_overlay::IpAllocator;

/// Daemon metadata written to `/var/lib/zlayer/daemon.json`.
#[derive(Serialize)]
struct DaemonMetadata {
    pid: u32,
    started_at: String,
    api_bind: String,
    socket_path: String,
    host_network: bool,
    overlay_cidr: String,
}

/// Start the daemon API server with full infrastructure.
pub(crate) async fn serve(
    bind: &str,
    jwt_secret: Option<String>,
    no_swagger: bool,
    socket_path: &str,
    host_network: bool,
) -> Result<()> {
    let jwt_secret = jwt_secret.unwrap_or_else(|| {
        warn!("Using default JWT secret - NOT SAFE FOR PRODUCTION");
        "CHANGE_ME_IN_PRODUCTION".to_string()
    });

    let bind_addr: std::net::SocketAddr = bind
        .parse()
        .context(format!("Invalid bind address: {}", bind))?;

    // -----------------------------------------------------------------------
    // 1. Create DaemonConfig
    // -----------------------------------------------------------------------
    let config = DaemonConfig {
        host_network,
        ..Default::default()
    };

    // -----------------------------------------------------------------------
    // 2. Initialise all daemon infrastructure
    // -----------------------------------------------------------------------
    info!("Initialising daemon infrastructure");
    let state = init_daemon(&config).await?;

    // -----------------------------------------------------------------------
    // 2b. Restore previously-persisted deployments
    // -----------------------------------------------------------------------
    if let Err(e) = restore_deployments(&state).await {
        warn!(error = %e, "Deployment restoration encountered errors (non-fatal)");
    }

    // Destructure the state so we can rewrap the ServiceManager for the router
    // while keeping shutdown-relevant handles separate.
    let crate::daemon::DaemonState {
        runtime: _runtime,
        overlay,
        dns,
        dns_handle,
        proxy,
        supervisor,
        supervisor_handle,
        manager,
        storage,
        secrets,
        credential_store,
        stream_registry: _stream_registry,
        service_registry: _service_registry,
        log_rotator_handle,
        health_checker_handle,
        node_config,
        raft: _raft,
        raft_server_handle,
    } = state;

    // -----------------------------------------------------------------------
    // 3. Write daemon metadata JSON
    // -----------------------------------------------------------------------
    let metadata = DaemonMetadata {
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339(),
        api_bind: bind.to_string(),
        socket_path: socket_path.to_string(),
        host_network: config.host_network,
        overlay_cidr: "10.200.0.0/16".to_string(),
    };

    let metadata_path = config.data_dir.join("daemon.json");
    let metadata_json =
        serde_json::to_string_pretty(&metadata).context("Failed to serialise daemon metadata")?;
    tokio::fs::write(&metadata_path, &metadata_json)
        .await
        .with_context(|| {
            format!(
                "Failed to write daemon metadata to {}",
                metadata_path.display()
            )
        })?;
    info!(path = %metadata_path.display(), "Daemon metadata written");

    // -----------------------------------------------------------------------
    // 4. Build the full API router
    // -----------------------------------------------------------------------
    let api_config = ApiConfig {
        bind: bind_addr,
        jwt_secret,
        swagger_enabled: !no_swagger,
        credential_store: Some(credential_store),
        ..Default::default()
    };

    // The router builder functions expect Arc<RwLock<ServiceManager>>.
    // DaemonState provides Arc<ServiceManager>. Unwrap the Arc (we are the
    // sole owner at this point) and re-wrap in RwLock + Arc.
    let service_manager = Arc::try_unwrap(manager)
        .map(|sm| Arc::new(RwLock::new(sm)))
        .map_err(|_| {
            anyhow::anyhow!(
                "BUG: ServiceManager Arc has multiple owners at serve() entry; \
                 this should never happen"
            )
        })?;

    // Build deployment state with orchestration wiring so create_deployment
    // actually registers services, sets up overlays, and scales containers.
    // Clone dns_handle so the deployment state keeps one for API handlers while
    // we retain the original for explicit shutdown cleanup.
    let deployment_state = zlayer_api::DeploymentState::with_orchestration(
        storage.clone() as Arc<dyn zlayer_api::DeploymentStorage + Send + Sync>,
        Arc::clone(&service_manager),
        overlay.clone(),
        Arc::clone(&proxy),
        dns_handle.clone(),
    );

    // Generate an internal token for scheduler-to-agent communication
    let internal_token = generate_internal_token();

    // Build the core router using the orchestration-wired deployment state.
    // This ensures create_deployment actually orchestrates containers.
    let base_router = zlayer_api::build_router_with_deployment_state(
        &api_config,
        deployment_state,
        service_manager.clone(),
        storage.clone() as Arc<dyn zlayer_api::DeploymentStorage + Send + Sync>,
    );

    // Add internal routes for scheduler-to-agent communication
    let internal_state = zlayer_api::InternalState::new(service_manager, internal_token);
    let internal_routes = zlayer_api::build_internal_routes(internal_state);
    let base_router = base_router.nest("/api/v1/internal", internal_routes);

    // Add secrets routes
    let secrets_state = zlayer_api::SecretsState::new(secrets);
    let secrets_routes = zlayer_api::build_secrets_routes(secrets_state);
    let mut router = base_router.nest("/api/v1/secrets", secrets_routes);

    // Merge node management routes
    let node_state = NodeApiState::new();
    let node_routes = build_node_routes(node_state);
    router = router.nest("/api/v1/nodes", node_routes);

    // Merge overlay network routes
    let overlay_state = OverlayApiState::new();
    let overlay_routes = build_overlay_routes(overlay_state);
    router = router.nest("/api/v1/overlay", overlay_routes);

    // Merge tunnel routes
    let tunnel_state = TunnelApiState::new();
    let tunnel_routes = build_tunnel_routes(tunnel_state);
    router = router.nest("/api/v1/tunnels", tunnel_routes);

    // Merge cluster routes (join, node listing)
    // Initialize CIDR-aware IP allocator for overlay address assignment
    let ip_allocator_path = config.data_dir.join("ip_allocator.json");
    let ip_allocator = {
        // Try to load persisted state first
        if ip_allocator_path.exists() {
            match IpAllocator::load(&ip_allocator_path).await {
                Ok(alloc) => {
                    info!(
                        path = %ip_allocator_path.display(),
                        allocated = alloc.allocated_count(),
                        "Loaded persisted IP allocator state"
                    );
                    alloc
                }
                Err(e) => {
                    warn!(
                        path = %ip_allocator_path.display(),
                        error = %e,
                        "Failed to load IP allocator state, creating fresh"
                    );
                    IpAllocator::new(&node_config.overlay_cidr)
                        .context("Invalid overlay CIDR in node config")?
                }
            }
        } else {
            IpAllocator::new(&node_config.overlay_cidr)
                .context("Invalid overlay CIDR in node config")?
        }
    };

    // For the leader node, reserve the first overlay IP if not already allocated
    let mut ip_allocator = ip_allocator;
    if node_config.is_leader {
        if let Ok(first_ip) = ip_allocator.allocate_first() {
            info!(overlay_ip = %first_ip, "Reserved first overlay IP for leader node");
        }
        // allocate_first() errors if already allocated -- that's fine on restart
    }

    // Persist the initial state
    if let Err(e) = ip_allocator.save(&ip_allocator_path).await {
        warn!("Failed to persist initial IP allocator state: {e}");
    }

    let ip_allocator = Arc::new(RwLock::new(ip_allocator));
    let cluster_state =
        ClusterApiState::new(_raft.clone(), None, ip_allocator, Some(ip_allocator_path));
    let cluster_routes = build_cluster_routes(cluster_state);
    router = router.nest("/api/v1/cluster", cluster_routes);

    // Merge build routes
    let build_dir = config.data_dir.join("builds");
    let build_state = BuildState::new(build_dir);
    let build_api_routes = build_routes().with_state(build_state);
    router = router.nest("/api/v1", build_api_routes);

    info!(
        bind = %bind_addr,
        socket = %socket_path,
        swagger = api_config.swagger_enabled,
        "Starting ZLayer daemon API server"
    );

    // -----------------------------------------------------------------------
    // 5. Setup graceful shutdown signal handler
    // -----------------------------------------------------------------------
    let shutdown = async {
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };

        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => {},
            _ = terminate => {},
        }

        info!("Shutdown signal received, starting graceful shutdown");
    };

    // -----------------------------------------------------------------------
    // 6. Run dual TCP + Unix socket server (with local auth bypass on UDS)
    // -----------------------------------------------------------------------
    ApiServer::run_dual_with_local_auth(
        bind_addr,
        socket_path,
        router,
        &api_config.jwt_secret,
        shutdown,
    )
    .await?;

    // -----------------------------------------------------------------------
    // 7. Post-shutdown cleanup: tear down infrastructure in reverse order
    // -----------------------------------------------------------------------
    info!("API server stopped, shutting down daemon infrastructure");

    // Stop Raft RPC server
    if let Some(handle) = raft_server_handle {
        handle.abort();
        let _ = handle.await;
    }
    // Shut down Raft coordinator
    if let Some(raft) = _raft {
        if let Err(e) = raft.shutdown().await {
            warn!("Raft shutdown error: {e}");
        }
    }
    info!("Raft coordinator stopped");

    // Stop health checker
    if let Some(handle) = health_checker_handle {
        handle.abort();
        let _ = handle.await;
    }
    info!("Stream health checker stopped");

    // Stop log rotator
    if let Some(handle) = log_rotator_handle {
        handle.abort();
        let _ = handle.await;
    }
    info!("Log rotator stopped");

    // Stop container supervisor
    supervisor.shutdown();
    if let Some(handle) = supervisor_handle {
        let _ = handle.await;
    }
    info!("Container supervisor stopped");

    // Stop proxy
    proxy.stop().await;
    info!("Proxy manager stopped");

    // Stop DNS server: dropping the DnsHandle and DnsServer Arc stops the
    // background UDP listener task (it is cancelled when the last Arc is dropped).
    drop(dns_handle);
    drop(dns);
    info!("DNS server stopped");

    // Cleanup overlay networks
    if let Some(om) = overlay {
        let mut om_guard = om.write().await;
        if let Err(e) = om_guard.cleanup().await {
            warn!("Failed to cleanup overlay networks: {e}");
        } else {
            info!("Overlay networks cleaned up");
        }
    }

    // Clean up daemon metadata file
    let _ = tokio::fs::remove_file(&metadata_path).await;
    info!(path = %metadata_path.display(), "Daemon metadata cleaned up");

    // Clean up PID file
    let pid_file = config.run_dir.join("zlayer.pid");
    let _ = tokio::fs::remove_file(&pid_file).await;

    info!("Daemon shutdown complete");
    Ok(())
}

/// Generate a random internal token for scheduler-to-agent communication.
fn generate_internal_token() -> String {
    use std::fmt::Write;
    let mut buf = String::with_capacity(64);
    for _ in 0..32 {
        let byte: u8 = rand::random();
        let _ = write!(buf, "{:02x}", byte);
    }
    buf
}
