//! `zlayer image` subcommand handlers.
//!
//! Dispatches to the daemon's image management endpoints (`GET /images`,
//! `DELETE /images/{image}`) via [`DaemonClient`].
//!
//! The outer `#[cfg(unix)]` on `pub mod image` in `commands/mod.rs` already
//! gates this file to Unix, so no inner `#![cfg(unix)]` is needed here.

use anyhow::Result;

use crate::cli::{Cli, ImageCommands};
use crate::daemon_client::DaemonClient;

/// Entry point for `zlayer image <subcommand>`.
pub(crate) async fn handle_image(_cli: &Cli, cmd: &ImageCommands) -> Result<()> {
    match cmd {
        ImageCommands::Ls { output } => list_images(output).await,
        ImageCommands::Rm { image, force } => remove_image(image, *force).await,
    }
}

async fn list_images(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let images = client.list_images().await?;

    if output == "json" {
        let json = serde_json::to_string_pretty(&images)?;
        println!("{json}");
    } else {
        // Table output
        println!("{:<50} {:<72} {:>12}", "REFERENCE", "DIGEST", "SIZE");
        for info in &images {
            let digest = info.digest.as_deref().unwrap_or("-");
            let size = info
                .size_bytes
                .map_or_else(|| "-".to_string(), format_bytes);
            println!("{:<50} {:<72} {:>12}", info.reference, digest, size);
        }
        if images.is_empty() {
            println!("(no cached images)");
        }
    }
    Ok(())
}

async fn remove_image(image: &str, force: bool) -> Result<()> {
    let client = DaemonClient::connect().await?;
    client.remove_image(image, force).await?;
    println!("Removed {image}");
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
