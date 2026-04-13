//! `zlayer secret` subcommand handlers.
//!
//! Dispatches to the daemon's secrets management endpoints (`GET /api/v1/secrets`,
//! `POST /api/v1/secrets`, `GET /api/v1/secrets/{name}`,
//! `DELETE /api/v1/secrets/{name}`) via [`DaemonClient`].
//!
//! The outer `#[cfg(unix)]` on `pub mod secret` in `commands/mod.rs` already
//! gates this file to Unix, so no inner `#![cfg(unix)]` is needed here.

use anyhow::Result;

use crate::cli::{Cli, SecretCommands};
use crate::daemon_client::DaemonClient;

/// Entry point for `zlayer secret <subcommand>`.
pub(crate) async fn handle_secret(_cli: &Cli, cmd: &SecretCommands) -> Result<()> {
    match cmd {
        SecretCommands::Ls { output } => list_secrets(output).await,
        SecretCommands::Create { name, value } => create_secret(name, value).await,
        SecretCommands::Get { name } => get_secret(name).await,
        SecretCommands::Rm { name } => remove_secret(name).await,
    }
}

async fn list_secrets(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let secrets = client.list_secrets().await?;

    if output == "json" {
        let json = serde_json::to_string_pretty(&secrets)?;
        println!("{json}");
    } else {
        // Table output
        println!(
            "{:<30} {:>10} {:>22} {:>22}",
            "NAME", "VERSION", "CREATED", "UPDATED"
        );
        for secret in &secrets {
            let name = secret.get("name").and_then(|v| v.as_str()).unwrap_or("-");
            let version = secret
                .get("version")
                .and_then(serde_json::Value::as_u64)
                .map_or_else(|| "-".to_string(), |v| v.to_string());
            let created = secret
                .get("created_at")
                .and_then(serde_json::Value::as_i64)
                .map_or_else(|| "-".to_string(), format_timestamp);
            let updated = secret
                .get("updated_at")
                .and_then(serde_json::Value::as_i64)
                .map_or_else(|| "-".to_string(), format_timestamp);
            println!("{name:<30} {version:>10} {created:>22} {updated:>22}");
        }
        if secrets.is_empty() {
            println!("(no secrets)");
        }
    }
    Ok(())
}

async fn create_secret(name: &str, value: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let result = client.create_secret(name, value).await?;

    let version = result
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    println!("Secret '{name}' stored (version {version})");
    Ok(())
}

async fn get_secret(name: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let secret = client.get_secret(name).await?;

    let version = secret
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .map_or_else(|| "-".to_string(), |v| v.to_string());
    let created = secret
        .get("created_at")
        .and_then(serde_json::Value::as_i64)
        .map_or_else(|| "-".to_string(), format_timestamp);
    let updated = secret
        .get("updated_at")
        .and_then(serde_json::Value::as_i64)
        .map_or_else(|| "-".to_string(), format_timestamp);

    println!("Name:       {name}");
    println!("Version:    {version}");
    println!("Created at: {created}");
    println!("Updated at: {updated}");
    Ok(())
}

async fn remove_secret(name: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    client.delete_secret(name).await?;
    println!("Removed secret '{name}'");
    Ok(())
}

/// Format a Unix timestamp as a human-readable UTC date-time string.
fn format_timestamp(ts: i64) -> String {
    // Simple formatting: seconds since epoch -> "YYYY-MM-DD HH:MM:SS"
    // Using chrono would be nicer, but we avoid adding a dependency just for this.
    // Fall back to raw timestamp if the value seems invalid.
    #[allow(clippy::cast_sign_loss)]
    if ts > 0 {
        let secs = ts as u64;
        // Days/hours/minutes/seconds arithmetic from epoch (1970-01-01)
        let days = secs / 86400;
        let time_of_day = secs % 86400;
        let hours = time_of_day / 3600;
        let minutes = (time_of_day % 3600) / 60;
        let seconds = time_of_day % 60;

        // Convert days since epoch to Y-M-D (simplified Gregorian)
        let (year, month, day) = days_to_ymd(days);
        format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}:{seconds:02}")
    } else {
        ts.to_string()
    }
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from Howard Hinnant's `civil_from_days`
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
