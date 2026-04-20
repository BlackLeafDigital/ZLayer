//! `zlayer run` — locally spawn a command with secrets injected as env vars.
//!
//! Merges secrets from the primary env with any extra merge envs (including
//! an implicit `global` unless --no-global), inherits the parent process env,
//! then execs the target command via `tokio::process`. Exit code propagates.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use tokio::process::Command;
use tracing::{debug, warn};

use zlayer_client::DaemonClient;

use crate::commands::secret::resolve_env_id;

/// Implementation — called from main dispatch.
///
/// # Errors
///
/// Returns errors for: daemon connection failures, unknown env ids,
/// non-existent merge envs (other than `global` which is treated as optional),
/// child-process spawn failures, or --unmask without admin.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub async fn handle_run(
    env: &str,
    no_global: bool,
    merge: &[String],
    project: Option<&str>,
    dry_run: bool,
    unmask: bool,
    command: &[String],
) -> Result<()> {
    if command.is_empty() {
        bail!("No command provided (expected after `--`)");
    }

    let client = DaemonClient::connect().await?;

    // 1. Resolve every participating env id in the order we'll merge them.
    //    `global` goes first (lowest priority), then --merge flags,
    //    then the primary --env (highest priority).
    let mut merged: HashMap<String, String> = HashMap::new();

    if !no_global {
        match resolve_env_id(&client, "global", project).await {
            Ok(global_id) => {
                let secrets = client
                    .reveal_all_secrets_in_env(&global_id)
                    .await
                    .context("Failed to fetch secrets from the `global` env")?;
                debug!(count = secrets.len(), "merged global env secrets");
                merged.extend(secrets);
            }
            Err(e) => {
                // Silent if global just doesn't exist; loud if something else
                // went wrong.
                debug!(error = %e, "`global` env not found or inaccessible — skipping");
            }
        }
    }

    for extra in merge {
        let extra_id = resolve_env_id(&client, extra, project)
            .await
            .with_context(|| format!("Failed to resolve --merge env '{extra}'"))?;
        let secrets = client
            .reveal_all_secrets_in_env(&extra_id)
            .await
            .with_context(|| format!("Failed to fetch secrets from merge env '{extra}'"))?;
        debug!(env = %extra, count = secrets.len(), "merged extra env secrets");
        merged.extend(secrets);
    }

    let primary_id = resolve_env_id(&client, env, project)
        .await
        .with_context(|| format!("Failed to resolve --env '{env}'"))?;
    let primary_secrets = client
        .reveal_all_secrets_in_env(&primary_id)
        .await
        .with_context(|| format!("Failed to fetch secrets from env '{env}'"))?;
    debug!(env = %env, count = primary_secrets.len(), "merged primary env secrets");
    merged.extend(primary_secrets);

    if dry_run {
        let mut keys: Vec<&String> = merged.keys().collect();
        keys.sort();
        for k in keys {
            if unmask {
                let v = merged.get(k).map_or("", String::as_str);
                println!("{k}={v}");
            } else {
                println!("{k}=***");
            }
        }
        return Ok(());
    }

    // Spawn: child inherits parent env, then our merged map overlays on top.
    let program = &command[0];
    let args = &command[1..];

    let mut cmd = Command::new(program);
    cmd.args(args);
    for (k, v) in &merged {
        cmd.env(k, v);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("Failed to spawn `{program}`. Is it in PATH?"))?;
    let status = child
        .wait()
        .await
        .map_err(|e| anyhow!("Failed while waiting for child process: {e}"))?;

    let code = status.code().unwrap_or_else(|| {
        warn!("Child process killed by signal; exiting with 1");
        1
    });
    std::process::exit(code);
}
