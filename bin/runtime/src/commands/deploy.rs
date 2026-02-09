//! Deploy command handlers.
//!
//! Contains the `deploy`, `up`, and `down` command implementations along with
//! the shared `deploy_services` orchestration logic and deployment plan display.
//!
//! All user-facing output is routed through `DeployEvent`s, which are consumed
//! by the `PlainDeployLogger` (or a future TUI). The deploy orchestration sends
//! events via `std::sync::mpsc::Sender<DeployEvent>`.

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::mpsc;
use std::sync::Arc;
use tracing::{info, warn};
use zlayer_spec::DeploymentSpec;

use crate::cli::{Cli, DeployMode};
use crate::config::build_runtime_config;
use crate::deploy_tui::{
    DeployEvent, DeployTui, InfraPhase, LogLevel, PlainDeployLogger, ServiceHealth, ServicePlan,
    ServiceStatus,
};
use crate::util::{discover_spec_path, parse_spec};

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

/// Set up the plain (non-TUI) event channel and logger thread, returning the sender
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

/// Deploy services from a spec file
pub(crate) async fn deploy(cli: &Cli, spec_path: &Path, dry_run: bool) -> Result<()> {
    let spec = parse_spec(spec_path)?;
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

    let mode = if cli.detach {
        DeployMode::Detach
    } else {
        DeployMode::Foreground
    };

    let use_tui = !cli.no_tui && std::io::IsTerminal::is_terminal(&std::io::stdout());

    if use_tui {
        run_with_tui(cli, &spec, mode).await
    } else {
        let tx = setup_plain_channel();
        deploy_services(cli, &spec, mode, tx).await
    }
}

/// Run the deploy orchestration with the interactive TUI
///
/// Spawns the TUI on a dedicated OS thread (since it blocks on terminal I/O),
/// wires up a shared shutdown signal so the TUI's Ctrl+C can reach the deploy
/// task, and waits for both sides to finish.
async fn run_with_tui(cli: &Cli, spec: &DeploymentSpec, mode: DeployMode) -> Result<()> {
    let shutdown_signal = Arc::new(tokio::sync::Notify::new());
    let (tx, rx) = mpsc::channel::<DeployEvent>();

    let shutdown_clone = shutdown_signal.clone();
    let tui_handle = std::thread::spawn(move || {
        let mut tui = DeployTui::new(rx, shutdown_clone);
        tui.run()
    });

    // Run deploy with a shutdown select
    let result = tokio::select! {
        result = deploy_services(cli, spec, mode, tx.clone()) => result,
        _ = shutdown_signal.notified() => {
            // TUI requested shutdown via Ctrl+C.
            // deploy_services handles its own ctrl_c, so this path is for
            // when the TUI's Ctrl+C fires before the tokio ctrl_c handler.
            Ok(())
        }
    };

    // Drop the sender so the TUI thread knows to exit after processing remaining events
    drop(tx);

    // Wait for TUI thread to finish (restores terminal)
    if let Err(e) = tui_handle.join() {
        eprintln!("TUI thread panicked: {:?}", e);
    }

    result
}

/// Shared deployment logic used by both `deploy` and `up` commands
///
/// Sets up the full infrastructure stack (runtime, overlay, DNS, proxy, supervisor, API)
/// then deploys all services from the spec. In foreground mode, waits for Ctrl+C
/// and performs graceful shutdown. In background mode, returns immediately after deploying.
///
/// The caller provides the `event_tx` channel sender, which allows choosing between
/// the plain logger or the interactive TUI as the event consumer.
pub(crate) async fn deploy_services(
    cli: &Cli,
    spec: &DeploymentSpec,
    mode: DeployMode,
    event_tx: mpsc::Sender<DeployEvent>,
) -> Result<()> {
    use std::time::Duration;

    // 1. Runtime
    let runtime_config = build_runtime_config(cli);
    emit(
        &event_tx,
        DeployEvent::InfraPhaseStarted {
            phase: InfraPhase::Runtime,
        },
    );

    let runtime = zlayer_agent::create_runtime(runtime_config)
        .await
        .context("Failed to create container runtime")?;

    emit(
        &event_tx,
        DeployEvent::InfraPhaseComplete {
            phase: InfraPhase::Runtime,
            success: true,
            message: None,
        },
    );

    // 2. Overlay Manager
    emit(
        &event_tx,
        DeployEvent::InfraPhaseStarted {
            phase: InfraPhase::Overlay,
        },
    );

    let mut overlay_error: Option<String> = None;

    let overlay_manager = if cli.host_network {
        info!("Host networking mode: skipping overlay network setup");
        emit(
            &event_tx,
            DeployEvent::InfraPhaseComplete {
                phase: InfraPhase::Overlay,
                success: true,
                message: Some("skipped (--host-network)".to_string()),
            },
        );
        None
    } else {
        match zlayer_agent::OverlayManager::new(spec.deployment.clone()).await {
            Ok(mut om) => {
                if let Err(e) = om.setup_global_overlay().await {
                    let err_msg = format!("global overlay setup failed: {}", e);
                    warn!("Failed to setup global overlay: {}", e);
                    overlay_error = Some(err_msg.clone());
                    emit(
                        &event_tx,
                        DeployEvent::InfraPhaseComplete {
                            phase: InfraPhase::Overlay,
                            success: false,
                            message: Some(err_msg),
                        },
                    );
                    None
                } else {
                    emit(
                        &event_tx,
                        DeployEvent::InfraPhaseComplete {
                            phase: InfraPhase::Overlay,
                            success: true,
                            message: Some("global overlay created".to_string()),
                        },
                    );
                    Some(Arc::new(tokio::sync::RwLock::new(om)))
                }
            }
            Err(e) => {
                let err_msg = format!("overlay manager creation failed: {}", e);
                warn!("Overlay networks disabled: {}", e);
                overlay_error = Some(err_msg.clone());
                emit(
                    &event_tx,
                    DeployEvent::InfraPhaseComplete {
                        phase: InfraPhase::Overlay,
                        success: false,
                        message: Some(format!("disabled: {}", e)),
                    },
                );
                None
            }
        }
    };

    // If overlay failed, check whether any service actually needs networking.
    // Services with endpoints need overlay networking to function.
    if overlay_manager.is_none() && !cli.host_network {
        let services_needing_network: Vec<&str> = spec
            .services
            .iter()
            .filter(|(_, svc)| !svc.endpoints.is_empty())
            .map(|(name, _)| name.as_str())
            .collect();

        if !services_needing_network.is_empty() {
            let err_detail = overlay_error.as_deref().unwrap_or("unknown error");
            anyhow::bail!(
                "Overlay network failed: {}. Services needing overlay: [{}]. \
                 Ensure CAP_NET_ADMIN capability or use --host-network flag to skip overlay.",
                err_detail,
                services_needing_network.join(", ")
            );
        }
    }

    // 3. DNS Server
    emit(
        &event_tx,
        DeployEvent::InfraPhaseStarted {
            phase: InfraPhase::Dns,
        },
    );

    let dns_server = {
        let dns_addr = std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            15353,
        );
        let zone = format!("{}.local.", spec.deployment);
        match zlayer_overlay::DnsServer::new(dns_addr, &zone) {
            Ok(dns) => {
                let dns = Arc::new(dns);
                match dns.start_background().await {
                    Ok(_handle) => {
                        emit(
                            &event_tx,
                            DeployEvent::InfraPhaseComplete {
                                phase: InfraPhase::Dns,
                                success: true,
                                message: Some(format!("{}", dns_addr)),
                            },
                        );
                        Some(dns)
                    }
                    Err(e) => {
                        warn!("Failed to start DNS server: {}", e);
                        emit(
                            &event_tx,
                            DeployEvent::InfraPhaseComplete {
                                phase: InfraPhase::Dns,
                                success: false,
                                message: Some(format!("start failed: {}", e)),
                            },
                        );
                        None
                    }
                }
            }
            Err(e) => {
                warn!("Failed to create DNS server: {}", e);
                emit(
                    &event_tx,
                    DeployEvent::InfraPhaseComplete {
                        phase: InfraPhase::Dns,
                        success: false,
                        message: Some(format!("create failed: {}", e)),
                    },
                );
                None
            }
        }
    };

    // 4. Proxy Manager
    emit(
        &event_tx,
        DeployEvent::InfraPhaseStarted {
            phase: InfraPhase::Proxy,
        },
    );

    let proxy_manager = {
        let config = zlayer_agent::ProxyManagerConfig::default();
        let pm = Arc::new(zlayer_agent::ProxyManager::new(config));
        emit(
            &event_tx,
            DeployEvent::InfraPhaseComplete {
                phase: InfraPhase::Proxy,
                success: true,
                message: None,
            },
        );
        Some(pm)
    };

    // 5. Container Supervisor
    emit(
        &event_tx,
        DeployEvent::InfraPhaseStarted {
            phase: InfraPhase::Supervisor,
        },
    );

    let container_supervisor = {
        let supervisor = Arc::new(zlayer_agent::ContainerSupervisor::new(runtime.clone()));
        emit(
            &event_tx,
            DeployEvent::InfraPhaseComplete {
                phase: InfraPhase::Supervisor,
                success: true,
                message: None,
            },
        );
        Some(supervisor)
    };

    // 6. ServiceManager with builder chain
    let mut manager_builder = if let Some(om) = overlay_manager.clone() {
        zlayer_agent::ServiceManager::with_overlay(runtime.clone(), om)
    } else {
        zlayer_agent::ServiceManager::new(runtime.clone())
    };
    manager_builder.set_deployment_name(spec.deployment.clone());
    if let Some(pm) = &proxy_manager {
        manager_builder.set_proxy_manager(Arc::clone(pm));
    }
    if let Some(dns) = &dns_server {
        manager_builder.set_dns_server(Arc::clone(dns));
    }
    if let Some(sup) = &container_supervisor {
        manager_builder.set_container_supervisor(Arc::clone(sup));
    }
    let manager = Arc::new(manager_builder);

    // 7. Start container supervisor loop
    let supervisor_handle = if let Some(sup) = &container_supervisor {
        let sup_clone = Arc::clone(sup);
        Some(tokio::spawn(async move { sup_clone.run_loop().await }))
    } else {
        None
    };

    // 8. API Server
    let api_shutdown = Arc::new(tokio::sync::Notify::new());
    let api_handle = if spec.api.enabled {
        emit(
            &event_tx,
            DeployEvent::InfraPhaseStarted {
                phase: InfraPhase::Api,
            },
        );

        let jwt_secret = spec
            .api
            .jwt_secret
            .clone()
            .or_else(|| std::env::var("ZLAYER_JWT_SECRET").ok())
            .unwrap_or_else(|| {
                warn!("Using default JWT secret - NOT SAFE FOR PRODUCTION");
                emit(
                    &event_tx,
                    DeployEvent::Log {
                        level: LogLevel::Warn,
                        message: "Using default JWT secret - NOT SAFE FOR PRODUCTION".to_string(),
                    },
                );
                "CHANGE_ME_IN_PRODUCTION".to_string()
            });
        let bind_addr: std::net::SocketAddr = spec
            .api
            .bind
            .parse()
            .context(format!("Invalid API bind address: {}", spec.api.bind))?;
        let api_config = zlayer_api::ApiConfig {
            bind: bind_addr,
            jwt_secret,
            swagger_enabled: spec.api.swagger,
            ..Default::default()
        };
        emit(
            &event_tx,
            DeployEvent::InfraPhaseComplete {
                phase: InfraPhase::Api,
                success: true,
                message: Some(format!("bind={}, swagger={}", bind_addr, spec.api.swagger)),
            },
        );
        let server = zlayer_api::ApiServer::new(api_config);
        let shutdown_notify = api_shutdown.clone();
        Some(tokio::spawn(async move {
            let shutdown_future = async move {
                shutdown_notify.notified().await;
            };
            if let Err(e) = server.run_with_shutdown(shutdown_future).await {
                tracing::error!(error = %e, "API server error");
            }
        }))
    } else {
        None
    };

    // 9. Emit deployment plan and deploy services
    let plans: Vec<ServicePlan> = spec
        .services
        .iter()
        .map(|(name, svc)| build_service_plan(name, svc))
        .collect();
    emit(
        &event_tx,
        DeployEvent::PlanReady {
            deployment_name: spec.deployment.clone(),
            version: spec.version.clone(),
            services: plans,
        },
    );

    // Track deployment results
    let mut deployed_services: Vec<(String, u32)> = Vec::new();
    let mut failed_services: Vec<(String, String)> = Vec::new();

    // Deploy each service
    for (name, service_spec) in &spec.services {
        emit(
            &event_tx,
            DeployEvent::ServiceDeployStarted { name: name.clone() },
        );

        // Propagate host_network flag from CLI to the service spec
        let mut service_spec_with_flags = service_spec.clone();
        if cli.host_network {
            service_spec_with_flags.host_network = true;
        }

        // Register the service with ServiceManager
        match manager
            .upsert_service(name.clone(), service_spec_with_flags.clone())
            .await
        {
            Ok(()) => {
                emit(
                    &event_tx,
                    DeployEvent::ServiceRegistered { name: name.clone() },
                );

                // Setup service overlay network before scaling
                if let Some(om) = &overlay_manager {
                    let om_guard = om.read().await;
                    match om_guard.setup_service_overlay(name).await {
                        Ok(iface) => {
                            info!(service = %name, interface = %iface, "Service overlay created");
                        }
                        Err(e) => {
                            let error_msg = format!(
                                "Failed to setup service overlay for '{}': {}. \
                                 Service networking will not function. \
                                 Use --host-network flag to bypass overlay networking.",
                                name, e
                            );
                            warn!(service = %name, error = %e, "Failed to setup service overlay");
                            emit(
                                &event_tx,
                                DeployEvent::ServiceDeployFailed {
                                    name: name.clone(),
                                    error: error_msg.clone(),
                                },
                            );
                            failed_services.push((name.clone(), error_msg));
                            continue;
                        }
                    }
                }

                // Register service with proxy manager
                if let Some(pm) = &proxy_manager {
                    pm.add_service(name, &service_spec_with_flags).await;
                    if let Err(e) = pm.ensure_ports_for_service(&service_spec_with_flags).await {
                        warn!(service = %name, error = %e, "Failed to setup proxy ports (non-fatal)");
                    }
                }
            }
            Err(e) => {
                let error_msg = format!("Failed to register service: {}", e);
                warn!(service = %name, error = %e, "Failed to register service");
                emit(
                    &event_tx,
                    DeployEvent::ServiceDeployFailed {
                        name: name.clone(),
                        error: error_msg.clone(),
                    },
                );
                failed_services.push((name.clone(), error_msg));
                continue;
            }
        }

        // Determine initial replica count from scale spec
        let replicas = match &service_spec_with_flags.scale {
            zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
            zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
            zlayer_spec::ScaleSpec::Manual => 0,
        };

        if replicas > 0 {
            emit(
                &event_tx,
                DeployEvent::ServiceScaling {
                    name: name.clone(),
                    target_replicas: replicas,
                },
            );

            match manager.scale_service(name, replicas).await {
                Ok(()) => {
                    info!(service = %name, replicas = replicas, "Service scaled successfully");
                    deployed_services.push((name.clone(), replicas));
                    emit(
                        &event_tx,
                        DeployEvent::ServiceDeployComplete {
                            name: name.clone(),
                            replicas,
                        },
                    );
                }
                Err(e) => {
                    let error_msg = format!("Failed to scale service: {}", e);
                    warn!(service = %name, error = %e, "Failed to scale service");
                    emit(
                        &event_tx,
                        DeployEvent::ServiceDeployFailed {
                            name: name.clone(),
                            error: error_msg.clone(),
                        },
                    );
                    failed_services.push((name.clone(), error_msg));
                }
            }
        } else {
            emit(
                &event_tx,
                DeployEvent::ServiceDeployComplete {
                    name: name.clone(),
                    replicas: 0,
                },
            );
            emit(
                &event_tx,
                DeployEvent::Log {
                    level: LogLevel::Info,
                    message: format!("  {} registered (manual scaling - 0 replicas)", name),
                },
            );
            deployed_services.push((name.clone(), 0));
        }
    }

    // Wait for services to stabilize with a timeout
    if !deployed_services.is_empty() {
        emit(
            &event_tx,
            DeployEvent::Log {
                level: LogLevel::Info,
                message: "Waiting for services to stabilize...".to_string(),
            },
        );
        let stabilize_timeout = Duration::from_secs(30);
        let start = std::time::Instant::now();

        while start.elapsed() < stabilize_timeout {
            let mut all_ready = true;

            for (name, expected_replicas) in &deployed_services {
                if *expected_replicas == 0 {
                    continue;
                }

                match manager.service_replica_count(name).await {
                    Ok(count) if count as u32 == *expected_replicas => {
                        // Service has expected replicas
                    }
                    Ok(count) => {
                        emit(
                            &event_tx,
                            DeployEvent::ServiceReplicaUpdate {
                                name: name.clone(),
                                current: count as u32,
                                target: *expected_replicas,
                            },
                        );
                        all_ready = false;
                    }
                    Err(_) => {
                        all_ready = false;
                    }
                }
            }

            if all_ready {
                break;
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        if start.elapsed() >= stabilize_timeout {
            warn!("Timeout waiting for all services to stabilize");
            emit(
                &event_tx,
                DeployEvent::Log {
                    level: LogLevel::Warn,
                    message: "Timeout waiting for all services to reach desired state".to_string(),
                },
            );
        }
    }

    // Build final service list with actual counts for the summary
    let mut summary_services: Vec<(String, u32)> = Vec::new();
    for (name, replicas) in &deployed_services {
        let actual_count = manager.service_replica_count(name).await.unwrap_or(0) as u32;
        summary_services.push((name.clone(), actual_count));
        // Log mismatch at warn level via tracing (not user-facing)
        if actual_count != *replicas && *replicas > 0 {
            warn!(
                service = %name,
                actual = actual_count,
                expected = replicas,
                "Service replica count mismatch"
            );
        }
    }

    // Report failed services
    for (name, error) in &failed_services {
        emit(
            &event_tx,
            DeployEvent::ServiceDeployFailed {
                name: name.clone(),
                error: error.clone(),
            },
        );
    }

    // List all managed services for tracing
    let all_services = manager.list_services().await;
    info!(
        services = ?all_services,
        deployed = deployed_services.len(),
        failed = failed_services.len(),
        "Deployment complete"
    );

    if !failed_services.is_empty() {
        let details: Vec<String> = failed_services
            .iter()
            .map(|(name, err)| format!("  - {}: {}", name, err))
            .collect();
        anyhow::bail!(
            "Deployment completed with {} failed service(s):\n{}",
            failed_services.len(),
            details.join("\n")
        )
    }

    // 10. Post-deploy based on mode
    match mode {
        DeployMode::Background => {
            emit(
                &event_tx,
                DeployEvent::DeploymentRunning {
                    services: summary_services,
                },
            );
            emit(
                &event_tx,
                DeployEvent::Log {
                    level: LogLevel::Info,
                    message: "Deployment running in background.".to_string(),
                },
            );
            return Ok(());
        }
        DeployMode::Detach => {
            emit(
                &event_tx,
                DeployEvent::DeploymentRunning {
                    services: summary_services,
                },
            );
            emit(
                &event_tx,
                DeployEvent::Log {
                    level: LogLevel::Info,
                    message: "Services running. Detaching.".to_string(),
                },
            );
            return Ok(());
        }
        DeployMode::Foreground => {
            // Handle autoscaling if needed
            let has_adaptive = zlayer_agent::has_adaptive_scaling(&spec.services);

            let autoscale_state = if has_adaptive {
                let autoscale_interval = Duration::from_secs(10);
                let controller = Arc::new(zlayer_agent::AutoscaleController::new(
                    manager.clone(),
                    runtime.clone(),
                    autoscale_interval,
                ));

                // Register adaptive services with the controller
                for (name, service_spec) in &spec.services {
                    if let zlayer_spec::ScaleSpec::Adaptive { .. } = &service_spec.scale {
                        let replicas =
                            manager.service_replica_count(name).await.unwrap_or(0) as u32;
                        controller
                            .register_service(name, &service_spec.scale, replicas)
                            .await;
                    }
                }

                let adaptive_count = controller.registered_service_count().await;
                info!(
                    count = adaptive_count,
                    interval_secs = autoscale_interval.as_secs(),
                    "Autoscale controller configured"
                );
                emit(
                    &event_tx,
                    DeployEvent::Log {
                        level: LogLevel::Info,
                        message: format!(
                            "Autoscaling enabled for {} service(s) (interval: {}s)",
                            adaptive_count,
                            autoscale_interval.as_secs()
                        ),
                    },
                );

                let controller_clone = controller.clone();
                let autoscale_handle = tokio::spawn(async move {
                    if let Err(e) = controller_clone.run_loop().await {
                        tracing::error!(error = %e, "Autoscale controller failed");
                    }
                });

                Some((controller, autoscale_handle))
            } else {
                None
            };

            emit(
                &event_tx,
                DeployEvent::DeploymentRunning {
                    services: summary_services,
                },
            );

            // Wait for Ctrl+C while emitting periodic StatusTick events
            {
                let mut tick_interval = tokio::time::interval(Duration::from_secs(5));
                // The first tick fires immediately; consume it so the first
                // real tick happens after 5 seconds.
                tick_interval.tick().await;

                loop {
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {
                            break;
                        }
                        _ = tick_interval.tick() => {
                            // Build current service status from deployed_services snapshot
                            let mut statuses = Vec::with_capacity(deployed_services.len());
                            for (name, target) in &deployed_services {
                                let running = manager
                                    .service_replica_count(name)
                                    .await
                                    .unwrap_or(0) as u32;
                                let health = if *target == 0 {
                                    ServiceHealth::Unknown
                                } else if running == *target {
                                    ServiceHealth::Healthy
                                } else if running == 0 {
                                    ServiceHealth::Unhealthy
                                } else {
                                    ServiceHealth::Degraded
                                };
                                statuses.push(ServiceStatus {
                                    name: name.clone(),
                                    replicas_running: running,
                                    replicas_target: *target,
                                    health,
                                });
                            }
                            emit(&event_tx, DeployEvent::StatusTick { services: statuses });
                        }
                    }
                }
            }

            emit(&event_tx, DeployEvent::ShutdownStarted);
            info!("Received shutdown signal");

            // Stop autoscaler if running
            if let Some((controller, autoscale_handle)) = autoscale_state {
                controller.shutdown();
                if let Err(e) = autoscale_handle.await {
                    warn!(error = %e, "Autoscale handle join error");
                }
                info!("Autoscale controller stopped");
            }
        }
    }

    // 11. Cleanup on exit (reverse order)

    // Scale all services to 0
    let service_names = manager.list_services().await;
    for name in &service_names {
        emit(
            &event_tx,
            DeployEvent::ServiceStopping { name: name.clone() },
        );
        if let Err(e) = manager.scale_service(name, 0).await {
            warn!(service = %name, error = %e, "Failed to stop service");
        }
        emit(
            &event_tx,
            DeployEvent::ServiceStopped { name: name.clone() },
        );
    }

    // Stop container supervisor
    if let Some(sup) = &container_supervisor {
        sup.shutdown();
    }
    if let Some(handle) = supervisor_handle {
        let _ = handle.await;
    }

    // Stop API server
    api_shutdown.notify_one();
    if let Some(handle) = api_handle {
        let _ = handle.await;
    }

    // Stop proxy
    if let Some(pm) = &proxy_manager {
        pm.stop().await;
    }

    // Cleanup overlay networks
    if let Some(om) = overlay_manager {
        let mut om_guard = om.write().await;
        if let Err(e) = om_guard.cleanup().await {
            warn!("Failed to cleanup overlay networks: {}", e);
        }
    }

    emit(&event_tx, DeployEvent::ShutdownComplete);
    info!("Shutdown complete");
    Ok(())
}

/// Deploy and start services (like docker compose up)
///
/// If `cli.background` is true, deploys and returns immediately.
/// If `cli.detach` is true, waits for services to stabilize then exits.
/// Otherwise (foreground, the default), deploys and waits for Ctrl+C.
/// Uses the interactive TUI when on a terminal, unless `--no-tui` was passed.
pub(crate) async fn up(cli: &Cli, spec_path: &Path) -> Result<()> {
    let spec = parse_spec(spec_path)?;
    let mode = if cli.background {
        DeployMode::Background
    } else if cli.detach {
        DeployMode::Detach
    } else {
        DeployMode::Foreground
    };

    let use_tui = !cli.no_tui && std::io::IsTerminal::is_terminal(&std::io::stdout());

    if use_tui {
        run_with_tui(cli, &spec, mode).await
    } else {
        let tx = setup_plain_channel();
        deploy_services(cli, &spec, mode, tx).await
    }
}

/// Stop all services in a deployment (like docker compose down)
///
/// Auto-discovers deployment name from .zlayer.yml if not given.
pub(crate) async fn down(deployment: Option<String>) -> Result<()> {
    let deployment_name = match deployment {
        Some(name) => name,
        None => {
            // Try to discover spec and extract deployment name
            let spec_path = discover_spec_path(None)?;
            let spec = parse_spec(&spec_path)?;
            spec.deployment.clone()
        }
    };

    info!(deployment = %deployment_name, "Stopping deployment");

    let tx = setup_plain_channel();
    emit(
        &tx,
        DeployEvent::Log {
            level: LogLevel::Info,
            message: format!("Stopping deployment: {}...", deployment_name),
        },
    );

    // Delegate to the existing stop logic with no specific service, graceful, 30s timeout
    crate::commands::lifecycle::stop(&deployment_name, None, false, 30).await
}
