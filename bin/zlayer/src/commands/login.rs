//! `zlayer login <registry>` -- ergonomic wrapper over `credential registry add`.
//!
//! Stores a registry credential in the daemon's credential store via the same
//! `POST /api/v1/credentials/registry` endpoint that
//! [`crate::commands::credential::registry_add`] uses. Adds interactive prompts,
//! a `ZAuth` OIDC client-credentials exchange (`--zauth`), and an optional
//! `--default` flag that persists `ZLAYER_DEFAULT_REGISTRY` into the installed
//! daemon service unit so future daemon pulls use it as the last-resort default.

use anyhow::{bail, Context, Result};
use dialoguer::Password;

use zlayer_client::DaemonClient;

/// `zlayer login <registry> [--username <u>] [--password <pw>] [--auth-type basic|token] [--default] [--zauth]`.
pub async fn run(
    registry: String,
    username: Option<String>,
    password: Option<String>,
    auth_type: String,
    default: bool,
    zauth: bool,
) -> Result<()> {
    // Resolve the (username, password, auth_type) triple. `--zauth` mints a JWT
    // via OIDC client-credentials and stores it as a `token` credential under
    // the `oauth` username sentinel; the registry client's Token→Basic mapping
    // turns the JWT-in-password into a bearer at pull time.
    let (username, password, auth_type) = if zauth {
        let token = mint_zauth_token(&registry).await?;
        ("oauth".to_string(), token, "token".to_string())
    } else {
        let username = match username {
            Some(u) => u,
            None => read_username_from_stdin(&format!("Username for {registry}: "))?,
        };
        let password = match password {
            Some(p) => p,
            None => Password::new()
                .with_prompt(format!("Password/token for {username}@{registry}"))
                .interact()
                .context("Failed to read password")?,
        };
        (username, password, auth_type)
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
    println!("Logged in to {registry} as {username} (credential id: {id})");

    if default {
        persist_default_registry(&registry);
    }

    Ok(())
}

/// Persist `ZLAYER_DEFAULT_REGISTRY=<registry>` into the installed daemon
/// service unit (the only consumer is `zlayer_registry::default_registry_from_env`
/// at daemon puller construction). Falls back to actionable guidance when the
/// daemon is not running as a managed service.
fn persist_default_registry(registry: &str) {
    match crate::commands::daemon::set_daemon_env_var("ZLAYER_DEFAULT_REGISTRY", registry) {
        Ok(()) => {
            println!("Set ZLAYER_DEFAULT_REGISTRY={registry} in the daemon service unit.");
            println!("Restart the daemon to apply: `zlayer daemon restart`");
        }
        Err(e) => {
            // Dev / unmanaged daemon: nothing reads a file we could write, so
            // tell the user exactly how to make it take effect themselves.
            eprintln!("Could not update the daemon service unit: {e}");
            eprintln!(
                "Export the default registry in the daemon's environment instead, e.g.:\n    \
                 export ZLAYER_DEFAULT_REGISTRY={registry}\n  \
                 then restart the daemon so it picks up the value."
            );
        }
    }
}

/// Mint a registry bearer token via a `ZAuth` OIDC client-credentials exchange.
///
/// Reads `ZLAYER_ZAUTH_URL`, `ZLAYER_ZAUTH_CLIENT_ID`,
/// `ZLAYER_ZAUTH_CLIENT_SECRET` (all required) and optional `ZLAYER_ZAUTH_SCOPE`
/// from the environment, POSTs a `client_credentials` grant to
/// `{ZLAYER_ZAUTH_URL}/v1/oidc/token`, and returns the raw `access_token` JWT.
async fn mint_zauth_token(registry: &str) -> Result<String> {
    let url = required_env("ZLAYER_ZAUTH_URL")?;
    let client_id = required_env("ZLAYER_ZAUTH_CLIENT_ID")?;
    let client_secret = required_env("ZLAYER_ZAUTH_CLIENT_SECRET")?;
    let scope = std::env::var("ZLAYER_ZAUTH_SCOPE").ok();

    let token_url = format!("{}/v1/oidc/token", url.trim_end_matches('/'));

    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "client_credentials"),
        ("client_id", &client_id),
        ("client_secret", &client_secret),
    ];
    if let Some(scope) = scope.as_deref() {
        if !scope.is_empty() {
            form.push(("scope", scope));
        }
    }

    let resp = reqwest::Client::new()
        .post(&token_url)
        .form(&form)
        .send()
        .await
        .with_context(|| format!("Failed to reach ZAuth token endpoint {token_url}"))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .context("Failed to read ZAuth token response body")?;

    if !status.is_success() {
        bail!("ZAuth token request for {registry} failed ({status}): {body}");
    }

    let json: serde_json::Value =
        serde_json::from_str(&body).context("ZAuth token response was not valid JSON")?;

    let access_token = json
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("ZAuth token response did not include an access_token"))?;

    Ok(access_token.to_string())
}

/// Read a required environment variable, bailing with a clear message if unset
/// or empty.
fn required_env(name: &str) -> Result<String> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => bail!("--zauth requires the {name} environment variable to be set"),
    }
}

/// Read a single trimmed line from stdin. Used for the username prompt because
/// `dialoguer::Input` requires a feature flag that isn't enabled in
/// `bin/zlayer`'s `dialoguer` dependency (only `password` is on).
fn read_username_from_stdin(prompt: &str) -> Result<String> {
    use std::io::{BufRead, Write};
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("Failed to read username from stdin")?;
    let trimmed = line.trim().to_string();
    if trimmed.is_empty() {
        bail!("A username is required");
    }
    Ok(trimmed)
}
