//! `zlayer group` subcommand handlers.
//!
//! Admin-only CRUD for user groups and membership management.

use anyhow::{bail, Context, Result};
use zlayer_api::storage::StoredUserGroup;
use zlayer_api::CreateGroupRequest;

use zlayer_client::DaemonClient;

/// `zlayer group list` -- print all groups as a table or JSON.
pub async fn list(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let groups = client
        .list_groups()
        .await
        .context("Failed to list groups")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&groups)?;
            println!("{json}");
        }
        _ => print_groups_table(&groups),
    }
    Ok(())
}

/// `zlayer group create` -- admin creates a new group.
pub async fn create(name: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let req = CreateGroupRequest {
        name: name.clone(),
        description: None,
    };
    let group = client
        .create_group(&req)
        .await
        .context("Failed to create group")?;

    println!("Created group '{}' (id: {})", group.name, group.id);
    Ok(())
}

/// `zlayer group delete` -- admin deletes a group.
pub async fn delete(id: String, yes: bool) -> Result<()> {
    if !yes {
        use dialoguer::Confirm;
        if !Confirm::new()
            .with_prompt(format!("Delete group {id}? This cannot be undone."))
            .default(false)
            .interact()
            .context("Failed to read confirmation")?
        {
            bail!("Aborted");
        }
    }

    let client = DaemonClient::connect().await?;
    client
        .delete_group(&id)
        .await
        .context("Failed to delete group")?;

    println!("Deleted group {id}");
    Ok(())
}

/// `zlayer group member add` -- admin adds a user to a group.
pub async fn member_add(group_id: String, user_id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    client
        .add_group_member(&group_id, &user_id)
        .await
        .context("Failed to add member")?;

    println!("Added user {user_id} to group {group_id}");
    Ok(())
}

/// `zlayer group member remove` -- admin removes a user from a group.
pub async fn member_remove(group_id: String, user_id: String) -> Result<()> {
    let client = DaemonClient::connect().await?;
    client
        .remove_group_member(&group_id, &user_id)
        .await
        .context("Failed to remove member")?;

    println!("Removed user {user_id} from group {group_id}");
    Ok(())
}

/// Pretty-print groups as a plain-text table.
fn print_groups_table(groups: &[StoredUserGroup]) {
    if groups.is_empty() {
        println!("(no groups)");
        return;
    }
    println!("{:<38} {:<30} {:<40}", "ID", "NAME", "DESCRIPTION");
    for g in groups {
        let desc = g.description.as_deref().unwrap_or("-");
        println!("{:<38} {:<30} {:<40}", g.id, g.name, desc);
    }
}
