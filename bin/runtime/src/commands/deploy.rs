//! Deploy command handlers (thin daemon client).
//!
//! Contains the `deploy`, `up`, and `down` command implementations.
//! These are thin clients that delegate to the daemon process over a Unix socket.
//!
//! The daemon (`zlayer-runtime serve --daemon`) owns all infrastructure: overlay
//! networking, DNS, proxy, container supervisor, etc.  These commands simply send
//! the deployment spec to the daemon API and poll for status.
//!
//! Dry-run mode is the only path that does NOT require the daemon -- it validates
//! the spec locally and displays the deployment plan.

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
use tracing::{info, warn};

use crate::cli::Cli;
use crate::daemon_client::DaemonClient;
use crate::deploy_tui::{DeployEvent, LogLevel, PlainDeployLogger, ServicePlan};
use crate::util::{discover_spec_path, parse_spec};

// ---------------------------------------------------------------------------
// Helpers (kept for dry-run display)
// ---------------------------------------------------------------------------

/// Helper to send an event, ignoring send errors (receiver may have dropped)
fn emit(tx: &mpsc::Sender<DeployEvent>, event: DeployEvent) {
    let _ = tx.send(event);
}

/// Build a `ServicePlan` from a spec entry for the `PlanReady` event
fn build_service_plan(name: &str, service: &zlayer_spec::ServiceSpec) -> ServicePlan {
    let scale_mode = match &service.scale {
        zlayer_spec::ScaleSpec::Fixed { replicas } => format!("fixed({})", replicas),
        zlayer_spec::ScaleSpec::Adaptive { min, max, .. } => {
            format!("adaptive({}-{})", min, max)
        }
        zlayer_spec::ScaleSpec::Manual => "manual".to_string(),
    };

    let endpoints = service
        .endpoints
        .iter()
        .map(|ep| format!("{:?}:{} ({:?})", ep.protocol, ep.port, ep.expose))
        .collect();

    ServicePlan {
        name: name.to_string(),
        image: service.image.name.clone(),
        scale_mode,
        endpoints,
    }
}

/// Set up the plain (non-TUI) event channel and logger thread, returning the sender.
///
/// Spawns a background thread that drains events through `PlainDeployLogger`.
/// The thread exits when the sender is dropped (channel closed).
fn setup_plain_channel() -> mpsc::Sender<DeployEvent> {
    let (tx, rx) = mpsc::channel::<DeployEvent>();

    let is_color = std::io::IsTerminal::is_terminal(&std::io::stdout());
    std::thread::spawn(move || {
        let logger = PlainDeployLogger::with_color(is_color);
        logger.process_events(rx);
    });

    tx
}

// ---------------------------------------------------------------------------
// deploy
// ---------------------------------------------------------------------------

/// Deploy services from a spec file.
///
/// In **dry-run** mode the spec is parsed and validated locally, then the
/// deployment plan is printed.  No daemon interaction occurs.
///
/// In **live** mode the raw YAML is sent to the daemon via
/// `POST /api/v1/deployments`.  The daemon handles all infrastructure setup,
/// service registration, scaling and health-checking.  This function polls
/// `GET /api/v1/deployments/{name}` until the deployment reaches a terminal
/// state (running / failed) or a timeout is hit.
pub(crate) async fn deploy(cli: &Cli, spec_path: &Path, dry_run: bool) -> Result<()> {
    let spec = parse_spec(spec_path)?;

    // ------------------------------------------------------------------
    // Dry-run: validate + display plan, no daemon needed
    // ------------------------------------------------------------------
    if dry_run {
        info!("Dry run mode - validating only");
        let tx = setup_plain_channel();
        let plans: Vec<ServicePlan> = spec
            .services
            .iter()
            .map(|(name, svc)| build_service_plan(name, svc))
            .collect();
        emit(
            &tx,
            DeployEvent::PlanReady {
                deployment_name: spec.deployment.clone(),
                version: spec.version.clone(),
                services: plans,
            },
        );
        return Ok(());
    }

    // ------------------------------------------------------------------
    // Live deploy via daemon
    // ------------------------------------------------------------------
    let spec_yaml = std::fs::read_to_string(spec_path)
        .with_context(|| format!("Failed to read spec file: {}", spec_path.display()))?;

    // Display the plan before submitting
    let tx = setup_plain_channel();
    let plans: Vec<ServicePlan> = spec
        .services
        .iter()
        .map(|(name, svc)| build_service_plan(name, svc))
        .collect();
    emit(
        &tx,
        DeployEvent::PlanReady {
            deployment_name: spec.deployment.clone(),
            version: spec.version.clone(),
            services: plans,
        },
    );

    // Connect to daemon (auto-starts if needed)
    let client = DaemonClient::connect().await?;

    emit(
        &tx,
        DeployEvent::Log {
            level: LogLevel::Info,
            message: "Submitting deployment to daemon...".to_string(),
        },
    );

    let result = client
        .create_deployment(&spec_yaml)
        .await
        .context("Failed to submit deployment to daemon")?;

    let deployment_name = result
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(&spec.deployment)
        .to_string();

    let status = result
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    info!(
        deployment = %deployment_name,
        status = %status,
        "Deployment submitted"
    );

    // Poll for readiness with a timeout
    let poll_timeout = Duration::from_secs(120);
    let poll_interval = Duration::from_secs(2);
    let start = std::time::Instant::now();
    let mut last_status = status.to_string();

    while start.elapsed() < poll_timeout {
        tokio::time::sleep(poll_interval).await;

        match client.get_deployment(&deployment_name).await {
            Ok(deployment) => {
                let current_status = deployment
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                if current_status != last_status {
                    emit(
                        &tx,
                        DeployEvent::Log {
                            level: LogLevel::Info,
                            message: format!(
                                "Deployment '{}': {} -> {}",
                                deployment_name, last_status, current_status
                            ),
                        },
                    );
                    last_status = current_status.clone();
                }

                // Terminal states
                match current_status.as_str() {
                    "running" | "active" => {
                        // Use service_health from daemon response if available,
                        // fall back to spec-based summary
                        let service_health =
                            deployment.get("service_health").and_then(|v| v.as_array());

                        let total_services = spec.services.len();
                        let healthy_count = if let Some(health_arr) = service_health {
                            health_arr
                                .iter()
                                .filter(|s| {
                                    let running = s
                                        .get("replicas_running")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0);
                                    let desired = s
                                        .get("replicas_desired")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0);
                                    running >= desired
                                })
                                .count()
                        } else {
                            total_services
                        };

                        // Print enhanced success output
                        println!();
                        println!(
                            "Deployment '{}' ready ({}/{} services healthy):",
                            deployment_name, healthy_count, total_services
                        );

                        if let Some(health_arr) = service_health {
                            for svc in health_arr {
                                let name = svc.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                                let running = svc
                                    .get("replicas_running")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                let desired = svc
                                    .get("replicas_desired")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                let endpoints = svc
                                    .get("endpoints")
                                    .and_then(|v| v.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|e| e.as_str())
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    })
                                    .unwrap_or_default();

                                if endpoints.is_empty() {
                                    println!("  {}: {}/{} replicas", name, running, desired);
                                } else {
                                    println!(
                                        "  {}: {} ({}/{} replicas)",
                                        name, endpoints, running, desired
                                    );
                                }
                            }
                        } else {
                            // Fall back to spec-based display
                            for (name, svc) in &spec.services {
                                let replicas = match &svc.scale {
                                    zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
                                    zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
                                    zlayer_spec::ScaleSpec::Manual => 0,
                                };
                                let endpoints: Vec<String> = svc
                                    .endpoints
                                    .iter()
                                    .map(|ep| {
                                        format!(
                                            "{}://localhost:{}",
                                            format!("{:?}", ep.protocol).to_lowercase(),
                                            ep.port
                                        )
                                    })
                                    .collect();
                                if endpoints.is_empty() {
                                    println!("  {}: {}/{} replicas", name, replicas, replicas);
                                } else {
                                    println!(
                                        "  {}: {} ({}/{} replicas)",
                                        name,
                                        endpoints.join(", "),
                                        replicas,
                                        replicas
                                    );
                                }
                            }
                        }
                        println!("Use 'zlayer ps' for details, 'zlayer logs SERVICE' for logs");

                        // Also emit the event for the TUI logger
                        let summary_services: Vec<(String, u32)> = spec
                            .services
                            .iter()
                            .map(|(name, svc)| {
                                let replicas = match &svc.scale {
                                    zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
                                    zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
                                    zlayer_spec::ScaleSpec::Manual => 0,
                                };
                                (name.clone(), replicas)
                            })
                            .collect();
                        emit(
                            &tx,
                            DeployEvent::DeploymentRunning {
                                services: summary_services,
                            },
                        );

                        // In foreground mode, wait for Ctrl+C
                        if !cli.detach && !cli.background {
                            emit(
                                &tx,
                                DeployEvent::Log {
                                    level: LogLevel::Info,
                                    message: "Deployment running. Press Ctrl+C to detach (daemon keeps running).".to_string(),
                                },
                            );
                            wait_for_ctrl_c_or_status(&client, &deployment_name, &tx, &spec).await;
                        }

                        return Ok(());
                    }
                    s if s.starts_with("failed") || s == "error" => {
                        // Print enhanced failure output
                        eprintln!();
                        eprintln!("Deployment '{}' failed:", deployment_name);

                        if let Some(health_arr) =
                            deployment.get("service_health").and_then(|v| v.as_array())
                        {
                            for svc in health_arr {
                                let name = svc.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                                let running = svc
                                    .get("replicas_running")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                let desired = svc
                                    .get("replicas_desired")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                let health = svc
                                    .get("health")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown");
                                eprintln!(
                                    "  {}: {}/{} replicas ready ({})",
                                    name, running, desired, health
                                );
                            }
                        } else {
                            // Fall back to spec-based display
                            for (name, svc) in &spec.services {
                                let desired = match &svc.scale {
                                    zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
                                    zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
                                    zlayer_spec::ScaleSpec::Manual => 0,
                                };
                                eprintln!("  {}: 0/{} replicas ready", name, desired);
                            }
                        }
                        eprintln!(
                            "Use 'zlayer logs SERVICE' for full logs, 'zlayer down' to clean up"
                        );

                        anyhow::bail!("Deployment '{}' failed", deployment_name,);
                    }
                    _ => {
                        // Still in progress -- keep polling
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to poll deployment status");
            }
        }
    }

    // Timeout
    warn!(
        deployment = %deployment_name,
        elapsed_secs = start.elapsed().as_secs(),
        "Timed out waiting for deployment to become ready"
    );
    emit(
        &tx,
        DeployEvent::Log {
            level: LogLevel::Warn,
            message: format!(
                "Timed out after {}s waiting for deployment '{}' to become ready. \
                 The daemon is still processing -- check `zlayer-runtime status` for updates.",
                poll_timeout.as_secs(),
                deployment_name,
            ),
        },
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// up
// ---------------------------------------------------------------------------

/// Deploy and start services (like docker compose up).
///
/// Resolves the deployment mode from CLI flags, then delegates to `deploy()`.
/// The daemon handles all infrastructure; this is a thin wrapper that decides
/// whether to stay attached after the deployment reaches a running state.
pub(crate) async fn up(cli: &Cli, spec_path: &Path) -> Result<()> {
    deploy(cli, spec_path, false).await
}

// ---------------------------------------------------------------------------
// down
// ---------------------------------------------------------------------------

/// Stop all services in a deployment (like docker compose down).
///
/// Auto-discovers the deployment name from the local spec file if not given.
/// Sends `DELETE /api/v1/deployments/{name}` to the daemon which handles all
/// container teardown, overlay cleanup, and state removal.
pub(crate) async fn down(deployment: Option<String>) -> Result<()> {
    // Resolve deployment name from spec if not provided explicitly
    let deployment_name = match deployment {
        Some(name) => name,
        None => {
            let spec_path = discover_spec_path(None)?;
            let spec = parse_spec(&spec_path)?;
            spec.deployment.clone()
        }
    };

    info!(deployment = %deployment_name, "Requesting deployment teardown");
    println!("Tearing down deployment: {}...", deployment_name);

    // Connect to daemon
    let client = DaemonClient::connect().await?;

    client
        .delete_deployment(&deployment_name)
        .await
        .with_context(|| format!("Failed to delete deployment '{}'", deployment_name))?;

    println!("Deployment '{}' stopped.", deployment_name);
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Wait for Ctrl+C in foreground mode, periodically polling the daemon for
/// deployment status and printing updates.
///
/// When the user presses Ctrl+C, we simply exit the CLI. The daemon continues
/// running and managing the deployment. This is intentional: the daemon is a
/// long-lived process, and `deploy`/`up` in foreground mode is just a "watch"
/// view.
async fn wait_for_ctrl_c_or_status(
    client: &DaemonClient,
    deployment_name: &str,
    tx: &mpsc::Sender<DeployEvent>,
    spec: &zlayer_spec::DeploymentSpec,
) {
    use crate::deploy_tui::{ServiceHealth, ServiceStatus};

    let mut tick_interval = tokio::time::interval(Duration::from_secs(5));
    // Consume the first immediate tick
    tick_interval.tick().await;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                emit(tx, DeployEvent::Log {
                    level: LogLevel::Info,
                    message: "Detaching from deployment (daemon continues running).".to_string(),
                });
                break;
            }
            _ = tick_interval.tick() => {
                // Poll daemon for current deployment status
                match client.get_deployment(deployment_name).await {
                    Ok(deployment) => {
                        let current_status = deployment
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");

                        // If the deployment stopped unexpectedly, inform and exit
                        if current_status == "stopped" || current_status.starts_with("failed") {
                            emit(tx, DeployEvent::Log {
                                level: LogLevel::Warn,
                                message: format!(
                                    "Deployment '{}' is now '{}'. Exiting watch.",
                                    deployment_name, current_status
                                ),
                            });
                            break;
                        }

                        // Use live service_health data from daemon if available,
                        // fall back to spec-based estimates
                        let statuses: Vec<ServiceStatus> = if let Some(health_arr) =
                            deployment.get("service_health").and_then(|v| v.as_array())
                        {
                            health_arr
                                .iter()
                                .map(|svc| {
                                    let name = svc.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string();
                                    let running = svc.get("replicas_running").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                                    let desired = svc.get("replicas_desired").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                                    let health_str = svc.get("health").and_then(|v| v.as_str()).unwrap_or("unknown");
                                    let health = match health_str {
                                        "healthy" => ServiceHealth::Healthy,
                                        s if s.starts_with("unhealthy") => ServiceHealth::Unhealthy,
                                        _ => ServiceHealth::Unknown,
                                    };
                                    ServiceStatus {
                                        name,
                                        replicas_running: running,
                                        replicas_target: desired,
                                        health,
                                    }
                                })
                                .collect()
                        } else {
                            spec.services
                                .iter()
                                .map(|(name, svc)| {
                                    let target = match &svc.scale {
                                        zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
                                        zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
                                        zlayer_spec::ScaleSpec::Manual => 0,
                                    };
                                    ServiceStatus {
                                        name: name.clone(),
                                        replicas_running: target,
                                        replicas_target: target,
                                        health: if target > 0 {
                                            ServiceHealth::Healthy
                                        } else {
                                            ServiceHealth::Unknown
                                        },
                                    }
                                })
                                .collect()
                        };

                        emit(tx, DeployEvent::StatusTick { services: statuses });
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to poll daemon for deployment status");
                    }
                }
            }
        }
    }
}
