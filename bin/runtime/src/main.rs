//! ZLayer Runtime CLI
//!
//! The main entry point for the ZLayer container orchestration runtime.
//! Provides commands for deploying, joining, and managing containerized services.
//!
//! # Feature Flags
//!
//! - `full` (default): Enable all commands
//! - `serve`: Enable the API server command
//! - `join`: Enable the join command for worker nodes
//! - `deploy`: Enable the deploy/orchestration commands

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use agent::{RuntimeConfig, YoukiConfig};
use observability::{init_observability, LogFormat, LogLevel, LoggingConfig, ObservabilityConfig};
use spec::DeploymentSpec;

/// ZLayer container orchestration runtime
#[derive(Parser)]
#[command(name = "zlayer")]
#[command(version, about = "ZLayer container orchestration runtime")]
#[command(propagate_version = true)]
struct Cli {
    /// Container runtime to use
    #[arg(long, default_value = "mock", value_enum)]
    runtime: RuntimeType,

    /// State directory for runtime data
    #[arg(long, default_value = "/var/lib/zlayer/containers")]
    state_dir: PathBuf,

    /// Enable verbose logging
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

/// Runtime type selection
#[derive(Debug, Clone, Copy, ValueEnum)]
enum RuntimeType {
    /// Mock runtime for testing and development
    Mock,
    /// Youki runtime for production deployments
    Youki,
}

/// CLI subcommands
#[derive(Subcommand)]
enum Commands {
    /// Deploy services from a spec file
    #[cfg(feature = "deploy")]
    Deploy {
        /// Path to the deployment spec YAML file
        spec_path: PathBuf,

        /// Dry run - parse and validate but don't actually deploy
        #[arg(long)]
        dry_run: bool,
    },

    /// Join an existing deployment
    #[cfg(feature = "join")]
    Join {
        /// Join token (contains deployment key and service info)
        token: String,

        /// Override the spec directory
        #[arg(long)]
        spec_dir: Option<String>,

        /// Service to run (if token doesn't specify)
        #[arg(short, long)]
        service: Option<String>,

        /// Number of replicas to run on this node
        #[arg(short, long, default_value = "1")]
        replicas: u32,
    },

    /// Start the API server
    #[cfg(feature = "serve")]
    Serve {
        /// Bind address (e.g., 0.0.0.0:8080)
        #[arg(long, default_value = "0.0.0.0:8080")]
        bind: String,

        /// JWT secret for authentication (can also be set via ZLAYER_JWT_SECRET env var)
        #[arg(long, env = "ZLAYER_JWT_SECRET")]
        jwt_secret: Option<String>,

        /// Disable Swagger UI
        #[arg(long)]
        no_swagger: bool,
    },

    /// Show runtime status
    Status,

    /// Validate a spec file without deploying
    Validate {
        /// Path to the deployment spec YAML file
        spec_path: PathBuf,
    },

    /// Stream logs from a service
    #[cfg(feature = "deploy")]
    Logs {
        /// Deployment name
        #[arg(short, long)]
        deployment: String,

        /// Service name
        service: String,

        /// Number of lines to show
        #[arg(short = 'n', long, default_value = "100")]
        lines: u32,

        /// Follow log output
        #[arg(short, long)]
        follow: bool,

        /// Filter by instance ID
        #[arg(short, long)]
        instance: Option<String>,
    },

    /// Stop a deployment or service
    #[cfg(feature = "deploy")]
    Stop {
        /// Deployment name
        deployment: String,

        /// Service name (optional, stops all if not specified)
        #[arg(short, long)]
        service: Option<String>,

        /// Force immediate shutdown (no graceful period)
        #[arg(short, long)]
        force: bool,

        /// Timeout for graceful shutdown in seconds
        #[arg(short, long, default_value = "30")]
        timeout: u64,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Configure observability based on verbosity and environment
    let log_level = match cli.verbose {
        0 => LogLevel::Info,
        1 => LogLevel::Debug,
        _ => LogLevel::Trace,
    };

    // Use pretty format for terminals, JSON for piped output
    let log_format = if std::io::stdout().is_terminal() {
        LogFormat::Pretty
    } else {
        LogFormat::Json
    };

    let config = ObservabilityConfig {
        logging: LoggingConfig {
            level: log_level,
            format: log_format,
            ..Default::default()
        },
        ..Default::default()
    };

    // Initialize observability - hold guards for application lifetime
    let _guards = init_observability(&config).context("Failed to initialize observability")?;

    // Run the async runtime
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to create tokio runtime")?
        .block_on(run(cli))
}

async fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        #[cfg(feature = "deploy")]
        Commands::Deploy { spec_path, dry_run } => deploy(&cli, spec_path, *dry_run).await,
        #[cfg(feature = "join")]
        Commands::Join {
            token,
            spec_dir,
            service,
            replicas,
        } => {
            join(
                &cli,
                token,
                spec_dir.as_deref(),
                service.as_deref(),
                *replicas,
            )
            .await
        }
        #[cfg(feature = "serve")]
        Commands::Serve {
            bind,
            jwt_secret,
            no_swagger,
        } => serve(bind, jwt_secret.clone(), *no_swagger).await,
        Commands::Status => status(&cli).await,
        Commands::Validate { spec_path } => validate(spec_path).await,
        #[cfg(feature = "deploy")]
        Commands::Logs {
            deployment,
            service,
            lines,
            follow,
            instance,
        } => logs(deployment, service, *lines, *follow, instance.clone()).await,
        #[cfg(feature = "deploy")]
        Commands::Stop {
            deployment,
            service,
            force,
            timeout,
        } => stop(deployment, service.clone(), *force, *timeout).await,
    }
}

/// Build runtime configuration from CLI arguments
#[cfg(any(feature = "deploy", feature = "join"))]
fn build_runtime_config(cli: &Cli) -> RuntimeConfig {
    match cli.runtime {
        RuntimeType::Mock => RuntimeConfig::Mock,
        RuntimeType::Youki => RuntimeConfig::Youki(YoukiConfig {
            state_dir: cli.state_dir.clone(),
            ..Default::default()
        }),
    }
}

/// Parse and validate a deployment spec file
fn parse_spec(spec_path: &Path) -> Result<DeploymentSpec> {
    info!(path = %spec_path.display(), "Parsing deployment spec");

    let spec = spec::from_yaml_file(spec_path)
        .with_context(|| format!("Failed to parse spec file: {}", spec_path.display()))?;

    info!(
        deployment = %spec.deployment,
        version = %spec.version,
        services = spec.services.len(),
        "Spec parsed successfully"
    );

    Ok(spec)
}

/// Deploy services from a spec file
#[cfg(feature = "deploy")]
async fn deploy(cli: &Cli, spec_path: &Path, dry_run: bool) -> Result<()> {
    use std::sync::Arc;
    use std::time::Duration;

    let spec = parse_spec(spec_path)?;

    if dry_run {
        info!("Dry run mode - validating only");
        print_deployment_plan(&spec);
        return Ok(());
    }

    // Build runtime configuration
    let runtime_config = build_runtime_config(cli);
    info!(runtime = ?cli.runtime, "Creating container runtime");

    // Create the runtime
    let runtime = agent::create_runtime(runtime_config)
        .await
        .context("Failed to create container runtime")?;

    info!("Runtime created successfully");

    // Print deployment plan
    print_deployment_plan(&spec);

    // Create ServiceManager (wrap in Arc for autoscaler)
    let manager = Arc::new(agent::ServiceManager::new(runtime.clone()));

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
            spec::ScaleSpec::Fixed { replicas } => *replicas,
            spec::ScaleSpec::Adaptive { min, .. } => *min,
            spec::ScaleSpec::Manual => 0,
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

    // Check if any service uses adaptive scaling
    let has_adaptive = agent::has_adaptive_scaling(&spec.services);

    if has_adaptive {
        // Create autoscale controller
        let autoscale_interval = Duration::from_secs(10);
        let controller = Arc::new(agent::AutoscaleController::new(
            manager.clone(),
            runtime.clone(),
            autoscale_interval,
        ));

        // Register adaptive services with the controller
        for (name, service_spec) in &spec.services {
            if let spec::ScaleSpec::Adaptive { .. } = &service_spec.scale {
                let replicas = manager.service_replica_count(name).await.unwrap_or(0) as u32;
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

        // Spawn autoscale loop
        let controller_clone = controller.clone();
        let autoscale_handle = tokio::spawn(async move {
            if let Err(e) = controller_clone.run_loop().await {
                tracing::error!(error = %e, "Autoscale controller failed");
            }
        });

        // Wait for Ctrl+C
        println!("\nDeployment running with autoscaling. Press Ctrl+C to stop.");
        tokio::signal::ctrl_c()
            .await
            .context("Failed to wait for Ctrl+C")?;

        println!("\nShutting down...");
        info!("Received shutdown signal");

        // Shutdown autoscaler
        controller.shutdown();
        if let Err(e) = autoscale_handle.await {
            warn!(error = %e, "Autoscale handle join error");
        }

        info!("Autoscale controller stopped");
    } else {
        println!("\nDeployment completed successfully!");
        println!("No adaptive scaling configured - deployment is static.");
    }

    Ok(())
}

/// Print the deployment plan for a spec
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
            spec::ScaleSpec::Fixed { replicas } => {
                println!("    Scale: fixed ({} replicas)", replicas);
            }
            spec::ScaleSpec::Adaptive { min, max, .. } => {
                println!("    Scale: adaptive ({}-{} replicas)", min, max);
            }
            spec::ScaleSpec::Manual => {
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

/// Join token information
#[cfg(feature = "join")]
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct JoinToken {
    /// API endpoint to contact
    api_endpoint: String,
    /// Deployment name
    deployment: String,
    /// Authentication key for the API
    key: String,
    /// Optional service name (if token is service-specific)
    #[serde(default)]
    service: Option<String>,
}

/// Parse a join token
#[cfg(feature = "join")]
fn parse_join_token(token: &str) -> Result<JoinToken> {
    use base64::Engine;

    // Try to decode as base64 (try URL-safe first, then standard)
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(token))
        .context("Invalid join token: not valid base64")?;

    // Parse as JSON
    let join_token: JoinToken =
        serde_json::from_slice(&decoded).context("Invalid join token: not valid JSON")?;

    Ok(join_token)
}

/// Join an existing deployment
#[cfg(feature = "join")]
async fn join(
    cli: &Cli,
    token: &str,
    _spec_dir: Option<&str>,
    service: Option<&str>,
    replicas: u32,
) -> Result<()> {
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::RwLock;

    info!(
        token_len = token.len(),
        service = ?service,
        replicas = replicas,
        "Joining deployment"
    );

    // Step 1: Parse join token
    let join_token = parse_join_token(token)?;

    info!(
        api_endpoint = %join_token.api_endpoint,
        deployment = %join_token.deployment,
        "Joining deployment"
    );

    println!("Joining deployment: {}", join_token.deployment);
    println!("API endpoint: {}", join_token.api_endpoint);

    // Step 2: Authenticate with API
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    info!("Authenticating with API...");
    let auth_response = client
        .post(format!("{}/api/v1/auth/verify", join_token.api_endpoint))
        .bearer_auth(&join_token.key)
        .send()
        .await
        .context("Failed to authenticate with API")?;

    if !auth_response.status().is_success() {
        let status = auth_response.status();
        let body = auth_response.text().await.unwrap_or_default();
        anyhow::bail!("Authentication failed: {} - {}", status, body);
    }
    info!("Authentication successful");
    println!("Authentication successful");

    // Step 3: Fetch deployment spec from API
    info!("Fetching deployment spec from API...");
    let spec_response = client
        .get(format!(
            "{}/api/v1/deployments/{}/spec",
            join_token.api_endpoint, join_token.deployment
        ))
        .bearer_auth(&join_token.key)
        .send()
        .await
        .context("Failed to fetch deployment spec")?;

    if !spec_response.status().is_success() {
        let status = spec_response.status();
        let body = spec_response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to fetch deployment spec: {} - {}", status, body);
    }

    let spec: DeploymentSpec = spec_response
        .json()
        .await
        .context("Failed to parse deployment spec")?;

    info!(
        deployment = %spec.deployment,
        services = spec.services.len(),
        "Fetched deployment spec"
    );
    println!("Fetched deployment spec: {} services", spec.services.len());

    // Step 4: Determine which service(s) to join
    let target_service = service.or(join_token.service.as_deref());
    let services_to_join: Vec<(String, spec::ServiceSpec)> = if let Some(svc) = target_service {
        // Join specific service
        if !spec.services.contains_key(svc) {
            anyhow::bail!("Service '{}' not found in deployment", svc);
        }
        vec![(svc.to_string(), spec.services.get(svc).unwrap().clone())]
    } else {
        // Join all services (for global join)
        spec.services
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };

    println!("\nServices to join:");
    for (name, svc_spec) in &services_to_join {
        println!("  - {} (image: {})", name, svc_spec.image.name);
    }

    // Step 5: Build runtime
    let runtime_config = build_runtime_config(cli);
    info!(runtime = ?cli.runtime, "Creating container runtime");

    let runtime = agent::create_runtime(runtime_config)
        .await
        .context("Failed to create container runtime")?;
    info!("Runtime created successfully");

    // Step 6: Setup overlay networks
    let overlay_manager = match agent::OverlayManager::new(spec.deployment.clone()).await {
        Ok(mut om) => {
            // Setup global overlay
            if let Err(e) = om.setup_global_overlay().await {
                warn!("Failed to setup global overlay (non-fatal): {}", e);
                println!("Warning: Overlay network setup failed: {}", e);
            } else {
                info!("Global overlay network created");
                println!("Global overlay network created");
            }
            Some(Arc::new(RwLock::new(om)))
        }
        Err(e) => {
            warn!("Overlay networks disabled: {}", e);
            println!("Warning: Overlay networks disabled: {}", e);
            None
        }
    };

    // Step 7: Create ServiceManager with overlay support
    let manager = if let Some(om) = overlay_manager.clone() {
        agent::ServiceManager::with_overlay(runtime.clone(), om)
    } else {
        agent::ServiceManager::new(runtime.clone())
    };

    println!("\n=== Starting Services ===\n");

    // Step 8: For each service, pull image, run init, register and scale
    for (service_name, service_spec) in services_to_join {
        info!(service = %service_name, "Joining service");
        println!("Joining service: {}", service_name);

        // Pull image
        println!("  Pulling image: {}...", service_spec.image.name);
        runtime
            .pull_image(&service_spec.image.name)
            .await
            .context(format!(
                "Failed to pull image for service '{}'",
                service_name
            ))?;
        info!(service = %service_name, image = %service_spec.image.name, "Image pulled");
        println!("  Image pulled successfully");

        // Run init steps (if any)
        if !service_spec.init.steps.is_empty() {
            println!(
                "  Running {} init step(s)...",
                service_spec.init.steps.len()
            );
            for step in &service_spec.init.steps {
                info!(service = %service_name, step = %step.id, "Running init step");
                println!("    Step: {}", step.id);

                let action =
                    init_actions::from_spec(&step.uses, &step.with, Duration::from_secs(300))
                        .context(format!("Invalid init action: {}", step.uses))?;

                action
                    .execute()
                    .await
                    .context(format!("Init step '{}' failed", step.id))?;

                info!(service = %service_name, step = %step.id, "Init step completed");
            }
        }

        // Register service
        manager
            .upsert_service(service_name.clone(), service_spec.clone())
            .await
            .context(format!("Failed to register service '{}'", service_name))?;
        info!(service = %service_name, "Service registered");

        // Determine replica count
        let target_replicas = if replicas > 0 {
            replicas
        } else {
            match &service_spec.scale {
                spec::ScaleSpec::Fixed { replicas } => *replicas,
                spec::ScaleSpec::Adaptive { min, .. } => *min,
                spec::ScaleSpec::Manual => 1, // Join implies at least 1 replica
            }
        };

        // Scale service
        println!("  Scaling to {} replica(s)...", target_replicas);
        manager
            .scale_service(&service_name, target_replicas)
            .await
            .context(format!("Failed to scale service '{}'", service_name))?;

        info!(
            service = %service_name,
            replicas = target_replicas,
            "Service joined"
        );
        println!(
            "  Service '{}' joined with {} replica(s)",
            service_name, target_replicas
        );
    }

    // Step 9: Wait for Ctrl+C
    println!("\n=== Join Complete ===");
    println!("Services are running. Press Ctrl+C to leave the deployment.");

    tokio::signal::ctrl_c()
        .await
        .context("Failed to wait for Ctrl+C")?;

    println!("\nShutting down...");
    info!("Received shutdown signal, cleaning up");

    // Step 10: Cleanup overlay networks on exit
    if let Some(om) = overlay_manager {
        info!("Cleaning up overlay networks");
        let mut om_guard = om.write().await;
        if let Err(e) = om_guard.cleanup().await {
            warn!("Failed to cleanup overlay networks: {}", e);
        } else {
            info!("Overlay networks cleaned up");
        }
    }

    println!("Goodbye!");
    Ok(())
}

/// Show runtime status
async fn status(cli: &Cli) -> Result<()> {
    info!("Checking runtime status");

    println!("\n=== ZLayer Runtime Status ===");
    println!("Runtime: {:?}", cli.runtime);

    match cli.runtime {
        RuntimeType::Mock => {
            println!("Status: Ready (mock mode)");
            println!("Note: Using mock runtime - no actual containers will be created");
        }
        RuntimeType::Youki => {
            println!("State Dir: {}", cli.state_dir.display());

            // Try to create the youki runtime to verify it's available
            let config = YoukiConfig {
                state_dir: cli.state_dir.clone(),
                ..Default::default()
            };

            match agent::create_runtime(RuntimeConfig::Youki(config)).await {
                Ok(_) => {
                    println!("Status: Youki runtime ready");
                }
                Err(e) => {
                    println!("Status: Youki runtime unavailable");
                    println!("Error: {}", e);
                    warn!("Consider using --runtime mock for testing");
                }
            }
        }
    }

    println!();

    Ok(())
}

/// Validate a spec file without deploying
async fn validate(spec_path: &Path) -> Result<()> {
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

/// Start the API server
#[cfg(feature = "serve")]
async fn serve(bind: &str, jwt_secret: Option<String>, no_swagger: bool) -> Result<()> {
    let jwt_secret = jwt_secret.unwrap_or_else(|| {
        warn!("Using default JWT secret - NOT SAFE FOR PRODUCTION");
        "CHANGE_ME_IN_PRODUCTION".to_string()
    });

    let bind_addr = bind
        .parse()
        .context(format!("Invalid bind address: {}", bind))?;

    let config = api::ApiConfig {
        bind: bind_addr,
        jwt_secret,
        swagger_enabled: !no_swagger,
        ..Default::default()
    };

    info!(
        bind = %config.bind,
        swagger = config.swagger_enabled,
        "Starting ZLayer API server"
    );

    let server = api::ApiServer::new(config);

    // Setup graceful shutdown on SIGTERM/SIGINT
    let shutdown = async {
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };

        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => {},
            _ = terminate => {},
        }

        info!("Shutdown signal received, starting graceful shutdown");
    };

    server.run_with_shutdown(shutdown).await?;

    info!("Server shutdown complete");
    Ok(())
}

/// Stream logs from a service
#[cfg(feature = "deploy")]
async fn logs(
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
#[cfg(feature = "deploy")]
async fn stop(deployment: &str, service: Option<String>, force: bool, timeout: u64) -> Result<()> {
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

    // TODO: Implement actual stop logic
    // 1. Send SIGTERM to containers
    // 2. Wait for graceful shutdown
    // 3. Send SIGKILL if timeout exceeded (and not force)
    println!("[Stop command not yet implemented]");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parsing() {
        // Test default runtime
        let cli = Cli::try_parse_from(["zlayer", "status"]).unwrap();
        assert!(matches!(cli.runtime, RuntimeType::Mock));
        assert!(matches!(cli.command, Commands::Status));
    }

    #[test]
    fn test_cli_youki_runtime() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "--runtime",
            "youki",
            "--state-dir",
            "/custom/state",
            "status",
        ])
        .unwrap();

        assert!(matches!(cli.runtime, RuntimeType::Youki));
        assert_eq!(cli.state_dir, PathBuf::from("/custom/state"));
    }

    #[test]
    #[cfg(feature = "deploy")]
    fn test_cli_deploy_command() {
        let cli = Cli::try_parse_from(["zlayer", "deploy", "test-spec.yaml"]).unwrap();

        match cli.command {
            Commands::Deploy { spec_path, dry_run } => {
                assert_eq!(spec_path, PathBuf::from("test-spec.yaml"));
                assert!(!dry_run);
            }
            _ => panic!("Expected Deploy command"),
        }
    }

    #[test]
    #[cfg(feature = "deploy")]
    fn test_cli_deploy_dry_run() {
        let cli = Cli::try_parse_from(["zlayer", "deploy", "--dry-run", "test-spec.yaml"]).unwrap();

        match cli.command {
            Commands::Deploy { dry_run, .. } => {
                assert!(dry_run);
            }
            _ => panic!("Expected Deploy command"),
        }
    }

    #[test]
    #[cfg(feature = "join")]
    fn test_cli_join_command() {
        let cli = Cli::try_parse_from(["zlayer", "join", "some-token"]).unwrap();

        match cli.command {
            Commands::Join {
                token,
                spec_dir,
                service,
                replicas,
            } => {
                assert_eq!(token, "some-token");
                assert!(spec_dir.is_none());
                assert!(service.is_none());
                assert_eq!(replicas, 1); // default
            }
            _ => panic!("Expected Join command"),
        }
    }

    #[test]
    #[cfg(feature = "join")]
    fn test_cli_join_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "join",
            "--spec-dir",
            "/custom/specs",
            "--service",
            "web",
            "--replicas",
            "3",
            "my-join-token",
        ])
        .unwrap();

        match cli.command {
            Commands::Join {
                token,
                spec_dir,
                service,
                replicas,
            } => {
                assert_eq!(token, "my-join-token");
                assert_eq!(spec_dir, Some("/custom/specs".to_string()));
                assert_eq!(service, Some("web".to_string()));
                assert_eq!(replicas, 3);
            }
            _ => panic!("Expected Join command"),
        }
    }

    #[test]
    #[cfg(feature = "join")]
    fn test_cli_join_command_short_flags() {
        let cli =
            Cli::try_parse_from(["zlayer", "join", "-s", "api", "-r", "5", "token123"]).unwrap();

        match cli.command {
            Commands::Join {
                token,
                service,
                replicas,
                ..
            } => {
                assert_eq!(token, "token123");
                assert_eq!(service, Some("api".to_string()));
                assert_eq!(replicas, 5);
            }
            _ => panic!("Expected Join command"),
        }
    }

    #[test]
    #[cfg(feature = "join")]
    fn test_parse_join_token() {
        use base64::Engine;

        let info = serde_json::json!({
            "api_endpoint": "http://localhost:8080",
            "deployment": "my-app",
            "key": "secret-auth-key",
            "service": "api"
        });

        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&info).unwrap());

        let parsed = super::parse_join_token(&token).unwrap();
        assert_eq!(parsed.api_endpoint, "http://localhost:8080");
        assert_eq!(parsed.deployment, "my-app");
        assert_eq!(parsed.key, "secret-auth-key");
        assert_eq!(parsed.service, Some("api".to_string()));
    }

    #[test]
    #[cfg(feature = "join")]
    fn test_parse_join_token_minimal() {
        use base64::Engine;

        let info = serde_json::json!({
            "api_endpoint": "http://api.example.com",
            "deployment": "my-deploy",
            "key": "auth-key"
        });

        let token =
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_string(&info).unwrap());

        let parsed = super::parse_join_token(&token).unwrap();
        assert_eq!(parsed.api_endpoint, "http://api.example.com");
        assert_eq!(parsed.deployment, "my-deploy");
        assert_eq!(parsed.key, "auth-key");
        assert!(parsed.service.is_none());
    }

    #[test]
    #[cfg(feature = "join")]
    fn test_parse_join_token_invalid_base64() {
        let result = super::parse_join_token("not-valid-base64!!!");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not valid base64"));
    }

    #[test]
    #[cfg(feature = "join")]
    fn test_parse_join_token_invalid_json() {
        use base64::Engine;

        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("not json");

        let result = super::parse_join_token(&token);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not valid JSON"));
    }

    #[test]
    fn test_cli_validate_command() {
        let cli = Cli::try_parse_from(["zlayer", "validate", "test-spec.yaml"]).unwrap();

        match cli.command {
            Commands::Validate { spec_path } => {
                assert_eq!(spec_path, PathBuf::from("test-spec.yaml"));
            }
            _ => panic!("Expected Validate command"),
        }
    }

    #[test]
    fn test_cli_verbose_levels() {
        let cli = Cli::try_parse_from(["zlayer", "status"]).unwrap();
        assert_eq!(cli.verbose, 0);

        let cli = Cli::try_parse_from(["zlayer", "-v", "status"]).unwrap();
        assert_eq!(cli.verbose, 1);

        let cli = Cli::try_parse_from(["zlayer", "-vv", "status"]).unwrap();
        assert_eq!(cli.verbose, 2);
    }

    #[test]
    #[cfg(feature = "deploy")]
    fn test_build_runtime_config_mock() {
        let cli = Cli::try_parse_from(["zlayer", "status"]).unwrap();
        let config = build_runtime_config(&cli);
        assert!(matches!(config, RuntimeConfig::Mock));
    }

    #[test]
    #[cfg(feature = "deploy")]
    fn test_build_runtime_config_youki() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "--runtime",
            "youki",
            "--state-dir",
            "/custom/state",
            "status",
        ])
        .unwrap();

        let config = build_runtime_config(&cli);
        match config {
            RuntimeConfig::Youki(c) => {
                assert_eq!(c.state_dir, PathBuf::from("/custom/state"));
            }
            _ => panic!("Expected Youki config"),
        }
    }

    #[test]
    #[cfg(feature = "serve")]
    fn test_cli_serve_command_defaults() {
        let cli = Cli::try_parse_from(["zlayer", "serve"]).unwrap();

        match cli.command {
            Commands::Serve {
                bind,
                jwt_secret,
                no_swagger,
            } => {
                assert_eq!(bind, "0.0.0.0:8080");
                assert!(jwt_secret.is_none());
                assert!(!no_swagger);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    #[cfg(feature = "serve")]
    fn test_cli_serve_command_custom_bind() {
        let cli = Cli::try_parse_from(["zlayer", "serve", "--bind", "127.0.0.1:9090"]).unwrap();

        match cli.command {
            Commands::Serve { bind, .. } => {
                assert_eq!(bind, "127.0.0.1:9090");
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    #[cfg(feature = "serve")]
    fn test_cli_serve_command_jwt_secret() {
        let cli = Cli::try_parse_from(["zlayer", "serve", "--jwt-secret", "my-super-secret-key"])
            .unwrap();

        match cli.command {
            Commands::Serve { jwt_secret, .. } => {
                assert_eq!(jwt_secret, Some("my-super-secret-key".to_string()));
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    #[cfg(feature = "serve")]
    fn test_cli_serve_command_no_swagger() {
        let cli = Cli::try_parse_from(["zlayer", "serve", "--no-swagger"]).unwrap();

        match cli.command {
            Commands::Serve { no_swagger, .. } => {
                assert!(no_swagger);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    #[cfg(feature = "serve")]
    fn test_cli_serve_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "serve",
            "--bind",
            "0.0.0.0:3000",
            "--jwt-secret",
            "test-secret",
            "--no-swagger",
        ])
        .unwrap();

        match cli.command {
            Commands::Serve {
                bind,
                jwt_secret,
                no_swagger,
            } => {
                assert_eq!(bind, "0.0.0.0:3000");
                assert_eq!(jwt_secret, Some("test-secret".to_string()));
                assert!(no_swagger);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    #[cfg(feature = "deploy")]
    fn test_cli_logs_command_minimal() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "logs",
            "--deployment",
            "my-deployment",
            "my-service",
        ])
        .unwrap();

        match cli.command {
            Commands::Logs {
                deployment,
                service,
                lines,
                follow,
                instance,
            } => {
                assert_eq!(deployment, "my-deployment");
                assert_eq!(service, "my-service");
                assert_eq!(lines, 100); // default
                assert!(!follow);
                assert!(instance.is_none());
            }
            _ => panic!("Expected Logs command"),
        }
    }

    #[test]
    #[cfg(feature = "deploy")]
    fn test_cli_logs_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "logs",
            "--deployment",
            "prod",
            "--lines",
            "50",
            "--follow",
            "--instance",
            "abc123",
            "web-server",
        ])
        .unwrap();

        match cli.command {
            Commands::Logs {
                deployment,
                service,
                lines,
                follow,
                instance,
            } => {
                assert_eq!(deployment, "prod");
                assert_eq!(service, "web-server");
                assert_eq!(lines, 50);
                assert!(follow);
                assert_eq!(instance, Some("abc123".to_string()));
            }
            _ => panic!("Expected Logs command"),
        }
    }

    #[test]
    #[cfg(feature = "deploy")]
    fn test_cli_logs_command_short_flags() {
        let cli = Cli::try_parse_from([
            "zlayer", "logs", "-d", "staging", "-n", "25", "-f", "-i", "inst-456", "api",
        ])
        .unwrap();

        match cli.command {
            Commands::Logs {
                deployment,
                service,
                lines,
                follow,
                instance,
            } => {
                assert_eq!(deployment, "staging");
                assert_eq!(service, "api");
                assert_eq!(lines, 25);
                assert!(follow);
                assert_eq!(instance, Some("inst-456".to_string()));
            }
            _ => panic!("Expected Logs command"),
        }
    }

    #[test]
    #[cfg(feature = "deploy")]
    fn test_cli_stop_command_minimal() {
        let cli = Cli::try_parse_from(["zlayer", "stop", "my-deployment"]).unwrap();

        match cli.command {
            Commands::Stop {
                deployment,
                service,
                force,
                timeout,
            } => {
                assert_eq!(deployment, "my-deployment");
                assert!(service.is_none());
                assert!(!force);
                assert_eq!(timeout, 30); // default
            }
            _ => panic!("Expected Stop command"),
        }
    }

    #[test]
    #[cfg(feature = "deploy")]
    fn test_cli_stop_command_with_service() {
        let cli =
            Cli::try_parse_from(["zlayer", "stop", "--service", "web", "my-deployment"]).unwrap();

        match cli.command {
            Commands::Stop {
                deployment,
                service,
                ..
            } => {
                assert_eq!(deployment, "my-deployment");
                assert_eq!(service, Some("web".to_string()));
            }
            _ => panic!("Expected Stop command"),
        }
    }

    #[test]
    #[cfg(feature = "deploy")]
    fn test_cli_stop_command_force() {
        let cli = Cli::try_parse_from(["zlayer", "stop", "--force", "my-deployment"]).unwrap();

        match cli.command {
            Commands::Stop { force, .. } => {
                assert!(force);
            }
            _ => panic!("Expected Stop command"),
        }
    }

    #[test]
    #[cfg(feature = "deploy")]
    fn test_cli_stop_command_timeout() {
        let cli =
            Cli::try_parse_from(["zlayer", "stop", "--timeout", "60", "my-deployment"]).unwrap();

        match cli.command {
            Commands::Stop { timeout, .. } => {
                assert_eq!(timeout, 60);
            }
            _ => panic!("Expected Stop command"),
        }
    }

    #[test]
    #[cfg(feature = "deploy")]
    fn test_cli_stop_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "stop",
            "-s",
            "database",
            "-f",
            "-t",
            "15",
            "production",
        ])
        .unwrap();

        match cli.command {
            Commands::Stop {
                deployment,
                service,
                force,
                timeout,
            } => {
                assert_eq!(deployment, "production");
                assert_eq!(service, Some("database".to_string()));
                assert!(force);
                assert_eq!(timeout, 15);
            }
            _ => panic!("Expected Stop command"),
        }
    }
}
