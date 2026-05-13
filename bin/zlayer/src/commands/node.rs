use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[cfg(not(unix))]
use crate::ui::consent::ConsentMode;

// =============================================================================
// Node management types
// =============================================================================

// NodeConfig and its helpers live in `crate::daemon` so they can be shared
// with the daemon init path on every platform (Windows included). Re-export
// them here so the rest of this module keeps using the short name.
pub(crate) use crate::daemon::{
    current_timestamp, detect_local_ip, generate_node_id, load_node_config, save_node_config,
    NodeConfig,
};

/// Join token payload
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClusterJoinToken {
    /// Leader's API endpoint
    api_endpoint: String,
    /// Leader's Raft endpoint
    raft_endpoint: String,
    /// Leader's overlay public key
    leader_wg_pubkey: String,
    /// Overlay network CIDR
    overlay_cidr: String,
    /// Cluster authentication secret
    auth_secret: String,
    /// Token creation timestamp
    created_at: String,
}

/// Join request sent to the leader
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeJoinRequest {
    /// Join token for authentication
    token: String,
    /// Joining node's advertise address
    advertise_addr: String,
    /// Joining node's overlay port
    overlay_port: u16,
    /// Joining node's Raft port
    raft_port: u16,
    /// Joining node's overlay public key
    wg_public_key: String,
    /// Node mode (full, replicate)
    mode: String,
    /// Services to replicate (if mode is replicate)
    services: Option<Vec<String>>,
    /// Operating system of the joining agent (detected via `std::env::consts::OS`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    os: Option<zlayer_spec::OsKind>,
    /// CPU architecture of the joining agent (detected via `std::env::consts::ARCH`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arch: Option<zlayer_spec::ArchKind>,
    /// Joiner's 32-byte X25519 pubkey for sealed-box DEK wrapping (Phase 1+).
    /// `None` only when the local keypair could not be created (extremely
    /// unusual — the leader will then treat this node as not eligible to
    /// host replicated-secret ciphertext).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secrets_pubkey: Option<[u8; 32]>,
}

/// Join response from the leader
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeJoinResponse {
    /// Assigned node ID
    node_id: String,
    /// Assigned Raft node ID
    raft_node_id: u64,
    /// Assigned overlay IP
    overlay_ip: String,
    /// Per-node slice CIDR assigned by the leader (e.g. "10.200.42.0/28").
    /// Empty string if the leader is not slice-aware yet.
    #[serde(default)]
    slice_cidr: String,
    /// Existing peers in the cluster
    peers: Vec<PeerNode>,
    /// Node JWT minted by the leader for this joiner — `roles: ["node"]`
    /// with the assigned `node_id` baked in. Used to authenticate inter-node
    /// calls separately from any user identity. `None` when the leader is a
    /// legacy build without secrets capability.
    #[serde(default)]
    node_jwt: Option<String>,
    /// Sealed-box-wrapped copy of the cluster DEK addressed to the joiner's
    /// `secrets_pubkey`. The daemon unwraps this on startup with the on-disk
    /// node X25519 private key. `None` on legacy responses or when the
    /// joiner did not provide a `secrets_pubkey`.
    #[serde(default)]
    wrapped_dek: Option<Vec<u8>>,
    /// Cluster DEK generation that `wrapped_dek` was sealed under. Lets the
    /// daemon detect rotation drift. `None` when `wrapped_dek` is `None`.
    #[serde(default)]
    dek_generation: Option<u64>,
}

/// Peer node information
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerNode {
    node_id: String,
    raft_node_id: u64,
    advertise_addr: String,
    overlay_port: u16,
    raft_port: u16,
    wg_public_key: String,
    overlay_ip: String,
}

/// Node status for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeStatus {
    id: String,
    address: String,
    status: String,
    mode: String,
    services: Vec<String>,
    is_leader: bool,
    /// Server-computed Raft role for this node: `"leader"`, `"voter"`, or
    /// `"learner"`. Defaulted so old daemons that don't yet return this
    /// field still deserialize cleanly (empty string then renders as
    /// "Unknown" in the table).
    #[serde(default)]
    role: String,
}

// =============================================================================
// Helper functions
// =============================================================================

/// Generate a secure random token
fn generate_secure_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
}

/// Load node config, or auto-initialize as single-node leader if none exists.
pub(crate) async fn load_or_init_node_config(data_dir: &Path) -> Result<NodeConfig> {
    if let Ok(cfg) = load_node_config(data_dir).await {
        Ok(cfg)
    } else {
        let advertise_addr = detect_local_ip();
        println!(
            "Node not yet initialized. Auto-initializing with advertise address {advertise_addr}..."
        );
        handle_node_init(
            advertise_addr,
            3669,
            9000,
            zlayer_core::DEFAULT_WG_PORT,
            data_dir.to_path_buf(),
            "10.200.0.0/16".to_string(),
            // Auto-init from `load_or_init_node_config` has no CLI args to read
            // `--install-wsl` from. `ConsentMode::Ask` honours `ZLAYER_INSTALL_WSL`
            // if set and otherwise falls back to the interactive prompt, matching
            // the top-level CLI default. The parameter is `#[cfg(not(unix))]` only
            // — Unix builds don't see it.
            #[cfg(not(unix))]
            ConsentMode::Ask,
        )
        .await?;
        load_node_config(data_dir)
            .await
            .context("Failed to load node config after auto-initialization")
    }
}

/// Generate a join token for the cluster
fn generate_join_token_data(
    advertise_addr: &str,
    api_port: u16,
    raft_port: u16,
    wg_public_key: &str,
    overlay_cidr: &str,
) -> Result<String> {
    let token_data = ClusterJoinToken {
        api_endpoint: format!("{advertise_addr}:{api_port}"),
        raft_endpoint: format!("{advertise_addr}:{raft_port}"),
        leader_wg_pubkey: wg_public_key.to_string(),
        overlay_cidr: overlay_cidr.to_string(),
        auth_secret: generate_secure_token(),
        created_at: current_timestamp(),
    };

    let json = serde_json::to_string(&token_data).context("Failed to serialize join token")?;

    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        json.as_bytes(),
    ))
}

/// Parse a join token
fn parse_cluster_join_token(token: &str) -> Result<ClusterJoinToken> {
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, token)
        .or_else(|_| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, token))
        .context("Invalid join token: not valid base64")?;

    let token_data: ClusterJoinToken =
        serde_json::from_slice(&decoded).context("Invalid join token: not valid JSON")?;

    Ok(token_data)
}

/// Write `bytes` to `path`, creating the file with restrictive permissions
/// (Unix mode 0600 — owner read/write only) **before** any data is written.
///
/// On non-Unix targets the file is written with the default platform
/// permissions. Windows ACL hardening is handled at the data-directory
/// level by the installer (`%ProgramData%\ZLayer` inherits SYSTEM +
/// Administrators write, Users read), so per-file ACL plumbing is not
/// duplicated here.
///
/// This helper is the canonical way to persist secrets-bearing files
/// (node JWTs, wrapped DEKs, etc.) from the CLI side. It mirrors the
/// open-with-mode-0600 pattern used by
/// [`zlayer_secrets::load_or_generate_node_keypair`] so the on-disk
/// permission story is consistent across all node-secret material.
fn write_secure(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // Make sure the parent directory exists. `secrets_dir` may have been
    // created by `load_or_generate_node_keypair` already, but callers are
    // free to use this helper for files that live elsewhere too.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;

        // Belt-and-suspenders: explicitly re-set perms in case a pre-existing
        // file at this path overrode the open-time mode (mode_t is only
        // honored on creation, not on truncation of an existing file).
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
    }

    Ok(())
}

/// File names for the on-disk secrets join artifacts under
/// [`zlayer_paths::ZLayerDirs::secrets()`] (`{data_dir}/secrets/`).
///
/// **Stable on-disk contract** consumed by the daemon-side bootstrap (Task #18).
///
/// Layout:
/// - `node_secrets.key` — raw 32-byte X25519 private key (mode 0600).
///   Written by `zlayer_secrets::load_or_generate_node_keypair` on the
///   first call from the CLI; the daemon loads the same file at startup.
/// - `node.jwt` — UTF-8 JWT minted by the leader, `roles: ["node"]`,
///   `node_id` set (mode 0600). Used to authenticate inter-node API calls
///   separately from user identity.
/// - `wrapped_dek.bin` — raw bytes of the sealed-box-wrapped cluster DEK
///   addressed to `node_secrets.key`'s pubkey (mode 0600). Daemon unwraps
///   with the X25519 private key on startup and holds the DEK in zeroized
///   memory.
/// - `dek_generation` — decimal-string DEK generation (e.g. `"3"`), used
///   for rotation drift detection (mode 0600).
const NODE_JWT_FILE: &str = "node.jwt";
const WRAPPED_DEK_FILE: &str = "wrapped_dek.bin";
const DEK_GENERATION_FILE: &str = "dek_generation";

/// Persist the secrets join material returned by the leader under
/// `secrets_dir` (which must already exist — typically created by
/// `load_or_generate_node_keypair`).
///
/// All three pieces (`jwt`, `wrapped_dek`, `dek_generation`) are written
/// atomically from the caller's perspective: any partial state is the
/// result of a process kill mid-write, in which case the daemon-side
/// loader treats a missing or short-read file as "no secrets capability"
/// and falls back to local-only secrets (matching the legacy-leader path).
///
/// Returns `Ok(())` on a clean write of all three files, otherwise an
/// `anyhow::Error` annotated with the path that failed.
fn persist_secrets_join_material(
    secrets_dir: &Path,
    jwt: &str,
    wrapped_dek: &[u8],
    dek_generation: u64,
) -> Result<()> {
    let jwt_path = secrets_dir.join(NODE_JWT_FILE);
    let dek_path = secrets_dir.join(WRAPPED_DEK_FILE);
    let gen_path = secrets_dir.join(DEK_GENERATION_FILE);

    write_secure(&jwt_path, jwt.as_bytes())
        .with_context(|| format!("write node JWT to {}", jwt_path.display()))?;
    write_secure(&dek_path, wrapped_dek)
        .with_context(|| format!("write wrapped DEK to {}", dek_path.display()))?;
    write_secure(&gen_path, dek_generation.to_string().as_bytes())
        .with_context(|| format!("write DEK generation to {}", gen_path.display()))?;

    info!(
        jwt_path = %jwt_path.display(),
        dek_path = %dek_path.display(),
        dek_generation,
        "persisted secrets join material"
    );

    Ok(())
}

// =============================================================================
// Command handlers
// =============================================================================

use crate::cli::NodeCommands;

/// Top-level dispatcher for node subcommands
#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_node(
    node_cmd: &NodeCommands,
    cli_data_dir: &std::path::Path,
) -> Result<()> {
    match node_cmd {
        NodeCommands::Init {
            advertise_addr,
            api_port,
            raft_port,
            overlay_port,
            data_dir,
            overlay_cidr,
            install_wsl,
        } => {
            let resolved_dir = data_dir
                .clone()
                .unwrap_or_else(|| cli_data_dir.to_path_buf());
            // The Windows (and exotic-target) branch of `handle_node_init` takes an
            // extra `ConsentMode` argument for the WSL2 auto-install consent flow.
            // Unix ignores the flag entirely, so the field is only threaded through
            // on non-Unix targets.
            #[cfg(not(unix))]
            let consent = install_wsl.mode();
            #[cfg(unix)]
            let _ = install_wsl; // silence unused-field warning on Unix.
            handle_node_init(
                advertise_addr.clone(),
                *api_port,
                *raft_port,
                *overlay_port,
                resolved_dir,
                overlay_cidr.clone(),
                #[cfg(not(unix))]
                consent,
            )
            .await
        }
        NodeCommands::Join {
            leader_addr,
            token,
            advertise_addr,
            mode,
            services,
            api_port,
            raft_port,
            overlay_port,
            install_wsl,
        } => {
            #[cfg(not(unix))]
            let consent = install_wsl.mode();
            #[cfg(unix)]
            let _ = install_wsl;
            handle_node_join(
                leader_addr.clone(),
                token.clone(),
                advertise_addr.clone(),
                mode.clone(),
                services.clone(),
                *api_port,
                *raft_port,
                *overlay_port,
                cli_data_dir.to_path_buf(),
                #[cfg(not(unix))]
                consent,
            )
            .await
        }
        NodeCommands::List { output } => handle_node_list(output.clone(), cli_data_dir).await,
        NodeCommands::Status { node_id } => handle_node_status(node_id.clone(), cli_data_dir).await,
        NodeCommands::Remove { node_id, force } => {
            handle_node_remove(node_id.clone(), *force, cli_data_dir).await
        }
        NodeCommands::SetMode {
            node_id,
            mode,
            services,
        } => {
            handle_node_set_mode(
                node_id.clone(),
                mode.clone(),
                services.clone(),
                cli_data_dir,
            )
            .await
        }
        NodeCommands::Label { node_id, label } => {
            handle_node_label(node_id.clone(), label.clone(), cli_data_dir).await
        }
        NodeCommands::ForceLeader { api_addr } => handle_node_force_leader(api_addr.clone()).await,
        NodeCommands::GenerateJoinToken {
            deployment,
            api,
            service,
            data_dir,
        } => {
            let resolved_dir = data_dir
                .clone()
                .unwrap_or_else(|| cli_data_dir.to_path_buf());
            handle_node_generate_join_token(
                deployment.clone(),
                api.clone(),
                service.clone(),
                resolved_dir,
            )
            .await
        }
    }
}

/// Initialize this node as cluster leader (Windows).
///
/// Mirrors the Unix implementation below, substituting the privileged-setup
/// steps for their Windows analogues:
///
/// - **Admin check** via [`zlayer_paths::is_root()`] (backed by `IsUserAnAdmin`
///   on Windows). Cluster bootstrap needs admin for firewall rules, Wintun
///   adapter creation, and `wsl --install`.
/// - **WSL2 backend bootstrap** via the G-6 consent flow. If the user declines
///   we print a note and continue — the cluster still comes up, but Linux
///   container dispatch has to go through a remote peer.
/// - **Overlay + Raft bootstrap** are fully cross-platform so they reuse the
///   Unix logic verbatim.
/// - **GPU detection** calls [`zlayer_agent::detect_gpus`] which now has a real
///   Windows branch (H-4).
/// - **Firewall rules** are installed via [`zlayer_overlay::firewall::ensure_overlay_rules`]
///   (H-6). No-ops on non-Windows.
/// - **Configs persistence** writes `node_config.json` + `overlay_bootstrap.json`
///   under `data_dir`, which resolves to `%ProgramData%\ZLayer\` on Windows
///   via `ZLayerDirs::default_data_dir()`.
#[cfg(windows)]
#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_node_init(
    advertise_addr: String,
    api_port: u16,
    raft_port: u16,
    overlay_port: u16,
    data_dir: PathBuf,
    overlay_cidr: String,
    install_wsl: ConsentMode,
) -> Result<()> {
    use zlayer_overlay::OverlayTransport;

    println!("Initializing ZLayer node as cluster leader...");

    // 1. Admin check — Windows analogue of the Unix root check. Cluster bootstrap
    //    writes firewall rules and (on the `wsl` feature) drives an elevated
    //    `wsl --install`. Both require administrator privileges.
    if !zlayer_paths::is_root() {
        anyhow::bail!(
            "`zlayer node init` on Windows requires running as Administrator. \
             Right-click PowerShell/cmd → Run as Administrator, then re-run this command."
        );
    }

    // 2. Create data directory.
    tokio::fs::create_dir_all(&data_dir)
        .await
        .with_context(|| format!("Failed to create data directory: {}", data_dir.display()))?;
    info!(path = %data_dir.display(), "Created data directory");

    // 3. Check if already initialized (idempotent).
    let config_path = data_dir.join("node_config.json");
    if config_path.exists() {
        let existing = crate::daemon::load_or_init_node_config(&data_dir).await?;
        println!();
        println!("Node already initialized.");
        println!("Node ID:        {}", existing.node_id);
        println!("Raft Node ID:   {}", existing.raft_node_id);
        println!(
            "API Server:     http://{}:{}",
            existing.advertise_addr, existing.api_port
        );
        println!(
            "Raft Address:   {}:{}",
            existing.advertise_addr, existing.raft_port
        );
        println!("Use 'zlayer node status' for full details.");
        return Ok(());
    }

    // 4. WSL2 bootstrap (G-6 consent flow). The `wsl` Cargo feature gates the
    //    entire WSL crate; without it the backend is simply unavailable on this
    //    build and we skip the step with a warning. When present, a user refusal
    //    (`--install-wsl no` or an interactive "n") gracefully degrades: the node
    //    still bootstraps, but local Linux container dispatch is offline and
    //    Linux workloads must be routed through a remote peer.
    #[cfg(feature = "wsl")]
    {
        use zlayer_wsl::errors::WslError;
        use zlayer_wsl::setup::ensure_wsl_backend_ready_with_consent;

        use crate::ui::consent::{wsl2_install_consent, ConsentDecision};

        println!("  Checking WSL2 backend (Linux container support)...");
        let consent_mode = install_wsl;
        let consent_closure = move || match wsl2_install_consent(consent_mode)? {
            ConsentDecision::Granted => Ok(true),
            ConsentDecision::Refused => Ok(false),
        };

        match ensure_wsl_backend_ready_with_consent(consent_closure).await {
            Ok(_) => {
                info!("WSL2 backend is ready");
                println!("  WSL2 backend: ready");
            }
            Err(err) => {
                if let Some(wsl_err) = err.downcast_ref::<WslError>() {
                    match wsl_err {
                        WslError::InstallRefused => {
                            warn!("WSL2 auto-install declined; continuing without local Linux container support");
                            println!(
                                "  WSL2 auto-install declined. The node will bootstrap without a local Linux runtime.\n\
                                 Windows containers (HCS) will still run locally; Linux workloads will be routed\n\
                                 to remote peers. Re-run with `--install-wsl yes` or install WSL2 manually\n\
                                 (`wsl.exe --install --no-distribution`) to enable local Linux containers."
                            );
                        }
                        WslError::RebootRequired => {
                            anyhow::bail!(
                                "WSL2 install completed but Windows requires a reboot. \
                                 Please reboot and re-run `zlayer node init`."
                            );
                        }
                        _ => {
                            warn!(error = %err, "WSL2 setup failed; continuing without local Linux container support");
                            println!(
                                "  WSL2 setup failed: {err}.\n  The node will bootstrap without a local Linux runtime."
                            );
                        }
                    }
                } else {
                    warn!(error = %err, "WSL2 setup failed with an unexpected error; continuing");
                    println!("  WSL2 setup failed: {err}.\n  The node will bootstrap without a local Linux runtime.");
                }
            }
        }
    }
    #[cfg(not(feature = "wsl"))]
    {
        let _ = install_wsl;
        warn!(
            "`wsl` feature not enabled in this build; skipping WSL2 backend bootstrap. \
             Local Linux container dispatch will be unavailable until WSL2 support is compiled in."
        );
    }

    // 5. Generate node ID.
    let node_id = generate_node_id();
    let raft_node_id: u64 = 1; // First node is always ID 1
    info!(node_id = %node_id, raft_node_id = raft_node_id, "Generated node ID");

    // 6. Generate overlay keypair.
    println!("  Generating overlay keypair...");
    let (private_key, public_key) = OverlayTransport::generate_keys()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to generate overlay keys: {e}"))?;
    info!("Generated overlay keypair");

    // 7. Save node config.
    let node_config = NodeConfig {
        node_id: node_id.clone(),
        raft_node_id,
        advertise_addr: advertise_addr.clone(),
        api_port,
        raft_port,
        overlay_port,
        overlay_cidr: overlay_cidr.clone(),
        wireguard_private_key: private_key,
        wireguard_public_key: public_key.clone(),
        is_leader: true,
        created_at: current_timestamp(),
    };
    save_node_config(&data_dir, &node_config).await?;

    // 8. Detect GPUs (H-4 — real Windows GPU enumeration via NVML + WMI).
    println!("  Detecting GPUs...");
    let detected_gpus = zlayer_agent::detect_gpus();
    if detected_gpus.is_empty() {
        info!("No GPUs detected on this node");
    } else {
        info!(
            gpu_count = detected_gpus.len(),
            "Detected GPUs on this node"
        );
        for gpu in &detected_gpus {
            info!(
                pci_bus_id = %gpu.pci_bus_id,
                vendor = %gpu.vendor,
                model = %gpu.model,
                memory_mb = gpu.memory_mb,
                device_path = %gpu.device_path,
                "Detected GPU"
            );
        }
    }

    // 9. Initialize Raft as leader (bootstrap single-node cluster) — cross-platform.
    println!("  Starting Raft consensus...");
    let raft_config = zlayer_scheduler::RaftConfig {
        node_id: raft_node_id,
        address: format!("{advertise_addr}:{raft_port}"),
        raft_port,
        ..Default::default()
    };

    let raft = zlayer_scheduler::RaftCoordinator::new(raft_config)
        .await
        .context("Failed to create Raft coordinator")?;

    raft.bootstrap()
        .await
        .context("Failed to bootstrap Raft cluster")?;
    info!("Raft cluster bootstrapped");

    // Register the leader node in the Raft state machine. See the Unix body for
    // the background on why we register with placeholder overlay metadata that
    // the daemon later overwrites from the persisted bootstrap state.
    let leader_overlay_ip = "10.200.0.1".to_string();
    let leader_slice_cidr = String::new();
    raft.propose(zlayer_scheduler::Request::RegisterNode {
        node_id: raft_node_id,
        address: format!("{advertise_addr}:{raft_port}"),
        wg_public_key: public_key.clone(),
        overlay_ip: leader_overlay_ip,
        overlay_port,
        advertise_addr: advertise_addr.clone(),
        api_port,
        cpu_total: 0.0,
        memory_total: 0,
        disk_total: 0,
        gpus: vec![],
        mode: "full".to_string(),
        os: zlayer_spec::OsKind::from_rust_os(std::env::consts::OS),
        arch: zlayer_spec::ArchKind::from_rust_arch(std::env::consts::ARCH),
        slice_cidr: leader_slice_cidr,
    })
    .await
    .context("Failed to register leader in Raft state")?;

    // 10. Install Windows Firewall rules (H-6). No-op on non-Windows targets.
    if let Err(err) =
        zlayer_overlay::firewall::ensure_overlay_rules(overlay_port, api_port, raft_port)
    {
        warn!(
            error = %err,
            "Failed to install Windows firewall rules for overlay/API/Raft ports. \
             Inbound cluster traffic may be blocked until rules are added manually."
        );
        println!(
            "  Warning: failed to install Windows firewall rules: {err}\n\
             Inbound cluster traffic may be blocked until rules are added manually."
        );
    } else {
        info!("Installed Windows firewall rules for overlay/API/Raft ports");
        println!(
            "  Firewall: rules installed for overlay/API/Raft ports (Private + Domain profiles)"
        );
    }

    // 11. Generate join token.
    let join_token = generate_join_token_data(
        &advertise_addr,
        api_port,
        raft_port,
        &public_key,
        &overlay_cidr,
    )?;

    // 12. Print success message.
    println!();
    println!("Node initialized successfully!");
    println!();
    println!("Node ID:        {node_id}");
    println!("Raft Node ID:   {raft_node_id}");
    println!("API Server:     http://{advertise_addr}:{api_port}");
    println!("Raft Address:   {advertise_addr}:{raft_port}");
    println!("Overlay Port:   {overlay_port}");
    println!("Overlay CIDR:   {overlay_cidr}");
    println!("WG Public Key:  {public_key}");
    if detected_gpus.is_empty() {
        println!("GPUs:           None detected");
    } else {
        println!("GPUs:           {} detected", detected_gpus.len());
        for gpu in &detected_gpus {
            println!(
                "  - {} ({}, {} MB VRAM) at {}",
                gpu.model, gpu.vendor, gpu.memory_mb, gpu.pci_bus_id
            );
        }
    }
    println!();
    println!("To join other nodes to this cluster, run:");
    println!();
    println!(
        "  zlayer node join {advertise_addr}:{api_port} --token {join_token} --advertise-addr <NODE_IP>"
    );
    println!();
    println!("Note: The API server starts automatically when deploying with 'zlayer deploy' or 'zlayer up'.");

    Ok(())
}

/// `node init` fallback for exotic targets that are neither Unix nor Windows.
///
/// We cannot build the overlay + agent runtime for these targets, so the
/// command bails with an actionable error. The main two supported platforms
/// (Unix — Linux/macOS — and Windows) each have a full implementation above.
#[cfg(all(not(unix), not(windows)))]
pub(crate) async fn handle_node_init(
    _advertise_addr: String,
    _api_port: u16,
    _raft_port: u16,
    _overlay_port: u16,
    _data_dir: PathBuf,
    _overlay_cidr: String,
    _install_wsl: ConsentMode,
) -> Result<()> {
    anyhow::bail!(
        "'node init' is only supported on Linux, macOS, and Windows. \
         This build targets an exotic platform where the overlay + agent runtime is unavailable."
    )
}

/// Initialize this node as cluster leader
#[cfg(unix)]
#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_node_init(
    advertise_addr: String,
    api_port: u16,
    raft_port: u16,
    overlay_port: u16,
    data_dir: PathBuf,
    overlay_cidr: String,
) -> Result<()> {
    use zlayer_overlay::OverlayTransport;

    println!("Initializing ZLayer node as cluster leader...");

    // 1. Create data directory
    tokio::fs::create_dir_all(&data_dir)
        .await
        .with_context(|| format!("Failed to create data directory: {}", data_dir.display()))?;
    info!(path = %data_dir.display(), "Created data directory");

    // 2. Check if already initialized -- use load_or_init_node_config()
    //    from daemon.rs for the load path.  If an existing config is found,
    //    we show its details and exit early (idempotent).
    let config_path = data_dir.join("node_config.json");
    if config_path.exists() {
        let existing = crate::daemon::load_or_init_node_config(&data_dir).await?;
        println!();
        println!("Node already initialized.");
        println!("Node ID:        {}", existing.node_id);
        println!("Raft Node ID:   {}", existing.raft_node_id);
        println!(
            "API Server:     http://{}:{}",
            existing.advertise_addr, existing.api_port
        );
        println!(
            "Raft Address:   {}:{}",
            existing.advertise_addr, existing.raft_port
        );
        println!("Use 'zlayer node status' for full details.");
        return Ok(());
    }

    // 3. Generate node ID
    let node_id = generate_node_id();
    let raft_node_id: u64 = 1; // First node is always ID 1
    info!(node_id = %node_id, raft_node_id = raft_node_id, "Generated node ID");

    // 4. Generate overlay keypair
    println!("  Generating overlay keypair...");
    let (private_key, public_key) = OverlayTransport::generate_keys()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to generate overlay keys: {e}"))?;
    info!("Generated overlay keypair");

    // 5. Save node config
    let node_config = NodeConfig {
        node_id: node_id.clone(),
        raft_node_id,
        advertise_addr: advertise_addr.clone(),
        api_port,
        raft_port,
        overlay_port,
        overlay_cidr: overlay_cidr.clone(),
        wireguard_private_key: private_key,
        wireguard_public_key: public_key.clone(),
        is_leader: true,
        created_at: current_timestamp(),
    };
    save_node_config(&data_dir, &node_config).await?;

    // 6. Detect GPUs on this node
    println!("  Detecting GPUs...");
    let detected_gpus = zlayer_agent::detect_gpus();
    if detected_gpus.is_empty() {
        info!("No GPUs detected on this node");
    } else {
        info!(
            gpu_count = detected_gpus.len(),
            "Detected GPUs on this node"
        );
        for gpu in &detected_gpus {
            info!(
                pci_bus_id = %gpu.pci_bus_id,
                vendor = %gpu.vendor,
                model = %gpu.model,
                memory_mb = gpu.memory_mb,
                device_path = %gpu.device_path,
                "Detected GPU"
            );
        }
    }

    // 7. Initialize Raft as leader (bootstrap single-node cluster)
    println!("  Starting Raft consensus...");
    let raft_config = zlayer_scheduler::RaftConfig {
        node_id: raft_node_id,
        address: format!("{advertise_addr}:{raft_port}"),
        raft_port,
        ..Default::default()
    };

    let raft = zlayer_scheduler::RaftCoordinator::new(raft_config)
        .await
        .context("Failed to create Raft coordinator")?;

    raft.bootstrap()
        .await
        .context("Failed to bootstrap Raft cluster")?;
    info!("Raft cluster bootstrapped");

    // Register the leader node in the Raft state machine with overlay metadata.
    // `handle_node_init` runs before the overlay bootstrap is persisted — the
    // daemon is responsible for computing the leader's `/28` slice on first
    // start (via `OverlayBootstrap::init_leader`), after which the daemon
    // re-registers the leader with the correct `slice_cidr`. Here we register
    // with the legacy hardcoded `10.200.0.1` overlay IP and an empty
    // `slice_cidr`; the daemon overwrites both once the bootstrap state
    // exists on disk.
    let leader_overlay_ip = "10.200.0.1".to_string();
    let leader_slice_cidr = String::new();
    raft.propose(zlayer_scheduler::Request::RegisterNode {
        node_id: raft_node_id,
        address: format!("{advertise_addr}:{raft_port}"),
        wg_public_key: public_key.clone(),
        overlay_ip: leader_overlay_ip,
        overlay_port,
        advertise_addr: advertise_addr.clone(),
        api_port,
        cpu_total: 0.0,
        memory_total: 0,
        disk_total: 0,
        gpus: vec![],
        mode: "full".to_string(),
        os: zlayer_spec::OsKind::from_rust_os(std::env::consts::OS),
        arch: zlayer_spec::ArchKind::from_rust_arch(std::env::consts::ARCH),
        slice_cidr: leader_slice_cidr,
    })
    .await
    .context("Failed to register leader in Raft state")?;

    // 8. Generate join token
    let join_token = generate_join_token_data(
        &advertise_addr,
        api_port,
        raft_port,
        &public_key,
        &overlay_cidr,
    )?;

    // 9. Print success message
    println!();
    println!("Node initialized successfully!");
    println!();
    println!("Node ID:        {node_id}");
    println!("Raft Node ID:   {raft_node_id}");
    println!("API Server:     http://{advertise_addr}:{api_port}");
    println!("Raft Address:   {advertise_addr}:{raft_port}");
    println!("Overlay Port:   {overlay_port}");
    println!("Overlay CIDR:   {overlay_cidr}");
    println!("WG Public Key:  {public_key}");
    if detected_gpus.is_empty() {
        println!("GPUs:           None detected");
    } else {
        println!("GPUs:           {} detected", detected_gpus.len());
        for gpu in &detected_gpus {
            println!(
                "  - {} ({}, {} MB VRAM) at {}",
                gpu.model, gpu.vendor, gpu.memory_mb, gpu.pci_bus_id
            );
        }
    }
    println!();
    println!("To join other nodes to this cluster, run:");
    println!();
    println!(
        "  zlayer node join {advertise_addr}:{api_port} --token {join_token} --advertise-addr <NODE_IP>"
    );
    println!();
    println!("Note: The API server starts automatically when deploying with 'zlayer deploy' or 'zlayer up'.");

    Ok(())
}

/// Join an existing cluster as a worker node (Windows).
///
/// See the Unix body below for the canonical flow. The Windows version differs
/// only in the privileged-setup steps:
///
/// - **Admin check** via [`zlayer_paths::is_root()`] (H-5).
/// - **WSL2 bootstrap** via the G-6 consent flow. `InstallRefused` is a soft
///   failure: the node still joins, but without a local Linux runtime.
/// - **Wintun** interface creation is handled transparently by
///   [`zlayer_overlay::OverlayTransport`] on Windows — the call sites are
///   identical to the Unix code.
/// - **GPU detection** uses the Windows branch of `detect_gpus()` (H-4).
/// - **Firewall rules** are installed via [`zlayer_overlay::firewall::ensure_overlay_rules`]
///   (H-6). No-op on non-Windows.
/// - **Configs persistence** writes `node_config.json` + `overlay_bootstrap.json`
///   under `data_dir`, which resolves to `%ProgramData%\ZLayer\` on Windows.
#[cfg(windows)]
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_node_join(
    leader_addr: String,
    token: String,
    advertise_addr: String,
    mode: String,
    services: Option<Vec<String>>,
    api_port: u16,
    raft_port: u16,
    overlay_port: u16,
    data_dir_override: PathBuf,
    install_wsl: ConsentMode,
) -> Result<()> {
    use std::time::Duration;
    use zlayer_overlay::OverlayTransport;

    println!("Joining ZLayer cluster at {leader_addr}...");

    // 1. Admin check (H-5) — same reasoning as `handle_node_init`.
    if !zlayer_paths::is_root() {
        anyhow::bail!(
            "`zlayer node join` on Windows requires running as Administrator. \
             Right-click PowerShell/cmd → Run as Administrator, then re-run this command."
        );
    }

    // 2. WSL2 bootstrap (G-6). Mirrors the `handle_node_init` flow — a refusal
    //    degrades gracefully to "join without local Linux runtime" rather than
    //    hard-failing.
    #[cfg(feature = "wsl")]
    {
        use zlayer_wsl::errors::WslError;
        use zlayer_wsl::setup::ensure_wsl_backend_ready_with_consent;

        use crate::ui::consent::{wsl2_install_consent, ConsentDecision};

        println!("  Checking WSL2 backend (Linux container support)...");
        let consent_mode = install_wsl;
        let consent_closure = move || match wsl2_install_consent(consent_mode)? {
            ConsentDecision::Granted => Ok(true),
            ConsentDecision::Refused => Ok(false),
        };

        match ensure_wsl_backend_ready_with_consent(consent_closure).await {
            Ok(_) => {
                info!("WSL2 backend is ready");
                println!("  WSL2 backend: ready");
            }
            Err(err) => {
                if let Some(wsl_err) = err.downcast_ref::<WslError>() {
                    match wsl_err {
                        WslError::InstallRefused => {
                            warn!("WSL2 auto-install declined; joining without local Linux container support");
                            println!(
                                "  WSL2 auto-install declined. The node will join without a local Linux runtime.\n\
                                 Windows containers (HCS) will still run locally; Linux workloads will be routed\n\
                                 to remote peers. Re-run with `--install-wsl yes` or install WSL2 manually\n\
                                 (`wsl.exe --install --no-distribution`) to enable local Linux containers."
                            );
                        }
                        WslError::RebootRequired => {
                            anyhow::bail!(
                                "WSL2 install completed but Windows requires a reboot. \
                                 Please reboot and re-run `zlayer node join`."
                            );
                        }
                        _ => {
                            warn!(error = %err, "WSL2 setup failed; joining without local Linux container support");
                            println!(
                                "  WSL2 setup failed: {err}.\n  The node will join without a local Linux runtime."
                            );
                        }
                    }
                } else {
                    warn!(error = %err, "WSL2 setup failed with an unexpected error; continuing");
                    println!("  WSL2 setup failed: {err}.\n  The node will join without a local Linux runtime.");
                }
            }
        }
    }
    #[cfg(not(feature = "wsl"))]
    {
        let _ = install_wsl;
        warn!(
            "`wsl` feature not enabled in this build; skipping WSL2 backend bootstrap. \
             Local Linux container dispatch will be unavailable until WSL2 support is compiled in."
        );
    }

    // 3. Parse and validate the join token.
    let token_data = parse_cluster_join_token(&token).context("Invalid join token")?;

    info!(
        api_endpoint = %token_data.api_endpoint,
        overlay_cidr = %token_data.overlay_cidr,
        "Parsed join token"
    );

    // 4. Generate overlay keypair.
    println!("  Generating overlay keypair...");
    let (private_key, public_key) = OverlayTransport::generate_keys()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to generate overlay keys: {e}"))?;

    // 5. Determine data directory.
    let data_dir = data_dir_override;
    tokio::fs::create_dir_all(&data_dir)
        .await
        .context("Failed to create data directory")?;

    // 6. Check if already initialized.
    let config_path = data_dir.join("node_config.json");
    if config_path.exists() {
        anyhow::bail!(
            "Node already initialized. Configuration exists at {}. \
            Remove the config file to join a different cluster.",
            config_path.display()
        );
    }

    // 7. Load (or generate) the node X25519 secrets keypair and prepare the
    //    pubkey for the join request. The private key file is left on disk
    //    under `{secrets_dir}/node_secrets.key` for the daemon to load on
    //    startup (Task #18). We deliberately do not keep the private key in
    //    memory after this point — the join flow only needs the pubkey.
    let secrets_dir = zlayer_paths::ZLayerDirs::new(data_dir.clone()).secrets();
    let secrets_pubkey: Option<[u8; 32]> =
        match zlayer_secrets::load_or_generate_node_keypair(&secrets_dir) {
            Ok((_node_priv, node_pub)) => Some(*node_pub.as_bytes()),
            Err(e) => {
                warn!(
                    error = %e,
                    secrets_dir = %secrets_dir.display(),
                    "failed to load/generate node X25519 keypair; joining without \
                     replicated-secrets capability — node will not host secret \
                     ciphertext until the keypair is created and the join is retried"
                );
                None
            }
        };

    // 8. Send join request to the leader.
    println!("  Contacting leader at {}...", token_data.api_endpoint);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let join_request = NodeJoinRequest {
        token: token.clone(),
        advertise_addr: advertise_addr.clone(),
        overlay_port,
        raft_port,
        wg_public_key: public_key.clone(),
        mode: mode.clone(),
        services: services.clone(),
        os: zlayer_spec::OsKind::from_rust_os(std::env::consts::OS),
        arch: zlayer_spec::ArchKind::from_rust_arch(std::env::consts::ARCH),
        secrets_pubkey,
    };

    let response = client
        .post(format!(
            "http://{}/api/v1/cluster/join",
            token_data.api_endpoint
        ))
        .json(&join_request)
        .send()
        .await
        .context("Failed to send join request to leader")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Join request failed: {status} - {body}");
    }

    let join_response: NodeJoinResponse = response
        .json()
        .await
        .context("Failed to parse join response")?;

    info!(
        node_id = %join_response.node_id,
        raft_node_id = join_response.raft_node_id,
        overlay_ip = %join_response.overlay_ip,
        "Received join response"
    );

    // 8a. Persist the secrets join material returned by the leader so the
    //     daemon can pick it up on startup (Task #18). Three pieces are
    //     written into `{secrets_dir}` (mode 0600 on Unix):
    //       - `node.jwt`         — node-scoped JWT for inter-node calls
    //       - `wrapped_dek.bin`  — sealed-box-wrapped cluster DEK
    //       - `dek_generation`   — decimal generation marker
    //     A legacy leader returns all three as `None`; in that case we log a
    //     warning and continue — the join still succeeds and Raft membership
    //     is not gated on the secrets material.
    if let (Some(jwt), Some(wrapped), Some(generation)) = (
        join_response.node_jwt.as_ref(),
        join_response.wrapped_dek.as_ref(),
        join_response.dek_generation,
    ) {
        persist_secrets_join_material(&secrets_dir, jwt, wrapped, generation)
            .context("failed to persist secrets join material")?;
    } else {
        warn!(
            "leader did not return secrets capability — node will run without \
             cluster-replicated secrets access; re-run join against a Phase-1+ \
             leader to enable replicated secrets"
        );
    }

    // 9. Save node configuration.
    let node_config = NodeConfig {
        node_id: join_response.node_id.clone(),
        raft_node_id: join_response.raft_node_id,
        advertise_addr: advertise_addr.clone(),
        api_port,
        raft_port,
        overlay_port,
        overlay_cidr: token_data.overlay_cidr.clone(),
        wireguard_private_key: private_key,
        wireguard_public_key: public_key.clone(),
        is_leader: false,
        created_at: current_timestamp(),
    };
    save_node_config(&data_dir, &node_config).await?;

    // 10. Configure overlay with peers.
    println!("  Configuring overlay network...");

    let our_overlay_ip: std::net::IpAddr = join_response
        .overlay_ip
        .parse()
        .context("Invalid overlay IP in join response")?;

    let our_slice_cidr: Option<zlayer_overlay::ipnet::IpNet> =
        if join_response.slice_cidr.is_empty() {
            None
        } else {
            Some(
                join_response
                    .slice_cidr
                    .parse()
                    .context("Invalid slice CIDR in join response")?,
            )
        };

    let overlay_prefix_len = token_data
        .overlay_cidr
        .split('/')
        .nth(1)
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(16);

    let overlay_ip_cidr = format!("{our_overlay_ip}/{overlay_prefix_len}");

    let mut overlay_peers: Vec<zlayer_overlay::PeerConfig> = Vec::new();
    for peer in &join_response.peers {
        if peer.wg_public_key.is_empty() {
            warn!(
                peer_id = %peer.node_id,
                "Peer has no WireGuard public key, skipping overlay setup for this peer"
            );
            continue;
        }
        let peer_overlay_ip: std::net::IpAddr = match peer.overlay_ip.parse() {
            Ok(ip) => ip,
            Err(e) => {
                warn!(
                    peer_id = %peer.node_id,
                    overlay_ip = %peer.overlay_ip,
                    error = %e,
                    "Failed to parse peer overlay IP, skipping"
                );
                continue;
            }
        };
        let endpoint = format!("{}:{}", peer.advertise_addr, peer.overlay_port);
        overlay_peers.push(zlayer_overlay::PeerConfig::new(
            peer.node_id.clone(),
            peer.wg_public_key.clone(),
            endpoint,
            peer_overlay_ip,
        ));
        info!(
            peer_id = %peer.node_id,
            peer_addr = %peer.advertise_addr,
            peer_overlay_ip = %peer_overlay_ip,
            "Configured overlay peer"
        );
    }

    // 11a. Persist overlay bootstrap state (same schema as Unix).
    let bootstrap_state = zlayer_overlay::BootstrapState {
        config: zlayer_overlay::BootstrapConfig {
            cidr: token_data.overlay_cidr.clone(),
            node_ip: our_overlay_ip,
            interface: zlayer_overlay::DEFAULT_INTERFACE_NAME.to_string(),
            port: overlay_port,
            private_key: node_config.wireguard_private_key.clone(),
            public_key: node_config.wireguard_public_key.clone(),
            is_leader: false,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            slice_cidr: our_slice_cidr,
        },
        peers: overlay_peers.clone(),
        allocator_state: None,
        slice_allocator_state: None,
    };
    let bootstrap_path = data_dir.join("overlay_bootstrap.json");
    let bootstrap_json = serde_json::to_string_pretty(&bootstrap_state)
        .context("Failed to serialize overlay bootstrap state")?;
    tokio::fs::write(&bootstrap_path, bootstrap_json)
        .await
        .with_context(|| {
            format!(
                "Failed to write overlay bootstrap state to {}",
                bootstrap_path.display()
            )
        })?;
    info!(path = %bootstrap_path.display(), "Saved overlay bootstrap state");

    // 11. Create and configure the overlay transport (Wintun on Windows).
    #[allow(clippy::needless_update)]
    let overlay_config = zlayer_overlay::OverlayConfig {
        local_endpoint: std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            overlay_port,
        ),
        private_key: node_config.wireguard_private_key.clone(),
        public_key: node_config.wireguard_public_key.clone(),
        overlay_cidr: overlay_ip_cidr,
        peer_discovery_interval: Duration::from_secs(30),
        ..zlayer_overlay::OverlayConfig::default()
    };

    let peer_infos: Vec<zlayer_overlay::PeerInfo> = overlay_peers
        .iter()
        .filter_map(|p| match p.to_peer_info() {
            Ok(info) => Some(info),
            Err(e) => {
                warn!(peer = %p.node_id, error = %e, "Failed to convert peer info, skipping");
                None
            }
        })
        .collect();

    let interface_name = zlayer_overlay::DEFAULT_INTERFACE_NAME.to_string();
    let mut transport = zlayer_overlay::OverlayTransport::new(overlay_config, interface_name);

    match transport.create_interface().await {
        Ok(()) => match transport.configure(&peer_infos).await {
            Ok(()) => {
                info!(
                    overlay_ip = %our_overlay_ip,
                    peer_count = peer_infos.len(),
                    "Overlay network interface configured successfully"
                );
                println!(
                    "  Overlay network up: {} with {} peer(s)",
                    our_overlay_ip,
                    peer_infos.len()
                );
                drop(transport);
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Failed to configure overlay transport. \
                     The overlay bootstrap state has been saved and the daemon will \
                     retry on next start."
                );
                println!("  Warning: Overlay interface created but configuration failed: {e}");
            }
        },
        Err(e) => {
            warn!(
                error = %e,
                "Failed to create overlay (Wintun) network interface. \
                 Requires administrator privileges. The overlay bootstrap state has \
                 been saved and the daemon will create the interface on next start."
            );
            println!(
                "  Warning: Could not create overlay interface ({e}). \
                 The daemon will set it up on next start."
            );
        }
    }

    // 12. Detect GPUs (H-4) for the node-registration payload/info output.
    println!("  Detecting GPUs...");
    let detected_gpus = zlayer_agent::detect_gpus();
    if detected_gpus.is_empty() {
        info!("No GPUs detected on this node");
    } else {
        info!(
            gpu_count = detected_gpus.len(),
            "Detected GPUs on this node"
        );
        for gpu in &detected_gpus {
            info!(
                pci_bus_id = %gpu.pci_bus_id,
                vendor = %gpu.vendor,
                model = %gpu.model,
                memory_mb = gpu.memory_mb,
                device_path = %gpu.device_path,
                "Detected GPU"
            );
        }
    }

    // 13. Install Windows Firewall rules (H-6). No-op on non-Windows.
    if let Err(err) =
        zlayer_overlay::firewall::ensure_overlay_rules(overlay_port, api_port, raft_port)
    {
        warn!(
            error = %err,
            "Failed to install Windows firewall rules for overlay/API/Raft ports"
        );
        println!(
            "  Warning: failed to install Windows firewall rules: {err}\n\
             Inbound cluster traffic may be blocked until rules are added manually."
        );
    } else {
        info!("Installed Windows firewall rules for overlay/API/Raft ports");
        println!(
            "  Firewall: rules installed for overlay/API/Raft ports (Private + Domain profiles)"
        );
    }

    // 14. Print success message.
    println!();
    println!("Successfully joined cluster!");
    println!();
    println!("Node ID:        {}", join_response.node_id);
    println!("Raft Node ID:   {}", join_response.raft_node_id);
    println!("Overlay IP:     {}", join_response.overlay_ip);
    if !join_response.slice_cidr.is_empty() {
        println!("Slice CIDR:     {}", join_response.slice_cidr);
    }
    println!("Mode:           {mode}");
    if let Some(svcs) = services {
        println!("Services:       {}", svcs.join(", "));
    }
    println!("Peers:          {}", join_response.peers.len());
    if detected_gpus.is_empty() {
        println!("GPUs:           None detected");
    } else {
        println!("GPUs:           {} detected", detected_gpus.len());
        for gpu in &detected_gpus {
            println!(
                "  - {} ({}, {} MB VRAM) at {}",
                gpu.model, gpu.vendor, gpu.memory_mb, gpu.pci_bus_id
            );
        }
    }
    println!();
    println!("Run 'zlayer deploy' or 'zlayer up' with a spec file to start processing workloads.");

    Ok(())
}

/// `node join` fallback for exotic targets that are neither Unix nor Windows.
#[cfg(all(not(unix), not(windows)))]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_node_join(
    _leader_addr: String,
    _token: String,
    _advertise_addr: String,
    _mode: String,
    _services: Option<Vec<String>>,
    _api_port: u16,
    _raft_port: u16,
    _overlay_port: u16,
    _data_dir_override: PathBuf,
    _install_wsl: ConsentMode,
) -> Result<()> {
    anyhow::bail!(
        "'node join' is only supported on Linux, macOS, and Windows. \
         This build targets an exotic platform where the overlay + agent runtime is unavailable."
    )
}

/// Join an existing cluster as a worker node
#[cfg(unix)]
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_node_join(
    leader_addr: String,
    token: String,
    advertise_addr: String,
    mode: String,
    services: Option<Vec<String>>,
    api_port: u16,
    raft_port: u16,
    overlay_port: u16,
    data_dir_override: PathBuf,
) -> Result<()> {
    use std::time::Duration;
    use zlayer_overlay::OverlayTransport;

    println!("Joining ZLayer cluster at {leader_addr}...");

    // 1. Parse and validate the join token
    let token_data = parse_cluster_join_token(&token).context("Invalid join token")?;

    info!(
        api_endpoint = %token_data.api_endpoint,
        overlay_cidr = %token_data.overlay_cidr,
        "Parsed join token"
    );

    // 2. Generate overlay keypair for this node
    println!("  Generating overlay keypair...");
    let (private_key, public_key) = OverlayTransport::generate_keys()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to generate overlay keys: {e}"))?;

    // 3. Determine data directory
    let data_dir = data_dir_override;
    tokio::fs::create_dir_all(&data_dir)
        .await
        .context("Failed to create data directory")?;

    // 4. Check if already initialized
    let config_path = data_dir.join("node_config.json");
    if config_path.exists() {
        anyhow::bail!(
            "Node already initialized. Configuration exists at {}. \
            Remove the config file to join a different cluster.",
            config_path.display()
        );
    }

    // 5. Load (or generate) the node X25519 secrets keypair and prepare the
    //    pubkey for the join request. The private key file is left on disk
    //    under `{secrets_dir}/node_secrets.key` for the daemon to load on
    //    startup (Task #18). We deliberately do not keep the private key in
    //    memory after this point — the join flow only needs the pubkey.
    let secrets_dir = zlayer_paths::ZLayerDirs::new(data_dir.clone()).secrets();
    let secrets_pubkey: Option<[u8; 32]> =
        match zlayer_secrets::load_or_generate_node_keypair(&secrets_dir) {
            Ok((_node_priv, node_pub)) => Some(*node_pub.as_bytes()),
            Err(e) => {
                warn!(
                    error = %e,
                    secrets_dir = %secrets_dir.display(),
                    "failed to load/generate node X25519 keypair; joining without \
                     replicated-secrets capability — node will not host secret \
                     ciphertext until the keypair is created and the join is retried"
                );
                None
            }
        };

    // 6. Send join request to the leader
    println!("  Contacting leader at {}...", token_data.api_endpoint);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let join_request = NodeJoinRequest {
        token: token.clone(),
        advertise_addr: advertise_addr.clone(),
        overlay_port,
        raft_port,
        wg_public_key: public_key.clone(),
        mode: mode.clone(),
        services: services.clone(),
        os: zlayer_spec::OsKind::from_rust_os(std::env::consts::OS),
        arch: zlayer_spec::ArchKind::from_rust_arch(std::env::consts::ARCH),
        secrets_pubkey,
    };

    let response = client
        .post(format!(
            "http://{}/api/v1/cluster/join",
            token_data.api_endpoint
        ))
        .json(&join_request)
        .send()
        .await
        .context("Failed to send join request to leader")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Join request failed: {status} - {body}");
    }

    let join_response: NodeJoinResponse = response
        .json()
        .await
        .context("Failed to parse join response")?;

    info!(
        node_id = %join_response.node_id,
        raft_node_id = join_response.raft_node_id,
        overlay_ip = %join_response.overlay_ip,
        "Received join response"
    );

    // 6a. Persist the secrets join material returned by the leader so the
    //     daemon can pick it up on startup (Task #18). Three pieces are
    //     written into `{secrets_dir}` (mode 0600 on Unix):
    //       - `node.jwt`         — node-scoped JWT for inter-node calls
    //       - `wrapped_dek.bin`  — sealed-box-wrapped cluster DEK
    //       - `dek_generation`   — decimal generation marker
    //     A legacy leader returns all three as `None`; in that case we log a
    //     warning and continue — the join still succeeds and Raft membership
    //     is not gated on the secrets material.
    if let (Some(jwt), Some(wrapped), Some(generation)) = (
        join_response.node_jwt.as_ref(),
        join_response.wrapped_dek.as_ref(),
        join_response.dek_generation,
    ) {
        persist_secrets_join_material(&secrets_dir, jwt, wrapped, generation)
            .context("failed to persist secrets join material")?;
    } else {
        warn!(
            "leader did not return secrets capability — node will run without \
             cluster-replicated secrets access; re-run join against a Phase-1+ \
             leader to enable replicated secrets"
        );
    }

    // 7. Save node configuration
    let node_config = NodeConfig {
        node_id: join_response.node_id.clone(),
        raft_node_id: join_response.raft_node_id,
        advertise_addr: advertise_addr.clone(),
        api_port,
        raft_port,
        overlay_port,
        overlay_cidr: token_data.overlay_cidr.clone(),
        wireguard_private_key: private_key,
        wireguard_public_key: public_key.clone(),
        is_leader: false,
        created_at: current_timestamp(),
    };
    save_node_config(&data_dir, &node_config).await?;

    // 8. Configure overlay with peers
    println!("  Configuring overlay network...");

    // Parse the overlay IP assigned by the leader
    let our_overlay_ip: std::net::IpAddr = join_response
        .overlay_ip
        .parse()
        .context("Invalid overlay IP in join response")?;

    // Parse the per-node slice CIDR assigned by the leader. Empty string
    // preserves pre-slice-aware behavior (allocator ranges over the full
    // cluster CIDR). Non-empty strings are required to parse; a malformed
    // slice from the leader is a hard error because the daemon would later
    // misconfigure overlay routing.
    let our_slice_cidr: Option<zlayer_overlay::ipnet::IpNet> =
        if join_response.slice_cidr.is_empty() {
            None
        } else {
            Some(
                join_response
                    .slice_cidr
                    .parse()
                    .context("Invalid slice CIDR in join response")?,
            )
        };

    // Extract the prefix length from the overlay CIDR (e.g. "10.200.0.0/16" -> 16)
    let overlay_prefix_len = token_data
        .overlay_cidr
        .split('/')
        .nth(1)
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(16);

    // Build the overlay IP with the network prefix for proper routing
    // e.g. "10.200.0.5/16" so all overlay peers are routable
    let overlay_ip_cidr = format!("{our_overlay_ip}/{overlay_prefix_len}");

    // Convert join response peers into zlayer_overlay PeerConfig entries
    let mut overlay_peers: Vec<zlayer_overlay::PeerConfig> = Vec::new();
    for peer in &join_response.peers {
        if peer.wg_public_key.is_empty() {
            warn!(
                peer_id = %peer.node_id,
                "Peer has no WireGuard public key, skipping overlay setup for this peer"
            );
            continue;
        }
        let peer_overlay_ip: std::net::IpAddr = match peer.overlay_ip.parse() {
            Ok(ip) => ip,
            Err(e) => {
                warn!(
                    peer_id = %peer.node_id,
                    overlay_ip = %peer.overlay_ip,
                    error = %e,
                    "Failed to parse peer overlay IP, skipping"
                );
                continue;
            }
        };
        let endpoint = format!("{}:{}", peer.advertise_addr, peer.overlay_port);
        overlay_peers.push(zlayer_overlay::PeerConfig::new(
            peer.node_id.clone(),
            peer.wg_public_key.clone(),
            endpoint,
            peer_overlay_ip,
        ));
        info!(
            peer_id = %peer.node_id,
            peer_addr = %peer.advertise_addr,
            peer_overlay_ip = %peer_overlay_ip,
            "Configured overlay peer"
        );
    }

    // Persist overlay bootstrap state so the daemon can reload it later
    // via OverlayBootstrap::load()
    let bootstrap_state = zlayer_overlay::BootstrapState {
        config: zlayer_overlay::BootstrapConfig {
            cidr: token_data.overlay_cidr.clone(),
            node_ip: our_overlay_ip,
            interface: zlayer_overlay::DEFAULT_INTERFACE_NAME.to_string(),
            port: overlay_port,
            private_key: node_config.wireguard_private_key.clone(),
            public_key: node_config.wireguard_public_key.clone(),
            is_leader: false,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            // Slice CIDR advertised by the leader (T8). Populated when the
            // leader is slice-aware; `None` for back-compat with pre-slice
            // leaders, in which case the agent allocator ranges over the
            // full cluster CIDR (legacy behavior).
            slice_cidr: our_slice_cidr,
        },
        peers: overlay_peers.clone(),
        allocator_state: None,
        slice_allocator_state: None,
    };
    let bootstrap_path = data_dir.join("overlay_bootstrap.json");
    let bootstrap_json = serde_json::to_string_pretty(&bootstrap_state)
        .context("Failed to serialize overlay bootstrap state")?;
    tokio::fs::write(&bootstrap_path, bootstrap_json)
        .await
        .with_context(|| {
            format!(
                "Failed to write overlay bootstrap state to {}",
                bootstrap_path.display()
            )
        })?;
    info!(path = %bootstrap_path.display(), "Saved overlay bootstrap state");

    // Create and configure the overlay transport (boringtun TUN device)
    // This gives the joining node immediate overlay connectivity.
    // The TUN device will be cleaned up when this process exits; the daemon
    // will re-create it from the persisted bootstrap state on next start.
    #[allow(clippy::needless_update)]
    let overlay_config = zlayer_overlay::OverlayConfig {
        local_endpoint: std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            overlay_port,
        ),
        private_key: node_config.wireguard_private_key.clone(),
        public_key: node_config.wireguard_public_key.clone(),
        overlay_cidr: overlay_ip_cidr,
        peer_discovery_interval: Duration::from_secs(30),
        ..zlayer_overlay::OverlayConfig::default()
    };

    // Convert PeerConfig to PeerInfo for the transport layer
    let peer_infos: Vec<zlayer_overlay::PeerInfo> = overlay_peers
        .iter()
        .filter_map(|p| match p.to_peer_info() {
            Ok(info) => Some(info),
            Err(e) => {
                warn!(peer = %p.node_id, error = %e, "Failed to convert peer info, skipping");
                None
            }
        })
        .collect();

    let interface_name = zlayer_overlay::DEFAULT_INTERFACE_NAME.to_string();
    let mut transport = zlayer_overlay::OverlayTransport::new(overlay_config, interface_name);

    match transport.create_interface().await {
        Ok(()) => {
            match transport.configure(&peer_infos).await {
                Ok(()) => {
                    info!(
                        overlay_ip = %our_overlay_ip,
                        peer_count = peer_infos.len(),
                        "Overlay network interface configured successfully"
                    );
                    println!(
                        "  Overlay network up: {} with {} peer(s)",
                        our_overlay_ip,
                        peer_infos.len()
                    );
                    // Keep the transport alive until we finish printing the
                    // success message below; it will be cleaned up on exit.
                    // The daemon re-creates the overlay from persisted state.
                    drop(transport);
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to configure overlay transport. \
                         The overlay bootstrap state has been saved and the daemon will \
                         retry on next start."
                    );
                    println!("  Warning: Overlay interface created but configuration failed: {e}");
                }
            }
        }
        Err(e) => {
            // Non-fatal: the overlay state is persisted, the daemon will set
            // it up when it starts. This commonly fails in unprivileged
            // environments or CI.
            warn!(
                error = %e,
                "Failed to create overlay network interface. \
                 Requires CAP_NET_ADMIN capability. The overlay bootstrap state has \
                 been saved and the daemon will create the interface on next start."
            );
            println!(
                "  Warning: Could not create overlay interface ({e}). \
                 The daemon will set it up on next start."
            );
        }
    }

    // TODO: Existing peers also need to add this new node as a peer. Currently
    // requires periodic reconciliation from Raft state or an explicit push
    // notification from the leader after processing the join request.

    // 8. Detect GPUs on this node
    println!("  Detecting GPUs...");
    let detected_gpus = zlayer_agent::detect_gpus();
    if detected_gpus.is_empty() {
        info!("No GPUs detected on this node");
    } else {
        info!(
            gpu_count = detected_gpus.len(),
            "Detected GPUs on this node"
        );
        for gpu in &detected_gpus {
            info!(
                pci_bus_id = %gpu.pci_bus_id,
                vendor = %gpu.vendor,
                model = %gpu.model,
                memory_mb = gpu.memory_mb,
                device_path = %gpu.device_path,
                "Detected GPU"
            );
        }
    }

    // 9. Print success message
    println!();
    println!("Successfully joined cluster!");
    println!();
    println!("Node ID:        {}", join_response.node_id);
    println!("Raft Node ID:   {}", join_response.raft_node_id);
    println!("Overlay IP:     {}", join_response.overlay_ip);
    if !join_response.slice_cidr.is_empty() {
        println!("Slice CIDR:     {}", join_response.slice_cidr);
    }
    println!("Mode:           {mode}");
    if let Some(svcs) = services {
        println!("Services:       {}", svcs.join(", "));
    }
    println!("Peers:          {}", join_response.peers.len());
    if detected_gpus.is_empty() {
        println!("GPUs:           None detected");
    } else {
        println!("GPUs:           {} detected", detected_gpus.len());
        for gpu in &detected_gpus {
            println!(
                "  - {} ({}, {} MB VRAM) at {}",
                gpu.model, gpu.vendor, gpu.memory_mb, gpu.pci_bus_id
            );
        }
    }
    println!();
    println!("Run 'zlayer deploy' or 'zlayer up' with a spec file to start processing workloads.");

    Ok(())
}

/// List all nodes in the cluster
pub(crate) async fn handle_node_list(output: String, cli_data_dir: &std::path::Path) -> Result<()> {
    use std::time::Duration;

    // Try to load local node config to get API endpoint
    let data_dir = cli_data_dir.to_path_buf();
    let node_config = load_or_init_node_config(&data_dir).await?;

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    // Fetch node list from API
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    let response = client
        .get(format!("http://{api_endpoint}/api/v1/cluster/nodes"))
        .send()
        .await;

    let nodes: Vec<NodeStatus> = match response {
        Ok(resp) if resp.status().is_success() => {
            resp.json().await.unwrap_or_else(|_| {
                // Return at least the local node if we can't parse
                vec![NodeStatus {
                    id: node_config.node_id.clone(),
                    address: format!("{}:{}", node_config.advertise_addr, node_config.api_port),
                    status: "unknown".to_string(),
                    mode: if node_config.is_leader {
                        "leader".to_string()
                    } else {
                        "worker".to_string()
                    },
                    services: vec![],
                    is_leader: node_config.is_leader,
                    role: if node_config.is_leader {
                        "leader".to_string()
                    } else {
                        "learner".to_string()
                    },
                }]
            })
        }
        _ => {
            // API not available, show local node info
            warn!("Could not connect to cluster API. Showing local node only.");
            vec![NodeStatus {
                id: node_config.node_id.clone(),
                address: format!("{}:{}", node_config.advertise_addr, node_config.api_port),
                status: "local".to_string(),
                mode: if node_config.is_leader {
                    "leader".to_string()
                } else {
                    "worker".to_string()
                },
                services: vec![],
                is_leader: node_config.is_leader,
                role: if node_config.is_leader {
                    "leader".to_string()
                } else {
                    "learner".to_string()
                },
            }]
        }
    };

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&nodes)?);
    } else {
        // Table format
        println!(
            "{:<36} {:<20} {:<10} {:<10} {:<20} SERVICES",
            "NODE ID", "ADDRESS", "STATUS", "MODE", "ROLE"
        );
        println!("{}", "-".repeat(114));

        for node in nodes {
            let services = if node.services.is_empty() {
                "-".to_string()
            } else {
                let s = node.services.join(", ");
                if s.len() > 20 {
                    format!("{}...", &s[..17])
                } else {
                    s
                }
            };

            // Map the server-provided Raft role string to a friendly label.
            // README §400 ("Adding Worker Nodes") uses "Worker" for the
            // non-leader/learner case, so we follow that convention here.
            let base_role = match node.role.as_str() {
                "leader" => "Leader",
                "voter" => "Voter",
                "learner" => "Worker",
                "" => "Unknown",
                other => other,
            };

            // Decorate the role with the node's lifecycle status when it's
            // not in the healthy "ready" / "local" state so operators
            // immediately see drains and dead nodes.
            let role_display = match node.status.as_str() {
                "draining" => format!("{base_role} (draining)"),
                "dead" => format!("{base_role} [DEAD]"),
                _ => base_role.to_string(),
            };

            println!(
                "{:<36} {:<20} {:<10} {:<10} {:<20} {}",
                node.id, node.address, node.status, node.mode, role_display, services
            );
        }
    }

    Ok(())
}

/// Show detailed status of a node
pub(crate) async fn handle_node_status(
    node_id: Option<String>,
    cli_data_dir: &std::path::Path,
) -> Result<()> {
    let data_dir = cli_data_dir.to_path_buf();
    let node_config = load_or_init_node_config(&data_dir).await?;

    // If no node_id specified, show this node
    let target_id = node_id.unwrap_or(node_config.node_id.clone());

    if target_id == node_config.node_id {
        // Show local node detailed status
        println!("Node Status");
        println!("{}", "=".repeat(50));
        println!();
        println!("Node ID:            {}", node_config.node_id);
        println!("Raft Node ID:       {}", node_config.raft_node_id);
        println!(
            "Role:               {}",
            if node_config.is_leader {
                "Leader"
            } else {
                "Worker"
            }
        );
        println!("Created At:         {}", node_config.created_at);
        println!();
        println!("Network Configuration:");
        println!("  Advertise Address: {}", node_config.advertise_addr);
        println!("  API Port:          {}", node_config.api_port);
        println!("  Raft Port:         {}", node_config.raft_port);
        println!("  Overlay Port:      {}", node_config.overlay_port);
        println!("  Overlay CIDR:      {}", node_config.overlay_cidr);
        println!();
        println!("Overlay:");
        println!("  Public Key:        {}", node_config.wireguard_public_key);
        println!();
        println!("Endpoints:");
        println!(
            "  API:   http://{}:{}",
            node_config.advertise_addr, node_config.api_port
        );
        println!(
            "  Raft:  {}:{}",
            node_config.advertise_addr, node_config.raft_port
        );
        println!(
            "  WG:    {}:{}/udp",
            node_config.advertise_addr, node_config.overlay_port
        );
    } else {
        // Fetch remote node status via API
        use std::time::Duration;
        let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to create HTTP client")?;

        let response = client
            .get(format!(
                "http://{api_endpoint}/api/v1/cluster/nodes/{target_id}"
            ))
            .send()
            .await
            .context("Failed to fetch node status")?;

        if !response.status().is_success() {
            anyhow::bail!("Node '{target_id}' not found in cluster");
        }

        let status: NodeStatus = response
            .json()
            .await
            .context("Failed to parse node status")?;

        println!("Node Status");
        println!("{}", "=".repeat(50));
        println!();
        println!("Node ID:    {}", status.id);
        println!("Address:    {}", status.address);
        println!("Status:     {}", status.status);
        println!("Mode:       {}", status.mode);
        println!(
            "Leader:     {}",
            if status.is_leader { "Yes" } else { "No" }
        );
        if !status.services.is_empty() {
            println!("Services:   {}", status.services.join(", "));
        }
    }

    Ok(())
}

/// Remove a node from the cluster
pub(crate) async fn handle_node_remove(
    node_id: String,
    force: bool,
    cli_data_dir: &std::path::Path,
) -> Result<()> {
    use std::time::Duration;

    let data_dir = cli_data_dir.to_path_buf();
    let node_config = load_or_init_node_config(&data_dir).await?;

    // Cannot remove yourself
    if node_id == node_config.node_id {
        anyhow::bail!(
            "Cannot remove the current node. To leave the cluster, stop the node and remove its data directory."
        );
    }

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    println!("Removing node '{node_id}' from cluster...");
    if force {
        warn!("Force removal enabled - services will not be migrated");
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let mut url = format!("http://{api_endpoint}/api/v1/cluster/nodes/{node_id}");
    if force {
        url.push_str("?force=true");
    }

    let response = client
        .delete(&url)
        .send()
        .await
        .context("Failed to send remove request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to remove node: {status} - {body}");
    }

    println!("Node '{node_id}' removed successfully.");
    if !force {
        println!("Services have been migrated to other nodes.");
    }

    Ok(())
}

/// Set node resource mode
pub(crate) async fn handle_node_set_mode(
    node_id: String,
    mode: String,
    services: Option<Vec<String>>,
    cli_data_dir: &std::path::Path,
) -> Result<()> {
    use std::time::Duration;

    // Validate mode
    let valid_modes = ["full", "dedicated", "replicate"];
    if !valid_modes.contains(&mode.as_str()) {
        anyhow::bail!(
            "Invalid mode '{}'. Valid modes are: {}",
            mode,
            valid_modes.join(", ")
        );
    }

    // Validate mode-service combination
    if (mode == "dedicated" || mode == "replicate") && services.is_none() {
        anyhow::bail!("Mode '{mode}' requires --services to be specified");
    }

    let data_dir = cli_data_dir.to_path_buf();
    let node_config = load_or_init_node_config(&data_dir).await?;

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    println!("Setting mode for node '{node_id}'...");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    #[allow(clippy::items_after_statements)]
    #[derive(Serialize)]
    struct SetModeRequest {
        mode: String,
        services: Option<Vec<String>>,
    }

    let request = SetModeRequest {
        mode: mode.clone(),
        services: services.clone(),
    };

    let response = client
        .put(format!(
            "http://{api_endpoint}/api/v1/cluster/nodes/{node_id}/mode"
        ))
        .json(&request)
        .send()
        .await
        .context("Failed to send set-mode request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to set node mode: {status} - {body}");
    }

    println!("Node '{node_id}' mode set to '{mode}'.");
    if let Some(svcs) = services {
        println!("Services: {}", svcs.join(", "));
    }

    Ok(())
}

/// Add label to a node
pub(crate) async fn handle_node_label(
    node_id: String,
    label: String,
    cli_data_dir: &std::path::Path,
) -> Result<()> {
    use std::time::Duration;

    // Parse label
    let parts: Vec<&str> = label.splitn(2, '=').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid label format '{label}'. Expected key=value format.");
    }
    let (key, value) = (parts[0], parts[1]);

    // Validate label key
    if key.is_empty() {
        anyhow::bail!("Label key cannot be empty");
    }

    let data_dir = cli_data_dir.to_path_buf();
    let node_config = load_or_init_node_config(&data_dir).await?;

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    println!("Adding label to node '{node_id}'...");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    #[allow(clippy::items_after_statements)]
    #[derive(Serialize)]
    struct AddLabelRequest {
        key: String,
        value: String,
    }

    let request = AddLabelRequest {
        key: key.to_string(),
        value: value.to_string(),
    };

    let response = client
        .post(format!(
            "http://{api_endpoint}/api/v1/cluster/nodes/{node_id}/labels"
        ))
        .json(&request)
        .send()
        .await
        .context("Failed to send label request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to add label: {status} - {body}");
    }

    println!("Label '{key}={value}' added to node '{node_id}'.");

    Ok(())
}

/// Force this node to become cluster leader (disaster recovery).
///
/// Sends a POST to the local API server's force-leader endpoint. Use this when
/// the original leader is permanently lost and the surviving learner needs to
/// take over the cluster.
async fn handle_node_force_leader(api_addr: String) -> Result<()> {
    use std::time::Duration;

    println!("WARNING: Force-leader will make this node the new cluster leader.");
    println!("         Only use when the original leader is permanently lost.\n");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    let url = format!("http://{api_addr}/api/v1/cluster/force-leader");
    let resp = client
        .post(&url)
        .json(&serde_json::json!({"confirm": "CONFIRM_FORCE_LEADER"}))
        .send()
        .await
        .context("Failed to connect to API server")?;

    if resp.status().is_success() {
        let body: serde_json::Value = resp
            .json()
            .await
            .context("Failed to parse force-leader response")?;
        println!("Force-leader initiated successfully.");
        if let Some(nodes) = body.get("preserved_nodes") {
            println!(
                "  Preserved {} nodes, {} services",
                nodes,
                body.get("preserved_services")
                    .unwrap_or(&serde_json::Value::Null)
            );
        }
        println!("\nRestart the daemon to complete recovery:");
        println!("  systemctl restart zlayer");
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Force-leader failed: {status} - {body}");
    }

    Ok(())
}

/// Try to discover the deployment name from a `.zlayer.yml` / `*.zlayer.yml` spec
/// in the current working directory.  Returns `None` if nothing is found.
fn try_discover_deployment_name() -> Option<String> {
    let spec_path = crate::util::discover_spec_path(None).ok()?;
    let spec = crate::util::parse_spec(&spec_path).ok()?;
    Some(spec.deployment)
}

/// Handle node generate-join-token command
///
/// When `api` or `deployment` are `None`, the function attempts to infer them:
///   - `api`: read from the node config at `data_dir/node_config.json`.
///     If no config exists, auto-initialize the node with sensible defaults.
///   - `deployment`: discover from `*.zlayer.yml` / `.zlayer.yml` in the cwd.
///     Falls back to `"default"` if nothing is found.
pub(crate) async fn handle_node_generate_join_token(
    deployment: Option<String>,
    api: Option<String>,
    service: Option<String>,
    data_dir: PathBuf,
) -> Result<()> {
    use base64::Engine;

    // --- Resolve the API endpoint ---
    let api_endpoint = if let Some(a) = api {
        a
    } else {
        let node_config = load_or_init_node_config(&data_dir).await?;
        format!(
            "http://{}:{}",
            node_config.advertise_addr, node_config.api_port
        )
    };

    // --- Resolve the deployment name ---
    let deployment_name = match deployment {
        Some(d) => d,
        None => {
            // Try to discover from spec files in the current directory
            if let Some(name) = try_discover_deployment_name() {
                info!(deployment = %name, "Auto-discovered deployment name from spec");
                name
            } else {
                warn!("No deployment spec found in current directory, using 'default'");
                "default".to_string()
            }
        }
    };

    let token_data = serde_json::json!({
        "deployment": deployment_name,
        "api_endpoint": api_endpoint,
        "service": service,
    });

    let json = serde_json::to_string(&token_data)?;
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&json);

    println!("Join Token Generated");
    println!("====================\n");
    println!("Deployment: {deployment_name}");
    println!("API: {api_endpoint}");
    if let Some(svc) = &service {
        println!("Service: {svc}");
    }
    println!("\nToken:");
    println!("{token}");
    println!("\nUsage:");
    println!("  zlayer node join <leader-addr> --token {token}");

    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Commands, NodeCommands};
    use clap::Parser;
    use std::path::PathBuf;
    use zlayer_paths::ZLayerDirs;

    #[test]
    fn test_cli_node_init_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "init", "--advertise-addr", "10.0.0.1"])
            .unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Init {
                advertise_addr,
                api_port,
                raft_port,
                overlay_port,
                data_dir,
                overlay_cidr,
                ..
            })) => {
                assert_eq!(advertise_addr, "10.0.0.1");
                assert_eq!(api_port, 3669);
                assert_eq!(raft_port, 9000);
                assert_eq!(overlay_port, zlayer_core::DEFAULT_WG_PORT);
                assert!(data_dir.is_none()); // resolved at runtime
                assert_eq!(overlay_cidr, "10.200.0.0/16");
            }
            _ => panic!("Expected Node Init command"),
        }
    }

    #[test]
    fn test_cli_node_init_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "init",
            "--advertise-addr",
            "192.168.1.100",
            "--api-port",
            "9090",
            "--raft-port",
            "9001",
            "--overlay-port",
            "51821",
            "--data-dir",
            "/custom/data",
            "--overlay-cidr",
            "10.100.0.0/16",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Init {
                advertise_addr,
                api_port,
                raft_port,
                overlay_port,
                data_dir,
                overlay_cidr,
                ..
            })) => {
                assert_eq!(advertise_addr, "192.168.1.100");
                assert_eq!(api_port, 9090);
                assert_eq!(raft_port, 9001);
                assert_eq!(overlay_port, 51821);
                assert_eq!(data_dir, Some(PathBuf::from("/custom/data")));
                assert_eq!(overlay_cidr, "10.100.0.0/16");
            }
            _ => panic!("Expected Node Init command"),
        }
    }

    #[test]
    fn test_cli_node_join_command() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "join",
            "10.0.0.1:3669",
            "--token",
            "abc123",
            "--advertise-addr",
            "10.0.0.2",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Join {
                leader_addr,
                token,
                advertise_addr,
                mode,
                services,
                ..
            })) => {
                assert_eq!(leader_addr, "10.0.0.1:3669");
                assert_eq!(token, "abc123");
                assert_eq!(advertise_addr, "10.0.0.2");
                assert_eq!(mode, "full");
                assert!(services.is_none());
            }
            _ => panic!("Expected Node Join command"),
        }
    }

    #[test]
    fn test_cli_node_join_command_with_services() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "join",
            "10.0.0.1:3669",
            "--token",
            "abc123",
            "--advertise-addr",
            "10.0.0.2",
            "--mode",
            "replicate",
            "--services",
            "api",
            "--services",
            "web",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Join { mode, services, .. })) => {
                assert_eq!(mode, "replicate");
                assert_eq!(services, Some(vec!["api".to_string(), "web".to_string()]));
            }
            _ => panic!("Expected Node Join command"),
        }
    }

    #[test]
    fn test_cli_node_list_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "list"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::List { output })) => {
                assert_eq!(output, "table");
            }
            _ => panic!("Expected Node List command"),
        }
    }

    #[test]
    fn test_cli_node_list_command_json() {
        let cli = Cli::try_parse_from(["zlayer", "node", "list", "--output", "json"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::List { output })) => {
                assert_eq!(output, "json");
            }
            _ => panic!("Expected Node List command"),
        }
    }

    #[test]
    fn test_cli_node_status_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "status"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Status { node_id })) => {
                assert!(node_id.is_none());
            }
            _ => panic!("Expected Node Status command"),
        }
    }

    #[test]
    fn test_cli_node_status_command_with_id() {
        let cli = Cli::try_parse_from(["zlayer", "node", "status", "node-abc-123"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Status { node_id })) => {
                assert_eq!(node_id, Some("node-abc-123".to_string()));
            }
            _ => panic!("Expected Node Status command"),
        }
    }

    #[test]
    fn test_cli_node_remove_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "remove", "node-123"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Remove { node_id, force })) => {
                assert_eq!(node_id, "node-123");
                assert!(!force);
            }
            _ => panic!("Expected Node Remove command"),
        }
    }

    #[test]
    fn test_cli_node_remove_command_force() {
        let cli = Cli::try_parse_from(["zlayer", "node", "remove", "--force", "node-123"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Remove { node_id, force })) => {
                assert_eq!(node_id, "node-123");
                assert!(force);
            }
            _ => panic!("Expected Node Remove command"),
        }
    }

    #[test]
    fn test_cli_node_set_mode_command() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "set-mode",
            "node-123",
            "--mode",
            "dedicated",
            "--services",
            "api",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::SetMode {
                node_id,
                mode,
                services,
            })) => {
                assert_eq!(node_id, "node-123");
                assert_eq!(mode, "dedicated");
                assert_eq!(services, Some(vec!["api".to_string()]));
            }
            _ => panic!("Expected Node SetMode command"),
        }
    }

    #[test]
    fn test_cli_node_label_command() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "label",
            "node-123",
            "environment=production",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Label { node_id, label })) => {
                assert_eq!(node_id, "node-123");
                assert_eq!(label, "environment=production");
            }
            _ => panic!("Expected Node Label command"),
        }
    }

    #[test]
    fn test_generate_secure_token() {
        let token1 = super::generate_secure_token();
        let token2 = super::generate_secure_token();

        // Tokens should be different
        assert_ne!(token1, token2);

        // Tokens should be base64 encoded (43 chars for 32 bytes URL-safe no padding)
        assert_eq!(token1.len(), 43);
        assert_eq!(token2.len(), 43);
    }

    #[test]
    fn test_generate_node_id() {
        let id1 = super::generate_node_id();
        let id2 = super::generate_node_id();

        // IDs should be different
        assert_ne!(id1, id2);

        // IDs should be valid UUIDs (36 chars with dashes)
        assert_eq!(id1.len(), 36);
        assert_eq!(id2.len(), 36);
    }

    #[test]
    fn test_join_token_roundtrip() {
        let token = super::generate_join_token_data(
            "192.168.1.1",
            3669,
            9000,
            "test-public-key",
            "10.200.0.0/16",
        )
        .unwrap();

        let parsed = super::parse_cluster_join_token(&token).unwrap();

        assert_eq!(parsed.api_endpoint, "192.168.1.1:3669");
        assert_eq!(parsed.raft_endpoint, "192.168.1.1:9000");
        assert_eq!(parsed.leader_wg_pubkey, "test-public-key");
        assert_eq!(parsed.overlay_cidr, "10.200.0.0/16");
        assert!(!parsed.auth_secret.is_empty());
    }

    #[test]
    fn test_parse_invalid_join_token() {
        // Invalid base64
        let result = super::parse_cluster_join_token("not-valid-base64!!!");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("base64"));

        // Valid base64 but invalid JSON
        let invalid_json = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            "not json",
        );
        let result = super::parse_cluster_join_token(&invalid_json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("JSON"));
    }

    /// On a non-admin Windows shell, `handle_node_init` should refuse to do any
    /// privileged work and surface an actionable "Run as Administrator" error
    /// **before** touching the filesystem, network, or subprocess APIs. The
    /// test executes only when the current process is not elevated so it never
    /// interferes with a genuine init on a developer machine.
    ///
    /// Also validates that the `#[cfg(windows)]` body compiles with the new
    /// `ConsentMode` parameter — a regression guard for the H-1 wiring.
    #[cfg(windows)]
    #[tokio::test]
    async fn handle_node_init_non_admin_errors_early() {
        // Skip if the test process happens to be running elevated — the
        // admin-check would then fall through and attempt real side effects.
        if zlayer_paths::is_root() {
            eprintln!("test skipped: running as Administrator");
            return;
        }
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("handle-node-init-non-admin-errors-early-")
            .expect("tempdir");
        let err = super::handle_node_init(
            "127.0.0.1".to_string(),
            3669,
            9000,
            zlayer_core::DEFAULT_WG_PORT,
            tmp.path().to_path_buf(),
            "10.200.0.0/16".to_string(),
            crate::ui::consent::ConsentMode::No,
        )
        .await
        .expect_err("non-admin init must error");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("Administrator"),
            "error should cite Administrator: {msg}"
        );
        // Filesystem must be untouched — no partial state written.
        assert!(
            !tmp.path().join("node_config.json").exists(),
            "node_config.json must not be written before admin check"
        );
    }

    /// Companion to `handle_node_init_non_admin_errors_early` for the join
    /// path. Validates the same admin precedence guard on H-2.
    #[cfg(windows)]
    #[tokio::test]
    async fn handle_node_join_non_admin_errors_early() {
        if zlayer_paths::is_root() {
            eprintln!("test skipped: running as Administrator");
            return;
        }
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("handle-node-join-non-admin-errors-early-")
            .expect("tempdir");
        let err = super::handle_node_join(
            "127.0.0.1:3669".to_string(),
            "dummy-token".to_string(),
            "127.0.0.1".to_string(),
            "full".to_string(),
            None,
            3669,
            9000,
            zlayer_core::DEFAULT_WG_PORT,
            tmp.path().to_path_buf(),
            crate::ui::consent::ConsentMode::No,
        )
        .await
        .expect_err("non-admin join must error");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("Administrator"),
            "error should cite Administrator: {msg}"
        );
        assert!(
            !tmp.path().join("node_config.json").exists(),
            "node_config.json must not be written before admin check"
        );
    }

    /// `write_secure` must round-trip exactly the bytes it is given. This is
    /// the contract relied on by `persist_secrets_join_material` for the
    /// node JWT (UTF-8) and the wrapped DEK (binary).
    #[test]
    fn write_secure_round_trip() {
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("write-secure-round-trip-")
            .expect("tempdir");
        let path = tmp.path().join("secret.bin");

        // Use a payload with explicit non-UTF8 bytes (0xFF, 0x00) to prove
        // the helper does not corrupt binary content.
        let payload: &[u8] = &[0x00, 0x01, 0xFE, 0xFF, b'h', b'i'];
        super::write_secure(&path, payload).expect("write_secure");

        let read_back = std::fs::read(&path).expect("read back");
        assert_eq!(read_back, payload);
    }

    /// On Unix, `write_secure` must create the file with mode 0600 so that
    /// a misconfigured umask cannot widen permissions. This is the same
    /// invariant that `zlayer_secrets::load_or_generate_node_keypair`
    /// enforces for `node_secrets.key`.
    #[cfg(unix)]
    #[test]
    fn write_secure_sets_mode_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = ZLayerDirs::system_default()
            .scratch_dir("write-secure-sets-mode-0600-on-unix-")
            .expect("tempdir");
        let path = tmp.path().join("secret.bin");

        super::write_secure(&path, b"sensitive").expect("write_secure");

        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "expected mode 0600, got {mode:o}");
    }

    /// `write_secure` must overwrite an existing file's content **and** ensure
    /// its mode is reset to 0600 — covering the case where a pre-existing file
    /// at the path was created with a more permissive mode (`mode_t` passed
    /// to `OpenOptions::mode` is only honored on file creation, not truncation).
    #[cfg(unix)]
    #[test]
    fn write_secure_overwrite_resets_mode_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = ZLayerDirs::system_default()
            .scratch_dir("write-secure-overwrite-resets-mode-0600-on-unix-")
            .expect("tempdir");
        let path = tmp.path().join("secret.bin");

        // Pre-create with a wide-open mode and unrelated content.
        std::fs::write(&path, b"old").expect("seed file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("seed perms");

        super::write_secure(&path, b"new").expect("write_secure overwrite");

        // Content was replaced.
        assert_eq!(std::fs::read(&path).expect("read"), b"new");

        // Mode was tightened back down to 0600 by the belt-and-suspenders
        // `set_permissions` call inside `write_secure`.
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "overwrite must reset mode to 0600, got {mode:o}"
        );
    }
}
