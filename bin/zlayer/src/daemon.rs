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

use serde::{Deserialize, Serialize};
use zlayer_agent::{
    ContainerSupervisor, CronScheduler, JobExecutor, OverlayManager, ProxyManager,
    ProxyManagerConfig, Runtime, RuntimeConfig, ServiceManager,
};
use zlayer_api::handlers::tunnels::TunnelTokenMap;
use zlayer_api::{
    DeploymentStatus, DeploymentStorage, SqlxStorage, StoredDeployment, TunnelApiState,
};
#[cfg(feature = "nat")]
use zlayer_overlay::NatConfig;
use zlayer_overlay::{DnsHandle, DnsServer, OverlayTransport};
use zlayer_proxy::{CertManager, ServiceRegistry, StreamRegistry};
use zlayer_scheduler::{
    RaftConfig, RaftCoordinator, RaftService, Request, Scheduler, SchedulerConfig,
};
use zlayer_secrets::{
    load_or_generate_node_keypair, CredentialStore, KeyManager, PersistentSecretsStore,
    RaftSecretsHandle, RaftSecretsStore, RecipientPrivateKey, SecretsStore,
};
use zlayer_tunnel::{
    AccessManager, ControlHandler, ListenerManager, NodeTunnelManager, TokenValidator, TunnelError,
    TunnelRegistry, TunnelServerConfig,
};

// ---------------------------------------------------------------------------
// Node identity (cross-platform)
// ---------------------------------------------------------------------------
//
// These types live here (rather than in `commands::node`) because the daemon
// initialization path needs them on every platform, including Windows, while
// `commands::node` is Unix-only. The Unix CLI handlers re-export these via
// `crate::daemon::<type>` when needed.

/// Node configuration stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NodeConfig {
    /// Unique node identifier
    pub(crate) node_id: String,
    /// Raft node ID (numeric)
    pub(crate) raft_node_id: u64,
    /// Public IP address for this node
    pub(crate) advertise_addr: String,
    /// API server port
    pub(crate) api_port: u16,
    /// Raft consensus port
    pub(crate) raft_port: u16,
    /// Overlay network port (`WireGuard` protocol)
    pub(crate) overlay_port: u16,
    /// Overlay network CIDR
    pub(crate) overlay_cidr: String,
    /// Overlay private key (x25519)
    pub(crate) wireguard_private_key: String,
    /// Overlay public key (x25519)
    pub(crate) wireguard_public_key: String,
    /// Whether this node is the cluster leader/bootstrap node
    pub(crate) is_leader: bool,
    /// Timestamp when node was created
    pub(crate) created_at: String,
}

/// Generate a unique node ID.
pub(crate) fn generate_node_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Get current timestamp as ISO 8601 string.
pub(crate) fn current_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Save node configuration to disk.
pub(crate) async fn save_node_config(
    data_dir: &std::path::Path,
    config: &NodeConfig,
) -> Result<()> {
    let config_path = data_dir.join("node_config.json");
    let content =
        serde_json::to_string_pretty(config).context("Failed to serialize node config")?;
    tokio::fs::write(&config_path, content)
        .await
        .with_context(|| format!("Failed to write node config to {}", config_path.display()))?;
    info!(path = %config_path.display(), "Saved node configuration");
    Ok(())
}

/// Load node configuration from disk.
pub(crate) async fn load_node_config(data_dir: &std::path::Path) -> Result<NodeConfig> {
    let config_path = data_dir.join("node_config.json");
    let content = tokio::fs::read_to_string(&config_path)
        .await
        .with_context(|| format!("Failed to read node config from {}", config_path.display()))?;
    let config: NodeConfig =
        serde_json::from_str(&content).context("Failed to parse node config")?;
    Ok(config)
}

/// Detect the local machine's outbound IP address.
///
/// Uses the UDP-connect trick: connect a UDP socket to an external address
/// (no traffic is actually sent) and read back the local address the OS chose.
/// Falls back to `0.0.0.0` if detection fails.
pub(crate) fn detect_local_ip() -> String {
    use std::net::UdpSocket;
    match UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => {
            // Connect to a well-known public address (Google DNS).
            // No actual traffic is sent over UDP until data is written.
            if socket.connect("8.8.8.8:80").is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    return addr.ip().to_string();
                }
            }
            "0.0.0.0".to_string()
        }
        Err(_) => "0.0.0.0".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the daemon's built-in tunnel server.
#[derive(Debug, Clone)]
pub struct TunnelDaemonConfig {
    /// Disable the tunnel server entirely. When `true`, the WebSocket accept
    /// loop is not spawned and the on-demand access manager is unavailable.
    pub disabled: bool,
    /// Address to bind the tunnel WebSocket control server to. Format: host:port.
    pub bind: std::net::SocketAddr,
    /// Optional path to a TLS certificate (PEM). When `Some`, callers are
    /// expected to pair it with [`TunnelDaemonConfig::tls_key`]. TLS termination
    /// itself is handled by an external reverse proxy or future enhancement
    /// of the tunnel WebSocket loop.
    pub tls_cert: Option<PathBuf>,
    /// Optional path to a TLS private key (PEM).
    pub tls_key: Option<PathBuf>,
    /// Underlying [`TunnelServerConfig`] (port range, heartbeat, limits, ...).
    pub server: TunnelServerConfig,
}

impl Default for TunnelDaemonConfig {
    fn default() -> Self {
        Self {
            disabled: false,
            // Bind on the same port the tunnel CLI defaults to (3679 = 3669+10).
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 3679),
            tls_cert: None,
            tls_key: None,
            server: TunnelServerConfig::default(),
        }
    }
}

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
    /// Default: `~/.zlayer` on macOS, `/var/lib/zlayer` (root) or `~/.zlayer` (user) on Linux.
    pub data_dir: PathBuf,

    /// Log directory.  Default: `{data_dir}/logs` on macOS, `/var/log/zlayer` on Linux.
    pub log_dir: PathBuf,

    /// Runtime directory (sockets, PID files).  Default: `{data_dir}/run` on macOS, `/var/run/zlayer` on Linux.
    pub run_dir: PathBuf,

    /// S3 configuration for `SQLite` replication (None = disabled).
    pub s3_storage: Option<zlayer_storage::LayerStorageConfig>,

    /// Auth context injected into every container so it can call back to the
    /// host API.  Built from the JWT secret and bind address in `serve()`.
    pub auth_context: Option<zlayer_agent::ContainerAuthContext>,

    /// Shared network policy list for proxy access control enforcement.
    /// When populated, the proxy checks incoming requests against these policies.
    pub network_policies: Option<Arc<RwLock<Vec<zlayer_spec::NetworkPolicySpec>>>>,

    /// NAT traversal configuration for the overlay network. Resolved by
    /// `serve()` from CLI flags + `ZLAYER_NAT_*` env vars + the
    /// `NatConfig::default()` baseline. Threaded into the [`OverlayManager`]
    /// at init time via [`zlayer_agent::OverlayManager::with_nat_config`].
    #[cfg(feature = "nat")]
    pub nat: NatConfig,

    /// Configuration for the daemon-side tunnel server. When
    /// [`TunnelDaemonConfig::disabled`] is `true`, the tunnel server is not
    /// started and the on-demand access endpoints return 503.
    pub tunnel: TunnelDaemonConfig,

    /// Logical name for this node, used as the source identifier for
    /// node-to-node tunnels. Mirrors `node_config.node_id` but is exposed
    /// separately so tests can override it.
    pub node_name: Option<String>,
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
            auth_context: None,
            network_policies: None,
            #[cfg(feature = "nat")]
            nat: NatConfig::default(),
            tunnel: TunnelDaemonConfig::default(),
            node_name: None,
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

    /// User-secrets store, chosen at startup based on whether this node is a
    /// member of a clustered deployment.
    ///
    /// - **Standalone** (no `wrapped_dek.bin` on disk): wraps the local
    ///   [`PersistentSecretsStore`] in a trait object so the same handler
    ///   wiring works on a single-node deployment.
    /// - **Clustered** (`wrapped_dek.bin` present after `zlayer node join`):
    ///   a [`RaftSecretsStore`] that reads/writes through the cluster Raft
    ///   state machine, decrypting with the node's on-disk X25519 key.
    ///
    /// The trait-object type lets the API handler state stay agnostic. See
    /// [`select_secrets_store`] for the selection logic.
    pub secrets: Arc<dyn SecretsStore + Send + Sync>,

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

    /// Background task driving periodic NAT traversal maintenance
    /// (STUN refresh, relay-to-direct upgrades). `None` when NAT is
    /// disabled or no overlay manager is active.
    pub nat_maintenance_handle: Option<tokio::task::JoinHandle<()>>,

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

    /// Job executor for running one-off and triggered jobs.
    pub job_executor: Arc<JobExecutor>,

    /// Cron scheduler for managing scheduled/recurring jobs.
    pub cron_scheduler: Arc<CronScheduler>,

    /// Tunnel server runtime handles. `None` when the tunnel server is
    /// disabled via configuration (e.g. `--no-tunnel-server`).
    pub tunnel: Option<TunnelHandles>,

    /// Shared API state for tunnel endpoints. The `tunnel_tokens` map within
    /// is populated by the API handler when a token is created and read by
    /// the tunnel server's [`TokenValidator`].
    pub tunnel_api_state: TunnelApiState,
}

/// Bag of tunnel-server runtime handles owned by [`DaemonState`].
pub struct TunnelHandles {
    /// Active tunnel registry (tokens, services, port pool).
    pub registry: Arc<TunnelRegistry>,
    /// Listener manager (per-service TCP/UDP data channels).
    pub listener_manager: Arc<ListenerManager>,
    /// Control channel handler (consumes WebSocket connections).
    pub control_handler: Arc<ControlHandler>,
    /// On-demand access session manager (used by `tunnel access`).
    pub access_manager: Arc<AccessManager>,
    /// Node-to-node tunnel manager (cluster outbound tunnels).
    pub node_manager: Arc<NodeTunnelManager>,
    /// Background task running the WebSocket accept loop. Aborted at
    /// shutdown.
    pub accept_handle: tokio::task::JoinHandle<()>,
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
    let overlay_port: u16 = zlayer_core::DEFAULT_WG_PORT;
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
    // Phase 0: Self-heal pre-0.11.20 on-disk layouts before anything else
    // opens the data dir. Pre-0.11.20 installs left a SQLite file at
    // `{data_dir}/secrets`, which collided with the directory the persistent
    // secrets store now expects. Running this first lets every daemon boot
    // migrate forward without operator intervention.
    // -----------------------------------------------------------------------
    let report = crate::migrations::migrate_data_dir(&config.data_dir)
        .context("Failed to migrate on-disk data directory layout")?;
    for step in &report.steps {
        info!("Migration: {step}");
    }

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
                    // Override Auto with an explicitly configured Youki runtime so the
                    // subdirectories (cache_dir, state_dir, etc.) land under the
                    // daemon's configured data_dir rather than the platform default,
                    // and the log directory + deployment name are propagated.
                    let mut youki_cfg = YoukiConfig::from_data_dir(&config.data_dir);
                    youki_cfg.log_base_dir = Some(config.log_dir.clone());
                    youki_cfg.deployment_name = Some(config.deployment_name.clone());
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

    let runtime = zlayer_agent::create_runtime(runtime_config, config.auth_context.clone())
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
            Ok(om) => {
                // Data-dir-aware UAPI socket dir: a daemon launched with
                // `--data-dir /tmp/foo` writes its boringtun sockets under
                // `/tmp/foo/run/wireguard` so it does not collide with a
                // system-wide zlayer install owning `/var/run/wireguard`.
                let uapi_sock_dir = zlayer_paths::ZLayerDirs::new(&config.data_dir).wireguard();
                let om = om.with_uapi_sock_dir(uapi_sock_dir);
                #[cfg(feature = "nat")]
                let mut om = om.with_nat_config(config.nat.clone());
                #[cfg(not(feature = "nat"))]
                let mut om = om;
                if let Err(e) = om.setup_global_overlay().await {
                    warn!("Global overlay failed (cross-node networking disabled): {e}");
                } else {
                    info!("Global overlay network created");
                }
                Some(Arc::new(RwLock::new(om)))
            }
            Err(e) => {
                warn!("Overlay manager init failed: {e}");
                None
            }
        }
    };

    // -----------------------------------------------------------------------
    // Phase 3b: NAT traversal bootstrap + periodic maintenance task
    //
    // After the overlay manager is up, kick off NAT candidate gathering
    // (host + STUN reflexive + relay) and spawn a tokio interval task
    // that periodically re-probes STUN to detect rebinding events and
    // attempts to upgrade relayed connections to direct/hole-punched.
    // No-op when NAT is disabled, no overlay is configured, or candidate
    // gathering fails (logged at warn level by start_nat_traversal).
    // -----------------------------------------------------------------------
    #[cfg(feature = "nat")]
    let nat_maintenance_handle: Option<tokio::task::JoinHandle<()>> = if let Some(om_arc) =
        overlay.clone()
    {
        let nat_enabled = {
            let guard = om_arc.read().await;
            guard.nat_enabled()
        };
        if nat_enabled {
            let started = {
                let guard = om_arc.read().await;
                guard.start_nat_traversal().await
            };
            match started {
                Ok(true) => {
                    let interval_secs = config.nat.stun_refresh_interval_secs.max(1);
                    let task_om = Arc::clone(&om_arc);
                    let handle = tokio::spawn(async move {
                        let mut tick =
                            tokio::time::interval(std::time::Duration::from_secs(interval_secs));
                        // Skip the immediate first tick — gather_candidates
                        // already populated state moments ago.
                        tick.tick().await;
                        loop {
                            tick.tick().await;
                            let res = {
                                let guard = task_om.read().await;
                                guard.nat_maintenance_tick().await
                            };
                            if let Err(e) = res {
                                tracing::warn!(error = %e, "NAT maintenance tick failed");
                            }
                        }
                    });
                    info!(
                        interval_secs = interval_secs,
                        "NAT traversal maintenance task started"
                    );
                    Some(handle)
                }
                Ok(false) => {
                    info!("NAT traversal not started (no candidates gathered)");
                    None
                }
                Err(e) => {
                    warn!(error = %e, "NAT traversal start failed");
                    None
                }
            }
        } else {
            info!("NAT traversal disabled in config; skipping maintenance task");
            None
        }
    } else {
        None
    };
    #[cfg(not(feature = "nat"))]
    let nat_maintenance_handle: Option<tokio::task::JoinHandle<()>> = None;

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
    if let Some(ref policies) = config.network_policies {
        proxy_builder.set_network_policy_checker(zlayer_proxy::NetworkPolicyChecker::new(
            Arc::clone(policies),
        ));
    }
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
    // Phase 10: Persistent secrets store + node X25519 keypair
    //
    // The persistent store always exists (it backs `credential_store`,
    // `RegistryCredentialStore`, and `GitCredentialStore`, which are
    // local-only). The user-secrets store handed to handler state is
    // chosen later (Phase 15b) once the Raft instance is up — see
    // [`select_secrets_store`]. The node keypair is loaded
    // unconditionally (even in standalone mode) so a daemon can later
    // be promoted into a cluster without re-keying.
    // -----------------------------------------------------------------------
    let key_manager = KeyManager::with_base_dir(&config.data_dir);
    let encryption_key = key_manager
        .get_or_create_key(&config.deployment_name)
        .context("Failed to resolve secrets encryption key")?;

    let secrets_dir = config.data_dir.join("secrets");
    let persistent_secrets = Arc::new(
        PersistentSecretsStore::open(&secrets_dir, encryption_key)
            .await
            .with_context(|| {
                format!("Failed to open secrets store at {}", secrets_dir.display())
            })?,
    );
    info!(path = %secrets_dir.display(), "Persistent secrets store opened");

    // Always load (or generate) the node X25519 keypair. Cheap on hot
    // path, and a standalone daemon that later joins a cluster can
    // present its existing pubkey instead of having to re-key.
    let (node_priv, node_pub) = load_or_generate_node_keypair(&secrets_dir)
        .map_err(|e| anyhow::anyhow!("Failed to load/generate node keypair: {e}"))?;
    let node_priv = Arc::new(node_priv);
    info!(
        node_pubkey_b64 = %base64::Engine::encode(&base64::engine::general_purpose::STANDARD, node_pub.as_bytes()),
        "Node X25519 keypair ready"
    );

    // -----------------------------------------------------------------------
    // Phase 11: Credential store + admin bootstrap
    // -----------------------------------------------------------------------
    let credential_store = Arc::new(CredentialStore::new(Arc::clone(&persistent_secrets)));

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
            let pw_path = config.data_dir.join("admin_password");
            if let Err(e) = std::fs::write(&pw_path, &admin_password) {
                warn!(error = %e, "Failed to persist admin password");
            } else {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(&pw_path, std::fs::Permissions::from_mode(0o644));
                }
                info!("Admin password persisted to {}", pw_path.display());
            }
        }
        Ok(false) => {
            let pw_path = config.data_dir.join("admin_password");
            if pw_path.exists() {
                info!("Admin API credential already exists");
                // Ensure password file is world-readable so non-root tools
                // (e.g. E2E tests, CLI) can auto-detect credentials.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(&pw_path, std::fs::Permissions::from_mode(0o644));
                }
            } else {
                warn!("Admin credential exists but password file is missing — regenerating");
                let new_password = generate_admin_password();

                if let Err(e) = credential_store.delete_api_key("admin").await {
                    warn!(error = %e, "Failed to delete old admin credential");
                }

                match credential_store
                    .create_api_key("admin", &new_password, &["admin"])
                    .await
                {
                    Ok(()) => {
                        if let Err(e) = std::fs::write(&pw_path, &new_password) {
                            warn!(error = %e, "Failed to persist regenerated admin password");
                        } else {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                let _ = std::fs::set_permissions(
                                    &pw_path,
                                    std::fs::Permissions::from_mode(0o644),
                                );
                            }
                            info!(
                                "Admin password regenerated and persisted to {}",
                                pw_path.display()
                            );
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to recreate admin credential");
                    }
                }
            }
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
    let mut node_config = load_or_init_node_config(&config.data_dir).await?;

    // -----------------------------------------------------------------------
    // Phase 14: Internal token (generated early for Raft auth + Scheduler)
    // -----------------------------------------------------------------------

    // Generate the internal token up-front so the Raft RPC layer, the
    // Scheduler (for dispatching scale requests), and the API InternalState
    // (for validating them) all share the same secret.
    let internal_token = generate_internal_token();

    // -----------------------------------------------------------------------
    // Phase 14b: Force-leader recovery check
    // -----------------------------------------------------------------------
    let force_leader_recovery = zlayer_scheduler::raft::load_and_clear_force_leader_state(
        &config.data_dir,
    )
    .unwrap_or_else(|e| {
        warn!("Failed to check force-leader recovery: {e}");
        None
    });
    if force_leader_recovery.is_some() {
        info!("Force-leader recovery detected, wiping Raft storage");
        let raft_log = config.data_dir.join("raft-log");
        let raft_sm = config.data_dir.join("raft-sm");
        let _ = std::fs::remove_dir_all(&raft_log);
        let _ = std::fs::remove_dir_all(&raft_sm);
        node_config.is_leader = true;
        node_config.raft_node_id = 1;
    }

    // -----------------------------------------------------------------------
    // Phase 15: Raft distributed consensus
    // -----------------------------------------------------------------------
    let (raft, raft_server_handle) = {
        let _raft_db_path = config.data_dir.join("raft.db");
        let raft_cfg = RaftConfig {
            node_id: node_config.raft_node_id,
            address: format!("{}:{}", node_config.advertise_addr, node_config.raft_port),
            raft_port: node_config.raft_port,
            data_dir: config.data_dir.join("raft"),
            ..Default::default()
        };

        // Wire the secrets capability into the coordinator so leader-side
        // helpers (`propose_register_node_and_rotate`, `propose_rotate_dek`)
        // can seal the cluster DEK to this node's X25519 pubkey. The pair
        // must be `Some(_)`/`Some(_)` together — a missing node_uuid would
        // disable the secrets helpers entirely.
        let raft_node_priv: RecipientPrivateKey = (*node_priv).clone();
        let raft_node_uuid = node_config.node_id.clone();
        match RaftCoordinator::with_auth_and_secrets(
            raft_cfg,
            Some(internal_token.clone()),
            Some(raft_node_priv),
            Some(raft_node_uuid),
        )
        .await
        {
            Ok(coordinator) => {
                // Bootstrap as single-node cluster if this is the leader (first node)
                if node_config.is_leader {
                    // Check metrics before bootstrapping to avoid openraft's
                    // internal ERROR log ("Can not initialize") on restart.
                    let metrics = coordinator.metrics();
                    if metrics.current_term > 0 {
                        info!("Raft already bootstrapped (resuming from persisted state)");
                    } else {
                        match coordinator.bootstrap().await {
                            Ok(()) => info!("Raft single-node cluster bootstrapped"),
                            Err(e) => {
                                info!("Raft bootstrap: {e} (already bootstrapped — resuming)");
                            }
                        }
                    }

                    // Register the leader node in the Raft state machine so it
                    // appears in cluster state with its overlay networking metadata.
                    //
                    // Read the leader's own overlay IP and slice CIDR from the
                    // live `OverlayManager` when one is available. This replaces
                    // the legacy hardcoded `10.200.0.1` and supports slice-aware
                    // deployments (T8). Falls back to the hardcode when the
                    // overlay is not configured (e.g. host-networking mode).
                    let (leader_overlay_ip, leader_slice_cidr) = if let Some(om) = &overlay {
                        let om_guard = om.read().await;
                        let ip = om_guard
                            .node_ip()
                            .map_or_else(|| "10.200.0.1".to_string(), |ip| ip.to_string());
                        let slice = om_guard
                            .slice_cidr()
                            .map(|s| s.to_string())
                            .unwrap_or_default();
                        (ip, slice)
                    } else {
                        ("10.200.0.1".to_string(), String::new())
                    };
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
                            mode: "full".to_string(),
                            os: zlayer_spec::OsKind::from_rust_os(std::env::consts::OS),
                            arch: zlayer_spec::ArchKind::from_rust_arch(std::env::consts::ARCH),
                            slice_cidr: leader_slice_cidr,
                        })
                        .await
                    {
                        warn!("Failed to register leader in Raft state: {e}");
                    }
                }

                // Replay saved state from force-leader recovery
                if let Some(ref saved_state) = force_leader_recovery {
                    info!(
                        "Replaying {} services from force-leader recovery",
                        saved_state.services.len()
                    );
                    for (name, svc) in &saved_state.services {
                        let _ = coordinator
                            .propose(Request::UpdateServiceState {
                                service_name: name.clone(),
                                state: svc.clone(),
                            })
                            .await;
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
                    om.read()
                        .await
                        .node_ip()
                        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
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
    // Phase 15b: Select user-secrets store (standalone vs clustered)
    //
    // Standalone-mode daemons (single node, no `wrapped_dek.bin` on disk)
    // continue to use the local `PersistentSecretsStore`. A node that has
    // joined a cluster (wrapped_dek.bin written by the join handler in
    // Task #17) switches to `RaftSecretsStore`, which reads from the
    // replicated SM and decrypts with the on-disk node X25519 key.
    //
    // We make the choice *after* Raft is up so the cluster store has a
    // live `RaftSecretsHandle` to bind to. When Raft failed to come up
    // (e.g. on a host without a writable raft data dir), we fall back to
    // the persistent store unconditionally — better than leaving the API
    // 503'ing on every secret read.
    //
    // TODO Phase-1.5: switch inter-node auth to node JWT (currently the
    // daemon attaches `state.internal_token` as `X-ZLayer-Internal-Token`
    // for inter-node calls; the JWT minted by the leader at join time
    // (`{secrets_dir}/node.jwt`) replaces that path in a later task).
    // -----------------------------------------------------------------------
    let secrets: Arc<dyn SecretsStore + Send + Sync> = select_secrets_store(
        &secrets_dir,
        Arc::clone(&persistent_secrets),
        raft.as_ref().map(Arc::clone),
        &node_priv,
        node_config.node_id.clone(),
    );

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
                data_dir: config.data_dir.join("raft"),
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
    // Phase 17: Tunnel server (control channel + listener + access manager).
    //
    // The tunnel server boots a single TcpListener that accepts WebSocket
    // upgrades and dispatches them to `ControlHandler::handle_connection`.
    // The token validator reads from a `TunnelTokenMap` shared with the
    // API handler so `POST /api/v1/tunnels` immediately makes a token
    // valid for the live tunnel server.
    //
    // The `AccessManager` is wired separately and exposed through the
    // tunnel API handler's `create_access_session` endpoint. That endpoint
    // is unavailable (503) when the tunnel server is disabled.
    // -----------------------------------------------------------------------
    let tunnel_api_state;
    let tunnel_handles_opt: Option<TunnelHandles> = if config.tunnel.disabled {
        info!("Tunnel server disabled by configuration; skipping");
        tunnel_api_state = TunnelApiState::new();
        None
    } else {
        let tunnel_registry = Arc::new(TunnelRegistry::new(config.tunnel.server.data_port_range));
        let listener_manager = Arc::new(ListenerManager::new(
            Arc::clone(&tunnel_registry),
            config.tunnel.server.heartbeat_timeout,
        ));
        let access_manager = Arc::new(AccessManager::new());

        // Build the API state first; it owns the canonical token map and
        // access manager, both shared with the tunnel server.
        tunnel_api_state = TunnelApiState::with_access_manager(Arc::clone(&access_manager));
        let token_map: TunnelTokenMap = tunnel_api_state.token_map();

        // Build the token validator. Looks up the SHA-256 hash of the
        // supplied token against the shared token map. The map is keyed by
        // tunnel ID, so we scan values for a hash match. Expired tokens are
        // rejected. The map is small in practice (one entry per active
        // tunnel) and scans are O(n) which is acceptable; a future
        // enhancement would index by hash.
        //
        // TODO: validate against IdentityManager-backed tokens once the
        // identity layer exposes a hashed-token table.
        let validator_tokens = Arc::clone(&token_map);
        let token_validator: TokenValidator = Arc::new(move |raw: &str| {
            let hash = zlayer_tunnel::hash_token(raw);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let map = validator_tokens.read();
            for stored in map.values() {
                if stored.token_hash == hash {
                    if stored.expires_at < now {
                        return Err(TunnelError::auth("tunnel token expired"));
                    }
                    return Ok(());
                }
            }
            Err(TunnelError::auth("invalid tunnel token"))
        });

        let control_handler = Arc::new(ControlHandler::new(
            Arc::clone(&tunnel_registry),
            config.tunnel.server.clone(),
            token_validator,
        ));

        let node_manager = Arc::new(NodeTunnelManager::new(
            config
                .node_name
                .clone()
                .unwrap_or_else(|| node_config.node_id.clone()),
            config.tunnel.server.clone(),
        ));

        // Spawn the WebSocket accept loop. ControlHandler::handle_connection
        // upgrades the TcpStream to a WebSocket, performs the AUTH handshake
        // and runs the message loop until disconnect. Each accepted
        // connection runs in its own task so a slow client doesn't block
        // others.
        let accept_addr = config.tunnel.bind;
        let accept_handler = Arc::clone(&control_handler);
        let accept_handle = tokio::spawn(async move {
            let listener = match tokio::net::TcpListener::bind(accept_addr).await {
                Ok(l) => l,
                Err(e) => {
                    warn!(
                        bind = %accept_addr,
                        error = %e,
                        "Tunnel server bind failed; tunnel functionality unavailable"
                    );
                    return;
                }
            };
            info!(bind = %accept_addr, "Tunnel WebSocket server listening");
            loop {
                match listener.accept().await {
                    Ok((stream, peer)) => {
                        let handler = Arc::clone(&accept_handler);
                        tokio::spawn(async move {
                            if let Err(e) = handler.handle_connection(stream, peer).await {
                                tracing::warn!(
                                    peer = %peer,
                                    error = %e,
                                    "Tunnel control connection ended with error"
                                );
                            }
                        });
                    }
                    Err(e) => {
                        warn!(error = %e, "Tunnel accept failed; retrying");
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    }
                }
            }
        });

        if let (Some(cert), Some(key)) = (
            config.tunnel.tls_cert.as_ref(),
            config.tunnel.tls_key.as_ref(),
        ) {
            // TLS termination on the WebSocket loop is not yet plumbed into
            // ControlHandler -- the typical deployment terminates TLS at the
            // L7 proxy / API frontend in front of the daemon. We log the
            // configured paths so operators see they were honoured at the
            // CLI level even if not yet used by the WebSocket loop.
            // TODO: thread tokio-rustls into the accept loop above.
            info!(
                cert = %cert.display(),
                key = %key.display(),
                "Tunnel TLS material configured (terminate at the API edge proxy)"
            );
        }

        Some(TunnelHandles {
            registry: tunnel_registry,
            listener_manager,
            control_handler,
            access_manager,
            node_manager,
            accept_handle,
        })
    };

    // -----------------------------------------------------------------------
    info!("Daemon infrastructure initialisation complete");

    let job_executor = Arc::new(JobExecutor::new(Arc::clone(&runtime)));
    let cron_scheduler = Arc::new(CronScheduler::new(Arc::clone(&job_executor)));

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
        nat_maintenance_handle,
        node_config,
        raft,
        raft_server_handle,
        heartbeat_handle,
        dead_node_detection_handle,
        scheduler,
        internal_token,
        replicator,
        job_executor,
        cron_scheduler,
        tunnel: tunnel_handles_opt,
        tunnel_api_state,
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

                // Rebalance voters to maintain odd count (promotes eligible learners
                // if the dead node was a voter).
                if let Err(e) = raft.rebalance_voters().await {
                    warn!(
                        node_id = id,
                        error = %e,
                        "Failed to rebalance voters after node death"
                    );
                }

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

/// Path to the bootstrap wrapped DEK file written by the join handler
/// (Task #17). Its presence is the signal that this daemon has joined a
/// cluster and should serve user secrets through `RaftSecretsStore`
/// rather than the local `PersistentSecretsStore`.
fn wrapped_dek_path(secrets_dir: &std::path::Path) -> std::path::PathBuf {
    secrets_dir.join("wrapped_dek.bin")
}

/// True iff this daemon should run with `RaftSecretsStore` for user
/// secrets.
///
/// The cluster-join flow drops `wrapped_dek.bin` into the secrets dir
/// alongside `node.jwt` and `dek_generation` (Task #17). Standalone
/// daemons never go through that flow, so the file is absent and we
/// stay on `PersistentSecretsStore`.
///
/// We do not key off `NodeConfig.is_leader` because a single-node
/// auto-init also sets `is_leader = true`; the file marker is the
/// least-ambiguous signal.
pub(crate) fn is_clustered_mode(secrets_dir: &std::path::Path) -> bool {
    wrapped_dek_path(secrets_dir).exists()
}

/// Pick the user-secrets store appropriate for this daemon.
///
/// - Standalone (or Raft init failed, or no `wrapped_dek.bin`): the
///   `PersistentSecretsStore` is widened to `Arc<dyn SecretsStore>` and
///   handed back unchanged. Existing standalone daemons continue to
///   behave bit-for-bit as before.
/// - Clustered (cluster-join artifacts on disk *and* the local Raft
///   coordinator is up): a fresh [`RaftSecretsStore`] is constructed
///   bound to the live coordinator. Reads pull from the replicated
///   [`zlayer_secrets::SecretsState`]; writes propose through the
///   leader. Decryption uses the local node's X25519 private key.
///
/// `node_priv` is *always* loaded by the caller (cheap, lets a
/// standalone node later upgrade into a cluster without re-keying), so
/// it is unconditionally present. `node_uuid` is the cluster-wide UUID
/// from `NodeConfig.node_id`.
pub(crate) fn select_secrets_store(
    secrets_dir: &std::path::Path,
    persistent: Arc<PersistentSecretsStore>,
    raft: Option<Arc<RaftCoordinator>>,
    node_priv: &Arc<RecipientPrivateKey>,
    node_uuid: String,
) -> Arc<dyn SecretsStore + Send + Sync> {
    // Up-cast `Arc<RaftCoordinator>` to `Arc<dyn RaftSecretsHandle>` and
    // hand off to the testable inner helper. The cast is free at runtime
    // (same fat pointer) and keeps the production call site short.
    let handle: Option<Arc<dyn RaftSecretsHandle>> = raft.map(|c| c as Arc<dyn RaftSecretsHandle>);
    select_secrets_store_inner(secrets_dir, persistent, handle, node_priv, node_uuid)
}

/// Inner store-selection logic that takes the raft handle as a trait
/// object so unit tests can inject an in-memory mock without standing up
/// a real `RaftCoordinator` (which would require a real network and a
/// writable redb store on disk).
///
/// Production callers should invoke [`select_secrets_store`] which does
/// the trivial up-cast for them.
pub(crate) fn select_secrets_store_inner(
    secrets_dir: &std::path::Path,
    persistent: Arc<PersistentSecretsStore>,
    raft: Option<Arc<dyn RaftSecretsHandle>>,
    node_priv: &Arc<RecipientPrivateKey>,
    node_uuid: String,
) -> Arc<dyn SecretsStore + Send + Sync> {
    if is_clustered_mode(secrets_dir) {
        if let Some(raft_handle) = raft {
            info!(
                node_uuid = %node_uuid,
                "Cluster mode: using RaftSecretsStore for user secrets"
            );
            // RaftSecretsStore takes RecipientPrivateKey by value; clone
            // out of the Arc so the original wrap can keep being shared
            // (e.g. for future helpers that re-seal DEKs via the same
            // key). `node_priv: &Arc<RecipientPrivateKey>` needs a double
            // deref to reach the inner value.
            return Arc::new(RaftSecretsStore::new(
                node_priv.as_ref().clone(),
                node_uuid,
                raft_handle,
            ));
        }
        // wrapped_dek.bin exists but Raft didn't come up. We can't serve
        // cluster secrets without a coordinator, so fall back to the
        // persistent store and warn loudly. The cluster's user-secret
        // reads will be unavailable until raft initialises (a restart
        // typically resolves it).
        warn!(
            "wrapped_dek.bin present but Raft is unavailable; \
             falling back to PersistentSecretsStore. Cluster-replicated \
             secrets will not be served until Raft is restored."
        );
    }

    info!("Standalone mode: using PersistentSecretsStore for user secrets");
    persistent
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
///   `{data_dir}/bundles/{container_id}/logs/{stdout,stderr}.log`
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
/// Walks `{bundle_dir}/*/logs/*.{log,jsonl}` and truncates any file larger than
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
            if !matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("log" | "jsonl")
            ) {
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
/// Walks `{log_dir}/**/*.{log,jsonl}` and truncates any file larger than
/// [`LOG_MAX_SIZE_BYTES`], keeping approximately the last 10% of content.
/// This handles the structured paths created by the youki runtime:
///   `{log_dir}/{deployment}/{service}/{container_id}.{stdout,stderr}.{log,jsonl}`
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

            if !matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("log" | "jsonl")
            ) {
                continue;
            }

            // Skip symlinks (bundle symlinks point to the real files we
            // already handle here)
            if tokio::fs::symlink_metadata(&path)
                .await
                .is_ok_and(|m| m.is_symlink())
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
/// directory.  This targets `/var/log/zlayer/**/*.{log,jsonl}` as well as
/// the `executions/` subdirectory.
async fn cleanup_old_logs(log_dir: &std::path::Path) -> Result<()> {
    if !log_dir.exists() {
        return Ok(());
    }

    let now = std::time::SystemTime::now();
    let max_age = std::time::Duration::from_secs(LOG_MAX_AGE_SECS);

    cleanup_old_logs_walk(log_dir, now, max_age).await?;

    // Also clean up the executions/ subdirectory (execution log artifacts)
    let executions_dir = log_dir.join("executions");
    if executions_dir.exists() {
        cleanup_old_logs_walk(&executions_dir, now, max_age).await?;
    }

    Ok(())
}

/// Walk a directory tree (iterative, no async recursion crate needed)
/// and remove `.log` / `.jsonl` files older than `max_age`.
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
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("log" | "jsonl")
            ) {
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
        match Box::pin(restore_single_deployment(state, stored)).await {
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
        if let Err(e) = Box::pin(
            state
                .manager
                .upsert_service(name.clone(), service_spec.clone()),
        )
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
            om.read().await.node_ip()
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

#[cfg(test)]
mod tests {
    //! Tests for daemon-level startup helpers added in Task #18:
    //! node-keypair load on first startup and the standalone-vs-clustered
    //! secrets-store selection.

    use super::*;
    use async_trait::async_trait;
    use tempfile::TempDir;
    use zlayer_secrets::{
        node_secrets_key_path, EncryptionKey, RaftSecretsHandle, SecretsError, SecretsState,
    };
    use zlayer_types::storage::ReplicatedSecret;

    /// Open a `PersistentSecretsStore` at `dir` with a freshly generated
    /// 32-byte symmetric key. Returns the `Arc` ready to feed into
    /// [`select_secrets_store`].
    async fn make_persistent(dir: &std::path::Path) -> Arc<PersistentSecretsStore> {
        let key = EncryptionKey::generate();
        Arc::new(
            PersistentSecretsStore::open(dir, key)
                .await
                .expect("open persistent store"),
        )
    }

    /// In-memory `RaftSecretsHandle` that just panics if anything calls
    /// it — the store-selection tests only need the *option* of a handle
    /// being present, never to actually use it.
    struct DummyRaftHandle;

    #[async_trait]
    impl RaftSecretsHandle for DummyRaftHandle {
        async fn secrets_state(&self) -> SecretsState {
            unreachable!("store-selection test should not invoke the handle")
        }
        async fn propose_put_secret(&self, _: ReplicatedSecret) -> Result<(), SecretsError> {
            unreachable!("store-selection test should not invoke the handle")
        }
        async fn propose_delete_secret(&self, _: &str) -> Result<(), SecretsError> {
            unreachable!("store-selection test should not invoke the handle")
        }
    }

    #[test]
    fn daemon_loads_node_keypair_on_first_startup() {
        let dir = TempDir::new().expect("tempdir");
        let path = node_secrets_key_path(dir.path());
        assert!(!path.exists(), "key file must not exist before first call");

        let (priv1, pub1) = load_or_generate_node_keypair(dir.path()).expect("first generate");

        // File now exists with exactly 32 bytes.
        assert!(path.exists(), "key file must exist after generation");
        let buf = std::fs::read(&path).expect("read key file");
        assert_eq!(buf.len(), 32, "raw private key must be 32 bytes");

        // Unix mode 0600 — never world-readable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("stat key file")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "key file must be 0600 (got {mode:o})");
        }

        // A second call returns the *same* keypair (no re-keying).
        let (priv2, pub2) = load_or_generate_node_keypair(dir.path()).expect("second load");
        assert_eq!(
            priv1.to_base64(),
            priv2.to_base64(),
            "private key must be stable across reloads"
        );
        assert_eq!(
            pub1.as_bytes(),
            pub2.as_bytes(),
            "public key must be stable across reloads"
        );
    }

    #[tokio::test]
    async fn select_secrets_store_picks_persistent_when_standalone() {
        let dir = TempDir::new().expect("tempdir");
        let secrets_dir = dir.path();
        let persistent = make_persistent(secrets_dir).await;
        let (sk, _pk) = RecipientPrivateKey::generate();

        // No wrapped_dek.bin → standalone mode.
        assert!(!is_clustered_mode(secrets_dir));
        let baseline_count = Arc::strong_count(&persistent);

        // Even with a Raft handle wired in, absence of wrapped_dek.bin
        // forces the persistent store.
        let raft_handle: Arc<dyn RaftSecretsHandle> = Arc::new(DummyRaftHandle);
        let node_priv = Arc::new(sk);
        let chosen = select_secrets_store_inner(
            secrets_dir,
            Arc::clone(&persistent),
            Some(raft_handle),
            &node_priv,
            "node-uuid".to_string(),
        );

        // The function returned the persistent Arc widened to a trait
        // object. The strong count on `persistent` should reflect that
        // `chosen` is another reference to the *same* allocation.
        // (baseline + 1 for `chosen`. The clone passed into the call
        // is consumed by the function's argument and either dropped or
        // re-returned; in the standalone branch it's re-returned, so
        // the count is baseline + 1.)
        let chosen_count = Arc::strong_count(&persistent);
        assert!(
            chosen_count > baseline_count,
            "standalone branch must hand back the persistent Arc \
             (strong count was {baseline_count}, became {chosen_count})"
        );

        // Drop the chosen Arc and confirm the count returns to baseline.
        drop(chosen);
        assert_eq!(
            Arc::strong_count(&persistent),
            baseline_count,
            "dropping `chosen` should restore the strong count if it pointed at `persistent`"
        );
    }

    #[tokio::test]
    async fn select_secrets_store_picks_raft_when_clustered() {
        let dir = TempDir::new().expect("tempdir");
        let secrets_dir = dir.path();
        let persistent = make_persistent(secrets_dir).await;
        let (sk, _pk) = RecipientPrivateKey::generate();

        // Drop a non-empty wrapped_dek.bin to flip the clustered marker.
        std::fs::write(wrapped_dek_path(secrets_dir), b"placeholder")
            .expect("write wrapped_dek.bin");
        assert!(is_clustered_mode(secrets_dir));
        let baseline_count = Arc::strong_count(&persistent);

        // With a live Raft handle, we must construct a RaftSecretsStore.
        let raft_handle: Arc<dyn RaftSecretsHandle> = Arc::new(DummyRaftHandle);
        let node_priv = Arc::new(sk);
        let chosen = select_secrets_store_inner(
            secrets_dir,
            Arc::clone(&persistent),
            Some(raft_handle),
            &node_priv,
            "node-uuid".to_string(),
        );

        // The chosen store must NOT be the persistent Arc — it should
        // be a freshly-allocated RaftSecretsStore wrapper. So the
        // strong count on `persistent` is back at baseline (the clone
        // passed into the function was dropped on the way out of the
        // clustered branch).
        assert_eq!(
            Arc::strong_count(&persistent),
            baseline_count,
            "clustered branch must NOT hold a reference to the persistent Arc"
        );

        // And `chosen` is the only owner of its (Raft-backed) allocation.
        assert_eq!(
            Arc::strong_count(&chosen),
            1,
            "chosen RaftSecretsStore must be a fresh, sole-owner Arc"
        );
    }

    #[tokio::test]
    async fn select_secrets_store_falls_back_when_clustered_but_raft_down() {
        // wrapped_dek.bin present but Raft failed to come up — degrade
        // gracefully to the persistent store with a logged warning
        // rather than refusing every secret op.
        let dir = TempDir::new().expect("tempdir");
        let secrets_dir = dir.path();
        let persistent = make_persistent(secrets_dir).await;
        let (sk, _pk) = RecipientPrivateKey::generate();

        std::fs::write(wrapped_dek_path(secrets_dir), b"placeholder")
            .expect("write wrapped_dek.bin");
        assert!(is_clustered_mode(secrets_dir));
        let baseline_count = Arc::strong_count(&persistent);

        let node_priv = Arc::new(sk);
        let chosen = select_secrets_store_inner(
            secrets_dir,
            Arc::clone(&persistent),
            None, // raft unavailable
            &node_priv,
            "node-uuid".to_string(),
        );

        // Same as the standalone case: the persistent Arc count must
        // grow because `chosen` is another reference to it.
        let chosen_count = Arc::strong_count(&persistent);
        assert!(
            chosen_count > baseline_count,
            "fallback branch must hand back the persistent Arc \
             (strong count was {baseline_count}, became {chosen_count})"
        );
        drop(chosen);
        assert_eq!(Arc::strong_count(&persistent), baseline_count);
    }
}
