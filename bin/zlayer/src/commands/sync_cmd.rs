//! `zlayer sync` subcommand handlers.
//!
//! CRUD on syncs, plus diff and apply.  Dispatches to the daemon's sync
//! endpoints (`GET/POST/DELETE /api/v1/syncs[/{id}]`) via [`DaemonClient`].

use anyhow::{bail, Context, Result};
use dialoguer::Confirm;

use crate::daemon_client::DaemonClient;

/// `zlayer sync ls [--output table|json]`.
pub async fn list(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let syncs = client.list_syncs().await.context("Failed to list syncs")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&syncs)?;
            println!("{json}");
        }
        _ => print_syncs_table(&syncs),
    }
    Ok(())
}

/// `zlayer sync create --name <name> [--project <id>] --path <git_path> [--auto-apply]`.
pub async fn create(
    name: String,
    project_id: Option<String>,
    git_path: String,
    auto_apply: bool,
) -> Result<()> {
    let client = DaemonClient::connect().await?;

    let body = serde_json::json!({
        "name": name,
        "project_id": project_id,
        "git_path": git_path,
        "auto_apply": auto_apply,
    });

    let sync = client
        .create_sync(&body)
        .await
        .context("Failed to create sync")?;

    let id = sync["id"].as_str().unwrap_or("(unknown)");
    let sname = sync["name"].as_str().unwrap_or("(unknown)");
    println!("Created sync '{sname}' (id: {id})");
    Ok(())
}

/// `zlayer sync diff <id>`.
pub async fn diff(id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let diff = client
        .diff_sync(&id)
        .await
        .context("Failed to compute sync diff")?;

    let json = serde_json::to_string_pretty(&diff)?;
    println!("{json}");
    Ok(())
}

/// `zlayer sync apply <id>`.
///
/// The daemon returns `{ results: [...], applied_sha?, summary }`. Each
/// result carries `{ resource, kind, action, status, error? }` where
/// `action` is one of `create | update | delete | skip` and `status` is
/// `ok | error`.
pub async fn apply(id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let result = client
        .apply_sync(&id)
        .await
        .context("Failed to apply sync")?;

    // Top-line: summary + applied SHA.
    if let Some(summary) = result["summary"].as_str() {
        println!("{summary}");
    }
    if let Some(sha) = result["applied_sha"].as_str() {
        println!("applied at: {sha}");
    }

    // Per-resource table.
    if let Some(results) = result["results"].as_array() {
        if results.is_empty() {
            println!("(no resources reconciled)");
            return Ok(());
        }

        println!();
        println!(
            "{action:<7}  {status:<5}  {kind:<11}  {resource:<30}  DETAIL",
            action = "ACTION",
            status = "STAT",
            kind = "KIND",
            resource = "RESOURCE",
        );
        println!("{}", "-".repeat(100));
        for r in results {
            let action = r["action"].as_str().unwrap_or("-");
            let status = r["status"].as_str().unwrap_or("-");
            let kind = r["kind"].as_str().unwrap_or("-");
            let resource = r["resource"].as_str().unwrap_or("-");
            let detail = r["error"].as_str().unwrap_or("");
            println!("{action:<7}  {status:<5}  {kind:<11}  {resource:<30}  {detail}");
        }
    }

    Ok(())
}

/// `zlayer sync delete <id> [--yes]`.
pub async fn delete(id: String, skip_confirm: bool) -> Result<()> {
    if !skip_confirm {
        let confirmed = Confirm::new()
            .with_prompt(format!("Delete sync '{id}'?"))
            .default(false)
            .interact()
            .context("confirmation prompt failed")?;
        if !confirmed {
            bail!("aborted");
        }
    }

    let client = DaemonClient::connect().await?;
    client
        .delete_sync(&id)
        .await
        .context("Failed to delete sync")?;
    println!("Deleted sync '{id}'");
    Ok(())
}

// ---------------------------------------------------------------------------
// Table printer
// ---------------------------------------------------------------------------

fn print_syncs_table(syncs: &[serde_json::Value]) {
    if syncs.is_empty() {
        println!("No syncs found.");
        return;
    }

    println!(
        "{id:<36}  {name:<20}  {project:<36}  {path:<30}  AUTO-APPLY",
        id = "ID",
        name = "NAME",
        project = "PROJECT",
        path = "PATH",
    );
    println!("{}", "-".repeat(160));

    for s in syncs {
        let id = s["id"].as_str().unwrap_or("-");
        let name = s["name"].as_str().unwrap_or("-");
        let project = s["project_id"].as_str().unwrap_or("-");
        let path = s["git_path"].as_str().unwrap_or("-");
        let auto = s["auto_apply"].as_bool().unwrap_or(false);
        println!("{id:<36}  {name:<20}  {project:<36}  {path:<30}  {auto}");
    }
}
