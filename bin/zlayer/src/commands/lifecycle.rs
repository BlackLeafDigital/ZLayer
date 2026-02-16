//! Lifecycle command handlers: status, validate, logs, stop.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, warn};

use crate::cli::Cli;
use crate::util::{discover_spec_path, parse_spec};

use zlayer_agent::RuntimeConfig;
use zlayer_spec::DeploymentSpec;

/// Show daemon and deployment status.
///
/// When the daemon is running, displays PID, API bind address, socket path,
/// runtime type, and a summary of active deployments.  When the daemon is
/// not running, shows helpful instructions for starting it.
pub(crate) async fn status(_cli: &Cli) -> Result<()> {
    info!("Checking daemon status");

    let data_dir = _cli.effective_data_dir();
    let socket_path = _cli.effective_socket_path();

    // Try reading daemon.json for metadata (PID, bind address, etc.)
    let metadata = read_daemon_metadata(&data_dir).await;

    // Try connecting to the daemon without auto-starting it
    let client = crate::daemon_client::DaemonClient::try_connect_to(&socket_path).await;

    match client {
        Ok(Some(client)) => {
            // Daemon is running -- show rich status
            println!();
            println!("ZLayer Daemon");

            // PID from daemon.json
            if let Some(ref meta) = metadata {
                if let Some(pid) = meta.get("pid").and_then(|v| v.as_u64()) {
                    println!("  Status:    running (PID {})", pid);
                } else {
                    println!("  Status:    running");
                }
                if let Some(api_bind) = meta.get("api_bind").and_then(|v| v.as_str()) {
                    println!("  API:       {}", api_bind);
                }
            } else {
                println!("  Status:    running");
            }

            println!("  Socket:    {}", socket_path);

            // Detect runtime from metadata or platform
            if let Some(ref meta) = metadata {
                if let Some(host_net) = meta.get("host_network").and_then(|v| v.as_bool()) {
                    if host_net {
                        println!("  Network:   host");
                    }
                }
            }

            println!("  Runtime:   {}", detect_runtime_name());

            // Fetch deployment info
            println!();
            match client.list_deployments().await {
                Ok(deployments) if deployments.is_empty() => {
                    println!("Deployments: none");
                }
                Ok(deployments) => {
                    let active_count = deployments.len();
                    println!("Deployments: {} active", active_count);

                    for dep in &deployments {
                        let name = dep
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let status = dep
                            .get("status")
                            .and_then(|v| {
                                v.as_str()
                                    .map(|s| s.to_string())
                                    .or_else(|| serde_json::to_string(v).ok())
                            })
                            .unwrap_or_else(|| "unknown".to_string());

                        // Try to get service/replica counts from the spec
                        let (svc_count, replica_count) = extract_deployment_counts(dep);

                        if svc_count > 0 {
                            println!(
                                "  {}: {} services, {} replicas ({})",
                                name, svc_count, replica_count, status
                            );
                        } else {
                            println!("  {}: ({})", name, status);
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to fetch deployments");
                    println!("Deployments: error fetching ({})", e);
                }
            }

            println!();
        }
        Ok(None) | Err(_) => {
            // Daemon is not running
            println!();
            println!("ZLayer Daemon: not running");
            println!();
            println!("  Start:  zlayer serve --daemon");
            println!("  Or:     zlayer up (auto-starts daemon)");
            println!();
        }
    }

    Ok(())
}

/// Read and parse `{data_dir}/daemon.json` if it exists.
async fn read_daemon_metadata(data_dir: &std::path::Path) -> Option<serde_json::Value> {
    let path = data_dir.join("daemon.json");
    let contents = tokio::fs::read_to_string(&path).await.ok()?;
    serde_json::from_str(&contents).ok()
}

/// Extract service count and total replica count from a deployment JSON value.
fn extract_deployment_counts(dep: &serde_json::Value) -> (usize, u32) {
    // The deployment response may include a nested "spec" with services
    let services = dep
        .get("spec")
        .and_then(|s| s.get("services"))
        .and_then(|s| s.as_object());

    if let Some(services) = services {
        let svc_count = services.len();
        let mut total_replicas: u32 = 0;
        for (_name, svc) in services {
            if let Some(scale) = svc.get("scale") {
                if let Some(replicas) = scale.get("replicas").and_then(|r| r.as_u64()) {
                    total_replicas += replicas as u32;
                } else if let Some(min) = scale.get("min").and_then(|r| r.as_u64()) {
                    total_replicas += min as u32;
                } else {
                    total_replicas += 1;
                }
            } else {
                total_replicas += 1;
            }
        }
        (svc_count, total_replicas)
    } else {
        (0, 0)
    }
}

/// Return a human-readable name for the current platform's default runtime.
fn detect_runtime_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "mac-sandbox"
    }
    #[cfg(target_os = "linux")]
    {
        "youki"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        "auto"
    }
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

/// Stream logs from a service via the daemon API.
///
/// When `follow` is false, fetches the last `lines` lines and prints them.
/// When `follow` is true, opens an SSE stream to the daemon and prints log
/// lines as they arrive in real time (until the user presses Ctrl+C).
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

    // Connect to the daemon (auto-starts if needed)
    let client = crate::daemon_client::DaemonClient::connect()
        .await
        .context("Failed to connect to zlayer daemon")?;

    if follow {
        // ---- Follow mode: SSE streaming ----
        let mut rx = client
            .get_logs_streaming(deployment, service, lines, instance.as_deref())
            .await
            .context("Failed to start log streaming from daemon")?;

        // Read lines from the channel until the stream ends or Ctrl+C.
        while let Some(line) = rx.recv().await {
            println!("{}", line);
        }
    } else {
        // ---- Non-follow mode: one-shot fetch ----
        let log_output = client
            .get_logs_with_instance(deployment, service, lines, false, instance.as_deref())
            .await
            .context("Failed to fetch logs from daemon")?;

        print!("{}", log_output);
    }

    Ok(())
}

/// Stop a deployment or specific service
///
/// Directly stops and removes containers via the runtime, rather than going through
/// ServiceManager (which has no knowledge of already-running containers).
/// If a spec is found matching the deployment, it iterates over services and replicas.
/// Also scans the state directory for any extra containers beyond the spec's replica count.
pub(crate) async fn stop(
    deployment: &str,
    service: Option<String>,
    force: bool,
    timeout: u64,
    state_dir: &std::path::Path,
) -> Result<()> {
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

    // If we have a spec matching this deployment, use it to enumerate containers
    if let Some(spec) = &spec {
        if spec.deployment == deployment {
            // Filter to targeted services
            let target_services: Vec<_> = if let Some(svc) = &service {
                spec.services
                    .iter()
                    .filter(|(name, _)| name.as_str() == svc.as_str())
                    .collect()
            } else {
                spec.services.iter().collect()
            };

            let mut stopped_count: u32 = 0;

            for (name, service_spec) in &target_services {
                let replicas = match &service_spec.scale {
                    zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
                    zlayer_spec::ScaleSpec::Adaptive { max, .. } => *max,
                    zlayer_spec::ScaleSpec::Manual => 1,
                };

                println!("  Stopping service: {} (up to {} replicas)", name, replicas);

                for replica in 1..=replicas {
                    let id = zlayer_agent::ContainerId {
                        service: (*name).clone(),
                        replica,
                    };
                    if let Err(e) = runtime.stop_container(&id, timeout_duration).await {
                        warn!(container = %id, error = %e, "Failed to stop container (may not exist)");
                    } else {
                        stopped_count += 1;
                    }
                    if let Err(e) = runtime.remove_container(&id).await {
                        warn!(container = %id, error = %e, "Failed to remove container (may not exist)");
                    }
                }

                // Scan state dir for any extra containers beyond the spec replica count
                let prefix = format!("{}-", name);
                if let Ok(mut entries) = tokio::fs::read_dir(state_dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let entry_name = entry.file_name().to_string_lossy().to_string();
                        if entry_name.starts_with(&prefix) {
                            if let Some(rep_str) = entry_name.strip_prefix(&prefix) {
                                if let Ok(rep_num) = rep_str.parse::<u32>() {
                                    if rep_num > replicas {
                                        let id = zlayer_agent::ContainerId {
                                            service: (*name).clone(),
                                            replica: rep_num,
                                        };
                                        info!(container = %id, "Found extra container beyond spec");
                                        if let Err(e) =
                                            runtime.stop_container(&id, timeout_duration).await
                                        {
                                            warn!(container = %id, error = %e, "Failed to stop extra container");
                                        } else {
                                            stopped_count += 1;
                                        }
                                        if let Err(e) = runtime.remove_container(&id).await {
                                            warn!(container = %id, error = %e, "Failed to remove extra container");
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            println!("Stopped {} container(s).", stopped_count);
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
