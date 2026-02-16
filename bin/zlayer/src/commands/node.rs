use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

// =============================================================================
// Node management types
// =============================================================================

/// Node configuration stored on disk
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
    /// Overlay network port (WireGuard protocol)
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
    /// Existing peers in the cluster
    peers: Vec<PeerNode>,
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
}

// =============================================================================
// Helper functions
// =============================================================================

/// Generate a secure random token
fn generate_secure_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.gen();
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
}

/// Generate a unique node ID
pub(crate) fn generate_node_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Get current timestamp as ISO 8601 string
pub(crate) fn current_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Save node configuration to disk
pub(crate) async fn save_node_config(data_dir: &Path, config: &NodeConfig) -> Result<()> {
    let config_path = data_dir.join("node_config.json");
    let content =
        serde_json::to_string_pretty(config).context("Failed to serialize node config")?;
    tokio::fs::write(&config_path, content)
        .await
        .with_context(|| format!("Failed to write node config to {}", config_path.display()))?;
    info!(path = %config_path.display(), "Saved node configuration");
    Ok(())
}

/// Load node configuration from disk
pub(crate) async fn load_node_config(data_dir: &Path) -> Result<NodeConfig> {
    let config_path = data_dir.join("node_config.json");
    let content = tokio::fs::read_to_string(&config_path)
        .await
        .with_context(|| format!("Failed to read node config from {}", config_path.display()))?;
    let config: NodeConfig =
        serde_json::from_str(&content).context("Failed to parse node config")?;
    Ok(config)
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
        api_endpoint: format!("{}:{}", advertise_addr, api_port),
        raft_endpoint: format!("{}:{}", advertise_addr, raft_port),
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

// =============================================================================
// Command handlers
// =============================================================================

use crate::cli::NodeCommands;

/// Top-level dispatcher for node subcommands
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
        } => {
            let resolved_dir = data_dir
                .clone()
                .unwrap_or_else(|| cli_data_dir.to_path_buf());
            handle_node_init(
                advertise_addr.clone(),
                *api_port,
                *raft_port,
                *overlay_port,
                resolved_dir,
                overlay_cidr.clone(),
            )
            .await
        }
        NodeCommands::Join {
            leader_addr,
            token,
            advertise_addr,
            mode,
            services,
        } => {
            handle_node_join(
                leader_addr.clone(),
                token.clone(),
                advertise_addr.clone(),
                mode.clone(),
                services.clone(),
                cli_data_dir.to_path_buf(),
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

/// Initialize this node as cluster leader
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
        .map_err(|e| anyhow::anyhow!("Failed to generate overlay keys: {}", e))?;
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
        address: format!("{}:{}", advertise_addr, raft_port),
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

    // Register the leader node in the Raft state machine with overlay metadata
    let leader_overlay_ip = "10.200.0.1".to_string();
    raft.propose(zlayer_scheduler::Request::RegisterNode {
        node_id: raft_node_id,
        address: format!("{}:{}", advertise_addr, raft_port),
        wg_public_key: public_key.clone(),
        overlay_ip: leader_overlay_ip,
        overlay_port,
        advertise_addr: advertise_addr.clone(),
        api_port,
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
    println!("Node ID:        {}", node_id);
    println!("Raft Node ID:   {}", raft_node_id);
    println!("API Server:     http://{}:{}", advertise_addr, api_port);
    println!("Raft Address:   {}:{}", advertise_addr, raft_port);
    println!("Overlay Port:   {}", overlay_port);
    println!("Overlay CIDR:   {}", overlay_cidr);
    println!("WG Public Key:  {}", public_key);
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
        "  zlayer node join {}:{} --token {} --advertise-addr <NODE_IP>",
        advertise_addr, api_port, join_token
    );
    println!();
    println!("Note: The API server starts automatically when deploying with 'zlayer deploy' or 'zlayer up'.");

    Ok(())
}

/// Join an existing cluster as a worker node
pub(crate) async fn handle_node_join(
    leader_addr: String,
    token: String,
    advertise_addr: String,
    mode: String,
    services: Option<Vec<String>>,
    data_dir_override: PathBuf,
) -> Result<()> {
    use std::time::Duration;
    use zlayer_overlay::OverlayTransport;

    println!("Joining ZLayer cluster at {}...", leader_addr);

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
        .map_err(|e| anyhow::anyhow!("Failed to generate overlay keys: {}", e))?;

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

    // 5. Send join request to the leader
    println!("  Contacting leader at {}...", token_data.api_endpoint);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    // Parse overlay port from advertise address or use default
    let overlay_port: u16 = 51820;
    let raft_port: u16 = 9000;

    let join_request = NodeJoinRequest {
        token: token.clone(),
        advertise_addr: advertise_addr.clone(),
        overlay_port,
        raft_port,
        wg_public_key: public_key.clone(),
        mode: mode.clone(),
        services: services.clone(),
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
        anyhow::bail!("Join request failed: {} - {}", status, body);
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

    // 6. Save node configuration
    let node_config = NodeConfig {
        node_id: join_response.node_id.clone(),
        raft_node_id: join_response.raft_node_id,
        advertise_addr: advertise_addr.clone(),
        api_port: 3669, // Default for workers
        raft_port,
        overlay_port,
        overlay_cidr: token_data.overlay_cidr.clone(),
        wireguard_private_key: private_key,
        wireguard_public_key: public_key.clone(),
        is_leader: false,
        created_at: current_timestamp(),
    };
    save_node_config(&data_dir, &node_config).await?;

    // 7. Configure overlay with peers
    println!("  Configuring overlay network...");

    // Parse the overlay IP assigned by the leader
    let our_overlay_ip: std::net::Ipv4Addr = join_response
        .overlay_ip
        .parse()
        .context("Invalid overlay IP in join response")?;

    // Extract the prefix length from the overlay CIDR (e.g. "10.200.0.0/16" -> 16)
    let overlay_prefix_len = token_data
        .overlay_cidr
        .split('/')
        .nth(1)
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(16);

    // Build the overlay IP with the network prefix for proper routing
    // e.g. "10.200.0.5/16" so all overlay peers are routable
    let overlay_ip_cidr = format!("{}/{}", our_overlay_ip, overlay_prefix_len);

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
        let peer_overlay_ip: std::net::Ipv4Addr = match peer.overlay_ip.parse() {
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
        },
        peers: overlay_peers.clone(),
        allocator_state: None,
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
    let overlay_config = zlayer_overlay::OverlayConfig {
        local_endpoint: std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)),
            overlay_port,
        ),
        private_key: node_config.wireguard_private_key.clone(),
        public_key: node_config.wireguard_public_key.clone(),
        overlay_cidr: overlay_ip_cidr,
        peer_discovery_interval: Duration::from_secs(30),
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
                    println!(
                        "  Warning: Overlay interface created but configuration failed: {}",
                        e
                    );
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
                "  Warning: Could not create overlay interface ({}). \
                 The daemon will set it up on next start.",
                e
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
    println!("Mode:           {}", mode);
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
    let node_config = match load_node_config(&data_dir).await {
        Ok(config) => config,
        Err(_) => {
            anyhow::bail!(
                "Node not initialized. Run 'zlayer node init' or 'zlayer node join' first."
            );
        }
    };

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    // Fetch node list from API
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    let response = client
        .get(format!("http://{}/api/v1/cluster/nodes", api_endpoint))
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
            }]
        }
    };

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&nodes)?);
    } else {
        // Table format
        println!(
            "{:<36} {:<20} {:<10} {:<10} {:<6} SERVICES",
            "NODE ID", "ADDRESS", "STATUS", "MODE", "LEADER"
        );
        println!("{}", "-".repeat(100));

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
            let leader_marker = if node.is_leader { "*" } else { "" };
            println!(
                "{:<36} {:<20} {:<10} {:<10} {:<6} {}",
                node.id, node.address, node.status, node.mode, leader_marker, services
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
    let node_config = match load_node_config(&data_dir).await {
        Ok(config) => config,
        Err(_) => {
            anyhow::bail!(
                "Node not initialized. Run 'zlayer node init' or 'zlayer node join' first."
            );
        }
    };

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
                "http://{}/api/v1/cluster/nodes/{}",
                api_endpoint, target_id
            ))
            .send()
            .await
            .context("Failed to fetch node status")?;

        if !response.status().is_success() {
            anyhow::bail!("Node '{}' not found in cluster", target_id);
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
    let node_config = match load_node_config(&data_dir).await {
        Ok(config) => config,
        Err(_) => {
            anyhow::bail!(
                "Node not initialized. Run 'zlayer node init' or 'zlayer node join' first."
            );
        }
    };

    // Cannot remove yourself
    if node_id == node_config.node_id {
        anyhow::bail!(
            "Cannot remove the current node. To leave the cluster, stop the node and remove its data directory."
        );
    }

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    println!("Removing node '{}' from cluster...", node_id);
    if force {
        warn!("Force removal enabled - services will not be migrated");
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let mut url = format!("http://{}/api/v1/cluster/nodes/{}", api_endpoint, node_id);
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
        anyhow::bail!("Failed to remove node: {} - {}", status, body);
    }

    println!("Node '{}' removed successfully.", node_id);
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
        anyhow::bail!("Mode '{}' requires --services to be specified", mode);
    }

    let data_dir = cli_data_dir.to_path_buf();
    let node_config = match load_node_config(&data_dir).await {
        Ok(config) => config,
        Err(_) => {
            anyhow::bail!(
                "Node not initialized. Run 'zlayer node init' or 'zlayer node join' first."
            );
        }
    };

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    println!("Setting mode for node '{}'...", node_id);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

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
            "http://{}/api/v1/cluster/nodes/{}/mode",
            api_endpoint, node_id
        ))
        .json(&request)
        .send()
        .await
        .context("Failed to send set-mode request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to set node mode: {} - {}", status, body);
    }

    println!("Node '{}' mode set to '{}'.", node_id, mode);
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
        anyhow::bail!(
            "Invalid label format '{}'. Expected key=value format.",
            label
        );
    }
    let (key, value) = (parts[0], parts[1]);

    // Validate label key
    if key.is_empty() {
        anyhow::bail!("Label key cannot be empty");
    }

    let data_dir = cli_data_dir.to_path_buf();
    let node_config = match load_node_config(&data_dir).await {
        Ok(config) => config,
        Err(_) => {
            anyhow::bail!(
                "Node not initialized. Run 'zlayer node init' or 'zlayer node join' first."
            );
        }
    };

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    println!("Adding label to node '{}'...", node_id);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

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
            "http://{}/api/v1/cluster/nodes/{}/labels",
            api_endpoint, node_id
        ))
        .json(&request)
        .send()
        .await
        .context("Failed to send label request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to add label: {} - {}", status, body);
    }

    println!("Label '{}={}' added to node '{}'.", key, value, node_id);

    Ok(())
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
    let api_endpoint = match api {
        Some(a) => a,
        None => {
            // Try to load from node config; auto-init if it doesn't exist.
            let node_config = match load_node_config(&data_dir).await {
                Ok(cfg) => cfg,
                Err(_) => {
                    // Node was never initialized -- auto-init with defaults.
                    let advertise_addr = detect_local_ip();
                    let api_port: u16 = 3669;
                    let raft_port: u16 = 9000;
                    let overlay_port: u16 = 51820;
                    let overlay_cidr = "10.200.0.0/16".to_string();

                    info!(
                        advertise_addr = %advertise_addr,
                        "Node not initialized -- auto-initializing with detected IP"
                    );
                    println!(
                        "Node not yet initialized. Auto-initializing with advertise address {}...",
                        advertise_addr
                    );

                    handle_node_init(
                        advertise_addr,
                        api_port,
                        raft_port,
                        overlay_port,
                        data_dir.clone(),
                        overlay_cidr,
                    )
                    .await?;

                    // Now load the freshly-written config
                    load_node_config(&data_dir)
                        .await
                        .context("Failed to load node config after auto-initialization")?
                }
            };

            format!(
                "http://{}:{}",
                node_config.advertise_addr, node_config.api_port
            )
        }
    };

    // --- Resolve the deployment name ---
    let deployment_name = match deployment {
        Some(d) => d,
        None => {
            // Try to discover from spec files in the current directory
            match try_discover_deployment_name() {
                Some(name) => {
                    info!(deployment = %name, "Auto-discovered deployment name from spec");
                    name
                }
                None => {
                    warn!("No deployment spec found in current directory, using 'default'");
                    "default".to_string()
                }
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
    println!("Deployment: {}", deployment_name);
    println!("API: {}", api_endpoint);
    if let Some(svc) = &service {
        println!("Service: {}", svc);
    }
    println!("\nToken:");
    println!("{}", token);
    println!("\nUsage:");
    println!("  zlayer node join <leader-addr> --token {}", token);

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
            })) => {
                assert_eq!(advertise_addr, "10.0.0.1");
                assert_eq!(api_port, 3669);
                assert_eq!(raft_port, 9000);
                assert_eq!(overlay_port, 51820);
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
}
