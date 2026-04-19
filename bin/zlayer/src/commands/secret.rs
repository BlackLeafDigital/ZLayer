//! `zlayer secret` subcommand handlers.
//!
//! Dispatches to the daemon's secrets management endpoints.
//!
//! Two code paths coexist:
//!
//! 1. **Legacy scope-based** (`--env` not provided): routes through the
//!    existing `GET /api/v1/secrets`, `POST /api/v1/secrets`,
//!    `GET /api/v1/secrets/{name}`, `DELETE /api/v1/secrets/{name}` endpoints.
//! 2. **Environment-aware** (`--env <id|name>` provided): appends
//!    `?environment={env_id}` to the above endpoints and unlocks the new
//!    `bulk-import` and `?reveal=true` flows.
//!
//! When `--env` is a UUID it is passed through verbatim. When it is a name
//! (anything else), we resolve it by listing environments in the caller's
//! project (or globals if `--project` is omitted) and filtering by name.
//!
//! The outer `#[cfg(unix)]` on `pub mod secret` in `commands/mod.rs` already
//! gates this file to Unix, so no inner `#![cfg(unix)]` is needed here.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use zlayer_api::handlers::secrets::SecretMetadataResponse;

use crate::cli::{Cli, SecretCommands};
use zlayer_client::DaemonClient;

/// Entry point for `zlayer secret <subcommand>`.
pub(crate) async fn handle_secret(_cli: &Cli, cmd: &SecretCommands) -> Result<()> {
    match cmd {
        SecretCommands::Ls {
            output,
            env,
            project,
        } => list_secrets(output, env.as_deref(), project.as_deref()).await,
        SecretCommands::Create {
            name,
            value,
            env,
            project,
        } => create_secret(name, value, env.as_deref(), project.as_deref()).await,
        SecretCommands::Get {
            name,
            env,
            project,
            reveal,
        } => get_secret(name, env.as_deref(), project.as_deref(), *reveal).await,
        SecretCommands::Rm { name, env, project } => {
            remove_secret(name, env.as_deref(), project.as_deref()).await
        }
        SecretCommands::Set {
            assignment,
            env,
            project,
        } => set_secret(assignment, env, project.as_deref()).await,
        SecretCommands::Unset { name, env, project } => {
            unset_secret(name, env, project.as_deref()).await
        }
        SecretCommands::Import { file, env, project } => {
            import_secrets(file, env, project.as_deref()).await
        }
        SecretCommands::Export {
            env,
            format,
            project,
        } => export_secrets(env, format, project.as_deref()).await,
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_secrets(output: &str, env: Option<&str>, project: Option<&str>) -> Result<()> {
    let client = DaemonClient::connect().await?;

    if let Some(env_ref) = env {
        let env_id = resolve_env_id(&client, env_ref, project).await?;
        let secrets = client
            .list_secrets_in_env(&env_id)
            .await
            .context("Failed to list secrets")?;
        if output == "json" {
            println!("{}", serde_json::to_string_pretty(&secrets)?);
        } else {
            print_secrets_table(&secrets);
        }
        return Ok(());
    }

    // Legacy scope-based listing.
    let secrets = client.list_secrets().await?;
    if output == "json" {
        let json = serde_json::to_string_pretty(&secrets)?;
        println!("{json}");
    } else {
        println!(
            "{:<30} {:>10} {:>22} {:>22}",
            "NAME", "VERSION", "CREATED", "UPDATED"
        );
        for secret in &secrets {
            let name = secret.get("name").and_then(|v| v.as_str()).unwrap_or("-");
            let version = secret
                .get("version")
                .and_then(serde_json::Value::as_u64)
                .map_or_else(|| "-".to_string(), |v| v.to_string());
            let created = secret
                .get("created_at")
                .and_then(serde_json::Value::as_i64)
                .map_or_else(|| "-".to_string(), format_timestamp);
            let updated = secret
                .get("updated_at")
                .and_then(serde_json::Value::as_i64)
                .map_or_else(|| "-".to_string(), format_timestamp);
            println!("{name:<30} {version:>10} {created:>22} {updated:>22}");
        }
        if secrets.is_empty() {
            println!("(no secrets)");
        }
    }
    Ok(())
}

async fn create_secret(
    name: &str,
    value: &str,
    env: Option<&str>,
    project: Option<&str>,
) -> Result<()> {
    let client = DaemonClient::connect().await?;

    if let Some(env_ref) = env {
        let env_id = resolve_env_id(&client, env_ref, project).await?;
        let meta = client
            .set_secret_in_env(&env_id, name, value)
            .await
            .context("Failed to store secret")?;
        println!(
            "Secret '{}' stored in environment {} (version {})",
            meta.name, env_id, meta.version
        );
        return Ok(());
    }

    let result = client.create_secret(name, value).await?;
    let version = result
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    println!("Secret '{name}' stored (version {version})");
    Ok(())
}

async fn get_secret(
    name: &str,
    env: Option<&str>,
    project: Option<&str>,
    reveal: bool,
) -> Result<()> {
    let client = DaemonClient::connect().await?;

    if let Some(env_ref) = env {
        let env_id = resolve_env_id(&client, env_ref, project).await?;

        if reveal {
            let value = client
                .reveal_secret_in_env(&env_id, name)
                .await
                .context("Failed to reveal secret")?;
            println!("{value}");
            return Ok(());
        }

        // Metadata-only: fetch the list and find our name.
        let list = client
            .list_secrets_in_env(&env_id)
            .await
            .context("Failed to list secrets")?;
        let meta = list
            .iter()
            .find(|m| m.name == name)
            .ok_or_else(|| anyhow::anyhow!("Secret '{name}' not found in environment {env_id}"))?;
        println!("Name:       {}", meta.name);
        println!("Version:    {}", meta.version);
        println!("Created at: {}", format_timestamp(meta.created_at));
        println!("Updated at: {}", format_timestamp(meta.updated_at));
        return Ok(());
    }

    if reveal {
        bail!("--reveal requires --env (environment scope)");
    }

    let secret = client.get_secret(name).await?;
    let version = secret
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .map_or_else(|| "-".to_string(), |v| v.to_string());
    let created = secret
        .get("created_at")
        .and_then(serde_json::Value::as_i64)
        .map_or_else(|| "-".to_string(), format_timestamp);
    let updated = secret
        .get("updated_at")
        .and_then(serde_json::Value::as_i64)
        .map_or_else(|| "-".to_string(), format_timestamp);

    println!("Name:       {name}");
    println!("Version:    {version}");
    println!("Created at: {created}");
    println!("Updated at: {updated}");
    Ok(())
}

async fn remove_secret(name: &str, env: Option<&str>, project: Option<&str>) -> Result<()> {
    let client = DaemonClient::connect().await?;

    if let Some(env_ref) = env {
        let env_id = resolve_env_id(&client, env_ref, project).await?;
        client
            .delete_secret_in_env(&env_id, name)
            .await
            .context("Failed to remove secret")?;
        println!("Removed secret '{name}' from environment {env_id}");
        return Ok(());
    }

    client.delete_secret(name).await?;
    println!("Removed secret '{name}'");
    Ok(())
}

async fn set_secret(assignment: &str, env: &str, project: Option<&str>) -> Result<()> {
    let (name, value) = assignment
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("Expected NAME=VALUE, got '{assignment}'"))?;
    let name = name.trim();
    if name.is_empty() {
        bail!("Secret name cannot be empty");
    }

    let client = DaemonClient::connect().await?;
    let env_id = resolve_env_id(&client, env, project).await?;
    let meta = client
        .set_secret_in_env(&env_id, name, value)
        .await
        .context("Failed to set secret")?;
    println!(
        "Secret '{}' set in environment {} (version {})",
        meta.name, env_id, meta.version
    );
    Ok(())
}

async fn unset_secret(name: &str, env: &str, project: Option<&str>) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let env_id = resolve_env_id(&client, env, project).await?;
    client
        .delete_secret_in_env(&env_id, name)
        .await
        .context("Failed to unset secret")?;
    println!("Unset secret '{name}' in environment {env_id}");
    Ok(())
}

async fn import_secrets(file: &Path, env: &str, project: Option<&str>) -> Result<()> {
    let body = std::fs::read_to_string(file)
        .with_context(|| format!("Failed to read {}", file.display()))?;

    let client = DaemonClient::connect().await?;
    let env_id = resolve_env_id(&client, env, project).await?;
    let summary = client
        .bulk_import_secrets(&env_id, &body)
        .await
        .context("Bulk import failed")?;

    let created = summary
        .get("created")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let updated = summary
        .get("updated")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let errors: Vec<String> = summary
        .get("errors")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();

    println!(
        "Imported into environment {env_id}: {created} created, {updated} updated, {} error(s)",
        errors.len()
    );
    for err in &errors {
        eprintln!("  {err}");
    }
    if !errors.is_empty() {
        bail!("Import completed with errors");
    }
    Ok(())
}

async fn export_secrets(env: &str, format: &str, project: Option<&str>) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let env_id = resolve_env_id(&client, env, project).await?;

    let list = client
        .list_secrets_in_env(&env_id)
        .await
        .context("Failed to list secrets for export")?;

    // Reveal every secret one at a time — the daemon gates reveal to admins,
    // so a 403 here will propagate back to the user cleanly.
    let mut pairs: BTreeMap<String, String> = BTreeMap::new();
    for meta in &list {
        let value = client
            .reveal_secret_in_env(&env_id, &meta.name)
            .await
            .with_context(|| format!("Failed to reveal secret '{}'", meta.name))?;
        pairs.insert(meta.name.clone(), value);
    }

    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&pairs)?);
        }
        "env" => {
            for (name, value) in &pairs {
                println!("{name}={}", escape_dotenv_value(value));
            }
        }
        other => bail!("Unsupported --format '{other}' (expected 'env' or 'json')"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve an `--env` argument into a concrete environment id.
///
/// If `env` looks like a UUID, it is returned verbatim. Otherwise it is
/// treated as a name and resolved by listing environments in the given
/// `project` scope (or globals when `project` is `None`) and filtering by
/// name. An unknown name produces an error with the list of names that were
/// found in the scope so the user can pick one.
async fn resolve_env_id(client: &DaemonClient, env: &str, project: Option<&str>) -> Result<String> {
    if looks_like_uuid(env) {
        return Ok(env.to_string());
    }

    let envs = client
        .list_environments(project)
        .await
        .context("Failed to list environments while resolving --env")?;

    if let Some(found) = envs.iter().find(|e| e.name == env) {
        return Ok(found.id.clone());
    }

    let names: Vec<&str> = envs.iter().map(|e| e.name.as_str()).collect();
    let scope_desc = project.map_or_else(|| "global".to_string(), |p| format!("project {p}"));
    if names.is_empty() {
        bail!("No environments found in {scope_desc} scope; create one with 'zlayer env create'");
    }
    bail!(
        "Environment '{env}' not found in {scope_desc} scope; known environments: {}",
        names.join(", ")
    )
}

/// Cheap heuristic to decide whether a string looks like a UUID-v4 id.
///
/// The daemon produces UUIDs via `uuid::Uuid::new_v4().to_string()`, so we
/// can assume the canonical 8-4-4-4-12 hex layout (36 chars with four
/// hyphens). This is intentionally strict — anything else is treated as a
/// name so an accidental typo doesn't silently skip the name-resolution
/// lookup.
fn looks_like_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    let hyphen_positions = [8usize, 13, 18, 23];
    for (i, &b) in bytes.iter().enumerate() {
        let is_hyphen_position = hyphen_positions.contains(&i);
        if is_hyphen_position {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// Escape a value for dotenv output.
///
/// Wraps the value in double quotes if it contains whitespace, `#`, `=`, or
/// embedded quotes; otherwise returns it verbatim. This pairs with the
/// daemon's bulk-import behaviour (see `strip_dotenv_quotes` in
/// `handlers/secrets.rs`), which strips a single pair of surrounding quotes.
fn escape_dotenv_value(value: &str) -> String {
    let needs_quotes = value
        .chars()
        .any(|c| c.is_whitespace() || c == '#' || c == '=' || c == '"' || c == '\'');
    if !needs_quotes {
        return value.to_string();
    }
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn print_secrets_table(secrets: &[SecretMetadataResponse]) {
    if secrets.is_empty() {
        println!("(no secrets)");
        return;
    }
    println!(
        "{:<30} {:>10} {:>22} {:>22}",
        "NAME", "VERSION", "CREATED", "UPDATED"
    );
    for s in secrets {
        println!(
            "{:<30} {:>10} {:>22} {:>22}",
            s.name,
            s.version,
            format_timestamp(s.created_at),
            format_timestamp(s.updated_at),
        );
    }
}

/// Format a Unix timestamp as a human-readable UTC date-time string.
fn format_timestamp(ts: i64) -> String {
    // Simple formatting: seconds since epoch -> "YYYY-MM-DD HH:MM:SS"
    // Using chrono would be nicer, but we avoid adding a dependency just for this.
    // Fall back to raw timestamp if the value seems invalid.
    #[allow(clippy::cast_sign_loss)]
    if ts > 0 {
        let secs = ts as u64;
        // Days/hours/minutes/seconds arithmetic from epoch (1970-01-01)
        let days = secs / 86400;
        let time_of_day = secs % 86400;
        let hours = time_of_day / 3600;
        let minutes = (time_of_day % 3600) / 60;
        let seconds = time_of_day % 60;

        // Convert days since epoch to Y-M-D (simplified Gregorian)
        let (year, month, day) = days_to_ymd(days);
        format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}:{seconds:02}")
    } else {
        ts.to_string()
    }
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from Howard Hinnant's `civil_from_days`
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_looks_like_uuid_canonical() {
        assert!(looks_like_uuid("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn test_looks_like_uuid_rejects_names() {
        assert!(!looks_like_uuid("dev"));
        assert!(!looks_like_uuid("my-env"));
        assert!(!looks_like_uuid(""));
        // Right length, wrong hyphens.
        assert!(!looks_like_uuid("550e8400Xe29bX41d4Xa716X446655440000"));
        // Right layout, non-hex char.
        assert!(!looks_like_uuid("550e8400-e29b-41d4-a716-44665544000Z"));
    }

    #[test]
    fn test_escape_dotenv_value_plain() {
        assert_eq!(escape_dotenv_value("abc123"), "abc123");
    }

    #[test]
    fn test_escape_dotenv_value_with_spaces() {
        assert_eq!(escape_dotenv_value("a b"), "\"a b\"");
    }

    #[test]
    fn test_escape_dotenv_value_with_quote() {
        assert_eq!(escape_dotenv_value(r#"a"b"#), r#""a\"b""#);
    }

    #[test]
    fn test_escape_dotenv_value_with_equals() {
        assert_eq!(escape_dotenv_value("k=v"), "\"k=v\"");
    }
}
