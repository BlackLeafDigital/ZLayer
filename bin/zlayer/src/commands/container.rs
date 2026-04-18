//! `zlayer container` subcommand handlers.
//!
//! Dispatches to the daemon's container management endpoints
//! (`GET /api/v1/containers`, `DELETE /api/v1/containers/{id}`, etc.)
//! via [`DaemonClient`].
//!
//! The outer `#[cfg(unix)]` on `pub mod container` in `commands/mod.rs` already
//! gates this file to Unix, so no inner `#![cfg(unix)]` is needed here.

use anyhow::Result;

use crate::cli::{Cli, ContainerCommands};
use zlayer_client::DaemonClient;

/// Entry point for `zlayer container <subcommand>`.
pub(crate) async fn handle_container(_cli: &Cli, cmd: &ContainerCommands) -> Result<()> {
    match cmd {
        ContainerCommands::Ls { output } => list_containers(output).await,
        ContainerCommands::Inspect { id } => inspect_container(id).await,
        ContainerCommands::Rm { id, force } => remove_container(id, *force).await,
        ContainerCommands::Logs { id, tail } => container_logs(id, *tail).await,
        ContainerCommands::Stats { id } => container_stats(id).await,
    }
}

async fn list_containers(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let containers = client.get_all_containers().await?;

    if output == "json" {
        let json = serde_json::to_string_pretty(&containers)?;
        println!("{json}");
    } else {
        // Table output
        println!(
            "{:<16} {:<20} {:<12} {:<24} {:<30}",
            "CONTAINER ID", "NAME", "STATUS", "CREATED", "IMAGE"
        );

        if let Some(arr) = containers.as_array() {
            for c in arr {
                let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                // Truncate ID to 12 chars for display, like Docker
                let short_id = if id.len() > 12 { &id[..12] } else { id };
                let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let status = c.get("status").and_then(|v| v.as_str()).unwrap_or("-");
                let created = c
                    .get("created")
                    .or_else(|| c.get("created_at"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                let image = c.get("image").and_then(|v| v.as_str()).unwrap_or("-");
                println!("{short_id:<16} {name:<20} {status:<12} {created:<24} {image:<30}");
            }
            if arr.is_empty() {
                println!("(no containers)");
            }
        } else {
            // Unexpected shape -- just dump as JSON
            let json = serde_json::to_string_pretty(&containers)?;
            println!("{json}");
        }
    }
    Ok(())
}

async fn inspect_container(id: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let info = client.get_container(id).await?;
    let json = serde_json::to_string_pretty(&info)?;
    println!("{json}");
    Ok(())
}

async fn remove_container(id: &str, force: bool) -> Result<()> {
    let client = DaemonClient::connect().await?;
    client.delete_container(id, force).await?;
    println!("Removed {id}");
    Ok(())
}

async fn container_logs(id: &str, tail: Option<u32>) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let logs = client.get_container_logs(id, tail).await?;
    print!("{logs}");
    if !logs.ends_with('\n') {
        println!();
    }
    Ok(())
}

async fn container_stats(id: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let stats = client.get_container_stats(id).await?;
    let json = serde_json::to_string_pretty(&stats)?;
    println!("{json}");
    Ok(())
}
