//! `zlayer network` subcommand handlers.
//!
//! Dispatches to the daemon's network and overlay management endpoints
//! (`GET /api/v1/networks`, `GET /api/v1/overlay/*`, etc.) via
//! [`DaemonClient`].
//!
//! The outer `#[cfg(unix)]` on `pub mod network` in `commands/mod.rs` already
//! gates this file to Unix, so no inner `#![cfg(unix)]` is needed here.

use anyhow::Result;

use crate::cli::{Cli, NetworkCommands};
use zlayer_client::DaemonClient;

/// Entry point for `zlayer network <subcommand>`.
pub(crate) async fn handle_network(_cli: &Cli, cmd: &NetworkCommands) -> Result<()> {
    match cmd {
        NetworkCommands::Ls { output } => list_networks(output).await,
        NetworkCommands::Inspect { name } => inspect_network(name).await,
        NetworkCommands::Create { name } => create_network(name).await,
        NetworkCommands::Rm { name } => remove_network(name).await,
        NetworkCommands::Status => overlay_status().await,
        NetworkCommands::Peers => overlay_peers().await,
        NetworkCommands::Dns => overlay_dns().await,
    }
}

async fn list_networks(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let networks: Vec<serde_json::Value> = client.list_networks().await?;

    if output == "json" {
        let json = serde_json::to_string_pretty(&networks)?;
        println!("{json}");
    } else {
        // Table output
        println!(
            "{:<30} {:<40} {:>6} {:>8} {:>6}",
            "NAME", "DESCRIPTION", "CIDRS", "MEMBERS", "RULES"
        );
        for net in &networks {
            let name = net["name"].as_str().unwrap_or("-");
            let desc = net["description"].as_str().unwrap_or("-");
            let cidrs = net["cidr_count"].as_u64().unwrap_or(0);
            let members = net["member_count"].as_u64().unwrap_or(0);
            let rules = net["rule_count"].as_u64().unwrap_or(0);
            println!("{name:<30} {desc:<40} {cidrs:>6} {members:>8} {rules:>6}");
        }
        if networks.is_empty() {
            println!("(no networks)");
        }
    }
    Ok(())
}

async fn inspect_network(name: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let network: serde_json::Value = client.get_network(name).await?;
    let json = serde_json::to_string_pretty(&network)?;
    println!("{json}");
    Ok(())
}

async fn create_network(name: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    client.create_network(name).await?;
    println!("Created network '{name}'");
    Ok(())
}

async fn remove_network(name: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    client.delete_network(name).await?;
    println!("Removed network '{name}'");
    Ok(())
}

async fn overlay_status() -> Result<()> {
    let client = DaemonClient::connect().await?;
    let status: serde_json::Value = client.get_overlay_status().await?;
    let json = serde_json::to_string_pretty(&status)?;
    println!("{json}");
    Ok(())
}

async fn overlay_peers() -> Result<()> {
    let client = DaemonClient::connect().await?;
    let peers: serde_json::Value = client.get_overlay_peers().await?;
    let json = serde_json::to_string_pretty(&peers)?;
    println!("{json}");
    Ok(())
}

async fn overlay_dns() -> Result<()> {
    let client = DaemonClient::connect().await?;
    let dns: serde_json::Value = client.get_overlay_dns().await?;
    let json = serde_json::to_string_pretty(&dns)?;
    println!("{json}");
    Ok(())
}
