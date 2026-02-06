//! Lifecycle command handlers: status, validate, logs, stop.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, warn};

use crate::cli::{Cli, RuntimeType};
use crate::util::{discover_spec_path, parse_spec};

use zlayer_agent::RuntimeConfig;
#[cfg(target_os = "linux")]
use zlayer_agent::YoukiConfig;
use zlayer_spec::DeploymentSpec;

/// Show runtime status
pub(crate) async fn status(cli: &Cli) -> Result<()> {
    info!("Checking runtime status");

    println!("\n=== ZLayer Runtime Status ===");
    println!("Runtime: {:?}", cli.runtime);

    match cli.runtime {
        RuntimeType::Auto => {
            println!("Status: Auto-detect mode");
            match zlayer_agent::create_runtime(RuntimeConfig::Auto).await {
                Ok(_) => {
                    println!("Status: Runtime auto-detected and ready");
                }
                Err(e) => {
                    println!("Status: No suitable runtime found");
                    println!("Error: {}", e);
                }
            }
        }
        #[cfg(feature = "docker")]
        RuntimeType::Docker => match zlayer_agent::create_runtime(RuntimeConfig::Docker).await {
            Ok(_) => {
                println!("Status: Docker runtime ready");
            }
            Err(e) => {
                println!("Status: Docker runtime unavailable");
                println!("Error: {}", e);
            }
        },
        #[cfg(target_os = "linux")]
        RuntimeType::Youki => {
            println!("State Dir: {}", cli.state_dir.display());

            let config = YoukiConfig {
                state_dir: cli.state_dir.clone(),
                ..Default::default()
            };

            match zlayer_agent::create_runtime(RuntimeConfig::Youki(config)).await {
                Ok(_) => {
                    println!("Status: Youki runtime ready");
                }
                Err(e) => {
                    println!("Status: Youki runtime unavailable");
                    println!("Error: {}", e);
                }
            }
        }
    }

    println!();

    Ok(())
}

/// Validate a spec file without deploying
pub(crate) async fn validate(spec_path: &Path) -> Result<()> {
    info!(path = %spec_path.display(), "Validating spec file");

    match parse_spec(spec_path) {
        Ok(spec) => {
            println!("Spec validation: PASSED");
            print_deployment_plan(&spec);
            Ok(())
        }
        Err(e) => {
            println!("Spec validation: FAILED");
            println!("Error: {}", e);
            Err(e)
        }
    }
}

/// Stream logs from a service
pub(crate) async fn logs(
    deployment: &str,
    service: &str,
    lines: u32,
    follow: bool,
    instance: Option<String>,
) -> Result<()> {
    info!(
        deployment = %deployment,
        service = %service,
        lines = lines,
        follow = follow,
        instance = ?instance,
        "Fetching logs"
    );

    // TODO: Implement actual log fetching from containerd or log aggregator
    // For now, show a helpful message
    println!("Log streaming for {}/{}", deployment, service);
    println!("Lines: {}, Follow: {}", lines, follow);
    if let Some(inst) = instance {
        println!("Instance: {}", inst);
    }

    // Placeholder - in production this would:
    // 1. Connect to the scheduler to get service instances
    // 2. Stream logs from containerd for each instance
    // 3. Merge and format the log streams
    println!("\n[Log streaming not yet implemented]");
    println!("Use 'docker logs' or 'ctr tasks logs' for container logs");

    Ok(())
}

/// Stop a deployment or service
pub(crate) async fn stop(
    deployment: &str,
    service: Option<String>,
    force: bool,
    timeout: u64,
) -> Result<()> {
    use std::sync::Arc;
    use std::time::Duration;

    let target = match &service {
        Some(s) => format!("{}/{}", deployment, s),
        None => deployment.to_string(),
    };

    info!(
        target = %target,
        force = force,
        timeout_secs = timeout,
        "Stopping"
    );

    if force {
        println!("Force stopping {}...", target);
    } else {
        println!("Gracefully stopping {} (timeout: {}s)...", target, timeout);
    }

    // Try to discover and parse the spec for this deployment
    let spec_path = discover_spec_path(None).ok();
    let spec = spec_path.as_ref().and_then(|p| parse_spec(p).ok());

    // Create a runtime to interact with containers
    let runtime = zlayer_agent::create_runtime(RuntimeConfig::Auto)
        .await
        .context("Failed to create container runtime")?;

    let timeout_duration = if force {
        Duration::from_secs(0)
    } else {
        Duration::from_secs(timeout)
    };

    // If we have a spec, use the ServiceManager for clean shutdown
    if let Some(spec) = &spec {
        if spec.deployment == deployment {
            let manager = Arc::new(zlayer_agent::ServiceManager::new(runtime.clone()));

            // Register services so we can scale them down
            let target_services: Vec<_> = if let Some(svc) = &service {
                spec.services
                    .iter()
                    .filter(|(name, _)| name.as_str() == svc.as_str())
                    .collect()
            } else {
                spec.services.iter().collect()
            };

            for (name, service_spec) in &target_services {
                if let Err(e) = manager
                    .upsert_service((*name).clone(), (*service_spec).clone())
                    .await
                {
                    warn!(service = %name, error = %e, "Failed to register service for shutdown");
                    continue;
                }

                info!(service = %name, "Scaling service to 0");
                println!("  Stopping service: {}", name);
                if let Err(e) = manager.scale_service(name, 0).await {
                    warn!(service = %name, error = %e, "Failed to scale down service");
                }
            }

            // Wait for containers to stop
            if !force && timeout > 0 {
                let start = std::time::Instant::now();
                while start.elapsed() < timeout_duration {
                    let mut all_stopped = true;
                    for (name, _) in &target_services {
                        if let Ok(count) = manager.service_replica_count(name).await {
                            if count > 0 {
                                all_stopped = false;
                                break;
                            }
                        }
                    }
                    if all_stopped {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }

            println!("Stopped.");
            return Ok(());
        }
    }

    // Fallback: no spec available, just print message
    println!("No spec found for deployment '{}'. Use the spec file path or run from the deployment directory.", deployment);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn print_deployment_plan(spec: &DeploymentSpec) {
    println!("\n=== Deployment Plan ===");
    println!("Deployment: {}", spec.deployment);
    println!("Version: {}", spec.version);
    println!("Services: {}", spec.services.len());
    println!();

    for (name, service) in &spec.services {
        println!("  Service: {}", name);
        println!("    Image: {}", service.image.name);
        println!("    Type: {:?}", service.rtype);

        // Print scaling info
        match &service.scale {
            zlayer_spec::ScaleSpec::Fixed { replicas } => {
                println!("    Scale: fixed ({} replicas)", replicas);
            }
            zlayer_spec::ScaleSpec::Adaptive { min, max, .. } => {
                println!("    Scale: adaptive ({}-{} replicas)", min, max);
            }
            zlayer_spec::ScaleSpec::Manual => {
                println!("    Scale: manual");
            }
        }

        // Print resources if specified
        if service.resources.cpu.is_some() || service.resources.memory.is_some() {
            print!("    Resources:");
            if let Some(cpu) = service.resources.cpu {
                print!(" cpu={}", cpu);
            }
            if let Some(ref mem) = service.resources.memory {
                print!(" memory={}", mem);
            }
            println!();
        }

        // Print endpoints
        if !service.endpoints.is_empty() {
            println!("    Endpoints:");
            for ep in &service.endpoints {
                println!(
                    "      - {} ({:?}:{}, {:?})",
                    ep.name, ep.protocol, ep.port, ep.expose
                );
            }
        }

        // Print dependencies
        if !service.depends.is_empty() {
            println!("    Dependencies:");
            for dep in &service.depends {
                println!("      - {} ({:?})", dep.service, dep.condition);
            }
        }

        println!();
    }
}
