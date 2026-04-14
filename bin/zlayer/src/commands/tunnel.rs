use anyhow::{Context, Result};
use tracing::info;

use crate::cli::{Cli, TunnelCommands};
#[cfg(unix)]
use crate::daemon_client::DaemonClient;
#[cfg(unix)]
use crate::util::parse_duration;

/// Handle tunnel subcommands
pub(crate) async fn handle_tunnel(_cli: &Cli, cmd: &TunnelCommands) -> Result<()> {
    match cmd {
        TunnelCommands::Create {
            name,
            services,
            ttl_hours,
        } => handle_tunnel_create(name, services.clone(), *ttl_hours),
        TunnelCommands::Connect {
            server,
            token,
            services,
        } => handle_tunnel_connect(server, token, services.clone()).await,
        #[cfg(unix)]
        TunnelCommands::List { output } => handle_tunnel_list(output).await,
        #[cfg(unix)]
        TunnelCommands::Revoke { id } => handle_tunnel_revoke(id).await,
        #[cfg(unix)]
        TunnelCommands::Add {
            name,
            from,
            to,
            local_port,
            remote_port,
            expose,
        } => handle_tunnel_add(name, from, to, *local_port, *remote_port, expose).await,
        #[cfg(unix)]
        TunnelCommands::Remove { name } => handle_tunnel_remove(name).await,
        #[cfg(unix)]
        TunnelCommands::Status { id } => handle_tunnel_status(id.clone()).await,
        #[cfg(unix)]
        TunnelCommands::Access {
            endpoint,
            local_port,
            ttl,
        } => handle_tunnel_access(endpoint, *local_port, ttl).await,
        #[cfg(not(unix))]
        _ => anyhow::bail!(
            "This tunnel command requires the ZLayer runtime which is not available on Windows.\n\
             Start the WSL2 daemon with 'zlayer serve' and use the daemon API."
        ),
    }
}

/// Create a new tunnel token
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn handle_tunnel_create(
    name: &str,
    services: Option<String>,
    ttl_hours: u64,
) -> Result<()> {
    info!(name = %name, ttl_hours = ttl_hours, "Creating tunnel token");

    // Parse services
    let services: Vec<String> = services
        .map(|s| {
            s.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // Generate token
    let token = format!("tun_{}", uuid::Uuid::new_v4().to_string().replace('-', ""));
    let id = uuid::Uuid::new_v4().to_string();

    println!("Tunnel token created successfully!\n");
    println!("Tunnel ID: {id}");
    println!("Name: {name}");
    if services.is_empty() {
        println!("Allowed services: all");
    } else {
        println!("Allowed services: {}", services.join(", "));
    }
    println!("Expires in: {ttl_hours} hours");
    println!("\nToken:");
    println!("{token}");
    println!("\nUsage:");
    println!(
        "  zlayer tunnel connect --server wss://your-server/tunnel/v1 --token {token} --service name:local:remote"
    );

    Ok(())
}

/// List tunnel tokens
#[cfg(unix)]
pub(crate) async fn handle_tunnel_list(output: &str) -> Result<()> {
    info!(output = %output, "Listing tunnels");

    let client = DaemonClient::connect().await?;
    let tunnels = client.list_tunnels().await?;

    let arr = tunnels.as_array().unwrap_or(&Vec::new()).clone();

    if output == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(&tunnels).unwrap_or_default()
        );
        return Ok(());
    }

    // Table output
    if arr.is_empty() {
        println!("No tunnels configured.");
        return Ok(());
    }

    println!(
        "{:<38} {:<20} {:<12} {:<20} {:<20}",
        "ID", "NAME", "STATUS", "SERVICES", "EXPIRES"
    );
    println!("{}", "-".repeat(110));
    for t in &arr {
        let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("-");
        let status = t.get("status").and_then(|v| v.as_str()).unwrap_or("-");
        let services_display = t.get("services").and_then(|v| v.as_array()).map_or_else(
            || "all".to_string(),
            |arr| {
                let joined = arr
                    .iter()
                    .filter_map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                if joined.is_empty() {
                    "all".to_string()
                } else {
                    joined
                }
            },
        );
        let expires = t
            .get("expires_at")
            .and_then(serde_json::Value::as_u64)
            .map_or_else(|| "-".to_string(), |ts| format!("{ts}"));
        println!("{id:<38} {name:<20} {status:<12} {services_display:<20} {expires:<20}");
    }

    Ok(())
}

/// Revoke a tunnel token
#[cfg(unix)]
pub(crate) async fn handle_tunnel_revoke(id: &str) -> Result<()> {
    info!(id = %id, "Revoking tunnel");

    let client = DaemonClient::connect().await?;
    let result = client.revoke_tunnel(id).await?;

    let msg = result
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("Tunnel revoked successfully");
    println!("{msg}");

    Ok(())
}

/// Connect to a tunnel server as a client
pub(crate) async fn handle_tunnel_connect(
    server: &str,
    token: &str,
    services: Vec<String>,
) -> Result<()> {
    use zlayer_tunnel::{ServiceConfig, TunnelAgent, TunnelClientConfig};

    info!(server = %server, services = ?services, "Connecting to tunnel server");

    // Validate server URL
    if !server.starts_with("ws://") && !server.starts_with("wss://") {
        anyhow::bail!("Server URL must start with ws:// or wss://");
    }

    // Parse services
    let mut service_configs = Vec::new();
    for service in &services {
        let parts: Vec<&str> = service.split(':').collect();
        if parts.len() < 2 {
            anyhow::bail!(
                "Invalid service format '{service}'. Expected name:local_port[:remote_port]"
            );
        }

        let name = parts[0].to_string();
        let local_port: u16 = parts[1]
            .parse()
            .with_context(|| format!("Invalid local port in '{service}'"))?;
        let remote_port: u16 = if parts.len() > 2 {
            parts[2]
                .parse()
                .with_context(|| format!("Invalid remote port in '{service}'"))?
        } else {
            0 // Auto-assign
        };

        service_configs.push(ServiceConfig::tcp(name, local_port).with_remote_port(remote_port));
    }

    if service_configs.is_empty() {
        anyhow::bail!("At least one service must be specified with --service");
    }

    // Create tunnel client config
    let mut config = TunnelClientConfig::new(server, token);
    for svc in service_configs {
        config = config.with_service(svc);
    }

    // Validate config
    config
        .validate()
        .map_err(|e| anyhow::anyhow!("Invalid configuration: {e}"))?;

    println!("Connecting to tunnel server: {server}");
    println!("Services to expose:");
    for svc in &config.services {
        if svc.remote_port > 0 {
            println!(
                "  - {}: {} -> {}",
                svc.name, svc.local_port, svc.remote_port
            );
        } else {
            println!("  - {}: {} -> (auto)", svc.name, svc.local_port);
        }
    }
    println!();

    // Create and run the agent
    let agent = TunnelAgent::new(config);

    println!("Establishing connection... Press Ctrl+C to disconnect.");

    // Run with graceful shutdown
    let shutdown = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
        println!("\nShutting down...");
    };

    tokio::select! {
        result = agent.run() => {
            match result {
                Ok(()) => {
                    println!("Tunnel disconnected gracefully.");
                }
                Err(e) => {
                    println!("Tunnel error: {e}");
                    return Err(e.into());
                }
            }
        }
        () = shutdown => {
            agent.disconnect();
            println!("Disconnected.");
        }
    }

    Ok(())
}

/// Add a node-to-node tunnel
#[cfg(unix)]
pub(crate) async fn handle_tunnel_add(
    name: &str,
    from: &str,
    to: &str,
    local_port: u16,
    remote_port: u16,
    expose: &str,
) -> Result<()> {
    info!(
        name = %name,
        from = %from,
        to = %to,
        local_port = local_port,
        remote_port = remote_port,
        expose = %expose,
        "Adding node-to-node tunnel"
    );

    if expose != "public" && expose != "internal" {
        anyhow::bail!("Expose must be 'public' or 'internal'");
    }

    let client = DaemonClient::connect().await?;
    let result = client
        .add_node_tunnel(name, from, to, local_port, remote_port, expose)
        .await?;

    println!("Node-to-node tunnel created:");
    println!(
        "  Name:        {}",
        result.get("name").and_then(|v| v.as_str()).unwrap_or(name)
    );
    println!(
        "  From:        {} (port {})",
        result
            .get("from_node")
            .and_then(|v| v.as_str())
            .unwrap_or(from),
        local_port
    );
    println!(
        "  To:          {} (port {})",
        result.get("to_node").and_then(|v| v.as_str()).unwrap_or(to),
        remote_port
    );
    println!(
        "  Expose:      {}",
        result
            .get("expose")
            .and_then(|v| v.as_str())
            .unwrap_or(expose)
    );
    println!(
        "  Status:      {}",
        result
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending")
    );

    Ok(())
}

/// Remove a node-to-node tunnel
#[cfg(unix)]
pub(crate) async fn handle_tunnel_remove(name: &str) -> Result<()> {
    info!(name = %name, "Removing node-to-node tunnel");

    let client = DaemonClient::connect().await?;
    let result = client.remove_node_tunnel(name).await?;

    let msg = result
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("Node tunnel removed successfully");
    println!("{msg}");

    Ok(())
}

/// Show tunnel status
#[cfg(unix)]
pub(crate) async fn handle_tunnel_status(id: Option<String>) -> Result<()> {
    info!(id = ?id, "Getting tunnel status");

    let client = DaemonClient::connect().await?;

    if let Some(tunnel_id) = id {
        let status = client.get_tunnel_status(&tunnel_id).await?;
        println!(
            "{}",
            serde_json::to_string_pretty(&status).unwrap_or_default()
        );
    } else {
        // No specific ID -- list all tunnels with their status
        let tunnels = client.list_tunnels().await?;
        let arr = tunnels.as_array().unwrap_or(&Vec::new()).clone();

        if arr.is_empty() {
            println!("No tunnels configured.");
            return Ok(());
        }

        println!("{:<38} {:<20} {:<12}", "ID", "NAME", "STATUS");
        println!("{}", "-".repeat(70));
        for t in &arr {
            let tid = t.get("id").and_then(|v| v.as_str()).unwrap_or("-");
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("-");
            let st = t.get("status").and_then(|v| v.as_str()).unwrap_or("-");
            println!("{tid:<38} {name:<20} {st:<12}");
        }
    }

    Ok(())
}

/// Request temporary access to a tunneled service
#[cfg(unix)]
pub(crate) async fn handle_tunnel_access(
    endpoint: &str,
    local_port: Option<u16>,
    ttl: &str,
) -> Result<()> {
    info!(
        endpoint = %endpoint,
        local_port = ?local_port,
        ttl = %ttl,
        "Requesting access to tunneled service"
    );

    // Parse TTL for validation
    let _ttl_secs = parse_duration(ttl)?;

    let port = local_port.unwrap_or(0);

    // Verify daemon is reachable
    let _client = DaemonClient::connect().await?;

    println!("Requesting temporary access to: {endpoint}");
    println!(
        "  Local port: {}",
        if port == 0 {
            "auto-assign".to_string()
        } else {
            port.to_string()
        }
    );
    println!("  TTL: {ttl}");
    println!();

    // Access requires a daemon-side temporary token + LocalProxy integration.
    // The daemon is reachable, but the access-token API endpoint is not yet
    // available. Once it is, this command will:
    //   1. Request a temporary token from the daemon API
    //   2. Start a LocalProxy to the target service
    //   3. Display the local port for connection
    println!("Access proxy is not yet available in the daemon API.");
    println!("Use 'zlayer tunnel connect' to establish a full tunnel instead.");

    Ok(())
}
