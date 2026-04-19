//! `zlayer user` subcommand handlers.
//!
//! Admin-only operations on user accounts. Over the local Unix socket the
//! CLI is auto-authenticated as the `local-admin` role, so these commands
//! work out of the box on a fresh install. Non-admin users hitting a remote
//! daemon receive a 403.

use anyhow::{bail, Context, Result};
use dialoguer::Password;
use zlayer_api::handlers::auth::UserView;
use zlayer_api::handlers::users::{CreateUserRequest, SetPasswordRequest, UpdateUserRequest};
use zlayer_api::storage::UserRole;

use zlayer_client::DaemonClient;

/// `zlayer user list` -- print all users as a table or JSON.
pub async fn list(output: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let users = client.list_users().await.context("Failed to list users")?;

    match output {
        "json" => {
            let json = serde_json::to_string_pretty(&users)?;
            println!("{json}");
        }
        _ => print_users_table(&users),
    }
    Ok(())
}

/// `zlayer user create` -- admin creates a new user account.
pub async fn create(
    email: String,
    password: Option<String>,
    role: UserRole,
    display_name: Option<String>,
) -> Result<()> {
    let password = match password {
        Some(p) => p,
        None => Password::new()
            .with_prompt(format!("Password for {email}"))
            .with_confirmation("Confirm password", "Passwords don't match")
            .interact()
            .context("Failed to read password")?,
    };

    let client = DaemonClient::connect().await?;
    let req = CreateUserRequest {
        email: email.clone(),
        password,
        display_name,
        role,
    };
    let user = client
        .create_user(&req)
        .await
        .context("Failed to create user")?;

    println!(
        "Created user {} <{}> (role: {}, id: {})",
        user.display_name, user.email, user.role, user.id
    );
    Ok(())
}

/// `zlayer user set-role <id> --role <role>` -- change a user's role.
pub async fn set_role(id: String, role: UserRole) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let req = UpdateUserRequest {
        display_name: None,
        role: Some(role),
        is_active: None,
    };
    let user = client
        .update_user(&id, &req)
        .await
        .context("Failed to update user role")?;
    println!(
        "User {} <{}> role is now {}",
        user.display_name, user.email, user.role
    );
    Ok(())
}

/// `zlayer user set-password <id>` -- admin resets another user's password
/// (or self-service when id matches the caller's session).
pub async fn set_password(id: String) -> Result<()> {
    let new_password = Password::new()
        .with_prompt("New password")
        .with_confirmation("Confirm new password", "Passwords don't match")
        .interact()
        .context("Failed to read password")?;

    let client = DaemonClient::connect().await?;
    let req = SetPasswordRequest {
        new_password,
        // Admin resetting someone else's password: current_password not required.
        // If the daemon decides this is a self-service request it will return
        // 400 asking for current_password; we can add an interactive prompt for
        // that path in a follow-up if the need arises.
        current_password: None,
    };

    client
        .set_user_password(&id, &req)
        .await
        .context("Failed to set password")?;

    println!("Password updated for user {id}");
    Ok(())
}

/// `zlayer user delete <id>` -- admin deletes a user account.
pub async fn delete(id: String, yes: bool) -> Result<()> {
    if !yes {
        use dialoguer::Confirm;
        if !Confirm::new()
            .with_prompt(format!("Delete user {id}? This cannot be undone."))
            .default(false)
            .interact()
            .context("Failed to read confirmation")?
        {
            bail!("Aborted");
        }
    }

    let client = DaemonClient::connect().await?;
    client
        .delete_user(&id)
        .await
        .context("Failed to delete user")?;

    println!("Deleted user {id}");
    Ok(())
}

/// Pretty-print users as a plain-text table. Matches the style used by
/// `commands/job.rs::list_jobs`.
fn print_users_table(users: &[UserView]) {
    if users.is_empty() {
        println!("(no users)");
        return;
    }
    println!(
        "{:<38} {:<30} {:<20} {:<6} {:<8}",
        "ID", "EMAIL", "DISPLAY NAME", "ROLE", "ACTIVE"
    );
    for u in users {
        let active = if u.is_active { "yes" } else { "no" };
        println!(
            "{:<38} {:<30} {:<20} {:<6} {:<8}",
            u.id, u.email, u.display_name, u.role, active
        );
    }
}
