//! `zlayer user` subcommand handlers.
//!
//! Admin-only operations on user accounts. Over the local Unix socket the
//! CLI is auto-authenticated as the `local-admin` role, so these commands
//! work out of the box on a fresh install. Non-admin users hitting a remote
//! daemon receive a 403.

use std::path::Path;

use anyhow::{bail, Context, Result};
use dialoguer::Password;
use rand::distr::Alphanumeric;
use rand::Rng;
use zlayer_types::api::auth::UserView;
use zlayer_types::api::users::{CreateUserRequest, SetPasswordRequest, UpdateUserRequest};
use zlayer_types::storage::UserRole;

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

/// `zlayer user set-password` -- admin resets a user's password (or
/// self-service when the target matches the caller's session). The target is
/// selected either by the positional `id` or `--email`. The new password
/// source is one of `--password`, `--password-file`, `--random`, or an
/// interactive prompt when none is supplied.
#[allow(clippy::too_many_arguments)]
pub async fn set_password(
    id: Option<String>,
    email: Option<String>,
    password: Option<String>,
    password_file: Option<std::path::PathBuf>,
    random: bool,
    no_confirm: bool,
) -> Result<()> {
    if id.is_some() && email.is_some() {
        bail!("--email and a positional id are mutually exclusive");
    }

    let client = DaemonClient::connect().await?;

    let resolved_id = match (id, email) {
        (Some(i), None) => i,
        (None, Some(email)) => resolve_email_to_id(&client, &email).await?,
        (None, None) => bail!("either a positional id or --email is required"),
        (Some(_), Some(_)) => unreachable!(),
    };

    let (new_password, generated) =
        resolve_new_password(password, password_file.as_deref(), random)?;

    if !no_confirm {
        use dialoguer::Confirm;
        let prompt = format!("Rotate password for user {resolved_id}?");
        let ok = Confirm::new()
            .with_prompt(prompt)
            .default(false)
            .interact()
            .context("Failed to read confirmation")?;
        if !ok {
            bail!("Aborted");
        }
    }

    let req = SetPasswordRequest {
        new_password: new_password.clone(),
        current_password: None,
    };

    client
        .set_user_password(&resolved_id, &req)
        .await
        .context("Failed to set password")?;

    if generated {
        println!();
        println!("Generated password for {resolved_id}:");
        println!("  {new_password}");
        println!();
        println!("This is the only time it will be shown. Store it now.");
    } else {
        println!("Password updated for user {resolved_id}");
    }
    Ok(())
}

async fn resolve_email_to_id(client: &DaemonClient, email: &str) -> Result<String> {
    let email_lc = email.trim().to_lowercase();
    let users = client.list_users().await.context("Failed to list users")?;
    users
        .into_iter()
        .find(|u| u.email.eq_ignore_ascii_case(&email_lc))
        .map(|u| u.id)
        .ok_or_else(|| anyhow::anyhow!("No user with email {email_lc}"))
}

fn resolve_new_password(
    inline: Option<String>,
    file: Option<&Path>,
    random: bool,
) -> Result<(String, bool)> {
    let sources = [inline.is_some(), file.is_some(), random]
        .iter()
        .filter(|b| **b)
        .count();
    if sources > 1 {
        bail!("at most one of --password, --password-file, --random may be supplied");
    }

    if let Some(p) = inline {
        return Ok((p, false));
    }
    if let Some(path) = file {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading --password-file {}", path.display()))?;
        let trimmed = raw.trim_end_matches(['\n', '\r']).to_string();
        if trimmed.is_empty() {
            bail!("--password-file {} is empty", path.display());
        }
        return Ok((trimmed, false));
    }
    if random {
        let pw: String = rand::rng()
            .sample_iter(Alphanumeric)
            .take(32)
            .map(char::from)
            .collect();
        return Ok((pw, true));
    }

    let pw = Password::new()
        .with_prompt("New password")
        .with_confirmation("Confirm new password", "Passwords don't match")
        .interact()
        .context("Failed to read password")?;
    Ok((pw, false))
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

#[cfg(test)]
mod tests {
    use super::resolve_new_password;
    use std::io::Write;
    use zlayer_paths::ZLayerDirs;

    #[test]
    fn inline_password_returned_as_is() {
        let (pw, generated) =
            resolve_new_password(Some("hunter2hunter2".to_string()), None, false).unwrap();
        assert_eq!(pw, "hunter2hunter2");
        assert!(!generated);
    }

    #[test]
    fn password_file_is_read_and_trimmed() {
        let tmp = ZLayerDirs::system_default()
            .scratch_file("password-file-is-read-and-trimmed-")
            .unwrap();
        writeln!(tmp.as_file(), "filesecret123").unwrap();
        let (pw, generated) = resolve_new_password(None, Some(tmp.path()), false).unwrap();
        assert_eq!(pw, "filesecret123");
        assert!(!generated);
    }

    #[test]
    fn password_file_empty_errors() {
        let tmp = ZLayerDirs::system_default()
            .scratch_file("password-file-empty-errors-")
            .unwrap();
        let err = resolve_new_password(None, Some(tmp.path()), false).unwrap_err();
        assert!(err.to_string().contains("empty"), "unexpected: {err}");
    }

    #[test]
    fn password_file_missing_errors() {
        let path = std::path::Path::new("/nonexistent/zlayer/set_password/pw");
        let err = resolve_new_password(None, Some(path), false).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("reading --password-file"), "unexpected: {msg}");
    }

    #[test]
    fn random_generates_32_char_alphanumeric() {
        let (pw, generated) = resolve_new_password(None, None, true).unwrap();
        assert_eq!(pw.len(), 32);
        assert!(pw.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(generated);
    }

    #[test]
    fn multiple_sources_rejected() {
        let err = resolve_new_password(Some("a".to_string()), None, true).unwrap_err();
        assert!(err.to_string().contains("at most one"), "unexpected: {err}");
    }
}
