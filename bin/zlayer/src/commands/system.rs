//! `zlayer system` subcommand handlers.
//!
//! Dispatches to the daemon's system-maintenance endpoints via
//! [`DaemonClient`], starting with `POST /system/prune`.
//!
//! The outer `#[cfg(unix)]` on `pub mod system` in `commands/mod.rs` already
//! gates this file to Unix, so no inner `#![cfg(unix)]` is needed here.

use std::io::{self, Write};

use anyhow::Result;

use crate::cli::{Cli, SystemCommands};
use crate::daemon_client::DaemonClient;

/// Entry point for `zlayer system <subcommand>`.
pub(crate) async fn handle_system(_cli: &Cli, cmd: &SystemCommands) -> Result<()> {
    match cmd {
        SystemCommands::Prune { yes } => prune(*yes).await,
    }
}

#[allow(clippy::cast_precision_loss)]
async fn prune(skip_confirm: bool) -> Result<()> {
    if !skip_confirm {
        print!("This will remove dangling cached images. Continue? [y/N] ");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let answer = line.trim().to_lowercase();
        if answer != "y" && answer != "yes" {
            println!("Aborted.");
            return Ok(());
        }
    }

    let client = DaemonClient::connect().await?;
    let result = client.prune_images().await?;

    if result.deleted.is_empty() {
        println!("No images to prune.");
    } else {
        println!("Deleted {} image(s):", result.deleted.len());
        for d in &result.deleted {
            println!("  {d}");
        }
        println!(
            "Space reclaimed: {:.1} MB",
            result.space_reclaimed as f64 / (1024.0 * 1024.0)
        );
    }
    Ok(())
}
