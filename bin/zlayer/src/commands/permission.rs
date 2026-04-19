//! `zlayer permission` subcommand handlers.
//!
//! Admin-only operations for granting/revoking resource-level permissions.

use anyhow::{Context, Result};
use zlayer_api::storage::{PermissionLevel, StoredPermission, SubjectKind};
use zlayer_api::GrantPermissionRequest;

use zlayer_client::DaemonClient;

/// `zlayer permission list` -- list permissions for a user or group.
pub async fn list(user: Option<String>, group: Option<String>, output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let perms = client
        .list_permissions(user.as_deref(), group.as_deref())
        .await
        .context("Failed to list permissions")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&perms)?;
            println!("{json}");
        }
        _ => print_permissions_table(&perms),
    }
    Ok(())
}

/// `zlayer permission grant` -- admin grants a permission.
pub async fn grant(
    subject_kind: SubjectKind,
    subject_id: String,
    resource_kind: String,
    resource_id: Option<String>,
    level: PermissionLevel,
) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let req = GrantPermissionRequest {
        subject_kind,
        subject_id: subject_id.clone(),
        resource_kind: resource_kind.clone(),
        resource_id: resource_id.clone(),
        level,
    };
    let perm = client
        .grant_permission(&req)
        .await
        .context("Failed to grant permission")?;

    let resource = match resource_id {
        Some(rid) => format!("{resource_kind}/{rid}"),
        None => format!("{resource_kind} (wildcard)"),
    };

    println!(
        "Granted {level} on {resource} to {kind} {subject_id} (id: {pid})",
        level = perm.level,
        kind = perm.subject_kind,
        pid = perm.id,
    );
    Ok(())
}

/// `zlayer permission revoke` -- admin revokes a permission.
pub async fn revoke(id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    client
        .revoke_permission(&id)
        .await
        .context("Failed to revoke permission")?;

    println!("Revoked permission {id}");
    Ok(())
}

/// Pretty-print permissions as a plain-text table.
fn print_permissions_table(perms: &[StoredPermission]) {
    if perms.is_empty() {
        println!("(no permissions)");
        return;
    }
    println!(
        "{:<38} {:<8} {:<38} {:<16} {:<38} {:<8}",
        "ID", "SUBJECT", "SUBJECT_ID", "RESOURCE_KIND", "RESOURCE_ID", "LEVEL"
    );
    for p in perms {
        let rid = p.resource_id.as_deref().unwrap_or("(all)");
        println!(
            "{:<38} {:<8} {:<38} {:<16} {:<38} {:<8}",
            p.id, p.subject_kind, p.subject_id, p.resource_kind, rid, p.level
        );
    }
}
