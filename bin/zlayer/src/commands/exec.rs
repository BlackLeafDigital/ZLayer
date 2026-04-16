//! `zlayer exec` command -- execute a command inside a running service container.
//!
//! Connects to the daemon via [`DaemonClient`] and sends an exec request.
//! If `--deployment` is omitted, the target deployment is resolved from the
//! service name via the shared resolver (auto-pick / interactive prompt /
//! error).  Stdout/stderr from the container are printed to the local
//! terminal and the process exits with the container's exit code.

use anyhow::{bail, Context, Result};
use tracing::info;

use crate::daemon_client::DaemonClient;

/// Execute the `exec` command.
pub(crate) async fn exec(
    deployment: Option<String>,
    service: &str,
    replica: Option<u32>,
    cmd: &[String],
) -> Result<()> {
    if cmd.is_empty() {
        bail!("No command specified. Usage: zlayer exec <service> -- <command> [args...]");
    }

    let client = DaemonClient::connect().await?;
    let resolved =
        crate::commands::resolver::resolve_service(&client, service, deployment.as_deref())
            .await
            .context("Failed to resolve service")?;

    info!(
        deployment = %resolved.deployment,
        service = %resolved.service,
        replica = ?replica,
        cmd = ?cmd,
        "Executing command in container"
    );

    // Call the exec endpoint
    let result = client
        .exec_command_with_replica(&resolved.deployment, &resolved.service, cmd, replica)
        .await
        .context("Exec command failed")?;

    // Extract exit code, stdout, stderr from response
    #[allow(clippy::cast_possible_truncation)]
    let exit_code = result["exit_code"].as_i64().unwrap_or(1) as i32;
    let stdout = result["stdout"].as_str().unwrap_or("");
    let stderr = result["stderr"].as_str().unwrap_or("");

    // Print output
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}
