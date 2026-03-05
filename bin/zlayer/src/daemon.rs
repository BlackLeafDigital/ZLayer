//! Daemon infrastructure initialization.
//!
//! Extracts the infrastructure setup phases from `deploy_services()` into a
//! reusable `init_daemon()` function. This allows the runtime to initialize all
//! subsystems once and then serve multiple deployments, API requests, etc.
//!
//! The initialization sequence mirrors phases 1-7 of `deploy_services()` in
//! `commands/deploy.rs`, plus new persistent storage and L4 proxy registries.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;
use tracing::{info, warn};

use zlayer_agent::{
    ContainerSupervisor, OverlayManager, ProxyManager, ProxyManagerConfig, Runtime, RuntimeConfig,
    ServiceManager,
};
use zlayer_api::{DeploymentStatus, DeploymentStorage, SqlxStorage, StoredDeployment};
use zlayer_overlay::{DnsHandle, DnsServer, OverlayTransport};
use zlayer_proxy::{CertManager, ServiceRegistry, StreamRegistry};
use zlayer_scheduler::{
    RaftConfig, RaftCoordinator, RaftService, Request, Scheduler, SchedulerConfig,
};
use zlayer_secrets::{CredentialStore, KeyManager, PersistentSecretsStore};

use crate::commands::node::{
    current_timestamp, detect_local_ip, generate_node_id, load_node_config, save_node_config,
    NodeConfig,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for daemon infrastructure initialization.
pub struct DaemonConfig {
    /// Skip overlay networking and use the host network stack directly.
    pub host_network: bool,

    /// Deployment name used for overlay interface naming and DNS zone.
    pub deployment_name: String,

    /// Container runtime selection (Auto, Youki, Docker, etc.).
    pub runtime_config: RuntimeConfig,

    /// DNS listen port for overlay service discovery (default: 15353).
    pub dns_port: u16,

    /// Root data directory (databases, state).
    /// Default: `~/.local/share/zlayer` on macOS, `/var/lib/zlayer` on Linux.
    pub data_dir: PathBuf,

    /// Log directory.  Default: `{data_dir}/logs` on macOS, `/var/log/zlayer` on Linux.
    pub log_dir: PathBuf,

    /// Runtime directory (sockets, PID files).  Default: `{data_dir}/run` on macOS, `/var/run/zlayer` on Linux.
    pub run_dir: PathBuf,

    /// S3 configuration for `SQLite` replication (None = disabled).
    pub s3_storage: Option<zlayer_storage::LayerStorageConfig>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        let data_dir = crate::cli::default_data_dir();
        let log_dir = crate::cli::default_log_dir(&data_dir);
        let run_dir = crate::cli::default_run_dir(&data_dir);
        Self {
            host_network: false,
            deployment_name: "zlayer".to_string(),
            runtime_config: RuntimeConfig::Auto,
            dns_port: 15353,
            data_dir,
            log_dir,
            run_dir,
            s3_storage: None,
        }
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Holds all initialised daemon infrastructure handles.
///
/// Callers can pull out what they need (e.g. `manager` for deploying services,
/// `storage` for recording deployment state) without re-initialising anything.
pub struct DaemonState {
    /// Container runtime (libcontainer / Docker / WASM / mock).
    pub runtime: Arc<dyn Runtime + Send + Sync>,

    /// Overlay network manager.  `None` when `--host-network` is active or
    /// overlay setup failed non-fatally.
    pub overlay: Option<Arc<RwLock<OverlayManager>>>,

    /// DNS server for service discovery.  `None` if DNS setup failed.
    pub dns: Option<Arc<DnsServer>>,

    /// Handle for adding/removing DNS records at runtime.
    pub dns_handle: Option<DnsHandle>,

    /// Proxy manager for HTTP route/backend management.
    pub proxy: Arc<ProxyManager>,

    /// Container supervisor for crash/panic policy enforcement.
    pub supervisor: Arc<ContainerSupervisor>,

    /// Background task running `ContainerSupervisor::run_loop()`.
    pub supervisor_handle: Option<tokio::task::JoinHandle<()>>,

    /// Service manager wired to all subsystems.
    pub manager: Arc<ServiceManager>,

    /// Persistent deployment storage (`SQLite`).
    pub storage: Arc<SqlxStorage>,

    /// Persistent encrypted secrets store (`SQLite` + XChaCha20-Poly1305).
    pub secrets: Arc<PersistentSecretsStore>,

    /// Credential store for API key authentication.
    pub credential_store: Arc<CredentialStore<Arc<PersistentSecretsStore>>>,

    /// L4 stream registry for TCP/UDP proxy routing.
    pub stream_registry: Arc<StreamRegistry>,

    /// TLS certificate manager (ACME / self-signed).
    pub cert_manager: Arc<CertManager>,

    /// Background task for log rotation (hourly).
    pub log_rotator_handle: Option<tokio::task::JoinHandle<()>>,

    /// Background task for L4 stream backend health checking.
    pub health_checker_handle: Option<tokio::task::JoinHandle<()>>,

    /// Node configuration (identity, networking, `WireGuard` keys).
    pub(crate) node_config: NodeConfig,

    /// Raft coordinator for distributed consensus.
    pub raft: Option<Arc<RaftCoordinator>>,

    /// Background task running the Raft RPC server.
    pub raft_server_handle: Option<tokio::task::JoinHandle<()>>,

    /// Background task for worker heartbeat (non-leader nodes).
    pub heartbeat_handle: Option<tokio::task::JoinHandle<()>>,

    /// Background task for dead-node detection (leader nodes).
    pub dead_node_detection_handle: Option<tokio::task::JoinHandle<()>>,

    /// Scheduler for distributed autoscaling and rescheduling (leader nodes).
    /// `None` if Raft is not initialised or this is not the leader.
    pub scheduler: Option<Arc<Scheduler>>,

    /// Internal authentication token shared between the scheduler and the API.
    /// Needed by serve.rs to create `InternalState` with the same token.
    pub internal_token: String,

    /// `SQLite` replicator for deployment DB backup to S3.
    /// `None` when S3 storage is not configured.
    pub replicator: Option<Arc<zlayer_storage::SqliteReplicator>>,
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Load an existing node configuration or auto-initialise one.
///
/// Checks for `{data_dir}/node_config.json`.
/// - If present: loads and returns it.
/// - If absent: generates a new UUID `node_id`, sets `raft_node_id = 1`,
///   auto-detects the machine IP, generates a `WireGuard` keypair, sets
///   `is_leader = true`, writes the file and returns the config.
pub(crate) async fn load_or_init_node_config(data_dir: &std::path::Path) -> Result<NodeConfig> {
    let config_path = data_dir.join("node_config.json");

    if config_path.exists() {
        let cfg = load_node_config(data_dir)
            .await
            .context("Failed to load existing node config")?;
        info!(
            node_id = %cfg.node_id,
            raft_node_id = cfg.raft_node_id,
            is_leader = cfg.is_leader,
            "Loaded existing node configuration"
        );
        return Ok(cfg);
    }

    // --- First-time init ---
    info!("No node configuration found, auto-initializing");

    let node_id = generate_node_id();
    let raft_node_id: u64 = 1;
    let advertise_addr = detect_local_ip();
    let api_port: u16 = 3669;
    let raft_port: u16 = 9000;
    let overlay_port: u16 = 51820;
    let overlay_cidr = "10.200.0.0/16".to_string();

    let (private_key, public_key) = OverlayTransport::generate_keys()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to generate overlay keys: {e}"))?;

    let cfg = NodeConfig {
        node_id: node_id.clone(),
        raft_node_id,
        advertise_addr: advertise_addr.clone(),
        api_port,
        raft_port,
        overlay_port,
        overlay_cidr,
        wireguard_private_key: private_key,
        wireguard_public_key: public_key,
        is_leader: true,
        created_at: current_timestamp(),
    };

    save_node_config(data_dir, &cfg).await?;

    info!(
        node_id = %cfg.node_id,
        advertise_addr = %cfg.advertise_addr,
        "Auto-initialized node configuration (leader)"
    );

    Ok(cfg)
}

/// Initialise all daemon infrastructure.
///
/// This mirrors the infrastructure setup phases (1-7) of `deploy_services()`
/// in `commands/deploy.rs`, with the addition of persistent storage, secrets,
/// and L4 proxy registries.
///
/// # Phases
///
///  1. Create required directories
///  2. Create container runtime
///  3. Create overlay manager + global overlay (unless `host_network`)
///  4. Start DNS server for service discovery
///  5. Create proxy manager
///  6. Create stream registry + service registry (L4/L7 proxy)
///  7. Create container supervisor + spawn `run_loop`
///  8. Wire service manager with all subsystems
///  9. Open persistent deployment storage (`SQLite`)
/// 10. Open persistent secrets store (`SQLite` + encryption key)
///
/// # Errors
///
/// Returns an error if any infrastructure phase fails to initialize.
#[allow(clippy::too_many_lines)]
pub async fn init_daemon(config: &DaemonConfig) -> Result<DaemonState> {
    // -----------------------------------------------------------------------
    // Phase 1: Create directories
    // -----------------------------------------------------------------------
    for dir in [&config.data_dir, &config.log_dir, &config.run_dir] {
        tokio::fs::create_dir_all(dir)
            .await
            .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
    }
    info!(
        data_dir = %config.data_dir.display(),
        log_dir  = %config.log_dir.display(),
        run_dir  = %config.run_dir.display(),
        "Daemon directories ready"
    );

    // -----------------------------------------------------------------------
    // Phase 2: Container runtime
    //
    // When running on Linux, enrich the runtime configuration with the
    // daemon's log directory and deployment name so that container
    // stdout/stderr are written to the structured path:
    //   /var/log/zlayer/{deployment}/{service}/{container_id}.{stdout,stderr}.log
    // -----------------------------------------------------------------------
    let runtime_config = {
        #[cfg(target_os = "linux")]
        {
            use zlayer_agent::YoukiConfig;
            match config.runtime_config.clone() {
                RuntimeConfig::Auto => {
                    // Override Auto with an explicitly configured Youki runtime
                    // so that the log directory settings are propagated.
                    let youki_cfg = YoukiConfig {
                        log_base_dir: Some(config.log_dir.clone()),
                        deployment_name: Some(config.deployment_name.clone()),
                        ..Default::default()
                    };
                    RuntimeConfig::Youki(youki_cfg)
                }
                RuntimeConfig::Youki(mut youki_cfg) => {
                    if youki_cfg.log_base_dir.is_none() {
                        youki_cfg.log_base_dir = Some(config.log_dir.clone());
                    }
                    if youki_cfg.deployment_name.is_none() {
                        youki_cfg.deployment_name = Some(config.deployment_name.clone());
                    }
                    RuntimeConfig::Youki(youki_cfg)
                }
                other => other,
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            config.runtime_config.clone()
        }
    };

    let runtime = zlayer_agent::create_runtime(runtime_config)
        .await
        .context("Failed to create container runtime")?;

    info!("Container runtime initialised");

    // -----------------------------------------------------------------------
    // Phase 3: Overlay manager
    // -----------------------------------------------------------------------
    let overlay = if config.host_network {
        info!("Host networking mode: skipping overlay network setup");
        None
    } else {
        match OverlayManager::new(config.deployment_name.clone()).await {
            Ok(mut om) => {
                if let Err(e) = om.setup_global_overlay().await {
                    warn!(
                        "Global overlay failed (cross-node disabled): {e}. \
                         Container networking via veth pairs still available."
                    );
                } else {
                    info!("Global overlay network created");
                }
                Some(Arc::new(RwLock::new(om)))
            }
            Err(e) => {
                warn!("Overlay networks disabled: {e}");
                None
            }
        }
    };

    // -----------------------------------------------------------------------
    // Phase 4: DNS server
    // -----------------------------------------------------------------------
    let (dns, dns_handle) = {
        let dns_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), config.dns_port);
        let zone = format!("{}.local.", config.deployment_name);
        match DnsServer::new(dns_addr, &zone) {
            Ok(dns) => {
                let dns = Arc::new(dns);
                match dns.start_background().await {
                    Ok(handle) => {
                        info!(%dns_addr, "DNS server started");
                        (Some(dns), Some(handle))
                    }
                    Err(e) => {
                        warn!("Failed to start DNS server: {e}");
                        (None, None)
                    }
                }
            }
            Err(e) => {
                warn!("Failed to create DNS server: {e}");
                (None, None)
            }
        }
    };

    // -----------------------------------------------------------------------
    // Phase 5: Stream registry + service registry + cert manager (L4/L7)
    // -----------------------------------------------------------------------
    let stream_registry = Arc::new(StreamRegistry::new());
    let service_registry = Arc::new(ServiceRegistry::new());

    // Start background TCP health checker for stream backends (every 5s, 2s timeout)
    let health_checker_handle = stream_registry.spawn_health_checker(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_secs(2),
    );

    // Certificate manager for TLS termination (ACME / self-signed)
    let cert_manager = Arc::new(
        CertManager::new(
            config.data_dir.join("certs").to_string_lossy().to_string(),
            None, // ACME email - can be configured later
        )
        .await
        .map_err(|e| anyhow::anyhow!(e))
        .context("Failed to create certificate manager")?,
    );

    info!("Stream and service registries initialised (with health checker)");

    // -----------------------------------------------------------------------
    // Phase 6: Proxy manager (wired with registries + cert manager)
    // -----------------------------------------------------------------------
    let mut proxy_builder = ProxyManager::new(
        ProxyManagerConfig::default(),
        Arc::clone(&service_registry),
        Some(Arc::clone(&cert_manager)),
    );
    proxy_builder.set_stream_registry(Arc::clone(&stream_registry));
    let proxy = Arc::new(proxy_builder);
    info!("Proxy manager initialised (with L4 stream + TLS support)");

    // -----------------------------------------------------------------------
    // Phase 7: Container supervisor
    // -----------------------------------------------------------------------
    let supervisor = Arc::new(ContainerSupervisor::new(runtime.clone()));

    let sup_clone = Arc::clone(&supervisor);
    let supervisor_handle = Some(tokio::spawn(async move { sup_clone.run_loop().await }));

    info!("Container supervisor started");

    // -----------------------------------------------------------------------
    // Phase 8: Service manager – wired to all subsystems
    // -----------------------------------------------------------------------
    let mut builder = ServiceManager::builder(runtime.clone())
        .deployment_name(config.deployment_name.clone())
        .proxy_manager(Arc::clone(&proxy))
        .stream_registry(Arc::clone(&stream_registry))
        .container_supervisor(Arc::clone(&supervisor));

    if let Some(om) = overlay.clone() {
        builder = builder.overlay_manager(om);
    }

    if let Some(dns_ref) = &dns {
        builder = builder.dns_server(Arc::clone(dns_ref));
    }

    let manager = Arc::new(builder.build());
    info!("Service manager initialised");

    // -----------------------------------------------------------------------
    // Phase 9: Persistent deployment storage (SQLite)
    // -----------------------------------------------------------------------
    let db_path = config.data_dir.join("deployments.db");
    let storage =
        Arc::new(SqlxStorage::open(&db_path).await.with_context(|| {
            format!("Failed to open deployment storage at {}", db_path.display())
        })?);
    info!(path = %db_path.display(), "Deployment storage opened");

    // -----------------------------------------------------------------------
    // Phase 9b: SQLite replication to S3
    // -----------------------------------------------------------------------
    let replicator = if let Some(ref s3_config) = config.s3_storage {
        let replicator_config = zlayer_storage::SqliteReplicatorConfig::new(
            &db_path,
            &s3_config.bucket,
            format!("{}/deployments/", config.deployment_name),
        )
        .with_auto_restore(true)
        .with_cache_dir(config.data_dir.join("replicator-cache"));

        match zlayer_storage::SqliteReplicator::new(replicator_config, s3_config).await {
            Ok(replicator) => {
                // Auto-restore on startup if backup exists
                match replicator.restore().await {
                    Ok(true) => info!("Deployment DB restored from S3 backup"),
                    Ok(false) => info!("No S3 backup found, starting fresh"),
                    Err(e) => warn!("S3 restore failed (non-fatal): {e}"),
                }
                // Start background WAL monitoring
                if let Err(e) = replicator.start().await {
                    warn!("Failed to start SQLite replicator (non-fatal): {e}");
                    None
                } else {
                    info!("SQLite replication to S3 started");
                    Some(Arc::new(replicator))
                }
            }
            Err(e) => {
                warn!("Failed to create SQLite replicator (non-fatal): {e}");
                None
            }
        }
    } else {
        None
    };

    // -----------------------------------------------------------------------
    // Phase 10: Persistent secrets store
    // -----------------------------------------------------------------------
    let key_manager = KeyManager::with_base_dir(&config.data_dir);
    let encryption_key = key_manager
        .get_or_create_key(&config.deployment_name)
        .context("Failed to resolve secrets encryption key")?;

    let secrets_dir = config.data_dir.join("secrets");
    let secrets = Arc::new(
        PersistentSecretsStore::open(&secrets_dir, encryption_key)
            .await
            .with_context(|| {
                format!("Failed to open secrets store at {}", secrets_dir.display())
            })?,
    );
    info!(path = %secrets_dir.display(), "Secrets store opened");

    // -----------------------------------------------------------------------
    // Phase 11: Credential store + admin bootstrap
    // -----------------------------------------------------------------------
    let credential_store = Arc::new(CredentialStore::new(Arc::clone(&secrets)));

    // Bootstrap admin credential if none exists.
    let admin_password = generate_admin_password();
    match credential_store
        .ensure_admin("admin", &admin_password)
        .await
    {
        Ok(true) => {
            info!(
                api_key = "admin",
                "Admin API credential bootstrapped. \
                 Generated password: {admin_password}"
            );
        }
        Ok(false) => {
            info!("Admin API credential already exists");
        }
        Err(e) => {
            warn!(error = %e, "Failed to bootstrap admin credential (non-fatal)");
        }
    }

    // -----------------------------------------------------------------------
    // Phase 12: Log rotation background task
    // -----------------------------------------------------------------------
    let log_dir_clone = config.log_dir.clone();
    let data_dir_clone = config.data_dir.clone();
    let log_rotator_handle = Some(tokio::spawn(async move {
        log_rotation_loop(&log_dir_clone, &data_dir_clone).await;
    }));
    info!("Log rotation background task started");

    // -----------------------------------------------------------------------
    // Phase 13: Node configuration (auto-init if first run)
    // -----------------------------------------------------------------------
    let node_config = load_or_init_node_config(&config.data_dir).await?;

    // -----------------------------------------------------------------------
    // Phase 14: Internal token (generated early for Raft auth + Scheduler)
    // -----------------------------------------------------------------------

    // Generate the internal token up-front so the Raft RPC layer, the
    // Scheduler (for dispatching scale requests), and the API InternalState
    // (for validating them) all share the same secret.
    let internal_token = generate_internal_token();

    // -----------------------------------------------------------------------
    // Phase 15: Raft distributed consensus
    // -----------------------------------------------------------------------
    let (raft, raft_server_handle) = {
        let _raft_db_path = config.data_dir.join("raft.db");
        let raft_cfg = RaftConfig {
            node_id: node_config.raft_node_id,
            address: format!("{}:{}", node_config.advertise_addr, node_config.raft_port),
            raft_port: node_config.raft_port,
            ..Default::default()
        };

        match RaftCoordinator::with_auth(raft_cfg, Some(internal_token.clone())).await {
            Ok(coordinator) => {
                // Bootstrap as single-node cluster if this is the leader (first node)
                if node_config.is_leader {
                    if let Err(e) = coordinator.bootstrap().await {
                        // Already bootstrapped is fine (idempotent restart)
                        warn!("Raft bootstrap: {e} (may already be bootstrapped)");
                    } else {
                        info!("Raft single-node cluster bootstrapped");
                    }

                    // Register the leader node in the Raft state machine so it
                    // appears in cluster state with its overlay networking metadata.
                    let leader_overlay_ip = "10.200.0.1".to_string();
                    let leader_raft_addr =
                        format!("{}:{}", node_config.advertise_addr, node_config.raft_port);
                    let sys_res = crate::resources::detect_system_resources(&config.data_dir);
                    if let Err(e) = coordinator
                        .propose(Request::RegisterNode {
                            node_id: node_config.raft_node_id,
                            address: leader_raft_addr,
                            wg_public_key: node_config.wireguard_public_key.clone(),
                            overlay_ip: leader_overlay_ip,
                            overlay_port: node_config.overlay_port,
                            advertise_addr: node_config.advertise_addr.clone(),
                            api_port: node_config.api_port,
                            cpu_total: sys_res.cpu_total,
                            memory_total: sys_res.memory_total,
                            disk_total: sys_res.disk_total,
                            gpus: vec![],
                        })
                        .await
                    {
                        warn!("Failed to register leader in Raft state: {e}");
                    }
                }

                let coordinator = Arc::new(coordinator);

                // Start Raft RPC server in the background.
                // Bind to the overlay IP when available so Raft traffic stays on
                // the encrypted WireGuard mesh. Fall back to 127.0.0.1 (loopback)
                // in host-networking mode -- never bind to 0.0.0.0.
                let raft_service =
                    RaftService::with_auth(Arc::clone(&coordinator), Some(internal_token.clone()));

                let raft_bind_ip: std::net::IpAddr = if let Some(om) = &overlay {
                    om.read().await.node_ip().map_or(
                        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                        std::net::IpAddr::V4,
                    )
                } else {
                    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
                };
                let raft_addr = std::net::SocketAddr::new(raft_bind_ip, node_config.raft_port);

                let raft_handle = {
                    let svc = raft_service;
                    Some(tokio::spawn(async move {
                        if let Err(e) = svc.run(raft_addr).await {
                            warn!("Raft RPC server exited: {e}");
                        }
                    }))
                };

                info!(
                    raft_node_id = node_config.raft_node_id,
                    raft_addr = %raft_addr,
                    "Raft coordinator started"
                );

                (Some(coordinator), raft_handle)
            }
            Err(e) => {
                warn!("Failed to create Raft coordinator (non-fatal): {e}");
                (None, None)
            }
        }
    };

    // -----------------------------------------------------------------------
    // Phase 16: Scheduler + Heartbeat & dead-node detection
    // -----------------------------------------------------------------------

    // Create the Scheduler (with raft if available) for rescheduling on
    // node death.  On leader nodes the scheduler is passed to the
    // dead-node detection loop so it can call `handle_node_death()`.
    let api_port = node_config.api_port;
    let agent_base_url = format!("http://127.0.0.1:{api_port}");

    let scheduler: Option<Arc<Scheduler>> = raft.as_ref().map(|raft_ref| {
        let sched_config = SchedulerConfig {
            raft: RaftConfig {
                node_id: node_config.raft_node_id,
                address: format!("{}:{}", node_config.advertise_addr, node_config.raft_port),
                ..Default::default()
            },
            ..Default::default()
        };
        Arc::new(Scheduler::with_raft(
            sched_config,
            Arc::clone(raft_ref),
            internal_token.clone(),
            agent_base_url.clone(),
        ))
    });

    let (heartbeat_handle, dead_node_detection_handle) = match raft.as_ref() {
        Some(raft_ref) => {
            if node_config.is_leader {
                // Leader: spawn dead-node detection (30-second timeout)
                let raft_clone = Arc::clone(raft_ref);
                let scheduler_clone = scheduler.clone();
                let dead_node_handle = Some(tokio::spawn(async move {
                    dead_node_detection_loop(
                        raft_clone,
                        scheduler_clone,
                        std::time::Duration::from_secs(30),
                    )
                    .await;
                }));
                info!("Dead-node detection loop started (30s timeout)");
                (None, dead_node_handle)
            } else {
                // Worker: spawn heartbeat loop (5-second interval)
                let raft_clone = Arc::clone(raft_ref);
                let data_dir_clone = config.data_dir.clone();
                let raft_node_id = node_config.raft_node_id;
                let hb_handle = Some(tokio::spawn(async move {
                    heartbeat_loop(
                        raft_clone,
                        raft_node_id,
                        data_dir_clone,
                        std::time::Duration::from_secs(5),
                    )
                    .await;
                }));
                info!("Heartbeat loop started (5s interval)");
                (hb_handle, None)
            }
        }
        None => (None, None),
    };

    // -----------------------------------------------------------------------
    info!("Daemon infrastructure initialisation complete");

    Ok(DaemonState {
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
        stream_registry,
        cert_manager,
        log_rotator_handle,
        health_checker_handle: Some(health_checker_handle),
        node_config,
        raft,
        raft_server_handle,
        heartbeat_handle,
        dead_node_detection_handle,
        scheduler,
        internal_token,
        replicator,
    })
}

// ---------------------------------------------------------------------------
// Heartbeat & Dead-Node Detection
// ---------------------------------------------------------------------------

/// Worker heartbeat loop -- periodically reports resource usage to the leader.
///
/// Runs on non-leader nodes. Every `interval`, reads current CPU, memory, and
/// disk usage via [`crate::resources::detect_current_usage`] and POSTs the
/// data to the leader's `/api/v1/cluster/heartbeat` endpoint.
///
/// On failure, logs a warning and retries on the next cycle.
async fn heartbeat_loop(
    raft: Arc<zlayer_scheduler::RaftCoordinator>,
    node_id: u64,
    data_dir: std::path::PathBuf,
    interval: std::time::Duration,
) {
    let client = reqwest::Client::new();
    loop {
        tokio::time::sleep(interval).await;

        // Resolve the current leader's API address from Raft cluster state.
        let Some(leader_url) = resolve_leader_api_url(&raft).await else {
            warn!("Heartbeat: no leader address available, will retry next cycle");
            continue;
        };

        let usage = crate::resources::detect_current_usage(&data_dir);

        let body = serde_json::json!({
            "node_id": node_id,
            "cpu_used": usage.cpu_used,
            "memory_used": usage.memory_used,
            "disk_used": usage.disk_used,
        });

        match client
            .post(format!("{leader_url}/api/v1/cluster/heartbeat"))
            .json(&body)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!("Heartbeat sent successfully");
            }
            Ok(resp) => {
                warn!(
                    status = %resp.status(),
                    "Heartbeat response was not successful"
                );
            }
            Err(e) => {
                warn!("Heartbeat failed: {e}");
            }
        }
    }
}

/// Leader-side dead-node detection loop.
///
/// Runs on the leader node. Every 10 seconds, scans the Raft cluster state for
/// nodes whose `last_heartbeat` timestamp is older than `timeout`. Any such
/// node (excluding self and already-dead nodes) is marked `"dead"` via a Raft
/// proposal.
///
/// When a scheduler is provided, the loop also triggers rescheduling of the
/// dead node's containers to remaining live nodes.
#[allow(clippy::cast_possible_truncation)]
async fn dead_node_detection_loop(
    raft: Arc<zlayer_scheduler::RaftCoordinator>,
    scheduler: Option<Arc<Scheduler>>,
    timeout: std::time::Duration,
) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;

        // Only the leader should perform dead-node detection.
        if !raft.is_leader() {
            continue;
        }

        let state = raft.read_state().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64; // milliseconds since epoch fits in u64

        let self_id = raft.node_id();

        for (id, node_info) in &state.nodes {
            // Skip self (leader) and already-dead nodes.
            if *id == self_id || node_info.status == "dead" {
                continue;
            }

            if now.saturating_sub(node_info.last_heartbeat) > timeout.as_millis() as u64 {
                warn!(
                    node_id = id,
                    last_heartbeat_ms = node_info.last_heartbeat,
                    now_ms = now,
                    "Node missed heartbeat deadline, marking dead"
                );
                let _ = raft
                    .propose(Request::UpdateNodeStatus {
                        node_id: *id,
                        status: "dead".to_string(),
                    })
                    .await;

                // Trigger rescheduling of the dead node's containers
                if let Some(ref sched) = scheduler {
                    if let Err(e) = sched.handle_node_death(*id).await {
                        tracing::error!(
                            node_id = id,
                            error = %e,
                            "Failed to reschedule containers from dead node"
                        );
                    }
                }
            }
        }
    }
}

/// Generate a random internal token for scheduler-to-agent communication.
fn generate_internal_token() -> String {
    use std::fmt::Write;
    let mut buf = String::with_capacity(64);
    for _ in 0..32 {
        let byte: u8 = rand::random();
        let _ = write!(buf, "{byte:02x}");
    }
    buf
}

/// Resolve the current Raft leader's HTTP API URL from cluster state.
///
/// Returns `Some("http://{advertise_addr}:{api_port}")` if the leader is known
/// and present in the node registry, or `None` if the leader is unknown.
async fn resolve_leader_api_url(raft: &zlayer_scheduler::RaftCoordinator) -> Option<String> {
    let leader_id = raft.leader_id()?;
    let state = raft.read_state().await;
    let leader_info = state.nodes.get(&leader_id)?;
    Some(format!(
        "http://{}:{}",
        leader_info.advertise_addr, leader_info.api_port
    ))
}

// ---------------------------------------------------------------------------
// Log Rotation
// ---------------------------------------------------------------------------

/// Maximum log file size before rotation (100 MB).
const LOG_MAX_SIZE_BYTES: u64 = 100 * 1024 * 1024;

/// Maximum age for log files of stopped deployments (7 days).
const LOG_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

/// Background task that runs hourly to rotate and clean up container logs.
///
/// For the youki runtime, container logs are stored under the bundle directory:
///   `/var/lib/zlayer/bundles/{container_id}/logs/{stdout,stderr}.log`
///
/// This task:
///  1. Rotates any log file exceeding [`LOG_MAX_SIZE_BYTES`] by truncating it
///     (keeping the last 10% of lines to avoid losing the most recent output).
///  2. Cleans up log files older than [`LOG_MAX_AGE_SECS`] in the daemon's
///     log directory (`/var/log/zlayer/`).
async fn log_rotation_loop(log_dir: &std::path::Path, data_dir: &std::path::Path) {
    let bundle_dir = data_dir.join("bundles");

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;

        info!("Running log rotation check");

        // Phase 1: Rotate large log files in bundle directories
        if let Err(e) = rotate_large_logs(&bundle_dir).await {
            warn!(error = %e, "Log rotation (bundle) encountered an error");
        }

        // Phase 2: Rotate large log files in the structured log directory
        if let Err(e) = rotate_structured_logs(log_dir).await {
            warn!(error = %e, "Log rotation (structured) encountered an error");
        }

        // Phase 3: Clean up old log files in the daemon log directory
        if let Err(e) = cleanup_old_logs(log_dir).await {
            warn!(error = %e, "Log cleanup encountered an error");
        }
    }
}

/// Rotate log files that exceed the size threshold.
///
/// Walks `{bundle_dir}/*/logs/*.log` and truncates any file larger than
/// [`LOG_MAX_SIZE_BYTES`], keeping approximately the last 10% of content.
async fn rotate_large_logs(bundle_dir: &std::path::Path) -> Result<()> {
    if !bundle_dir.exists() {
        return Ok(());
    }

    let mut entries = tokio::fs::read_dir(bundle_dir).await?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let logs_dir = entry.path().join("logs");
        if !logs_dir.is_dir() {
            continue;
        }

        let Ok(mut log_entries) = tokio::fs::read_dir(&logs_dir).await else {
            continue;
        };

        while let Ok(Some(log_entry)) = log_entries.next_entry().await {
            let path = log_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("log") {
                continue;
            }

            let Ok(metadata) = tokio::fs::metadata(&path).await else {
                continue;
            };

            if metadata.len() > LOG_MAX_SIZE_BYTES {
                info!(
                    path = %path.display(),
                    size_mb = metadata.len() / (1024 * 1024),
                    "Rotating oversized log file"
                );

                // Read the file, keep the last 10% of lines
                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    let lines: Vec<&str> = content.lines().collect();
                    let keep = std::cmp::max(lines.len() / 10, 1000);
                    let start = lines.len().saturating_sub(keep);
                    let truncated = lines[start..].join("\n");

                    if let Err(e) = tokio::fs::write(&path, truncated.as_bytes()).await {
                        warn!(
                            path = %path.display(),
                            error = %e,
                            "Failed to write truncated log"
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

/// Rotate log files in the structured log directory that exceed the size
/// threshold.
///
/// Walks `{log_dir}/**/*.log` and truncates any file larger than
/// [`LOG_MAX_SIZE_BYTES`], keeping approximately the last 10% of content.
/// This handles the structured paths created by the youki runtime:
///   `{log_dir}/{deployment}/{service}/{container_id}.{stdout,stderr}.log`
async fn rotate_structured_logs(log_dir: &std::path::Path) -> Result<()> {
    if !log_dir.exists() {
        return Ok(());
    }

    let mut stack = vec![log_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let Ok(metadata) = tokio::fs::metadata(&path).await else {
                continue;
            };

            if metadata.is_dir() {
                stack.push(path);
                continue;
            }

            if path.extension().and_then(|e| e.to_str()) != Some("log") {
                continue;
            }

            // Skip symlinks (bundle symlinks point to the real files we
            // already handle here)
            if tokio::fs::symlink_metadata(&path)
                .await
                .map(|m| m.is_symlink())
                .unwrap_or(false)
            {
                continue;
            }

            if metadata.len() > LOG_MAX_SIZE_BYTES {
                info!(
                    path = %path.display(),
                    size_mb = metadata.len() / (1024 * 1024),
                    "Rotating oversized structured log file"
                );

                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    let lines: Vec<&str> = content.lines().collect();
                    let keep = std::cmp::max(lines.len() / 10, 1000);
                    let start = lines.len().saturating_sub(keep);
                    let truncated = lines[start..].join("\n");

                    if let Err(e) = tokio::fs::write(&path, truncated.as_bytes()).await {
                        warn!(
                            path = %path.display(),
                            error = %e,
                            "Failed to write truncated structured log"
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

/// Remove log files older than [`LOG_MAX_AGE_SECS`] from the daemon log
/// directory.  This targets `/var/log/zlayer/**/*.log`.
async fn cleanup_old_logs(log_dir: &std::path::Path) -> Result<()> {
    if !log_dir.exists() {
        return Ok(());
    }

    let now = std::time::SystemTime::now();
    let max_age = std::time::Duration::from_secs(LOG_MAX_AGE_SECS);

    cleanup_old_logs_walk(log_dir, now, max_age).await
}

/// Walk a directory tree (iterative, no async recursion crate needed)
/// and remove `.log` files older than `max_age`.
async fn cleanup_old_logs_walk(
    root: &std::path::Path,
    now: std::time::SystemTime,
    max_age: std::time::Duration,
) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let Ok(metadata) = tokio::fs::metadata(&path).await else {
                continue;
            };

            if metadata.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("log") {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(age) = now.duration_since(modified) {
                        if age > max_age {
                            info!(
                                path = %path.display(),
                                age_days = age.as_secs() / 86400,
                                "Removing old log file"
                            );
                            let _ = tokio::fs::remove_file(&path).await;
                        }
                    }
                }
            }
        }

        // Try to remove the directory if it's now empty (not the root)
        if dir != root {
            let _ = tokio::fs::remove_dir(&dir).await;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Deployment Restoration
// ---------------------------------------------------------------------------

/// Restore previously-persisted deployments after a daemon restart.
///
/// Loads all [`StoredDeployment`]s from persistent storage, skips any with a
/// `Stopped` status, and for each remaining deployment:
///
///  1. Registers every service with the [`ServiceManager`] via `upsert_service`.
///  2. Sets up the per-service overlay network (when overlay is available).
///  3. Registers proxy routes and ensures listening ports.
///  4. Scales to the desired replica count from the spec.
///  5. Updates the stored deployment status to `Running` (or `Failed` if any
///     service could not be restored).
///
/// A single deployment failing does **not** abort the rest — errors are logged
/// and the deployment is marked `Failed` in storage.
///
/// # Errors
///
/// Returns an error if listing deployments from storage fails.
pub async fn restore_deployments(state: &DaemonState) -> Result<()> {
    let deployments = state
        .storage
        .list()
        .await
        .context("Failed to list deployments from storage")?;

    if deployments.is_empty() {
        info!("No deployments to restore");
        return Ok(());
    }

    let active: Vec<&StoredDeployment> = deployments
        .iter()
        .filter(|d| !matches!(d.status, DeploymentStatus::Stopped))
        .collect();

    if active.is_empty() {
        info!(
            total = deployments.len(),
            "All stored deployments are stopped, nothing to restore"
        );
        return Ok(());
    }

    info!(
        total = deployments.len(),
        active = active.len(),
        "Restoring deployments from persistent storage"
    );

    let mut restored: u32 = 0;
    let mut failed: u32 = 0;

    for stored in &active {
        match restore_single_deployment(state, stored).await {
            Ok(()) => {
                // Mark as Running in storage
                let mut updated = (*stored).clone();
                updated.update_status(DeploymentStatus::Running);
                if let Err(e) = state.storage.store(&updated).await {
                    warn!(
                        deployment = %stored.name,
                        error = %e,
                        "Restored deployment but failed to update status in storage"
                    );
                }
                restored += 1;
            }
            Err(e) => {
                warn!(
                    deployment = %stored.name,
                    error = %e,
                    "Failed to restore deployment"
                );
                // Mark as Failed in storage so operators can see what went wrong
                let mut updated = (*stored).clone();
                updated.update_status(DeploymentStatus::Failed {
                    message: format!("Restore failed: {e}"),
                });
                if let Err(store_err) = state.storage.store(&updated).await {
                    warn!(
                        deployment = %stored.name,
                        error = %store_err,
                        "Additionally failed to persist Failed status"
                    );
                }
                failed += 1;
            }
        }
    }

    info!(
        restored = restored,
        failed = failed,
        "Deployment restoration complete"
    );

    Ok(())
}

/// Restore a single deployment: register services, set up networking, scale.
///
/// Returns `Ok(())` if all services in the deployment were successfully
/// registered and scaled. Returns `Err` with a summary if any service failed.
#[allow(clippy::too_many_lines)]
async fn restore_single_deployment(state: &DaemonState, stored: &StoredDeployment) -> Result<()> {
    let spec = &stored.spec;
    let deployment_name = &spec.deployment;

    info!(
        deployment = %deployment_name,
        services = spec.services.len(),
        status = %stored.status,
        "Restoring deployment"
    );

    let mut service_errors: Vec<String> = Vec::new();

    for (name, service_spec) in &spec.services {
        // 1. Register the service with ServiceManager
        if let Err(e) = state
            .manager
            .upsert_service(name.clone(), service_spec.clone())
            .await
        {
            let msg = format!("{name}: failed to register service: {e}");
            warn!(deployment = %deployment_name, service = %name, error = %e, "Failed to register service during restore");
            service_errors.push(msg);
            continue;
        }

        // 2. Set up service overlay network (non-fatal if overlay is unavailable)
        if let Some(om) = &state.overlay {
            let om_guard = om.read().await;
            match om_guard.setup_service_overlay(name).await {
                Ok(iface) => {
                    info!(
                        deployment = %deployment_name,
                        service = %name,
                        interface = %iface,
                        "Restored service overlay"
                    );
                }
                Err(e) => {
                    warn!(
                        deployment = %deployment_name,
                        service = %name,
                        error = %e,
                        "Failed to restore service overlay (non-fatal)"
                    );
                }
            }
        }

        // 3. Register proxy routes and ensure listening ports
        //    Resolve the node's overlay IP so internal endpoints bind to
        //    the overlay interface instead of 0.0.0.0.
        let overlay_ip: Option<std::net::IpAddr> = if let Some(om) = &state.overlay {
            om.read().await.node_ip().map(std::net::IpAddr::V4)
        } else {
            None
        };
        state.proxy.add_service(name, service_spec).await;
        if let Err(e) = state
            .proxy
            .ensure_ports_for_service(service_spec, overlay_ip)
            .await
        {
            warn!(
                deployment = %deployment_name,
                service = %name,
                error = %e,
                "Failed to setup proxy ports during restore (non-fatal)"
            );
        }

        // 4. Scale to the desired replica count
        let target_replicas = match &service_spec.scale {
            zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
            zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
            zlayer_spec::ScaleSpec::Manual => 0,
        };

        if target_replicas > 0 {
            if let Err(e) = state.manager.scale_service(name, target_replicas).await {
                let msg = format!("{name}: failed to scale to {target_replicas} replicas: {e}");
                warn!(
                    deployment = %deployment_name,
                    service = %name,
                    target = target_replicas,
                    error = %e,
                    "Failed to scale service during restore"
                );
                service_errors.push(msg);
            } else {
                info!(
                    deployment = %deployment_name,
                    service = %name,
                    replicas = target_replicas,
                    "Service restored and scaled"
                );
            }
        } else {
            info!(
                deployment = %deployment_name,
                service = %name,
                "Service restored (manual scaling, 0 replicas)"
            );
        }
    }

    if service_errors.is_empty() {
        info!(deployment = %deployment_name, "Deployment fully restored");
        Ok(())
    } else {
        anyhow::bail!(
            "Deployment '{}' partially restored with {} error(s): {}",
            deployment_name,
            service_errors.len(),
            service_errors.join("; ")
        )
    }
}

// ---------------------------------------------------------------------------
// Deployment Stabilization (re-exported from zlayer_agent::stabilization)
// ---------------------------------------------------------------------------

// The canonical implementation now lives in `zlayer_agent::stabilization`.
// We re-export the types here for backward compatibility with any code that
// referenced `daemon::StabilizationResult` or `daemon::ServiceHealthSummary`.
pub use zlayer_agent::stabilization::{
    wait_for_stabilization, ServiceHealthSummary, StabilizationOutcome, StabilizationResult,
};

// ---------------------------------------------------------------------------
// Admin Credential Bootstrap
// ---------------------------------------------------------------------------

/// Generate a random admin password (32 hex characters = 128 bits of entropy).
fn generate_admin_password() -> String {
    use std::fmt::Write;
    let mut buf = String::with_capacity(32);
    for _ in 0..16 {
        let byte: u8 = rand::random();
        let _ = write!(buf, "{byte:02x}");
    }
    buf
}

// ---------------------------------------------------------------------------
// Shutdown
// ---------------------------------------------------------------------------
//
// NOTE: Shutdown is handled inline in serve.rs after DaemonState is destructured
// for router setup. If you add a new subsystem to DaemonState, add its shutdown
// to the teardown sequence in commands/serve.rs (search for "Post-shutdown cleanup").
