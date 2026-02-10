//! `zlayer exec` command -- execute a command inside a running service container.
//!
//! Connects to the daemon via [`DaemonClient`] and sends an exec request.
//! If `--deployment` is omitted and only one deployment exists, it is
//! auto-detected.  Stdout/stderr from the container are printed to the
//! local terminal and the process exits with the container's exit code.

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

    info!(
        deployment = ?deployment,
        service = %service,
        replica = ?replica,
        cmd = ?cmd,
        "Executing command in container"
    );

    let client = DaemonClient::connect().await?;

    // Resolve deployment name
    let deploy_name = match deployment {
        Some(name) => name,
        None => auto_detect_deployment(&client).await?,
    };

    // Call the exec endpoint
    let result = client
        .exec_command_with_replica(&deploy_name, service, cmd, replica)
        .await
        .context("Exec command failed")?;

    // Extract exit code, stdout, stderr from response
    let exit_code = result["exit_code"].as_i64().unwrap_or(1) as i32;
    let stdout = result["stdout"].as_str().unwrap_or("");
    let stderr = result["stderr"].as_str().unwrap_or("");

    // Print output
    if !stdout.is_empty() {
        print!("{}", stdout);
    }
    if !stderr.is_empty() {
        eprint!("{}", stderr);
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}

/// Auto-detect the deployment name when only one deployment exists.
async fn auto_detect_deployment(client: &DaemonClient) -> Result<String> {
    let deployments = client
        .list_deployments()
        .await
        .context("Failed to list deployments for auto-detection")?;

    match deployments.len() {
        0 => bail!("No deployments found. Deploy something first, or specify --deployment."),
        1 => {
            let name = deployments[0]["name"]
                .as_str()
                .or_else(|| deployments[0]["deployment"].as_str())
                .ok_or_else(|| anyhow::anyhow!("Could not extract deployment name"))?
                .to_string();
            info!(deployment = %name, "Auto-detected single deployment");
            Ok(name)
        }
        n => {
            let names: Vec<&str> = deployments
                .iter()
                .filter_map(|d| d["name"].as_str().or_else(|| d["deployment"].as_str()))
                .collect();
            bail!(
                "Multiple deployments found ({}). Specify one with --deployment.\n  Available: {}",
                n,
                names.join(", ")
            );
        }
    }
}
