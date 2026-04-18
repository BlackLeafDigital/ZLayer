//! `zlayer project` subcommand handlers.
//!
//! Full CRUD on projects, plus deployment linking/unlinking.  Dispatches
//! to the daemon's project endpoints
//! (`GET/POST/PATCH/DELETE /api/v1/projects[/{id}]`) via [`DaemonClient`].

use anyhow::{bail, Context, Result};
use dialoguer::Confirm;

use zlayer_client::DaemonClient;

/// `zlayer project ls [--output table|json]`.
pub async fn list(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let projects = client
        .list_projects()
        .await
        .context("Failed to list projects")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&projects)?;
            println!("{json}");
        }
        _ => print_projects_table(&projects),
    }
    Ok(())
}

/// `zlayer project create <name> [options]`.
#[allow(clippy::too_many_arguments)]
pub async fn create(
    name: String,
    git_url: Option<String>,
    git_branch: Option<String>,
    build_kind: Option<String>,
    build_path: Option<String>,
    description: Option<String>,
    registry_credential: Option<String>,
    git_credential: Option<String>,
    default_env: Option<String>,
) -> Result<()> {
    let client = DaemonClient::connect().await?;

    let body = serde_json::json!({
        "name": name,
        "git_url": git_url,
        "git_branch": git_branch,
        "build_kind": build_kind,
        "build_path": build_path,
        "description": description,
        "registry_credential_id": registry_credential,
        "git_credential_id": git_credential,
        "default_environment_id": default_env,
    });

    let project = client
        .create_project(&body)
        .await
        .context("Failed to create project")?;

    let id = project["id"].as_str().unwrap_or("(unknown)");
    let pname = project["name"].as_str().unwrap_or("(unknown)");
    println!("Created project '{pname}' (id: {id})");
    Ok(())
}

/// `zlayer project show <id> [--output json|table]`.
pub async fn show(id: String, output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let project = client
        .get_project(&id)
        .await
        .context("Failed to fetch project")?;

    if output == "table" {
        print_projects_table(&[project]);
    } else {
        let json = serde_json::to_string_pretty(&project)?;
        println!("{json}");
    }
    Ok(())
}

/// `zlayer project update <id> [options]`.
#[allow(clippy::too_many_arguments)]
pub async fn update(
    id: String,
    name: Option<String>,
    description: Option<String>,
    git_url: Option<String>,
    git_branch: Option<String>,
    build_kind: Option<String>,
    build_path: Option<String>,
    registry_credential: Option<String>,
    git_credential: Option<String>,
    default_env: Option<String>,
) -> Result<()> {
    // At least one field must be provided.
    if name.is_none()
        && description.is_none()
        && git_url.is_none()
        && git_branch.is_none()
        && build_kind.is_none()
        && build_path.is_none()
        && registry_credential.is_none()
        && git_credential.is_none()
        && default_env.is_none()
    {
        bail!("Nothing to update: pass at least one --<field> flag");
    }

    let client = DaemonClient::connect().await?;

    let body = serde_json::json!({
        "name": name,
        "description": description,
        "git_url": git_url,
        "git_branch": git_branch,
        "build_kind": build_kind,
        "build_path": build_path,
        "registry_credential_id": registry_credential,
        "git_credential_id": git_credential,
        "default_environment_id": default_env,
    });

    let project = client
        .update_project(&id, &body)
        .await
        .context("Failed to update project")?;

    let pname = project["name"].as_str().unwrap_or("(unknown)");
    println!("Updated project '{pname}' (id: {id})");
    Ok(())
}

/// `zlayer project delete <id> [--yes]`.
pub async fn delete(id: String, yes: bool) -> Result<()> {
    if !yes
        && !Confirm::new()
            .with_prompt(format!("Delete project {id}? This cannot be undone."))
            .default(false)
            .interact()
            .context("Failed to read confirmation")?
    {
        bail!("Aborted");
    }

    let client = DaemonClient::connect().await?;
    client
        .delete_project(&id)
        .await
        .context("Failed to delete project")?;

    println!("Deleted project {id}");
    Ok(())
}

/// `zlayer project link-deployment <id> <deployment>`.
pub async fn link_deployment(id: String, deployment: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    client
        .link_project_deployment(&id, &deployment)
        .await
        .context("Failed to link deployment")?;

    println!("Linked deployment '{deployment}' to project {id}");
    Ok(())
}

/// `zlayer project unlink-deployment <id> <deployment>`.
pub async fn unlink_deployment(id: String, deployment: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    client
        .unlink_project_deployment(&id, &deployment)
        .await
        .context("Failed to unlink deployment")?;

    println!("Unlinked deployment '{deployment}' from project {id}");
    Ok(())
}

/// `zlayer project pull <id>`.
///
/// Triggers a clone (first time) or fast-forward pull (subsequent calls)
/// of the project's configured git repository on the daemon. Prints the
/// resulting HEAD SHA and working-copy path so the caller can chain it
/// into further tooling.
pub async fn pull(id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let resp = client
        .project_pull(&id)
        .await
        .context("Failed to pull project repository")?;

    let git_url = resp["git_url"].as_str().unwrap_or("(unknown)");
    let branch = resp["branch"].as_str().unwrap_or("(unknown)");
    let sha = resp["sha"].as_str().unwrap_or("(unknown)");
    let path = resp["path"].as_str().unwrap_or("(unknown)");

    println!("Pulled project {id}");
    println!("  repo:   {git_url}");
    println!("  branch: {branch}");
    println!("  sha:    {sha}");
    println!("  path:   {path}");
    Ok(())
}

/// `zlayer project list-deployments <id>`.
pub async fn list_deployments(id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let deployments = client
        .list_project_deployments(&id)
        .await
        .context("Failed to list project deployments")?;

    if deployments.is_empty() {
        println!("(no deployments linked)");
    } else {
        println!("DEPLOYMENT");
        for d in &deployments {
            println!("{d}");
        }
    }
    Ok(())
}

/// `zlayer project auto-deploy <id> --enabled true|false`.
///
/// Sets the `auto_deploy` flag on the project via PATCH.
pub async fn auto_deploy(id: String, enabled: bool) -> Result<()> {
    let client = DaemonClient::connect().await?;

    let body = serde_json::json!({ "auto_deploy": enabled });
    let project = client
        .update_project(&id, &body)
        .await
        .context("Failed to update project auto_deploy")?;

    let pname = project["name"].as_str().unwrap_or("(unknown)");
    let state = if enabled { "enabled" } else { "disabled" };
    println!("Auto-deploy {state} for project '{pname}' (id: {id})");
    Ok(())
}

/// `zlayer project poll-interval <id> --seconds N`.
///
/// Sets the `poll_interval_secs` on the project via PATCH.
/// Pass `--seconds 0` to disable polling.
pub async fn poll_interval(id: String, seconds: u64) -> Result<()> {
    let client = DaemonClient::connect().await?;

    let interval: Option<u64> = if seconds == 0 { None } else { Some(seconds) };
    let body = serde_json::json!({ "poll_interval_secs": interval });
    let project = client
        .update_project(&id, &body)
        .await
        .context("Failed to update project poll_interval_secs")?;

    let pname = project["name"].as_str().unwrap_or("(unknown)");
    if seconds == 0 {
        println!("Polling disabled for project '{pname}' (id: {id})");
    } else {
        println!("Poll interval set to {seconds}s for project '{pname}' (id: {id})");
    }
    Ok(())
}

/// `zlayer project webhook show <id>`.
///
/// Prints the webhook URL template and HMAC secret. The secret is
/// generated on first call if it does not exist yet.
pub async fn webhook_show(id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let resp = client
        .get_project_webhook(&id)
        .await
        .context("Failed to get webhook info")?;

    let url = resp["url"].as_str().unwrap_or("(unknown)");
    let secret = resp["secret"].as_str().unwrap_or("(unknown)");

    println!("Webhook for project {id}");
    println!("  url:    {url}");
    println!("  secret: {secret}");
    println!();
    println!("Replace {{provider}} in the URL with: github, gitea, forgejo, or gitlab");
    Ok(())
}

/// `zlayer project webhook rotate <id>`.
///
/// Generates a new webhook secret, invalidating the old one.
pub async fn webhook_rotate(id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let resp = client
        .rotate_project_webhook(&id)
        .await
        .context("Failed to rotate webhook secret")?;

    let url = resp["url"].as_str().unwrap_or("(unknown)");
    let secret = resp["secret"].as_str().unwrap_or("(unknown)");

    println!("Rotated webhook secret for project {id}");
    println!("  url:    {url}");
    println!("  secret: {secret}");
    println!();
    println!("Update your git host with the new secret.");
    Ok(())
}

/// Pretty-print projects as a plain-text table.
fn print_projects_table(projects: &[serde_json::Value]) {
    if projects.is_empty() {
        println!("(no projects)");
        return;
    }
    println!(
        "{:<38} {:<20} {:<40} {:<14} {:<20} {:<24}",
        "ID", "NAME", "GIT URL", "BUILD", "OWNER", "CREATED"
    );
    for p in projects {
        let id = p["id"].as_str().unwrap_or("");
        let name = p["name"].as_str().unwrap_or("");
        let git_url = p["git_url"].as_str().unwrap_or("");
        let build = p["build_kind"].as_str().unwrap_or("");
        let owner = p["owner_id"].as_str().unwrap_or("");
        let created = p["created_at"].as_str().unwrap_or("");
        println!("{id:<38} {name:<20} {git_url:<40} {build:<14} {owner:<20} {created:<24}");
    }
}
