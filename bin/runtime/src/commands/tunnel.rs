use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::cli::{Cli, TunnelCommands};
use crate::util::parse_duration;

/// Handle tunnel subcommands
pub(crate) async fn handle_tunnel(_cli: &Cli, cmd: &TunnelCommands) -> Result<()> {
    match cmd {
        TunnelCommands::Create {
            name,
            services,
            ttl_hours,
        } => handle_tunnel_create(name, services.clone(), *ttl_hours).await,
        TunnelCommands::List { output } => handle_tunnel_list(output).await,
        TunnelCommands::Revoke { id } => handle_tunnel_revoke(id).await,
        TunnelCommands::Connect {
            server,
            token,
            services,
        } => handle_tunnel_connect(server, token, services.clone()).await,
        TunnelCommands::Add {
            name,
            from,
            to,
            local_port,
            remote_port,
            expose,
        } => handle_tunnel_add(name, from, to, *local_port, *remote_port, expose).await,
        TunnelCommands::Remove { name } => handle_tunnel_remove(name).await,
        TunnelCommands::Status { id } => handle_tunnel_status(id.clone()).await,
        TunnelCommands::Access {
            endpoint,
            local_port,
            ttl,
        } => handle_tunnel_access(endpoint, *local_port, ttl).await,
    }
}

/// Create a new tunnel token
pub(crate) async fn handle_tunnel_create(
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
    println!("Tunnel ID: {}", id);
    println!("Name: {}", name);
    if !services.is_empty() {
        println!("Allowed services: {}", services.join(", "));
    } else {
        println!("Allowed services: all");
    }
    println!("Expires in: {} hours", ttl_hours);
    println!("\nToken:");
    println!("{}", token);
    println!("\nUsage:");
    println!(
        "  zlayer tunnel connect --server wss://your-server/tunnel/v1 --token {} --service name:local:remote",
        token
    );

    // Note: In production, this would store the token in the cluster state via API
    warn!("Note: Token created locally only. Use the API for persistent storage.");

    Ok(())
}

/// List tunnel tokens
pub(crate) async fn handle_tunnel_list(output: &str) -> Result<()> {
    info!(output = %output, "Listing tunnels");

    // In production, this would query the API
    println!("Tunnel List");
    println!("===========\n");
    println!("[No tunnels configured - use the API to manage tunnels]");
    println!("\nTo list tunnels via API:");
    println!("  curl -H 'Authorization: Bearer <token>' http://localhost:8080/api/v1/tunnels");

    Ok(())
}

/// Revoke a tunnel token
pub(crate) async fn handle_tunnel_revoke(id: &str) -> Result<()> {
    info!(id = %id, "Revoking tunnel");

    println!("Revoking tunnel: {}", id);
    println!("\nTo revoke via API:");
    println!(
        "  curl -X DELETE -H 'Authorization: Bearer <token>' http://localhost:8080/api/v1/tunnels/{}",
        id
    );

    // In production, this would call the API
    warn!("Note: Use the API for actual tunnel revocation.");

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
                "Invalid service format '{}'. Expected name:local_port[:remote_port]",
                service
            );
        }

        let name = parts[0].to_string();
        let local_port: u16 = parts[1]
            .parse()
            .with_context(|| format!("Invalid local port in '{}'", service))?;
        let remote_port: u16 = if parts.len() > 2 {
            parts[2]
                .parse()
                .with_context(|| format!("Invalid remote port in '{}'", service))?
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
        .map_err(|e| anyhow::anyhow!("Invalid configuration: {}", e))?;

    println!("Connecting to tunnel server: {}", server);
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
                    println!("Tunnel error: {}", e);
                    return Err(e.into());
                }
            }
        }
        _ = shutdown => {
            agent.disconnect();
            println!("Disconnected.");
        }
    }

    Ok(())
}

/// Add a node-to-node tunnel
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

    println!("Creating node-to-node tunnel: {}", name);
    println!("  From: {} (port {})", from, local_port);
    println!("  To: {} (port {})", to, remote_port);
    println!("  Expose: {}", expose);
    println!();
    println!("To create via API:");
    println!(
        r#"  curl -X POST -H 'Authorization: Bearer <token>' -H 'Content-Type: application/json' \
    -d '{{"name":"{}","from_node":"{}","to_node":"{}","local_port":{},"remote_port":{},"expose":"{}"}}' \
    http://localhost:8080/api/v1/tunnels/node"#,
        name, from, to, local_port, remote_port, expose
    );

    // In production, this would call the API
    warn!("Note: Use the API for actual tunnel creation.");

    Ok(())
}

/// Remove a node-to-node tunnel
pub(crate) async fn handle_tunnel_remove(name: &str) -> Result<()> {
    info!(name = %name, "Removing node-to-node tunnel");

    println!("Removing node-to-node tunnel: {}", name);
    println!("\nTo remove via API:");
    println!(
        "  curl -X DELETE -H 'Authorization: Bearer <token>' http://localhost:8080/api/v1/tunnels/node/{}",
        name
    );

    // In production, this would call the API
    warn!("Note: Use the API for actual tunnel removal.");

    Ok(())
}

/// Show tunnel status
pub(crate) async fn handle_tunnel_status(id: Option<String>) -> Result<()> {
    info!(id = ?id, "Getting tunnel status");

    match id {
        Some(tunnel_id) => {
            println!("Tunnel Status: {}", tunnel_id);
            println!("==============={}", "=".repeat(tunnel_id.len()));
            println!("\nTo get status via API:");
            println!(
                "  curl -H 'Authorization: Bearer <token>' http://localhost:8080/api/v1/tunnels/{}/status",
                tunnel_id
            );
        }
        None => {
            println!("All Tunnel Status");
            println!("=================\n");
            println!("[Use the API to list tunnel status]");
            println!("\nTo list all tunnels via API:");
            println!(
                "  curl -H 'Authorization: Bearer <token>' http://localhost:8080/api/v1/tunnels"
            );
        }
    }

    // In production, this would query the API
    warn!("Note: Use the API for actual tunnel status.");

    Ok(())
}

/// Request temporary access to a tunneled service
pub(crate) async fn handle_tunnel_access(
    endpoint: &str,
    local_port: Option<u16>,
    ttl: &str,
) -> Result<()> {
    // LocalProxy would be used here in full implementation
    // use zlayer_tunnel::LocalProxy;

    info!(
        endpoint = %endpoint,
        local_port = ?local_port,
        ttl = %ttl,
        "Requesting access to tunneled service"
    );

    // Parse TTL for validation
    let _ttl_secs = parse_duration(ttl)?;

    // Use provided port or auto-assign
    let port = local_port.unwrap_or(0);

    println!("Requesting temporary access to: {}", endpoint);
    println!(
        "  Local port: {}",
        if port == 0 {
            "auto-assign".to_string()
        } else {
            port.to_string()
        }
    );
    println!("  TTL: {}", ttl);
    println!();

    // In production, this would:
    // 1. Request a temporary token from the API
    // 2. Start a LocalProxy to the target service
    // 3. Display the local port for connection

    println!("[Access functionality requires API integration]");
    println!("\nOnce implemented, you would connect to:");
    println!("  localhost:{} -> {}", port, endpoint);
    println!("\nThe connection will automatically close after {}.", ttl);

    Ok(())
}
