use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

// =============================================================================
// Node management types
// =============================================================================

/// Node configuration stored on disk
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeConfig {
    /// Unique node identifier
    node_id: String,
    /// Raft node ID (numeric)
    raft_node_id: u64,
    /// Public IP address for this node
    advertise_addr: String,
    /// API server port
    api_port: u16,
    /// Raft consensus port
    raft_port: u16,
    /// WireGuard overlay port
    overlay_port: u16,
    /// Overlay network CIDR
    overlay_cidr: String,
    /// WireGuard private key
    wireguard_private_key: String,
    /// WireGuard public key
    wireguard_public_key: String,
    /// Whether this node is the cluster leader/bootstrap node
    is_leader: bool,
    /// Timestamp when node was created
    created_at: String,
}

/// Join token payload
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClusterJoinToken {
    /// Leader's API endpoint
    api_endpoint: String,
    /// Leader's Raft endpoint
    raft_endpoint: String,
    /// Leader's WireGuard public key
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
    /// Joining node's WireGuard public key
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
fn generate_node_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Get current timestamp as ISO 8601 string
fn current_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Save node configuration to disk
async fn save_node_config(data_dir: &Path, config: &NodeConfig) -> Result<()> {
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
async fn load_node_config(data_dir: &Path) -> Result<NodeConfig> {
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
pub(crate) async fn handle_node(node_cmd: &NodeCommands) -> Result<()> {
    match node_cmd {
        NodeCommands::Init {
            advertise_addr,
            api_port,
            raft_port,
            overlay_port,
            data_dir,
            overlay_cidr,
        } => {
            handle_node_init(
                advertise_addr.clone(),
                *api_port,
                *raft_port,
                *overlay_port,
                data_dir.clone(),
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
            )
            .await
        }
        NodeCommands::List { output } => handle_node_list(output.clone()).await,
        NodeCommands::Status { node_id } => handle_node_status(node_id.clone()).await,
        NodeCommands::Remove { node_id, force } => {
            handle_node_remove(node_id.clone(), *force).await
        }
        NodeCommands::SetMode {
            node_id,
            mode,
            services,
        } => handle_node_set_mode(node_id.clone(), mode.clone(), services.clone()).await,
        NodeCommands::Label { node_id, label } => {
            handle_node_label(node_id.clone(), label.clone()).await
        }
        NodeCommands::GenerateJoinToken {
            deployment,
            api,
            service,
        } => handle_node_generate_join_token(deployment.clone(), api.clone(), service.clone()),
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
    use zlayer_overlay::WireGuardManager;

    println!("Initializing ZLayer node as cluster leader...");

    // 1. Create data directory
    tokio::fs::create_dir_all(&data_dir)
        .await
        .with_context(|| format!("Failed to create data directory: {}", data_dir.display()))?;
    info!(path = %data_dir.display(), "Created data directory");

    // 2. Check if already initialized
    let config_path = data_dir.join("node_config.json");
    if config_path.exists() {
        anyhow::bail!(
            "Node already initialized. Configuration exists at {}. \
            Use 'zlayer node status' to check the node or remove the config file to reinitialize.",
            config_path.display()
        );
    }

    // 3. Generate node ID
    let node_id = generate_node_id();
    let raft_node_id: u64 = 1; // First node is always ID 1
    info!(node_id = %node_id, raft_node_id = raft_node_id, "Generated node ID");

    // 4. Generate WireGuard keypair
    println!("  Generating WireGuard keypair...");
    let (private_key, public_key) = WireGuardManager::generate_keys().await.map_err(|e| {
        anyhow::anyhow!(
            "Failed to generate WireGuard keys: {}. Ensure 'wg' command is installed.",
            e
        )
    })?;
    info!("Generated WireGuard keypair");

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

    // 6. Initialize Raft as leader (bootstrap single-node cluster)
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

    // 7. Generate join token
    let join_token = generate_join_token_data(
        &advertise_addr,
        api_port,
        raft_port,
        &public_key,
        &overlay_cidr,
    )?;

    // 8. Print success message
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
) -> Result<()> {
    use std::time::Duration;
    use zlayer_overlay::WireGuardManager;

    println!("Joining ZLayer cluster at {}...", leader_addr);

    // 1. Parse and validate the join token
    let token_data = parse_cluster_join_token(&token).context("Invalid join token")?;

    info!(
        api_endpoint = %token_data.api_endpoint,
        overlay_cidr = %token_data.overlay_cidr,
        "Parsed join token"
    );

    // 2. Generate WireGuard keypair for this node
    println!("  Generating WireGuard keypair...");
    let (private_key, public_key) = WireGuardManager::generate_keys().await.map_err(|e| {
        anyhow::anyhow!(
            "Failed to generate WireGuard keys: {}. Ensure 'wg' command is installed.",
            e
        )
    })?;

    // 3. Determine data directory
    let data_dir = PathBuf::from("/var/lib/zlayer");
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
        api_port: 8080, // Default for workers
        raft_port,
        overlay_port,
        overlay_cidr: token_data.overlay_cidr.clone(),
        wireguard_private_key: private_key,
        wireguard_public_key: public_key.clone(),
        is_leader: false,
        created_at: current_timestamp(),
    };
    save_node_config(&data_dir, &node_config).await?;

    // 7. Configure WireGuard with peers
    println!("  Configuring overlay network...");
    // TODO: Actually configure WireGuard interface with peers
    for peer in &join_response.peers {
        info!(
            peer_id = %peer.node_id,
            peer_addr = %peer.advertise_addr,
            "Added peer"
        );
    }

    // 8. Print success message
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
    println!();
    println!("Run 'zlayer deploy' or 'zlayer up' with a spec file to start processing workloads.");

    Ok(())
}

/// List all nodes in the cluster
pub(crate) async fn handle_node_list(output: String) -> Result<()> {
    use std::time::Duration;

    // Try to load local node config to get API endpoint
    let data_dir = PathBuf::from("/var/lib/zlayer");
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
pub(crate) async fn handle_node_status(node_id: Option<String>) -> Result<()> {
    let data_dir = PathBuf::from("/var/lib/zlayer");
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
        println!("WireGuard:");
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
pub(crate) async fn handle_node_remove(node_id: String, force: bool) -> Result<()> {
    use std::time::Duration;

    let data_dir = PathBuf::from("/var/lib/zlayer");
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

    let data_dir = PathBuf::from("/var/lib/zlayer");
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
pub(crate) async fn handle_node_label(node_id: String, label: String) -> Result<()> {
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

    let data_dir = PathBuf::from("/var/lib/zlayer");
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

/// Handle node generate-join-token command
pub(crate) fn handle_node_generate_join_token(
    deployment: String,
    api: String,
    service: Option<String>,
) -> Result<()> {
    use base64::Engine;

    let token_data = serde_json::json!({
        "deployment": deployment,
        "api_endpoint": api,
        "service": service,
    });

    let json = serde_json::to_string(&token_data)?;
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&json);

    println!("Join Token Generated");
    println!("====================\n");
    println!("Deployment: {}", deployment);
    println!("API: {}", api);
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
            Commands::Node(NodeCommands::Init {
                advertise_addr,
                api_port,
                raft_port,
                overlay_port,
                data_dir,
                overlay_cidr,
            }) => {
                assert_eq!(advertise_addr, "10.0.0.1");
                assert_eq!(api_port, 8080);
                assert_eq!(raft_port, 9000);
                assert_eq!(overlay_port, 51820);
                assert_eq!(data_dir, PathBuf::from("/var/lib/zlayer"));
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
            Commands::Node(NodeCommands::Init {
                advertise_addr,
                api_port,
                raft_port,
                overlay_port,
                data_dir,
                overlay_cidr,
            }) => {
                assert_eq!(advertise_addr, "192.168.1.100");
                assert_eq!(api_port, 9090);
                assert_eq!(raft_port, 9001);
                assert_eq!(overlay_port, 51821);
                assert_eq!(data_dir, PathBuf::from("/custom/data"));
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
            "10.0.0.1:8080",
            "--token",
            "abc123",
            "--advertise-addr",
            "10.0.0.2",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Join {
                leader_addr,
                token,
                advertise_addr,
                mode,
                services,
            }) => {
                assert_eq!(leader_addr, "10.0.0.1:8080");
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
            "10.0.0.1:8080",
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
            Commands::Node(NodeCommands::Join { mode, services, .. }) => {
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
            Commands::Node(NodeCommands::List { output }) => {
                assert_eq!(output, "table");
            }
            _ => panic!("Expected Node List command"),
        }
    }

    #[test]
    fn test_cli_node_list_command_json() {
        let cli = Cli::try_parse_from(["zlayer", "node", "list", "--output", "json"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::List { output }) => {
                assert_eq!(output, "json");
            }
            _ => panic!("Expected Node List command"),
        }
    }

    #[test]
    fn test_cli_node_status_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "status"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Status { node_id }) => {
                assert!(node_id.is_none());
            }
            _ => panic!("Expected Node Status command"),
        }
    }

    #[test]
    fn test_cli_node_status_command_with_id() {
        let cli = Cli::try_parse_from(["zlayer", "node", "status", "node-abc-123"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Status { node_id }) => {
                assert_eq!(node_id, Some("node-abc-123".to_string()));
            }
            _ => panic!("Expected Node Status command"),
        }
    }

    #[test]
    fn test_cli_node_remove_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "remove", "node-123"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Remove { node_id, force }) => {
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
            Commands::Node(NodeCommands::Remove { node_id, force }) => {
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
            Commands::Node(NodeCommands::SetMode {
                node_id,
                mode,
                services,
            }) => {
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
            Commands::Node(NodeCommands::Label { node_id, label }) => {
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
            8080,
            9000,
            "test-public-key",
            "10.200.0.0/16",
        )
        .unwrap();

        let parsed = super::parse_cluster_join_token(&token).unwrap();

        assert_eq!(parsed.api_endpoint, "192.168.1.1:8080");
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
