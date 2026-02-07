//! Deploy command handlers.
//!
//! Contains the `deploy`, `up`, and `down` command implementations along with
//! the shared `deploy_services` orchestration logic and deployment plan display.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, warn};
use zlayer_spec::DeploymentSpec;

use crate::cli::{Cli, DeployMode};
use crate::config::build_runtime_config;
use crate::util::{discover_spec_path, parse_spec};

/// Deploy services from a spec file
pub(crate) async fn deploy(cli: &Cli, spec_path: &Path, dry_run: bool) -> Result<()> {
    let spec = parse_spec(spec_path)?;
    if dry_run {
        info!("Dry run mode - validating only");
        print_deployment_plan(&spec);
        return Ok(());
    }
    deploy_services(cli, &spec, DeployMode::Foreground).await
}

/// Shared deployment logic used by both `deploy` and `up` commands
///
/// Sets up the full infrastructure stack (runtime, overlay, DNS, proxy, supervisor, API)
/// then deploys all services from the spec. In foreground mode, waits for Ctrl+C
/// and performs graceful shutdown. In background mode, returns immediately after deploying.
pub(crate) async fn deploy_services(
    cli: &Cli,
    spec: &DeploymentSpec,
    mode: DeployMode,
) -> Result<()> {
    use std::sync::Arc;
    use std::time::Duration;

    // 1. Runtime
    let runtime_config = build_runtime_config(cli);
    info!(runtime = ?cli.runtime, "Creating container runtime");

    let runtime = zlayer_agent::create_runtime(runtime_config)
        .await
        .context("Failed to create container runtime")?;

    info!("Runtime created successfully");

    // 2. Overlay Manager
    let overlay_manager = match zlayer_agent::OverlayManager::new(spec.deployment.clone()).await {
        Ok(mut om) => {
            if let Err(e) = om.setup_global_overlay().await {
                warn!("Failed to setup global overlay (non-fatal): {}", e);
            } else {
                info!("Global overlay network created");
            }
            Some(Arc::new(tokio::sync::RwLock::new(om)))
        }
        Err(e) => {
            warn!("Overlay networks disabled: {}", e);
            None
        }
    };

    // 3. DNS Server
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
                        info!(addr = %dns_addr, "DNS server started");
                        Some(dns)
                    }
                    Err(e) => {
                        warn!("Failed to start DNS server: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                warn!("Failed to create DNS server: {}", e);
                None
            }
        }
    };

    // 4. Proxy Manager
    let proxy_manager = {
        let config = zlayer_agent::ProxyManagerConfig::default();
        let pm = Arc::new(zlayer_agent::ProxyManager::new(config));
        Some(pm)
    };

    // 5. Container Supervisor
    let container_supervisor = {
        let supervisor = Arc::new(zlayer_agent::ContainerSupervisor::new(runtime.clone()));
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
        let jwt_secret = spec
            .api
            .jwt_secret
            .clone()
            .or_else(|| std::env::var("ZLAYER_JWT_SECRET").ok())
            .unwrap_or_else(|| {
                warn!("Using default JWT secret - NOT SAFE FOR PRODUCTION");
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
        info!(bind = %bind_addr, swagger = spec.api.swagger, "Starting API server");
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

    // 9. Print deployment plan and deploy services
    print_deployment_plan(spec);

    println!("\n=== Deploying Services ===\n");

    // Track deployment results
    let mut deployed_services: Vec<(String, u32)> = Vec::new();
    let mut failed_services: Vec<(String, String)> = Vec::new();

    // Deploy each service
    for (name, service_spec) in &spec.services {
        info!(service = %name, "Deploying service");
        println!("Deploying service: {}", name);

        // Register the service with ServiceManager
        match manager
            .upsert_service(name.clone(), service_spec.clone())
            .await
        {
            Ok(()) => {
                info!(service = %name, "Service registered");

                // Setup service overlay network before scaling
                if let Some(om) = &overlay_manager {
                    let om_guard = om.read().await;
                    match om_guard.setup_service_overlay(name).await {
                        Ok(iface) => {
                            info!(service = %name, interface = %iface, "Service overlay created");
                        }
                        Err(e) => {
                            warn!(service = %name, error = %e, "Failed to setup service overlay (non-fatal)");
                        }
                    }
                }

                // Register service with proxy manager
                if let Some(pm) = &proxy_manager {
                    pm.add_service(name, service_spec).await;
                    if let Err(e) = pm.ensure_ports_for_service(service_spec).await {
                        warn!(service = %name, error = %e, "Failed to setup proxy ports (non-fatal)");
                    }
                }
            }
            Err(e) => {
                let error_msg = format!("Failed to register service: {}", e);
                warn!(service = %name, error = %e, "Failed to register service");
                failed_services.push((name.clone(), error_msg));
                continue;
            }
        }

        // Determine initial replica count from scale spec
        let replicas = match &service_spec.scale {
            zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
            zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min,
            zlayer_spec::ScaleSpec::Manual => 0,
        };

        if replicas > 0 {
            info!(service = %name, replicas = replicas, "Scaling service");
            println!("  Scaling to {} replica(s)...", replicas);

            match manager.scale_service(name, replicas).await {
                Ok(()) => {
                    info!(service = %name, replicas = replicas, "Service scaled successfully");
                    deployed_services.push((name.clone(), replicas));
                }
                Err(e) => {
                    let error_msg = format!("Failed to scale service: {}", e);
                    warn!(service = %name, error = %e, "Failed to scale service");
                    failed_services.push((name.clone(), error_msg));
                }
            }
        } else {
            info!(service = %name, "Service registered with manual scaling (0 replicas)");
            println!("  Registered (manual scaling - 0 replicas)");
            deployed_services.push((name.clone(), 0));
        }
    }

    // Wait for services to stabilize with a timeout
    if !deployed_services.is_empty() {
        println!("\nWaiting for services to stabilize...");
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
                    Ok(_count) => {
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
            println!("Warning: Timeout waiting for all services to reach desired state");
        }
    }

    // Print deployment summary
    println!("\n=== Deployment Summary ===\n");
    println!("Deployment: {}", spec.deployment);
    println!("Version: {}", spec.version);
    println!();

    if !deployed_services.is_empty() {
        println!("Successfully deployed services:");
        for (name, replicas) in &deployed_services {
            let actual_count = manager.service_replica_count(name).await.unwrap_or(0);
            println!(
                "  - {} ({}/{} replicas running)",
                name, actual_count, replicas
            );
        }
    }

    if !failed_services.is_empty() {
        println!("\nFailed services:");
        for (name, error) in &failed_services {
            println!("  - {}: {}", name, error);
        }
    }

    // List all managed services
    let all_services = manager.list_services().await;
    info!(
        services = ?all_services,
        deployed = deployed_services.len(),
        failed = failed_services.len(),
        "Deployment complete"
    );

    if !failed_services.is_empty() {
        anyhow::bail!(
            "Deployment completed with {} failed service(s)",
            failed_services.len()
        )
    }

    // 10. Post-deploy based on mode
    match mode {
        DeployMode::Background => {
            println!("\nDeployment running in background.");
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
                println!("\nAutoscaling enabled for {} service(s)", adaptive_count);
                println!("  Evaluation interval: {}s", autoscale_interval.as_secs());

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

            println!("\nServices running. Press Ctrl+C to stop.");
            tokio::signal::ctrl_c()
                .await
                .context("Failed to wait for Ctrl+C")?;
            println!("\nShutting down...");
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
        if let Err(e) = manager.scale_service(name, 0).await {
            warn!(service = %name, error = %e, "Failed to stop service");
        }
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

    info!("Shutdown complete");
    Ok(())
}

/// Print the deployment plan for a spec
pub(crate) fn print_deployment_plan(spec: &DeploymentSpec) {
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

/// Deploy and start services (like docker compose up)
///
/// If `cli.background` is true, deploys and returns immediately.
/// Otherwise (foreground, the default), deploys and waits for Ctrl+C.
pub(crate) async fn up(cli: &Cli, spec_path: &Path) -> Result<()> {
    let spec = parse_spec(spec_path)?;
    let mode = if cli.background {
        DeployMode::Background
    } else {
        DeployMode::Foreground
    };
    deploy_services(cli, &spec, mode).await
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
    println!("Stopping deployment: {}...", deployment_name);

    // Delegate to the existing stop logic with no specific service, graceful, 30s timeout
    crate::commands::lifecycle::stop(&deployment_name, None, false, 30).await
}
