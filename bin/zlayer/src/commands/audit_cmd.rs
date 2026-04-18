//! `zlayer audit` subcommand handlers.
//!
//! Admin-only queries against the audit log.

use anyhow::{Context, Result};
use zlayer_api::storage::AuditEntry;

use zlayer_client::DaemonClient;

/// `zlayer audit tail` -- print recent audit log entries.
pub async fn tail(
    user: Option<String>,
    resource_kind: Option<String>,
    limit: Option<usize>,
    output: &str,
) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let entries = client
        .list_audit(user.as_deref(), resource_kind.as_deref(), limit)
        .await
        .context("Failed to list audit entries")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&entries)?;
            println!("{json}");
        }
        _ => print_audit_table(&entries),
    }
    Ok(())
}

/// Pretty-print audit entries as a plain-text table.
fn print_audit_table(entries: &[AuditEntry]) {
    if entries.is_empty() {
        println!("(no audit entries)");
        return;
    }
    println!(
        "{:<20} {:<38} {:<10} {:<18} {:<38}",
        "TIMESTAMP", "USER", "ACTION", "RESOURCE_KIND", "RESOURCE_ID"
    );
    for e in entries {
        let ts = e.created_at.format("%Y-%m-%d %H:%M:%S");
        let rid = e.resource_id.as_deref().unwrap_or("-");
        println!(
            "{:<20} {:<38} {:<10} {:<18} {:<38}",
            ts, e.user_id, e.action, e.resource_kind, rid
        );
    }
}
