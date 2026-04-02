use std::sync::Arc;

use anyhow::{Context, Result};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::daemon::{init_daemon, restore_deployments, DaemonConfig};
use zlayer_api::handlers::build::build_routes;
use zlayer_api::handlers::cluster::ClusterApiState;
use zlayer_api::handlers::nodes::NodeApiState;
use zlayer_api::handlers::overlay::OverlayApiState;
use zlayer_api::router::{
    build_cluster_routes, build_container_routes, build_cron_routes, build_job_routes,
    build_node_routes, build_overlay_routes, build_tunnel_routes,
};
use zlayer_api::{
    ApiConfig, ApiServer, BuildState, ContainerApiState, CronState, JobState, TunnelApiState,
};
use zlayer_overlay::IpAllocator;

/// Daemon metadata written to `{data_dir}/daemon.json`.
#[derive(Serialize)]
struct DaemonMetadata {
    pid: u32,
    started_at: String,
    api_bind: String,
    socket_path: String,
    host_network: bool,
    overlay_cidr: String,
}

/// Minimal struct for reading back the PID and bind address from a previous daemon's metadata.
#[derive(Deserialize)]
struct StaleDaemonMeta {
    pid: u32,
    /// The API bind address (e.g. "0.0.0.0:3669") so we can verify the port is
    /// free after killing the old process.
    #[serde(default)]
    api_bind: Option<String>,
}

/// Clean up a stale daemon process and leftover network state from a previous run.
///
/// This is best-effort: all errors are logged as warnings but never prevent startup.
#[allow(
    unsafe_code,
    clippy::too_many_lines,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
async fn cleanup_stale_daemon(config: &DaemonConfig, socket_path: &str, api_bind: &str) {
    let metadata_path = config.data_dir.join("daemon.json");
    let my_pid = std::process::id();

    // Track whether the PID-based kill was conclusive (i.e. daemon.json existed
    // and the named PID was either dead or successfully killed).
    let mut pid_kill_conclusive = false;

    // The port we need to verify is free before returning.  Default to the
    // port extracted from the *current* bind address; override with the port
    // from the old daemon.json if available.
    let mut api_port: u16 = parse_port_from_bind(api_bind).unwrap_or(3669);

    // -----------------------------------------------------------------------
    // 1. Read the old daemon PID and terminate it if still alive
    // -----------------------------------------------------------------------
    if let Ok(contents) = tokio::fs::read_to_string(&metadata_path).await {
        match serde_json::from_str::<StaleDaemonMeta>(&contents) {
            Ok(meta) => {
                // If the old daemon wrote an api_bind, use its port for the
                // port-free check so we wait on the right port even when the
                // operator changed the bind address between runs.
                if let Some(ref old_bind) = meta.api_bind {
                    if let Some(port) = parse_port_from_bind(old_bind) {
                        api_port = port;
                    }
                }

                // PID values from daemon.json are always positive and fit in i32.
                #[allow(clippy::cast_possible_wrap)]
                let old_pid = meta.pid as i32;
                // Do not kill ourselves (pid file left over from a clean restart).
                // PID is always positive here, safe to cast back to u32.
                #[allow(clippy::cast_sign_loss)]
                if old_pid as u32 == my_pid {
                    info!(
                        pid = old_pid,
                        "Stale daemon PID matches current process, skipping kill"
                    );
                    pid_kill_conclusive = true;
                } else if process_alive(old_pid) {
                    warn!(
                        pid = old_pid,
                        "Stale daemon process detected, sending SIGTERM"
                    );
                    // SAFETY: we validated the PID is alive and is not us.
                    unsafe { libc::kill(old_pid, libc::SIGTERM) };

                    // Poll for up to 5 seconds waiting for graceful exit.
                    let mut terminated = false;
                    for _ in 0..50 {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        if !process_alive(old_pid) {
                            terminated = true;
                            break;
                        }
                    }

                    if terminated {
                        info!(pid = old_pid, "Stale daemon exited after SIGTERM");
                        pid_kill_conclusive = true;
                    } else {
                        warn!(
                            pid = old_pid,
                            "Stale daemon did not exit in time, sending SIGKILL"
                        );
                        unsafe { libc::kill(old_pid, libc::SIGKILL) };
                        // Brief wait for the kernel to reap.
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        if process_alive(old_pid) {
                            warn!(pid = old_pid, "Stale daemon still alive after SIGKILL");
                        } else {
                            info!(pid = old_pid, "Stale daemon killed with SIGKILL");
                            pid_kill_conclusive = true;
                        }
                    }
                } else {
                    info!(
                        pid = old_pid,
                        "Previous daemon is not running, cleaning up stale files"
                    );
                    pid_kill_conclusive = true;
                }
            }
            Err(e) => {
                warn!(
                    path = %metadata_path.display(),
                    error = %e,
                    "Failed to parse stale daemon.json, ignoring"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // 1b. No pgrep fallback — if daemon.json was missing, we simply proceed.
    //     The old daemon is either gone or will be replaced when we bind the
    //     port/socket. Never kill arbitrary zlayer CLI processes.
    // -----------------------------------------------------------------------
    if !pid_kill_conclusive {
        info!("No daemon.json found, proceeding without killing any processes");
    }

    // -----------------------------------------------------------------------
    // 1c. Remove stale unix socket
    // -----------------------------------------------------------------------
    let socket = std::path::Path::new(socket_path);
    if socket.exists() {
        match tokio::fs::remove_file(socket).await {
            Ok(()) => info!(path = %socket_path, "Removed stale unix socket"),
            Err(e) => warn!(path = %socket_path, error = %e, "Failed to remove stale unix socket"),
        }
    }

    // -----------------------------------------------------------------------
    // 2. Clean up stale network interfaces
    // -----------------------------------------------------------------------
    #[cfg(target_os = "linux")]
    {
        // Linux: use `ip` to find and delete stale veth-* and zl-* interfaces.
        if let Ok(output) = tokio::process::Command::new("ip")
            .args(["-br", "link"])
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    // `ip -br link` format: "NAME  STATE  ..."
                    let Some(iface) = line.split_whitespace().next() else {
                        continue;
                    };

                    if iface.starts_with("veth-") || iface.starts_with("zl-") {
                        warn!(interface = %iface, "Deleting stale network interface");
                        let _ = tokio::process::Command::new("ip")
                            .args(["link", "delete", iface])
                            .output()
                            .await;
                    }
                }
            }
        } else {
            warn!("Failed to list network interfaces for stale cleanup");
        }
    }
    #[cfg(target_os = "macos")]
    {
        // macOS: use `ifconfig` to find stale utun devices that have a matching
        // WireGuard UAPI socket in /var/run/wireguard/, indicating they belong
        // to a previous zlayer run.
        let wg_sock_dir = std::path::Path::new("/var/run/wireguard");
        if let Ok(output) = tokio::process::Command::new("ifconfig")
            .args(["-l"])
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for iface in stdout.split_whitespace() {
                    if iface.starts_with("utun") {
                        let sock_path = wg_sock_dir.join(format!("{iface}.sock"));
                        if sock_path.exists() {
                            warn!(interface = %iface, "Destroying stale utun interface");
                            let _ = tokio::process::Command::new("ifconfig")
                                .args([iface, "destroy"])
                                .output()
                                .await;
                            let _ = tokio::fs::remove_file(&sock_path).await;
                        }
                    }
                }
            }
        } else {
            warn!("Failed to list network interfaces for stale cleanup");
        }
    }

    // -----------------------------------------------------------------------
    // 3. Remove stale WireGuard UAPI sockets
    // -----------------------------------------------------------------------
    let wg_sock_dir = std::path::Path::new("/var/run/wireguard");
    if wg_sock_dir.is_dir() {
        match tokio::fs::read_dir(wg_sock_dir).await {
            Ok(mut entries) => {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("sock") {
                        warn!(path = %path.display(), "Removing stale WireGuard socket");
                        let _ = tokio::fs::remove_file(&path).await;
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to read /var/run/wireguard for stale socket cleanup");
            }
        }
    }

    // -----------------------------------------------------------------------
    // 4. Wait for API port to be free
    // -----------------------------------------------------------------------
    {
        let bind_addr: std::net::SocketAddr = ([0, 0, 0, 0], api_port).into();
        let mut port_free = false;
        for attempt in 1..=10 {
            if let Ok(_listener) = std::net::TcpListener::bind(bind_addr) {
                // Listener is dropped immediately, port is free.
                if attempt > 1 {
                    info!(port = api_port, attempts = attempt, "API port is now free");
                }
                port_free = true;
                break;
            }
            if attempt == 1 {
                warn!(
                    port = api_port,
                    "API port still in use, waiting for it to be released"
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if !port_free {
            warn!(
                port = api_port,
                "API port still in use after 1s — startup may fail with 'Address already in use'"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 4b. Wait for WireGuard UDP port to be free (DEFAULT_WG_PORT in bootstrap.rs)
    // -----------------------------------------------------------------------
    {
        let wg_port: u16 = 51820;
        let wg_addr: std::net::SocketAddr = ([0, 0, 0, 0], wg_port).into();
        let mut wg_free = false;
        for attempt in 1..=10 {
            if std::net::UdpSocket::bind(wg_addr).is_ok() {
                if attempt > 1 {
                    info!(
                        port = wg_port,
                        attempts = attempt,
                        "WireGuard UDP port is now free"
                    );
                }
                wg_free = true;
                break;
            }
            if attempt == 1 {
                warn!(
                    port = wg_port,
                    "WireGuard UDP port still in use, waiting..."
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if !wg_free {
            warn!(
                port = wg_port,
                "WireGuard UDP port still in use after 1s — overlay may fail"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 5. Remove stale daemon metadata and PID files
    // -----------------------------------------------------------------------
    if metadata_path.exists() {
        let _ = tokio::fs::remove_file(&metadata_path).await;
        info!(path = %metadata_path.display(), "Removed stale daemon.json");
    }

    let pid_file = config.run_dir.join("zlayer.pid");
    if pid_file.exists() {
        let _ = tokio::fs::remove_file(&pid_file).await;
        info!(path = %pid_file.display(), "Removed stale zlayer.pid");
    }
}

/// Check whether a process with the given PID is still alive.
///
/// Uses `kill(pid, 0)` which checks for existence without sending a signal.
#[allow(unsafe_code)]
fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 is a null signal used purely for existence checking.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Extract the port number from a bind address string like `"0.0.0.0:3669"`.
fn parse_port_from_bind(bind: &str) -> Option<u16> {
    bind.rsplit_once(':')
        .and_then(|(_, port_str)| port_str.parse::<u16>().ok())
}

/// Start the daemon API server with full infrastructure.
#[allow(clippy::too_many_lines)]
pub(crate) async fn serve(
    bind: &str,
    jwt_secret: Option<String>,
    no_swagger: bool,
    socket_path: &str,
    host_network: bool,
    data_dir: std::path::PathBuf,
) -> Result<()> {
    let jwt_secret_raw = jwt_secret.unwrap_or_else(|| {
        warn!("Using default JWT secret - NOT SAFE FOR PRODUCTION");
        "CHANGE_ME_IN_PRODUCTION".to_string()
    });
    let jwt_secret = SecretString::from(jwt_secret_raw.clone());

    let bind_addr: std::net::SocketAddr = bind
        .parse()
        .context(format!("Invalid bind address: {bind}"))?;

    // -----------------------------------------------------------------------
    // 1. Create DaemonConfig
    // -----------------------------------------------------------------------
    let log_dir = crate::cli::default_log_dir(&data_dir);
    let run_dir = crate::cli::default_run_dir(&data_dir);
    let s3_storage = std::env::var("ZLAYER_S3_BUCKET").ok().map(|bucket| {
        let mut config = zlayer_storage::LayerStorageConfig::new(&bucket);
        if let Ok(region) = std::env::var("ZLAYER_S3_REGION") {
            config = config.with_region(&region);
        }
        if let Ok(endpoint) = std::env::var("ZLAYER_S3_ENDPOINT") {
            config = config.with_endpoint_url(&endpoint);
        }
        config = config
            .with_staging_dir(data_dir.join("layer-staging"))
            .with_state_db_path(data_dir.join("layer-sync-state.sqlite"));
        config
    });

    let config = DaemonConfig {
        host_network,
        data_dir,
        log_dir,
        run_dir,
        s3_storage,
        auth_context: Some(zlayer_agent::ContainerAuthContext {
            api_url: format!("http://{bind}"),
            jwt_secret: jwt_secret_raw.clone(),
            socket_path: socket_path.to_string(),
        }),
        ..Default::default()
    };

    // -----------------------------------------------------------------------
    // 1b. Clean up any stale daemon from a previous run
    // -----------------------------------------------------------------------
    cleanup_stale_daemon(&config, socket_path, bind).await;

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
        runtime,
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
        cert_manager: _cert_manager,
        log_rotator_handle,
        health_checker_handle,
        node_config,
        raft,
        raft_server_handle,
        heartbeat_handle,
        dead_node_detection_handle,
        scheduler: _scheduler,
        internal_token: daemon_internal_token,
        replicator,
        job_executor,
        cron_scheduler,
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
    // 3b. Generate or load the cluster join secret
    // -----------------------------------------------------------------------
    let join_secret = {
        let join_secret_path = config.data_dir.join("join_secret");
        if join_secret_path.exists() {
            match tokio::fs::read_to_string(&join_secret_path).await {
                Ok(secret) => {
                    let secret = secret.trim().to_string();
                    info!(
                        path = %join_secret_path.display(),
                        "Loaded existing cluster join secret"
                    );
                    Some(secret)
                }
                Err(e) => {
                    warn!(
                        path = %join_secret_path.display(),
                        error = %e,
                        "Failed to load join secret, generating new one"
                    );
                    None
                }
            }
        } else {
            None
        }
    };

    let join_secret = if let Some(s) = join_secret {
        s
    } else {
        // Generate a random 32-byte secret and hex-encode it
        use rand::Rng;
        let secret_bytes: [u8; 32] = rand::rng().random();
        let secret = hex::encode(secret_bytes);

        // Persist it so it survives restarts
        let join_secret_path = config.data_dir.join("join_secret");
        if let Err(e) = tokio::fs::write(&join_secret_path, &secret).await {
            warn!(
                path = %join_secret_path.display(),
                error = %e,
                "Failed to persist join secret (token validation will not work across restarts)"
            );
        } else {
            info!(
                path = %join_secret_path.display(),
                "Generated and persisted new cluster join secret"
            );
        }

        secret
    };

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

    // Use the internal token generated during daemon init so the scheduler
    // (which already has a copy) and the API InternalState share the same secret.
    let internal_token = daemon_internal_token;

    // Build the core router using the orchestration-wired deployment state.
    // This ensures create_deployment actually orchestrates containers.
    let base_router = zlayer_api::build_router_with_deployment_state(
        &api_config,
        deployment_state,
        service_manager.clone(),
        storage.clone() as Arc<dyn zlayer_api::DeploymentStorage + Send + Sync>,
    );

    // Add internal routes for scheduler-to-agent communication.
    // Include the overlay interface name so the add-peer endpoint can manage
    // WireGuard peers on this node.
    let overlay_interface = if config.host_network {
        None
    } else {
        Some(zlayer_overlay::DEFAULT_INTERFACE_NAME.to_string())
    };
    let internal_state = zlayer_api::InternalState::with_overlay(
        service_manager,
        internal_token.clone(),
        overlay_interface,
    );
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

    // Merge storage replication status routes
    let storage_api_state = zlayer_api::StorageState::new(replicator);
    let storage_routes = zlayer_api::build_storage_routes(storage_api_state);
    router = router.nest("/api/v1/storage", storage_routes);

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
    let cluster_state = ClusterApiState::with_internal_token(
        raft.clone(),
        Some(join_secret),
        ip_allocator,
        Some(ip_allocator_path),
        internal_token,
        Some(config.data_dir.clone()),
    );
    let cluster_routes = build_cluster_routes(cluster_state);
    router = router.nest("/api/v1/cluster", cluster_routes);

    // Merge build routes
    let build_dir = config.data_dir.join("builds");
    let build_state = BuildState::new(build_dir);
    let build_api_routes = build_routes().with_state(build_state);
    router = router.nest("/api/v1", build_api_routes);

    // Merge container lifecycle routes
    let container_state = ContainerApiState::new(runtime);
    let container_routes = build_container_routes(container_state);
    router = router.nest("/api/v1/containers", container_routes);

    // Merge job routes
    let job_state = JobState {
        executor: job_executor,
    };
    router = router.nest("/api/v1/jobs", build_job_routes(job_state));

    // Merge cron routes
    let cron_state = CronState {
        scheduler: cron_scheduler,
    };
    router = router.nest("/api/v1/cron", build_cron_routes(cron_state));

    // Re-apply the auth extension layer AFTER all .nest() calls so that
    // routes added after the base router also receive the AuthState extension.
    let auth_state = zlayer_api::AuthState {
        jwt_secret: api_config.jwt_secret.clone(),
        credential_store: api_config.credential_store.clone(),
    };
    router = router.layer(zlayer_api::Extension(auth_state));

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
            () = ctrl_c => {},
            () = terminate => {},
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
        api_config.jwt_secret.expose_secret(),
        shutdown,
    )
    .await?;

    // -----------------------------------------------------------------------
    // 7. Post-shutdown cleanup: tear down infrastructure in reverse order
    // -----------------------------------------------------------------------
    info!("API server stopped, shutting down daemon infrastructure");

    // Stop heartbeat / dead-node detection background tasks
    if let Some(handle) = heartbeat_handle {
        handle.abort();
        let _ = handle.await;
    }
    if let Some(handle) = dead_node_detection_handle {
        handle.abort();
        let _ = handle.await;
    }
    info!("Heartbeat / dead-node detection stopped");

    // Stop Raft RPC server
    if let Some(handle) = raft_server_handle {
        handle.abort();
        let _ = handle.await;
    }
    // Shut down Raft coordinator
    if let Some(raft_coord) = raft {
        if let Err(e) = raft_coord.shutdown().await {
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
