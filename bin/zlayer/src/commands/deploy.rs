//! Deploy command handlers (thin daemon client).
//!
//! Contains the `deploy`, `up`, and `down` command implementations.
//! These are thin clients that delegate to the daemon process over a Unix socket.
//!
//! The daemon (`zlayer serve --daemon`) owns all infrastructure: overlay
//! networking, DNS, proxy, container supervisor, etc.  These commands simply send
//! the deployment spec to the daemon API and poll for status.
//!
//! Dry-run mode is the only path that does NOT require the daemon -- it validates
//! the spec locally and displays the deployment plan.

use anyhow::{Context, Result};
use std::io::IsTerminal;
use std::path::Path;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::cli::Cli;
use crate::deploy_tui::{
    app::DeployTui, DeployEvent, InfraPhase, LogLevel, PlainDeployLogger, ServicePlan,
};
use crate::util::{discover_spec_path, parse_spec};
use zlayer_client::DaemonClient;

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
        zlayer_spec::ScaleSpec::Fixed { replicas } => format!("fixed({replicas})"),
        zlayer_spec::ScaleSpec::Adaptive { min, max, .. } => {
            format!("adaptive({min}-{max})")
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

    let is_color = std::io::stdout().is_terminal();
    std::thread::spawn(move || {
        let logger = PlainDeployLogger::with_color(is_color);
        logger.process_events(rx);
    });

    tx
}

/// Set up the interactive TUI event channel, returning the sender and a join
/// handle for the TUI task.
///
/// Spawns `DeployTui::run()` on a blocking task. The TUI owns the terminal
/// (raw mode + alternate screen) and exits on channel-close once the deploy
/// task drops the sender. Ctrl+C inside the TUI calls `shutdown.notify_one()`
/// rather than killing the process, so the caller must also listen on
/// `shutdown` and tear down the deploy flow when it fires.
fn setup_tui_channel(
    shutdown: Arc<Notify>,
) -> (mpsc::Sender<DeployEvent>, JoinHandle<std::io::Result<()>>) {
    let (tx, rx) = mpsc::channel::<DeployEvent>();

    let handle = tokio::task::spawn_blocking(move || {
        let mut tui = DeployTui::new(rx, shutdown);
        tui.run()
    });

    (tx, handle)
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
#[allow(
    clippy::too_many_lines,
    clippy::assigning_clones,
    clippy::cast_possible_truncation
)]
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

    // Decide between TUI and plain logger. TUI is the default when stdout is
    // a TTY and --no-tui wasn't passed; detach/background still use the TUI so
    // the submit -> register -> scale -> ready (or failed) sequence animates.
    let use_tui = !cli.no_tui && std::io::stdout().is_terminal();
    let shutdown = Arc::new(Notify::new());
    let (tx, tui_handle): (
        mpsc::Sender<DeployEvent>,
        Option<JoinHandle<std::io::Result<()>>>,
    ) = if use_tui {
        let (tx, h) = setup_tui_channel(shutdown.clone());
        (tx, Some(h))
    } else {
        (setup_plain_channel(), None)
    };

    let run_result: Result<()> = async {
        // Display the plan before submitting
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

        let submit_result = client
            .create_deployment(&spec_yaml)
            .await
            .context("Failed to submit deployment to daemon")?;

        let deployment_name = submit_result
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&spec.deployment)
            .to_string();

        let status = submit_result
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        info!(
            deployment = %deployment_name,
            status = %status,
            "Deployment submitted"
        );

    // ------------------------------------------------------------------
    // Foreground mode: stream SSE events for real-time progress
    // ------------------------------------------------------------------
    if !cli.detach && !cli.background {
        match client.watch_deployment(&deployment_name).await {
            Ok(mut rx) => {
                let mut deployment_ready = false;
                let mut deployment_failed = false;
                let mut failure_message = String::new();

                let mut infra_synthesized = false;
                while let Some((event_type, data)) = rx.recv().await {
                    match event_type.as_str() {
                        "started" => {
                            let services: Vec<String> = serde_json::from_str(&data)
                                .ok()
                                .and_then(|v: serde_json::Value| {
                                    v.get("services").and_then(|s| s.as_array()).map(|arr| {
                                        arr.iter()
                                            .filter_map(|s| s.as_str().map(String::from))
                                            .collect()
                                    })
                                })
                                .unwrap_or_default();
                            emit(
                                &tx,
                                DeployEvent::Log {
                                    level: LogLevel::Info,
                                    message: format!(
                                        "Orchestrating {} service(s): {}",
                                        services.len(),
                                        services.join(", ")
                                    ),
                                },
                            );
                            // The daemon doesn't publish infra-lifecycle events
                            // today; by the time "started" fires the infrastructure
                            // is already up, so mark each phase Complete once so
                            // the TUI's infra panel renders filled instead of all
                            // Pending spinners. Revisit if/when the daemon exposes
                            // real InfraPhase* events.
                            if !infra_synthesized {
                                infra_synthesized = true;
                                for phase in [
                                    InfraPhase::Runtime,
                                    InfraPhase::Overlay,
                                    InfraPhase::Dns,
                                    InfraPhase::Proxy,
                                    InfraPhase::Supervisor,
                                    InfraPhase::Api,
                                ] {
                                    emit(
                                        &tx,
                                        DeployEvent::InfraPhaseComplete {
                                            phase,
                                            success: true,
                                            message: None,
                                        },
                                    );
                                }
                            }
                            // Seed the service panel with one entry per planned
                            // service so the TUI has something to render while
                            // the per-service events stream in.
                            for name in services {
                                emit(&tx, DeployEvent::ServiceDeployStarted { name });
                            }
                        }
                        "service_registered" => {
                            if let Some(svc) = parse_service_field(&data) {
                                emit(
                                    &tx,
                                    DeployEvent::Log {
                                        level: LogLevel::Info,
                                        message: format!("  [{svc}] registered"),
                                    },
                                );
                                emit(&tx, DeployEvent::ServiceRegistered { name: svc });
                            }
                        }
                        "service_registration_failed" => {
                            if let Some((svc, err)) = parse_service_error_fields(&data) {
                                emit(
                                    &tx,
                                    DeployEvent::Log {
                                        level: LogLevel::Warn,
                                        message: format!("  [{svc}] registration failed: {err}"),
                                    },
                                );
                                emit(
                                    &tx,
                                    DeployEvent::ServiceDeployFailed {
                                        name: svc,
                                        error: err,
                                    },
                                );
                            }
                        }
                        "overlay_created" => {
                            if let Some(svc) = parse_service_field(&data) {
                                let iface = serde_json::from_str::<serde_json::Value>(&data)
                                    .ok()
                                    .and_then(|v| {
                                        v.get("interface")
                                            .and_then(|i| i.as_str())
                                            .map(String::from)
                                    })
                                    .unwrap_or_default();
                                emit(
                                    &tx,
                                    DeployEvent::Log {
                                        level: LogLevel::Info,
                                        message: format!("  [{svc}] overlay network: {iface}"),
                                    },
                                );
                            }
                        }
                        "overlay_failed" => {
                            if let Some((svc, err)) = parse_service_error_fields(&data) {
                                emit(
                                    &tx,
                                    DeployEvent::Log {
                                        level: LogLevel::Warn,
                                        message: format!(
                                            "  [{svc}] overlay failed (non-fatal): {err}"
                                        ),
                                    },
                                );
                            }
                        }
                        "proxy_configured" => {
                            if let Some(svc) = parse_service_field(&data) {
                                emit(
                                    &tx,
                                    DeployEvent::Log {
                                        level: LogLevel::Info,
                                        message: format!("  [{svc}] proxy configured"),
                                    },
                                );
                            }
                        }
                        "proxy_failed" => {
                            if let Some((svc, err)) = parse_service_error_fields(&data) {
                                emit(
                                    &tx,
                                    DeployEvent::Log {
                                        level: LogLevel::Warn,
                                        message: format!(
                                            "  [{svc}] proxy failed (non-fatal): {err}"
                                        ),
                                    },
                                );
                            }
                        }
                        "service_scaling" => {
                            if let Some(svc) = parse_service_field(&data) {
                                let target = serde_json::from_str::<serde_json::Value>(&data)
                                    .ok()
                                    .and_then(|v| {
                                        v.get("target").and_then(serde_json::Value::as_u64)
                                    })
                                    .unwrap_or(0);
                                emit(
                                    &tx,
                                    DeployEvent::Log {
                                        level: LogLevel::Info,
                                        message: format!(
                                            "  [{svc}] scaling to {target} replica(s)..."
                                        ),
                                    },
                                );
                                emit(
                                    &tx,
                                    DeployEvent::ServiceScaling {
                                        name: svc,
                                        target_replicas: target as u32,
                                    },
                                );
                            }
                        }
                        "service_scaled" => {
                            if let Some(svc) = parse_service_field(&data) {
                                let replicas = serde_json::from_str::<serde_json::Value>(&data)
                                    .ok()
                                    .and_then(|v| {
                                        v.get("replicas").and_then(serde_json::Value::as_u64)
                                    })
                                    .unwrap_or(0);
                                emit(
                                    &tx,
                                    DeployEvent::Log {
                                        level: LogLevel::Info,
                                        message: format!(
                                            "  [{svc}] scaled to {replicas} replica(s)"
                                        ),
                                    },
                                );
                                emit(
                                    &tx,
                                    DeployEvent::ServiceDeployComplete {
                                        name: svc,
                                        replicas: replicas as u32,
                                    },
                                );
                            }
                        }
                        "service_scale_failed" => {
                            if let Some((svc, err)) = parse_service_error_fields(&data) {
                                emit(
                                    &tx,
                                    DeployEvent::Log {
                                        level: LogLevel::Warn,
                                        message: format!("  [{svc}] scaling failed: {err}"),
                                    },
                                );
                                emit(
                                    &tx,
                                    DeployEvent::ServiceDeployFailed {
                                        name: svc,
                                        error: err,
                                    },
                                );
                            }
                        }
                        "stabilizing" => {
                            emit(
                                &tx,
                                DeployEvent::Log {
                                    level: LogLevel::Info,
                                    message: "Waiting for stabilization...".to_string(),
                                },
                            );
                        }
                        "ready" => {
                            deployment_ready = true;
                        }
                        "failed" => {
                            deployment_failed = true;
                            failure_message = serde_json::from_str::<serde_json::Value>(&data)
                                .ok()
                                .and_then(|v| {
                                    v.get("message").and_then(|m| m.as_str()).map(String::from)
                                })
                                .unwrap_or_else(|| "Unknown failure".to_string());
                        }
                        other => {
                            debug!(event = other, "Unrecognized SSE deployment event");
                        }
                    }
                }

                // Stream ended -- handle terminal state
                if deployment_ready {
                    return print_deployment_success(
                        &client,
                        &deployment_name,
                        &spec,
                        &tx,
                        cli,
                        shutdown.clone(),
                    )
                    .await;
                } else if deployment_failed {
                    return print_deployment_failure(
                        &client,
                        &deployment_name,
                        &spec,
                        &failure_message,
                    );
                }

                // Stream closed without terminal event -- fall through to poll
                emit(
                    &tx,
                    DeployEvent::Log {
                        level: LogLevel::Warn,
                        message:
                            "SSE stream ended without terminal event, falling back to polling..."
                                .to_string(),
                    },
                );
            }
            Err(e) => {
                debug!(error = %e, "Failed to connect to SSE event stream, falling back to polling");
            }
        }
    }

    // ------------------------------------------------------------------
    // Background / detach / fallback: poll for readiness
    // ------------------------------------------------------------------
    let poll_timeout = Duration::from_secs(120);
    let poll_interval = Duration::from_secs(2);
    let start = std::time::Instant::now();
    let mut last_status = status.to_string();
    let mut attempt: u32 = 0;

    while start.elapsed() < poll_timeout {
        tokio::time::sleep(poll_interval).await;
        attempt += 1;

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
                                "Deployment '{deployment_name}': {last_status} -> {current_status}"
                            ),
                        },
                    );
                    last_status = current_status.clone();
                }

                // Terminal states
                match current_status.as_str() {
                    "running" | "active" => {
                        return print_deployment_success(
                            &client,
                            &deployment_name,
                            &spec,
                            &tx,
                            cli,
                            shutdown.clone(),
                        )
                        .await;
                    }
                    s if s.starts_with("failed") || s == "error" => {
                        let msg = deployment
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown failure")
                            .to_string();
                        return print_deployment_failure(&client, &deployment_name, &spec, &msg);
                    }
                    _ => {
                        // Still in progress -- keep polling
                    }
                }
            }
            Err(e) => {
                // Use debug for the first 15 attempts, warn after
                if attempt <= 15 {
                    debug!(error = %e, attempt, "Failed to poll deployment status");
                } else {
                    warn!(error = %e, attempt, "Failed to poll deployment status");
                }
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
                     The daemon is still processing -- check `zlayer status` for updates.",
                    poll_timeout.as_secs(),
                    deployment_name,
                ),
            },
        );

        Ok(())
    }
    .await;

    // Cleanup: drop the sender so the channel closes, then wait for the TUI
    // task (if any) to restore the terminal and exit cleanly. The TUI
    // auto-exits on channel close when the phase is Running/Complete
    // (app.rs handles this).
    drop(tx);
    if let Some(h) = tui_handle {
        match h.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!(error = %e, "Deploy TUI reported error"),
            Err(e) => warn!(error = %e, "Deploy TUI task panicked"),
        }
    }

    run_result
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
    let deployment_name = if let Some(name) = deployment {
        name
    } else {
        let spec_path = discover_spec_path(None)?;
        let spec = parse_spec(&spec_path)?;
        spec.deployment.clone()
    };

    info!(deployment = %deployment_name, "Requesting deployment teardown");
    println!("Tearing down deployment: {deployment_name}...");

    // Connect to daemon
    let client = DaemonClient::connect().await?;

    client
        .delete_deployment(&deployment_name)
        .await
        .with_context(|| format!("Failed to delete deployment '{deployment_name}'"))?;

    println!("Deployment '{deployment_name}' stopped.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse the `service` field from a JSON data payload.
fn parse_service_field(data: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(data)
        .ok()
        .and_then(|v| v.get("service").and_then(|s| s.as_str()).map(String::from))
}

/// Parse the `service` and `error` fields from a JSON data payload.
fn parse_service_error_fields(data: &str) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let svc = v.get("service").and_then(|s| s.as_str())?.to_string();
    let err = v
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap_or("unknown")
        .to_string();
    Some((svc, err))
}

/// Print deployment success output, fetch final health info from daemon, then
/// optionally wait for Ctrl+C in foreground mode.
#[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
async fn print_deployment_success(
    client: &DaemonClient,
    deployment_name: &str,
    spec: &zlayer_spec::DeploymentSpec,
    tx: &mpsc::Sender<DeployEvent>,
    cli: &Cli,
    shutdown: Arc<Notify>,
) -> Result<()> {
    // Fetch final deployment details for health summary
    let deployment = client.get_deployment(deployment_name).await.ok();

    let service_health = deployment
        .as_ref()
        .and_then(|d| d.get("service_health"))
        .and_then(|v| v.as_array());

    let total_services = spec.services.len();
    let healthy_count = if let Some(health_arr) = service_health {
        health_arr
            .iter()
            .filter(|s| {
                let running = s
                    .get("replicas_running")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let desired = s
                    .get("replicas_desired")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                running >= desired
            })
            .count()
    } else {
        total_services
    };

    println!();
    println!(
        "Deployment '{deployment_name}' ready ({healthy_count}/{total_services} services healthy):"
    );

    if let Some(health_arr) = service_health {
        for svc in health_arr {
            let name = svc.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let running = svc
                .get("replicas_running")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let desired = svc
                .get("replicas_desired")
                .and_then(serde_json::Value::as_u64)
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
                println!("  {name}: {running}/{desired} replicas");
            } else {
                println!("  {name}: {endpoints} ({running}/{desired} replicas)");
            }
        }
    } else {
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
                println!("  {name}: {replicas}/{replicas} replicas");
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
        tx,
        DeployEvent::DeploymentRunning {
            services: summary_services,
        },
    );

    // In foreground mode, wait for Ctrl+C
    if !cli.detach && !cli.background {
        emit(
            tx,
            DeployEvent::Log {
                level: LogLevel::Info,
                message: "Deployment running. Press Ctrl+C to detach (daemon keeps running)."
                    .to_string(),
            },
        );
        wait_for_ctrl_c_or_status(client, deployment_name, tx, spec, shutdown).await;
    }

    Ok(())
}

/// Print deployment failure output and bail.
#[allow(clippy::cast_possible_truncation)]
fn print_deployment_failure(
    client: &DaemonClient,
    deployment_name: &str,
    spec: &zlayer_spec::DeploymentSpec,
    message: &str,
) -> Result<()> {
    // We intentionally don't fetch health info here to avoid blocking on a
    // potentially unresponsive daemon. The SSE stream already gave us the
    // failure message.
    let _ = client; // suppress unused warning

    eprintln!();
    eprintln!("Deployment '{deployment_name}' failed: {message}");

    for (name, svc) in &spec.services {
        let desired = match &svc.scale {
            zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
            zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
            zlayer_spec::ScaleSpec::Manual => 0,
        };
        eprintln!("  {name}: 0/{desired} replicas ready");
    }
    eprintln!("Use 'zlayer logs SERVICE' for full logs, 'zlayer down' to clean up");

    anyhow::bail!("Deployment '{deployment_name}' failed")
}

/// Wait for Ctrl+C in foreground mode, periodically polling the daemon for
/// deployment status and printing updates.
///
/// When the user presses Ctrl+C, we simply exit the CLI. The daemon continues
/// running and managing the deployment. This is intentional: the daemon is a
/// long-lived process, and `deploy`/`up` in foreground mode is just a "watch"
/// view.
#[allow(clippy::cast_possible_truncation)]
async fn wait_for_ctrl_c_or_status(
    client: &DaemonClient,
    deployment_name: &str,
    tx: &mpsc::Sender<DeployEvent>,
    spec: &zlayer_spec::DeploymentSpec,
    shutdown: Arc<Notify>,
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
            () = shutdown.notified() => {
                // TUI intercepts Ctrl+C as a key event in raw mode and signals
                // us via this Notify; without this branch the loop would hang.
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
                                    "Deployment '{deployment_name}' is now '{current_status}'. Exiting watch."
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
                                    let running = svc.get("replicas_running").and_then(serde_json::Value::as_u64).unwrap_or(0) as u32;
                                    let desired = svc.get("replicas_desired").and_then(serde_json::Value::as_u64).unwrap_or(0) as u32;
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
