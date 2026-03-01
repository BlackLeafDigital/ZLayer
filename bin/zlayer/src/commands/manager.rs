use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::info;

use crate::cli::ManagerCommands;

/// Handle zlayer-manager commands
pub(crate) async fn handle_manager(cmd: &ManagerCommands) -> Result<()> {
    match cmd {
        ManagerCommands::Init {
            output,
            port,
            deploy,
            version,
        } => handle_manager_init(output.clone(), *port, *deploy, version.clone()).await,
        ManagerCommands::Status => handle_manager_status().await,
        ManagerCommands::Stop { force } => handle_manager_stop(*force).await,
    }
}

/// Initialize zlayer-manager deployment spec
pub(crate) async fn handle_manager_init(
    output: PathBuf,
    port: u16,
    deploy: bool,
    version: Option<String>,
) -> Result<()> {
    let version = version.unwrap_or_else(|| "latest".to_string());
    info!(port = port, version = %version, "Initializing zlayer-manager deployment");

    // Build the manager service spec
    // The OCI image defaults to listening on 9120 (ZLAYER_MANAGER_ADDR).
    // We use target_port to tell the proxy where the container actually listens,
    // while `port` is the external-facing proxy listen port.
    let spec = format!(
        r#"version: v1
deployment: zlayer-manager

services:
  manager:
    rtype: service
    image:
      name: zachhandley/zlayer-manager:{version}
      pull_policy: always
    resources:
      cpu: 0.5
      memory: 256Mi
    endpoints:
      - name: http
        protocol: http
        port: {port}
        target_port: 9120
        expose: public
    scale:
      mode: fixed
      replicas: 1
    health:
      check:
        type: http
        url: http://localhost:9120/health
"#,
        version = version,
        port = port
    );

    // Ensure output directory exists
    if !output.exists() {
        std::fs::create_dir_all(&output)
            .with_context(|| format!("Failed to create output directory: {}", output.display()))?;
    }

    // Write the spec file as manager.zlayer.yml (auto-discovered by `zlayer deploy`)
    let spec_path = output.join("manager.zlayer.yml");
    std::fs::write(&spec_path, &spec)
        .with_context(|| format!("Failed to write spec file: {}", spec_path.display()))?;

    println!("Created deployment spec: {}", spec_path.display());
    println!();
    println!("Manager configuration:");
    println!("  - Image: zachhandley/zlayer-manager:{}", version);
    println!("  - Port: {}", port);
    println!();

    if deploy {
        println!("Deploying zlayer-manager...");
        // TODO: Integrate with deploy command
        // For now, print instructions
        println!();
        println!("To deploy manually, run:");
        println!("  zlayer deploy");
    } else {
        println!("To deploy, run:");
        println!("  zlayer deploy");
        println!();
        println!("Or use --deploy flag to deploy immediately:");
        println!("  zlayer manager init --deploy");
    }

    Ok(())
}

/// Show manager status
pub(crate) async fn handle_manager_status() -> Result<()> {
    info!("Checking zlayer-manager status");

    // TODO: Query the actual manager service status
    // This would check if the manager container is running, its health, etc.
    println!("zlayer-manager status:");
    println!("  Status: unknown (status check not yet implemented)");
    println!();
    println!("To check container status manually:");
    println!("  zlayer ps | grep manager");

    Ok(())
}

/// Stop the manager
pub(crate) async fn handle_manager_stop(force: bool) -> Result<()> {
    info!(force = force, "Stopping zlayer-manager");

    // TODO: Actually stop the manager service
    // This would find and stop the manager container
    if force {
        println!("Force stopping zlayer-manager...");
    } else {
        println!("Gracefully stopping zlayer-manager...");
    }

    println!();
    println!("Manager stop not yet implemented.");
    println!("To stop manually:");
    println!("  zlayer stop zlayer-manager");

    Ok(())
}
