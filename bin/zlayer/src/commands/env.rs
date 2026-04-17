//! `zlayer env` subcommand handlers.
//!
//! Manages deployment/runtime environments (e.g. `dev`, `staging`, `prod`).
//! Environments are isolated namespaces for secrets — they live either
//! globally (`project_id = None`) or under a specific project.
//!
//! Dispatches to the daemon's environment endpoints
//! (`GET/POST/PATCH/DELETE /api/v1/environments[/id]`) via [`DaemonClient`].
//!
//! The outer `#[cfg(unix)]` on `pub mod env` in `commands/mod.rs` already
//! gates this file to Unix, so no inner `#![cfg(unix)]` is needed here.

use anyhow::{bail, Context, Result};
use dialoguer::Confirm;
use zlayer_api::storage::StoredEnvironment;

use crate::daemon_client::DaemonClient;

/// `zlayer env ls [--project <id>] [--output table|json]`.
pub async fn list(project: Option<String>, output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let envs = client
        .list_environments(project.as_deref())
        .await
        .context("Failed to list environments")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&envs)?;
            println!("{json}");
        }
        _ => print_envs_table(&envs),
    }
    Ok(())
}

/// `zlayer env create <name> [--project <id>] [--description <text>]`.
pub async fn create(
    name: String,
    project: Option<String>,
    description: Option<String>,
) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let env = client
        .create_environment(&name, project.as_deref(), description.as_deref())
        .await
        .context("Failed to create environment")?;

    let scope = env
        .project_id
        .as_deref()
        .map_or_else(|| "global".to_string(), |p| format!("project {p}"));
    println!(
        "Created environment '{}' ({scope}, id: {})",
        env.name, env.id
    );
    Ok(())
}

/// `zlayer env show <id> [--output table|json]`.
pub async fn get(id: String, output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let env = client
        .get_environment(&id)
        .await
        .context("Failed to fetch environment")?;

    if output == "table" {
        print_envs_table(std::slice::from_ref(&env));
    } else {
        let json = serde_json::to_string_pretty(&env)?;
        println!("{json}");
    }
    Ok(())
}

/// `zlayer env update <id> [--name <name>] [--description <text>]`.
pub async fn update(id: String, name: Option<String>, description: Option<String>) -> Result<()> {
    if name.is_none() && description.is_none() {
        bail!("Nothing to update: pass --name and/or --description");
    }

    let client = DaemonClient::connect().await?;
    let env = client
        .update_environment(&id, name.as_deref(), description.as_deref())
        .await
        .context("Failed to update environment")?;

    println!("Updated environment '{}' (id: {})", env.name, env.id);
    Ok(())
}

/// `zlayer env delete <id> [--yes]`.
pub async fn delete(id: String, yes: bool) -> Result<()> {
    if !yes
        && !Confirm::new()
            .with_prompt(format!("Delete environment {id}? This cannot be undone."))
            .default(false)
            .interact()
            .context("Failed to read confirmation")?
    {
        bail!("Aborted");
    }

    let client = DaemonClient::connect().await?;
    client
        .delete_environment(&id)
        .await
        .context("Failed to delete environment")?;

    println!("Deleted environment {id}");
    Ok(())
}

/// Pretty-print environments as a plain-text table. Mirrors the style used
/// by `commands/user.rs::print_users_table`.
fn print_envs_table(envs: &[StoredEnvironment]) {
    if envs.is_empty() {
        println!("(no environments)");
        return;
    }
    println!(
        "{:<38} {:<20} {:<20} {:<30}",
        "ID", "NAME", "PROJECT", "DESCRIPTION"
    );
    for env in envs {
        let project = env.project_id.as_deref().unwrap_or("(global)");
        let description = env.description.as_deref().unwrap_or("");
        println!(
            "{:<38} {:<20} {:<20} {:<30}",
            env.id, env.name, project, description
        );
    }
}
