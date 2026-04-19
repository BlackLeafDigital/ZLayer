//! `zlayer auth` subcommand handlers.
//!
//! The CLI talks to the local daemon over a Unix socket. The socket has an
//! auto-injected local-admin bearer, so these commands work even on a fresh
//! install. `login` additionally persists a user-specific JWT to
//! `~/.zlayer/session.json` so subsequent commands run as that user rather
//! than the local admin. `logout` deletes that file and best-effort invalidates
//! the server-side cookie.

use anyhow::{bail, Context, Result};
use chrono::{Duration, Utc};
use dialoguer::{Confirm, Password};
use tracing::info;
use zlayer_api::handlers::auth::BootstrapRequest;

use zlayer_client::session::{self, Session};
use zlayer_client::DaemonClient;

/// `zlayer auth bootstrap` -- create the first admin user.
///
/// Fails with a clear error if any user already exists (the server returns
/// 409 Conflict).
pub async fn bootstrap(
    email: String,
    password: Option<String>,
    display_name: Option<String>,
) -> Result<()> {
    let password = match password {
        Some(p) => p,
        None => prompt_password_confirmed()?,
    };

    let client = DaemonClient::connect().await?;
    let req = BootstrapRequest {
        email: email.clone(),
        password,
        display_name,
    };
    let resp = client
        .auth_bootstrap(&req)
        .await
        .context("Bootstrap failed")?;

    info!(email = %resp.user.email, id = %resp.user.id, "Bootstrap complete");

    println!(
        "Created admin user: {} <{}> (id: {})",
        resp.user.display_name, resp.user.email, resp.user.id
    );
    println!("You can now run `zlayer auth login` to sign in as this user.");

    Ok(())
}

/// `zlayer auth login` -- authenticate and save a JWT session.
pub async fn login(email: String, password: Option<String>) -> Result<()> {
    let password = match password {
        Some(p) => p,
        None => Password::new()
            .with_prompt(format!("Password for {email}"))
            .interact()
            .context("Failed to read password")?,
    };

    let client = DaemonClient::connect().await?;

    // CLI holds a JWT (not a cookie jar), so hit `/auth/token` rather than
    // `/auth/login`. The endpoint accepts the email/password pair under the
    // same `{api_key, api_secret}` wire shape.
    let token_json = client
        .auth_token(&email, &password)
        .await
        .context("Login failed")?;

    let token = token_json
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Login response did not include an access_token"))?
        .to_string();

    let expires_in_secs = token_json
        .get("expires_in")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(3600);

    let expires_at = Utc::now()
        + i64::try_from(expires_in_secs)
            .ok()
            .and_then(Duration::try_seconds)
            .unwrap_or_else(|| Duration::hours(1));

    let session = Session {
        token,
        email: email.clone(),
        expires_at,
    };
    session::write_session(&session).context("Failed to persist session file")?;

    println!("Logged in as {email}");
    println!(
        "Session saved to ~/.zlayer/session.json (expires {})",
        session.expires_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    Ok(())
}

/// `zlayer auth logout` -- delete local session, best-effort notify server.
pub async fn logout() -> Result<()> {
    let existed = session::delete_session().context("Failed to delete session file")?;

    // Best-effort server-side logout. If the daemon isn't reachable we still
    // consider logout successful because the local session is gone.
    match DaemonClient::try_connect().await {
        Ok(Some(client)) => {
            if let Err(e) = client.auth_logout().await {
                tracing::debug!(error = %e, "Server-side logout failed (ignoring)");
            }
        }
        Ok(None) => {
            tracing::debug!("Daemon not running; skipping server-side logout");
        }
        Err(e) => {
            tracing::debug!(error = %e, "Daemon connect failed; skipping server-side logout");
        }
    }

    if existed {
        println!("Logged out");
    } else {
        println!("Not logged in (no session file found)");
    }
    Ok(())
}

/// `zlayer auth whoami` -- print the current authenticated user.
pub async fn whoami() -> Result<()> {
    let client = DaemonClient::connect().await?;
    let user = client
        .auth_whoami()
        .await
        .context("Failed to fetch current user")?;

    println!("Signed in as: {} <{}>", user.display_name, user.email);
    println!("  ID:     {}", user.id);
    println!("  Role:   {}", user.role);
    println!("  Active: {}", if user.is_active { "yes" } else { "no" });
    if let Some(last) = user.last_login_at {
        println!("  Last login: {}", last.format("%Y-%m-%d %H:%M:%S UTC"));
    }

    // Note: over the Unix socket this always returns "local-admin" unless the
    // caller has written a session file. Mention that for clarity.
    if session::read_session().ok().flatten().is_none() {
        println!();
        println!("(No session file; authenticated via local admin bearer)");
    }
    Ok(())
}

/// Prompt for a password twice and verify they match.
fn prompt_password_confirmed() -> Result<String> {
    let password = Password::new()
        .with_prompt("Password")
        .with_confirmation("Confirm password", "Passwords don't match")
        .interact()
        .context("Failed to read password")?;
    if password.len() < 8
        && !Confirm::new()
            .with_prompt("Password is shorter than 8 characters. Continue?")
            .default(false)
            .interact()
            .context("Failed to read confirmation")?
    {
        bail!("Aborted by user");
    }
    Ok(password)
}
