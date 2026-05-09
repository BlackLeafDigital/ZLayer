//! Lifecycle command handlers: status, validate, logs, stop.

use anyhow::Context;
use anyhow::Result;
use std::path::Path;
use tracing::info;
use tracing::warn;

use crate::cli::Cli;
#[cfg(unix)]
use crate::util::discover_spec_path;
use crate::util::parse_spec;

#[cfg(unix)]
use zlayer_agent::RuntimeConfig;
use zlayer_spec::DeploymentSpec;

/// Show daemon and deployment status.
///
/// When the daemon is running, displays PID, API bind address, socket path,
/// runtime type, and a summary of active deployments.  When the daemon is
/// not running, shows helpful instructions for starting it.
// Five distinct user-facing branches (Reachable/SocketMissing/PermissionDenied/
// ConnectionRefused/Other) each emit a multi-line diagnostic. Splitting them
// into helper fns would just shuffle the same line count across more functions.
#[allow(clippy::too_many_lines)]
pub(crate) async fn status(cli: &Cli) -> Result<()> {
    info!("Checking daemon status");

    let data_dir = cli.effective_data_dir();
    let socket_path = cli.effective_socket_path();

    // Try reading daemon.json for metadata (PID, bind address, etc.)
    let metadata = read_daemon_metadata(&data_dir).await;

    // Probe the daemon without auto-starting it -- this distinguishes
    // "not running" from "running but unreachable due to permissions".
    let reachability = zlayer_client::DaemonClient::probe(&socket_path).await;

    match reachability {
        zlayer_client::DaemonReachability::Reachable(client) => {
            // Daemon is running -- show rich status
            println!();
            println!("ZLayer Daemon");

            // PID from daemon.json
            if let Some(ref meta) = metadata {
                if let Some(pid) = meta.get("pid").and_then(serde_json::Value::as_u64) {
                    println!("  Status:    running (PID {pid})");
                } else {
                    println!("  Status:    running");
                }
                if let Some(api_bind) = meta.get("api_bind").and_then(|v| v.as_str()) {
                    println!("  API:       {api_bind}");
                }
            } else {
                println!("  Status:    running");
            }

            println!("  Socket:    {socket_path}");

            // Detect runtime from metadata or platform
            if let Some(ref meta) = metadata {
                if let Some(host_net) = meta
                    .get("host_network")
                    .and_then(serde_json::Value::as_bool)
                {
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
                    println!("Deployments: {active_count} active");

                    for dep in &deployments {
                        let name = dep
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let status = dep
                            .get("status")
                            .and_then(|v| {
                                v.as_str()
                                    .map(std::string::ToString::to_string)
                                    .or_else(|| serde_json::to_string(v).ok())
                            })
                            .unwrap_or_else(|| "unknown".to_string());

                        // Try to get service/replica counts from the spec
                        let (svc_count, replica_count) = extract_deployment_counts(dep);

                        if svc_count > 0 {
                            println!(
                                "  {name}: {svc_count} services, {replica_count} replicas ({status})"
                            );
                        } else {
                            println!("  {name}: ({status})");
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to fetch deployments");
                    println!("Deployments: error fetching ({e})");
                }
            }

            println!();
        }
        zlayer_client::DaemonReachability::SocketMissing => {
            println!();
            println!("ZLayer Daemon: not running");
            println!();
            println!("  Start:  zlayer serve --daemon");
            println!("  Or:     zlayer up (auto-starts daemon)");
            println!();
        }
        zlayer_client::DaemonReachability::PermissionDenied => {
            println!();
            println!(
                "ZLayer Daemon: running, but the socket at {socket_path} is not readable from your user."
            );
            println!();
            println!("  The CLI requires membership in the 'zlayer' group.");
            println!("    Refresh the current shell:  newgrp zlayer");
            println!("    Or open a new login shell after install.");
            println!();
            println!("  If neither works, the socket may be owned by root:root instead of");
            println!("  root:zlayer (older builds had this bug). Check with:");
            println!("    ls -la {socket_path}");
            println!();
        }
        zlayer_client::DaemonReachability::ConnectionRefused => {
            println!();
            println!("ZLayer Daemon: socket file is present but nothing is listening.");
            println!("  The daemon may have crashed. Check:");
            println!("    systemctl status zlayer.service");
            println!("    journalctl -fu zlayer.service");
            println!();
        }
        zlayer_client::DaemonReachability::Other(e) => {
            println!();
            println!("ZLayer Daemon: unknown failure connecting to socket: {e}");
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
#[allow(clippy::cast_possible_truncation)]
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
                if let Some(replicas) = scale.get("replicas").and_then(serde_json::Value::as_u64) {
                    total_replicas += replicas as u32;
                } else if let Some(min) = scale.get("min").and_then(serde_json::Value::as_u64) {
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
    #[cfg(target_os = "windows")]
    {
        "composite"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "auto"
    }
}

/// Validate a spec file without deploying
pub(crate) fn validate(spec_path: &Path) -> Result<()> {
    info!(path = %spec_path.display(), "Validating spec file");

    match parse_spec(spec_path) {
        Ok(spec) => {
            println!("Spec validation: PASSED");
            print_deployment_plan(&spec);
            Ok(())
        }
        Err(e) => {
            println!("Spec validation: FAILED");
            println!("Error: {e}");
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
    deployment: Option<&str>,
    service: &str,
    lines: u32,
    follow: bool,
    instance: Option<String>,
) -> Result<()> {
    // Connect to the daemon (auto-starts if needed)
    let client = zlayer_client::DaemonClient::connect()
        .await
        .context("Failed to connect to zlayer daemon")?;

    let resolved = crate::commands::resolver::resolve_service(&client, service, deployment)
        .await
        .context("Failed to resolve service")?;
    let deployment = resolved.deployment.as_str();
    let service = resolved.service.as_str();

    info!(
        deployment = %deployment,
        service = %service,
        lines = lines,
        follow = follow,
        instance = ?instance,
        "Fetching logs"
    );

    if follow {
        // ---- Follow mode: SSE streaming ----
        let mut rx = client
            .get_logs_streaming(deployment, service, lines, instance.as_deref())
            .await
            .context("Failed to start log streaming from daemon")?;

        // Read lines from the channel until the stream ends or Ctrl+C.
        while let Some(line) = rx.recv().await {
            println!("{line}");
        }
    } else {
        // ---- Non-follow mode: one-shot fetch ----
        let log_output = client
            .get_logs_with_instance(deployment, service, lines, false, instance.as_deref())
            .await
            .context("Failed to fetch logs from daemon")?;

        print!("{log_output}");
    }

    Ok(())
}

/// Stop a deployment or specific service
///
/// Directly stops and removes containers via the runtime, rather than going through
/// `ServiceManager` (which has no knowledge of already-running containers).
/// If a spec is found matching the deployment, it iterates over services and replicas.
/// Also scans the state directory for any extra containers beyond the spec's replica count.
async fn resolve_stop_deployment(hint: Option<&str>) -> Result<String> {
    let client = zlayer_client::DaemonClient::connect().await.context(
        "Failed to connect to zlayer daemon (pass <DEPLOYMENT> explicitly to skip auto-resolution)",
    )?;
    crate::commands::resolver::resolve_deployment(&client, hint)
        .await
        .context("Failed to resolve deployment")
}

#[cfg(unix)]
pub(crate) async fn stop(
    deployment: Option<&str>,
    service: Option<String>,
    force: bool,
    timeout: u64,
    state_dir: &std::path::Path,
) -> Result<()> {
    use std::time::Duration;

    let deployment = resolve_stop_deployment(deployment).await?;

    let target = match &service {
        Some(s) => format!("{deployment}/{s}"),
        None => deployment.clone(),
    };

    info!(
        target = %target,
        force = force,
        timeout_secs = timeout,
        "Stopping"
    );

    if force {
        println!("Force stopping {target}...");
    } else {
        println!("Gracefully stopping {target} (timeout: {timeout}s)...");
    }

    // Try to discover and parse the spec for this deployment
    let spec_path = discover_spec_path(None).ok();
    let spec = spec_path.as_ref().and_then(|p| parse_spec(p).ok());

    // Create a runtime to interact with containers
    let runtime = zlayer_agent::create_runtime(RuntimeConfig::Auto, None)
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

                println!("  Stopping service: {name} (up to {replicas} replicas)");

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
                let prefix = format!("{name}-");
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

            println!("Stopped {stopped_count} container(s).");
            return Ok(());
        }
    }

    // Fallback: no spec available, just print message
    println!("No spec found for deployment '{deployment}'. Use the spec file path or run from the deployment directory.");
    Ok(())
}

/// Windows variant of `stop` that routes through the daemon HTTP API
/// instead of talking to `zlayer_agent` directly.
///
/// The Linux fast-path creates a runtime in-process and iterates over the
/// spec's services/replicas to stop containers. On Windows the `zlayer_agent`
/// crate is built for the daemon (HCS/composite runtimes), and the CLI must
/// go through the daemon's REST API. We enumerate containers via
/// `DaemonClient::list_containers` per service and issue
/// `DaemonClient::stop_container` for each one.
///
/// The `state_dir` parameter is accepted for signature compatibility with
/// the Unix fast-path but is unused: container state is managed by the
/// daemon on Windows, so there is no local state dir to scan.
#[cfg(windows)]
pub(crate) async fn stop(
    deployment: Option<&str>,
    service: Option<String>,
    force: bool,
    timeout: u64,
    _state_dir: &std::path::Path,
) -> Result<()> {
    let deployment = resolve_stop_deployment(deployment).await?;

    let target = match &service {
        Some(s) => format!("{deployment}/{s}"),
        None => deployment.clone(),
    };

    info!(
        target = %target,
        force = force,
        timeout_secs = timeout,
        "Stopping"
    );

    if force {
        println!("Force stopping {target}...");
    } else {
        println!("Gracefully stopping {target} (timeout: {timeout}s)...");
    }

    let client = zlayer_client::DaemonClient::connect()
        .await
        .context("Failed to connect to zlayer daemon")?;

    // Determine the set of services to stop. If the user asked for a specific
    // service, just use that; otherwise query the daemon for the deployment's
    // full service list.
    let service_names: Vec<String> = if let Some(s) = &service {
        vec![s.clone()]
    } else {
        let services = client
            .list_services(&deployment)
            .await
            .with_context(|| format!("Failed to list services for deployment '{deployment}'"))?;
        services
            .iter()
            .filter_map(|svc| {
                svc.get("name")
                    .and_then(|v| v.as_str())
                    .map(std::string::ToString::to_string)
            })
            .collect()
    };

    // `force` requests an immediate kill; otherwise honor the user-supplied
    // graceful timeout. Pass `Some(0)` for force so the daemon skips the grace
    // window entirely.
    let timeout_arg: Option<u64> = if force { Some(0) } else { Some(timeout) };

    let mut stopped_count: u32 = 0;

    for name in &service_names {
        let containers = match client.list_containers(&deployment, name).await {
            Ok(c) => c,
            Err(e) => {
                warn!(service = %name, error = %e, "Failed to list containers for service");
                continue;
            }
        };

        println!(
            "  Stopping service: {name} ({count} container(s))",
            count = containers.len()
        );

        for c in &containers {
            let Some(id) = c.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            if let Err(e) = client.stop_container(id, timeout_arg).await {
                warn!(container = %id, error = %e, "Failed to stop container (may not exist)");
            } else {
                stopped_count += 1;
            }
        }
    }

    println!("Stopped {stopped_count} container(s).");
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
        println!("  Service: {name}");
        println!("    Image: {}", service.image.name);
        println!("    Type: {:?}", service.rtype);

        // Print scaling info
        match &service.scale {
            zlayer_spec::ScaleSpec::Fixed { replicas } => {
                println!("    Scale: fixed ({replicas} replicas)");
            }
            zlayer_spec::ScaleSpec::Adaptive { min, max, .. } => {
                println!("    Scale: adaptive ({min}-{max} replicas)");
            }
            zlayer_spec::ScaleSpec::Manual => {
                println!("    Scale: manual");
            }
        }

        // Print resources if specified
        if service.resources.cpu.is_some() || service.resources.memory.is_some() {
            print!("    Resources:");
            if let Some(cpu) = service.resources.cpu {
                print!(" cpu={cpu}");
            }
            if let Some(ref mem) = service.resources.memory {
                print!(" memory={mem}");
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
