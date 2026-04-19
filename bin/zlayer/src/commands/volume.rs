//! `zlayer volume` subcommand handlers.
//!
//! Dispatches to the daemon's volume management endpoints
//! (`GET /api/v1/volumes`, `DELETE /api/v1/volumes/{name}`) via
//! [`DaemonClient`].
//!
//! The outer `#[cfg(unix)]` on `pub mod volume` in `commands/mod.rs` already
//! gates this file to Unix, so no inner `#![cfg(unix)]` is needed here.

use anyhow::Result;

use crate::cli::{Cli, VolumeCommands};
use zlayer_client::DaemonClient;

/// Entry point for `zlayer volume <subcommand>`.
pub(crate) async fn handle_volume(_cli: &Cli, cmd: &VolumeCommands) -> Result<()> {
    match cmd {
        VolumeCommands::Ls { output } => list_volumes(output).await,
        VolumeCommands::Rm { name, force } => remove_volume(name, *force).await,
    }
}

async fn list_volumes(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let volumes = client.list_volumes().await?;

    if output == "json" {
        let json = serde_json::to_string_pretty(&volumes)?;
        println!("{json}");
    } else {
        // Table output
        println!("{:<30} {:<50} {:>12}", "NAME", "PATH", "SIZE");
        for vol in &volumes {
            let name = vol["name"].as_str().unwrap_or("-");
            let path = vol["path"].as_str().unwrap_or("-");
            let size = vol["size_bytes"]
                .as_u64()
                .map_or_else(|| "-".to_string(), format_bytes);
            println!("{name:<30} {path:<50} {size:>12}");
        }
        if volumes.is_empty() {
            println!("(no volumes)");
        }
    }
    Ok(())
}

async fn remove_volume(name: &str, force: bool) -> Result<()> {
    let client = DaemonClient::connect().await?;
    client.delete_volume(name, force).await?;
    println!("Removed volume '{name}'");
    Ok(())
}

#[allow(clippy::cast_precision_loss)]
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
