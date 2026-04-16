//! `zlayer credential` subcommand handlers.
//!
//! Manages registry and git credentials stored by the daemon.  Dispatches
//! to the credential endpoints (`/api/v1/credentials/{registry,git}`) via
//! [`DaemonClient`].

use anyhow::{bail, Context, Result};
use dialoguer::{Confirm, Password};

use crate::daemon_client::DaemonClient;

// ---- Registry credentials ----

/// `zlayer credential registry ls [--output table|json]`.
pub async fn registry_list(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let creds = client
        .list_registry_credentials()
        .await
        .context("Failed to list registry credentials")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&creds)?;
            println!("{json}");
        }
        _ => print_registry_table(&creds),
    }
    Ok(())
}

/// `zlayer credential registry add --registry <url> --username <user> [--password <pw>] [--auth-type basic|token]`.
pub async fn registry_add(
    registry: String,
    username: String,
    password: Option<String>,
    auth_type: String,
) -> Result<()> {
    let password = match password {
        Some(p) => p,
        None => Password::new()
            .with_prompt(format!("Password for {username}@{registry}"))
            .interact()
            .context("Failed to read password")?,
    };

    let client = DaemonClient::connect().await?;

    let body = serde_json::json!({
        "registry": registry,
        "username": username,
        "password": password,
        "auth_type": auth_type,
    });

    let cred = client
        .create_registry_credential(&body)
        .await
        .context("Failed to create registry credential")?;

    let id = cred["id"].as_str().unwrap_or("(unknown)");
    println!("Created registry credential for {username}@{registry} (id: {id})");
    Ok(())
}

/// `zlayer credential registry delete <id> [--yes]`.
pub async fn registry_delete(id: String, yes: bool) -> Result<()> {
    if !yes
        && !Confirm::new()
            .with_prompt(format!(
                "Delete registry credential {id}? This cannot be undone."
            ))
            .default(false)
            .interact()
            .context("Failed to read confirmation")?
    {
        bail!("Aborted");
    }

    let client = DaemonClient::connect().await?;
    client
        .delete_registry_credential(&id)
        .await
        .context("Failed to delete registry credential")?;

    println!("Deleted registry credential {id}");
    Ok(())
}

// ---- Git credentials ----

/// `zlayer credential git ls [--output table|json]`.
pub async fn git_list(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let creds = client
        .list_git_credentials()
        .await
        .context("Failed to list git credentials")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&creds)?;
            println!("{json}");
        }
        _ => print_git_table(&creds),
    }
    Ok(())
}

/// `zlayer credential git add --name <name> [--value <pat|key>] --kind pat|ssh-key`.
pub async fn git_add(name: String, value: Option<String>, kind: String) -> Result<()> {
    let prompt_label = match kind.as_str() {
        "ssh-key" => "SSH private key (paste, then Enter)",
        _ => "Personal access token",
    };

    let value = match value {
        Some(v) => v,
        None => Password::new()
            .with_prompt(prompt_label)
            .interact()
            .context("Failed to read credential value")?,
    };

    let client = DaemonClient::connect().await?;

    let body = serde_json::json!({
        "name": name,
        "value": value,
        "kind": kind,
    });

    let cred = client
        .create_git_credential(&body)
        .await
        .context("Failed to create git credential")?;

    let id = cred["id"].as_str().unwrap_or("(unknown)");
    println!("Created git credential '{name}' ({kind}, id: {id})");
    Ok(())
}

/// `zlayer credential git delete <id> [--yes]`.
pub async fn git_delete(id: String, yes: bool) -> Result<()> {
    if !yes
        && !Confirm::new()
            .with_prompt(format!(
                "Delete git credential {id}? This cannot be undone."
            ))
            .default(false)
            .interact()
            .context("Failed to read confirmation")?
    {
        bail!("Aborted");
    }

    let client = DaemonClient::connect().await?;
    client
        .delete_git_credential(&id)
        .await
        .context("Failed to delete git credential")?;

    println!("Deleted git credential {id}");
    Ok(())
}

// ---- Table printers ----

/// Pretty-print registry credentials as a plain-text table.
fn print_registry_table(creds: &[serde_json::Value]) {
    if creds.is_empty() {
        println!("(no registry credentials)");
        return;
    }
    println!(
        "{:<38} {:<30} {:<20} {:<10} {:<24}",
        "ID", "REGISTRY", "USERNAME", "AUTH", "CREATED"
    );
    for c in creds {
        let id = c["id"].as_str().unwrap_or("");
        let registry = c["registry"].as_str().unwrap_or("");
        let username = c["username"].as_str().unwrap_or("");
        let auth = c["auth_type"].as_str().unwrap_or("");
        let created = c["created_at"].as_str().unwrap_or("");
        println!("{id:<38} {registry:<30} {username:<20} {auth:<10} {created:<24}");
    }
}

/// Pretty-print git credentials as a plain-text table.
fn print_git_table(creds: &[serde_json::Value]) {
    if creds.is_empty() {
        println!("(no git credentials)");
        return;
    }
    println!(
        "{:<38} {:<30} {:<10} {:<24}",
        "ID", "NAME", "KIND", "CREATED"
    );
    for c in creds {
        let id = c["id"].as_str().unwrap_or("");
        let name = c["name"].as_str().unwrap_or("");
        let kind = c["kind"].as_str().unwrap_or("");
        let created = c["created_at"].as_str().unwrap_or("");
        println!("{id:<38} {name:<30} {kind:<10} {created:<24}");
    }
}
