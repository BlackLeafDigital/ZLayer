//! `zlayer notifier` subcommand handlers.
//!
//! Manages notifiers — named notification channels that send alerts to Slack,
//! Discord, generic webhooks, or SMTP endpoints.
//!
//! Dispatches to the daemon's notifier endpoints
//! (`GET/POST/PATCH/DELETE /api/v1/notifiers[/id[/test]]`) via [`DaemonClient`].

use anyhow::{Context, Result};
use zlayer_api::storage::{NotifierConfig, NotifierKind, StoredNotifier};

use zlayer_client::DaemonClient;

/// `zlayer notifier list [--output table|json]`.
pub async fn list(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let notifiers = client
        .list_notifiers()
        .await
        .context("Failed to list notifiers")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&notifiers)?;
            println!("{json}");
        }
        _ => print_notifiers_table(&notifiers),
    }
    Ok(())
}

/// `zlayer notifier create --name <name> --kind <kind> [--webhook-url <url>] [--url <url>]`.
pub async fn create(
    name: String,
    kind: String,
    webhook_url: Option<String>,
    url: Option<String>,
) -> Result<()> {
    let parsed_kind = parse_kind(&kind)?;
    let config = build_config(parsed_kind, webhook_url, url)?;

    let client = DaemonClient::connect().await?;
    let notifier = client
        .create_notifier(&name, parsed_kind, &config)
        .await
        .context("Failed to create notifier")?;

    println!(
        "Created notifier '{}' (kind: {}, id: {})",
        notifier.name, notifier.kind, notifier.id
    );
    Ok(())
}

/// `zlayer notifier test <id>`.
pub async fn test(id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let resp = client
        .test_notifier(&id)
        .await
        .context("Failed to test notifier")?;

    if resp.success {
        println!("Test notification sent successfully: {}", resp.message);
    } else {
        println!("Test notification failed: {}", resp.message);
    }
    Ok(())
}

/// `zlayer notifier delete <id> [-y]`.
pub async fn delete(id: String, yes: bool) -> Result<()> {
    if !yes {
        eprint!("Delete notifier {id}? [y/N] ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled");
            return Ok(());
        }
    }

    let client = DaemonClient::connect().await?;
    client
        .delete_notifier(&id)
        .await
        .context("Failed to delete notifier")?;

    println!("Deleted notifier {id}");
    Ok(())
}

/// Parse a kind string into `NotifierKind`.
fn parse_kind(kind: &str) -> Result<NotifierKind> {
    match kind.to_lowercase().as_str() {
        "slack" => Ok(NotifierKind::Slack),
        "discord" => Ok(NotifierKind::Discord),
        "webhook" => Ok(NotifierKind::Webhook),
        "smtp" => Ok(NotifierKind::Smtp),
        _ => anyhow::bail!("Unknown notifier kind '{kind}'. Valid: slack, discord, webhook, smtp"),
    }
}

/// Build a `NotifierConfig` from CLI flags based on the kind.
fn build_config(
    kind: NotifierKind,
    webhook_url: Option<String>,
    url: Option<String>,
) -> Result<NotifierConfig> {
    match kind {
        NotifierKind::Slack => {
            let webhook_url = webhook_url
                .ok_or_else(|| anyhow::anyhow!("--webhook-url is required for Slack notifiers"))?;
            Ok(NotifierConfig::Slack { webhook_url })
        }
        NotifierKind::Discord => {
            let webhook_url = webhook_url.ok_or_else(|| {
                anyhow::anyhow!("--webhook-url is required for Discord notifiers")
            })?;
            Ok(NotifierConfig::Discord { webhook_url })
        }
        NotifierKind::Webhook => {
            let url =
                url.ok_or_else(|| anyhow::anyhow!("--url is required for Webhook notifiers"))?;
            Ok(NotifierConfig::Webhook {
                url,
                method: None,
                headers: None,
            })
        }
        NotifierKind::Smtp => {
            anyhow::bail!(
                "SMTP notifiers cannot be created via CLI flags. Use the API with a JSON body."
            );
        }
    }
}

/// Pretty-print notifiers as a plain-text table.
fn print_notifiers_table(notifiers: &[StoredNotifier]) {
    if notifiers.is_empty() {
        println!("(no notifiers)");
        return;
    }
    println!(
        "{:<36} {:<24} {:<10} {:<8}",
        "ID", "NAME", "KIND", "ENABLED"
    );
    for n in notifiers {
        // Truncate name for display
        let name = if n.name.len() > 22 {
            format!("{}...", &n.name[..22])
        } else {
            n.name.clone()
        };
        let enabled = if n.enabled { "yes" } else { "no" };
        println!("{:<36} {:<24} {:<10} {:<8}", n.id, name, n.kind, enabled);
    }
}
