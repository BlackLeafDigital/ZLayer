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
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use tracing::{info, warn};

use agent::{RuntimeConfig, YoukiConfig};
use observability::{init_observability, LogFormat, LogLevel, LoggingConfig, ObservabilityConfig};
use spec::DeploymentSpec;

// Import API crate functions for token management
use api::create_token;

#[cfg(feature = "node")]
use serde::{Deserialize, Serialize};

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

    /// Build a container image from a Dockerfile
    ///
    /// Examples:
    ///   zlayer build .
    ///   zlayer build -t myapp:latest .
    ///   zlayer build --runtime node20 -t myapp:v1 ./my-node-app
    ///   zlayer build --runtime-auto -t myapp:latest .
    ///   zlayer build -f Dockerfile.prod --target production -t myapp:prod .
    #[command(verbatim_doc_comment)]
    Build {
        /// Build context directory
        #[arg(default_value = ".")]
        context: PathBuf,

        /// Dockerfile path (default: Dockerfile in context)
        #[arg(short = 'f', long)]
        file: Option<PathBuf>,

        /// Image tag (can be specified multiple times)
        #[arg(short = 't', long = "tag")]
        tags: Vec<String>,

        /// Use runtime template instead of Dockerfile
        #[arg(long, value_name = "RUNTIME")]
        runtime: Option<String>,

        /// Auto-detect runtime from project files
        #[arg(long)]
        runtime_auto: bool,

        /// Build argument (KEY=VALUE, can be specified multiple times)
        #[arg(long = "build-arg")]
        build_args: Vec<String>,

        /// Target stage for multi-stage builds
        #[arg(long)]
        target: Option<String>,

        /// Disable layer caching
        #[arg(long)]
        no_cache: bool,

        /// Push to registry after build
        #[arg(long)]
        push: bool,

        /// Disable TUI (plain output for CI)
        #[arg(long)]
        no_tui: bool,

        /// Verbose output (show all build output)
        #[arg(long)]
        verbose_build: bool,
    },

    /// List available runtime templates
    ///
    /// Shows all pre-built Dockerfile templates for common development
    /// environments. These can be used with `zlayer build --runtime <name>`.
    Runtimes,

    /// Token management commands
    ///
    /// Create, decode, and inspect JWT tokens for API authentication.
    #[command(subcommand)]
    Token(TokenCommands),

    /// Specification inspection commands
    ///
    /// Validate, dump, and inspect deployment specifications.
    #[command(subcommand)]
    Spec(SpecCommands),

    /// Manage cluster nodes
    #[cfg(feature = "node")]
    #[command(subcommand)]
    Node(NodeCommands),
}

/// Node management subcommands
#[cfg(feature = "node")]
#[derive(Subcommand)]
enum NodeCommands {
    /// Initialize this node as cluster leader
    ///
    /// This starts the control plane, overlay network, and API server.
    /// Other nodes can join using the token printed on success.
    ///
    /// Examples:
    ///   zlayer node init --advertise-addr 10.0.0.1
    ///   zlayer node init --advertise-addr $(curl -s ifconfig.me) --api-port 9090
    #[command(verbatim_doc_comment)]
    Init {
        /// Public IP address for other nodes to connect to
        #[arg(long, required = true)]
        advertise_addr: String,

        /// API server port
        #[arg(long, default_value = "8080")]
        api_port: u16,

        /// Raft consensus port
        #[arg(long, default_value = "9000")]
        raft_port: u16,

        /// WireGuard overlay port
        #[arg(long, default_value = "51820")]
        overlay_port: u16,

        /// Data directory
        #[arg(long, default_value = "/var/lib/zlayer")]
        data_dir: PathBuf,

        /// Overlay network CIDR
        #[arg(long, default_value = "10.200.0.0/16")]
        overlay_cidr: String,
    },

    /// Join an existing cluster as a worker node
    ///
    /// Examples:
    ///   zlayer node join 10.0.0.1:8080 --token <TOKEN> --advertise-addr 10.0.0.2
    #[command(verbatim_doc_comment)]
    Join {
        /// Leader address (host:port)
        leader_addr: String,

        /// Join token from leader
        #[arg(long, required = true)]
        token: String,

        /// This node's public IP address
        #[arg(long, required = true)]
        advertise_addr: String,

        /// Node mode: full (all resources) or replicate (specific services)
        #[arg(long, default_value = "full")]
        mode: String,

        /// Services to replicate (only with --mode replicate)
        #[arg(long)]
        services: Option<Vec<String>>,
    },

    /// List all nodes in the cluster
    List {
        /// Output format (table or json)
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Show detailed status of a node
    Status {
        /// Node ID (default: this node)
        node_id: Option<String>,
    },

    /// Remove a node from the cluster
    Remove {
        /// Node ID to remove
        node_id: String,

        /// Force removal without migrating services
        #[arg(long)]
        force: bool,
    },

    /// Set node resource mode
    SetMode {
        /// Node ID
        node_id: String,

        /// Mode: full, dedicated, or replicate
        #[arg(long)]
        mode: String,

        /// Services for dedicated/replicate mode
        #[arg(long)]
        services: Option<Vec<String>>,
    },

    /// Add label to a node
    Label {
        /// Node ID
        node_id: String,

        /// Label in key=value format
        label: String,
    },

    /// Generate a join token for worker nodes
    ///
    /// Creates a base64-encoded token that workers can use to join this cluster.
    ///
    /// Examples:
    ///   zlayer node generate-join-token -d my-deploy -a http://10.0.0.1:8080
    #[command(verbatim_doc_comment)]
    GenerateJoinToken {
        /// Deployment name/key
        #[arg(short, long)]
        deployment: String,

        /// API endpoint URL
        #[arg(short, long)]
        api: String,

        /// Service name (optional)
        #[arg(short, long)]
        service: Option<String>,
    },
}

/// Token management subcommands
#[derive(Subcommand)]
enum TokenCommands {
    /// Create a new JWT token
    Create {
        /// Subject (user ID or API key name)
        #[arg(short, long, default_value = "dev")]
        subject: String,

        /// JWT secret (or use ZLAYER_JWT_SECRET env var)
        #[arg(long)]
        secret: Option<String>,

        /// Token validity in hours
        #[arg(short = 'H', long, default_value = "24")]
        hours: u64,

        /// Roles to grant (comma-separated)
        #[arg(short, long, default_value = "admin")]
        roles: String,

        /// Output only the token (for scripting)
        #[arg(long)]
        quiet: bool,
    },

    /// Decode and display a JWT token
    Decode {
        /// The JWT token to decode
        token: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// List token capabilities and available roles
    Info,
}

/// Spec inspection subcommands
#[derive(Subcommand)]
enum SpecCommands {
    /// Dump the parsed specification
    Dump {
        /// Path to spec file
        spec: PathBuf,

        /// Output format (json, yaml)
        #[arg(short, long, default_value = "yaml")]
        format: String,
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
        Commands::Build {
            context,
            file,
            tags,
            runtime,
            runtime_auto,
            build_args,
            target,
            no_cache,
            push,
            no_tui,
            verbose_build,
        } => {
            handle_build(
                context.clone(),
                file.clone(),
                tags.clone(),
                runtime.clone(),
                *runtime_auto,
                build_args.clone(),
                target.clone(),
                *no_cache,
                *push,
                *no_tui,
                *verbose_build,
            )
            .await
        }
        Commands::Runtimes => handle_runtimes().await,
        Commands::Token(token_cmd) => handle_token(token_cmd),
        Commands::Spec(spec_cmd) => handle_spec(spec_cmd).await,
        #[cfg(feature = "node")]
        Commands::Node(node_cmd) => match node_cmd {
            NodeCommands::Init {
                advertise_addr,
                api_port,
                raft_port,
                overlay_port,
                data_dir,
                overlay_cidr,
            } => {
                handle_node_init(
                    advertise_addr.clone(),
                    *api_port,
                    *raft_port,
                    *overlay_port,
                    data_dir.clone(),
                    overlay_cidr.clone(),
                )
                .await
            }
            NodeCommands::Join {
                leader_addr,
                token,
                advertise_addr,
                mode,
                services,
            } => {
                handle_node_join(
                    leader_addr.clone(),
                    token.clone(),
                    advertise_addr.clone(),
                    mode.clone(),
                    services.clone(),
                )
                .await
            }
            NodeCommands::List { output } => handle_node_list(output.clone()).await,
            NodeCommands::Status { node_id } => handle_node_status(node_id.clone()).await,
            NodeCommands::Remove { node_id, force } => {
                handle_node_remove(node_id.clone(), *force).await
            }
            NodeCommands::SetMode {
                node_id,
                mode,
                services,
            } => handle_node_set_mode(node_id.clone(), mode.clone(), services.clone()).await,
            NodeCommands::Label { node_id, label } => {
                handle_node_label(node_id.clone(), label.clone()).await
            }
            NodeCommands::GenerateJoinToken {
                deployment,
                api,
                service,
            } => handle_node_generate_join_token(deployment.clone(), api.clone(), service.clone()),
        },
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

/// Build a container image from a Dockerfile or runtime template
#[allow(clippy::too_many_arguments)]
async fn handle_build(
    context: PathBuf,
    file: Option<PathBuf>,
    tags: Vec<String>,
    runtime: Option<String>,
    runtime_auto: bool,
    build_args: Vec<String>,
    target: Option<String>,
    no_cache: bool,
    push: bool,
    no_tui: bool,
    verbose_build: bool,
) -> Result<()> {
    use builder::{detect_runtime, BuildEvent, ImageBuilder, PlainLogger, Runtime};

    info!(
        context = %context.display(),
        tags = ?tags,
        runtime = ?runtime,
        runtime_auto = runtime_auto,
        "Starting build"
    );

    // Resolve runtime
    let resolved_runtime = if runtime_auto {
        info!("Auto-detecting runtime from project files");
        detect_runtime(&context)
    } else if let Some(name) = runtime {
        match Runtime::from_name(&name) {
            Some(rt) => {
                info!(runtime = %rt, "Using specified runtime template");
                Some(rt)
            }
            None => {
                // List available runtimes in error message
                let available: Vec<_> = Runtime::all().iter().map(|r| r.name).collect();
                anyhow::bail!(
                    "Unknown runtime: '{}'. Available runtimes: {}",
                    name,
                    available.join(", ")
                );
            }
        }
    } else {
        None
    };

    // Parse build args
    let build_args_map: HashMap<String, String> = build_args
        .iter()
        .filter_map(|arg| {
            let parts: Vec<&str> = arg.splitn(2, '=').collect();
            if parts.len() == 2 {
                Some((parts[0].to_string(), parts[1].to_string()))
            } else {
                warn!(arg = %arg, "Invalid build-arg format, expected KEY=VALUE");
                eprintln!("Warning: invalid build-arg '{}', expected KEY=VALUE", arg);
                None
            }
        })
        .collect();

    // Create event channel for progress updates
    let (event_tx, event_rx) = mpsc::channel::<BuildEvent>();

    // Build the ImageBuilder
    let mut builder = ImageBuilder::new(&context)
        .await
        .context("Failed to create image builder")?
        .with_events(event_tx);

    // Apply Dockerfile path if specified
    if let Some(dockerfile) = file {
        builder = builder.dockerfile(dockerfile);
    }

    // Apply runtime template if resolved
    if let Some(rt) = resolved_runtime {
        builder = builder.runtime(rt);
    }

    // Apply tags
    for tag in &tags {
        builder = builder.tag(tag);
    }

    // Apply build args
    builder = builder.build_args(build_args_map);

    // Apply target stage
    if let Some(t) = target {
        builder = builder.target(t);
    }

    // Apply no-cache
    if no_cache {
        builder = builder.no_cache();
    }

    // Apply push
    if push {
        builder = builder.push_without_auth();
    }

    // Determine if we should use TUI or plain output
    let use_tui = !no_tui && std::io::stdout().is_terminal();

    if use_tui {
        // TUI mode - run build with interactive progress display
        use builder::BuildTui;

        // Spawn build in background
        let build_handle = tokio::spawn(async move { builder.build().await });

        // Run TUI (blocking on the current thread)
        // We need to spawn a blocking task for this
        let tui_result = tokio::task::spawn_blocking(move || {
            let mut tui = BuildTui::new(event_rx);
            tui.run()
        })
        .await
        .context("TUI task panicked")?;

        if let Err(e) = tui_result {
            warn!(error = %e, "TUI error");
        }

        // Wait for build result
        let result = build_handle
            .await
            .context("Build task panicked")?
            .context("Build failed")?;

        println!("\nBuilt image: {}", result.image_id);
        for tag in &result.tags {
            println!("  Tagged: {}", tag);
        }
        println!("Build time: {}ms", result.build_time_ms);
    } else {
        // Plain output mode (CI or --no-tui)
        let logger = PlainLogger::new(verbose_build);

        // Spawn build in background
        let build_handle = tokio::spawn(async move { builder.build().await });

        // Process events in the main thread until build completes
        while let Ok(event) = event_rx.recv() {
            let is_terminal = matches!(
                event,
                BuildEvent::BuildComplete { .. } | BuildEvent::BuildFailed { .. }
            );
            logger.handle_event(&event);
            if is_terminal {
                break;
            }
        }

        // Wait for build result
        let result = build_handle
            .await
            .context("Build task panicked")?
            .context("Build failed")?;

        println!("\nBuilt image: {}", result.image_id);
        for tag in &result.tags {
            println!("  Tagged: {}", tag);
        }
        println!("Build time: {}ms", result.build_time_ms);
    }

    Ok(())
}

/// List available runtime templates
async fn handle_runtimes() -> Result<()> {
    use builder::{list_templates, Runtime};

    println!("Available runtime templates:\n");

    for info in list_templates() {
        println!("  {:12} - {}", info.name, info.description);
        println!("               Detects: {}", info.detect_files.join(", "));
        println!();
    }

    println!("Usage:");
    println!("  zlayer build --runtime node20 .");
    println!("  zlayer build --runtime-auto .   # auto-detect from project files");
    println!();
    println!(
        "All runtimes: {}",
        Runtime::all()
            .iter()
            .map(|r| r.name)
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(())
}

// =============================================================================
// Node Management Commands
// =============================================================================

/// Node configuration stored on disk
#[cfg(feature = "node")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeConfig {
    /// Unique node identifier
    node_id: String,
    /// Raft node ID (numeric)
    raft_node_id: u64,
    /// Public IP address for this node
    advertise_addr: String,
    /// API server port
    api_port: u16,
    /// Raft consensus port
    raft_port: u16,
    /// WireGuard overlay port
    overlay_port: u16,
    /// Overlay network CIDR
    overlay_cidr: String,
    /// WireGuard private key
    wireguard_private_key: String,
    /// WireGuard public key
    wireguard_public_key: String,
    /// Whether this node is the cluster leader/bootstrap node
    is_leader: bool,
    /// Timestamp when node was created
    created_at: String,
}

/// Join token payload
#[cfg(feature = "node")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClusterJoinToken {
    /// Leader's API endpoint
    api_endpoint: String,
    /// Leader's Raft endpoint
    raft_endpoint: String,
    /// Leader's WireGuard public key
    leader_wg_pubkey: String,
    /// Overlay network CIDR
    overlay_cidr: String,
    /// Cluster authentication secret
    auth_secret: String,
    /// Token creation timestamp
    created_at: String,
}

/// Join request sent to the leader
#[cfg(feature = "node")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeJoinRequest {
    /// Join token for authentication
    token: String,
    /// Joining node's advertise address
    advertise_addr: String,
    /// Joining node's overlay port
    overlay_port: u16,
    /// Joining node's Raft port
    raft_port: u16,
    /// Joining node's WireGuard public key
    wg_public_key: String,
    /// Node mode (full, replicate)
    mode: String,
    /// Services to replicate (if mode is replicate)
    services: Option<Vec<String>>,
}

/// Join response from the leader
#[cfg(feature = "node")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeJoinResponse {
    /// Assigned node ID
    node_id: String,
    /// Assigned Raft node ID
    raft_node_id: u64,
    /// Assigned overlay IP
    overlay_ip: String,
    /// Existing peers in the cluster
    peers: Vec<PeerNode>,
}

/// Peer node information
#[cfg(feature = "node")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerNode {
    node_id: String,
    raft_node_id: u64,
    advertise_addr: String,
    overlay_port: u16,
    raft_port: u16,
    wg_public_key: String,
    overlay_ip: String,
}

/// Node status for listing
#[cfg(feature = "node")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeStatus {
    id: String,
    address: String,
    status: String,
    mode: String,
    services: Vec<String>,
    is_leader: bool,
}

/// Generate a secure random token
#[cfg(feature = "node")]
fn generate_secure_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.gen();
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
}

/// Generate a unique node ID
#[cfg(feature = "node")]
fn generate_node_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Get current timestamp as ISO 8601 string
#[cfg(feature = "node")]
fn current_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Save node configuration to disk
#[cfg(feature = "node")]
async fn save_node_config(data_dir: &Path, config: &NodeConfig) -> Result<()> {
    let config_path = data_dir.join("node_config.json");
    let content =
        serde_json::to_string_pretty(config).context("Failed to serialize node config")?;
    tokio::fs::write(&config_path, content)
        .await
        .with_context(|| format!("Failed to write node config to {}", config_path.display()))?;
    info!(path = %config_path.display(), "Saved node configuration");
    Ok(())
}

/// Load node configuration from disk
#[cfg(feature = "node")]
async fn load_node_config(data_dir: &Path) -> Result<NodeConfig> {
    let config_path = data_dir.join("node_config.json");
    let content = tokio::fs::read_to_string(&config_path)
        .await
        .with_context(|| format!("Failed to read node config from {}", config_path.display()))?;
    let config: NodeConfig =
        serde_json::from_str(&content).context("Failed to parse node config")?;
    Ok(config)
}

/// Generate a join token for the cluster
#[cfg(feature = "node")]
fn generate_join_token_data(
    advertise_addr: &str,
    api_port: u16,
    raft_port: u16,
    wg_public_key: &str,
    overlay_cidr: &str,
) -> Result<String> {
    let token_data = ClusterJoinToken {
        api_endpoint: format!("{}:{}", advertise_addr, api_port),
        raft_endpoint: format!("{}:{}", advertise_addr, raft_port),
        leader_wg_pubkey: wg_public_key.to_string(),
        overlay_cidr: overlay_cidr.to_string(),
        auth_secret: generate_secure_token(),
        created_at: current_timestamp(),
    };

    let json = serde_json::to_string(&token_data).context("Failed to serialize join token")?;

    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        json.as_bytes(),
    ))
}

/// Parse a join token
#[cfg(feature = "node")]
fn parse_cluster_join_token(token: &str) -> Result<ClusterJoinToken> {
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, token)
        .or_else(|_| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, token))
        .context("Invalid join token: not valid base64")?;

    let token_data: ClusterJoinToken =
        serde_json::from_slice(&decoded).context("Invalid join token: not valid JSON")?;

    Ok(token_data)
}

/// Initialize this node as cluster leader
#[cfg(feature = "node")]
async fn handle_node_init(
    advertise_addr: String,
    api_port: u16,
    raft_port: u16,
    overlay_port: u16,
    data_dir: PathBuf,
    overlay_cidr: String,
) -> Result<()> {
    use overlay::WireGuardManager;

    println!("Initializing ZLayer node as cluster leader...");

    // 1. Create data directory
    tokio::fs::create_dir_all(&data_dir)
        .await
        .with_context(|| format!("Failed to create data directory: {}", data_dir.display()))?;
    info!(path = %data_dir.display(), "Created data directory");

    // 2. Check if already initialized
    let config_path = data_dir.join("node_config.json");
    if config_path.exists() {
        anyhow::bail!(
            "Node already initialized. Configuration exists at {}. \
            Use 'zlayer node status' to check the node or remove the config file to reinitialize.",
            config_path.display()
        );
    }

    // 3. Generate node ID
    let node_id = generate_node_id();
    let raft_node_id: u64 = 1; // First node is always ID 1
    info!(node_id = %node_id, raft_node_id = raft_node_id, "Generated node ID");

    // 4. Generate WireGuard keypair
    println!("  Generating WireGuard keypair...");
    let (private_key, public_key) = WireGuardManager::generate_keys().await.map_err(|e| {
        anyhow::anyhow!(
            "Failed to generate WireGuard keys: {}. Ensure 'wg' command is installed.",
            e
        )
    })?;
    info!("Generated WireGuard keypair");

    // 5. Save node config
    let node_config = NodeConfig {
        node_id: node_id.clone(),
        raft_node_id,
        advertise_addr: advertise_addr.clone(),
        api_port,
        raft_port,
        overlay_port,
        overlay_cidr: overlay_cidr.clone(),
        wireguard_private_key: private_key,
        wireguard_public_key: public_key.clone(),
        is_leader: true,
        created_at: current_timestamp(),
    };
    save_node_config(&data_dir, &node_config).await?;

    // 6. Initialize Raft as leader (bootstrap single-node cluster)
    println!("  Starting Raft consensus...");
    let raft_config = scheduler::RaftConfig {
        node_id: raft_node_id,
        address: format!("{}:{}", advertise_addr, raft_port),
        raft_port,
        ..Default::default()
    };

    let raft = scheduler::RaftCoordinator::new(raft_config)
        .await
        .context("Failed to create Raft coordinator")?;

    raft.bootstrap()
        .await
        .context("Failed to bootstrap Raft cluster")?;
    info!("Raft cluster bootstrapped");

    // 7. Generate join token
    let join_token = generate_join_token_data(
        &advertise_addr,
        api_port,
        raft_port,
        &public_key,
        &overlay_cidr,
    )?;

    // 8. Print success message
    println!();
    println!("Node initialized successfully!");
    println!();
    println!("Node ID:        {}", node_id);
    println!("Raft Node ID:   {}", raft_node_id);
    println!("API Server:     http://{}:{}", advertise_addr, api_port);
    println!("Raft Address:   {}:{}", advertise_addr, raft_port);
    println!("Overlay Port:   {}", overlay_port);
    println!("Overlay CIDR:   {}", overlay_cidr);
    println!("WG Public Key:  {}", public_key);
    println!();
    println!("To join other nodes to this cluster, run:");
    println!();
    println!(
        "  zlayer node join {}:{} --token {} --advertise-addr <NODE_IP>",
        advertise_addr, api_port, join_token
    );
    println!();
    println!("Note: Start the control plane with 'zlayer serve' to accept join requests.");

    Ok(())
}

/// Join an existing cluster as a worker node
#[cfg(feature = "node")]
async fn handle_node_join(
    leader_addr: String,
    token: String,
    advertise_addr: String,
    mode: String,
    services: Option<Vec<String>>,
) -> Result<()> {
    use overlay::WireGuardManager;
    use std::time::Duration;

    println!("Joining ZLayer cluster at {}...", leader_addr);

    // 1. Parse and validate the join token
    let token_data = parse_cluster_join_token(&token).context("Invalid join token")?;

    info!(
        api_endpoint = %token_data.api_endpoint,
        overlay_cidr = %token_data.overlay_cidr,
        "Parsed join token"
    );

    // 2. Generate WireGuard keypair for this node
    println!("  Generating WireGuard keypair...");
    let (private_key, public_key) = WireGuardManager::generate_keys().await.map_err(|e| {
        anyhow::anyhow!(
            "Failed to generate WireGuard keys: {}. Ensure 'wg' command is installed.",
            e
        )
    })?;

    // 3. Determine data directory
    let data_dir = PathBuf::from("/var/lib/zlayer");
    tokio::fs::create_dir_all(&data_dir)
        .await
        .context("Failed to create data directory")?;

    // 4. Check if already initialized
    let config_path = data_dir.join("node_config.json");
    if config_path.exists() {
        anyhow::bail!(
            "Node already initialized. Configuration exists at {}. \
            Remove the config file to join a different cluster.",
            config_path.display()
        );
    }

    // 5. Send join request to the leader
    println!("  Contacting leader at {}...", token_data.api_endpoint);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    // Parse overlay port from advertise address or use default
    let overlay_port: u16 = 51820;
    let raft_port: u16 = 9000;

    let join_request = NodeJoinRequest {
        token: token.clone(),
        advertise_addr: advertise_addr.clone(),
        overlay_port,
        raft_port,
        wg_public_key: public_key.clone(),
        mode: mode.clone(),
        services: services.clone(),
    };

    let response = client
        .post(format!(
            "http://{}/api/v1/cluster/join",
            token_data.api_endpoint
        ))
        .json(&join_request)
        .send()
        .await
        .context("Failed to send join request to leader")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Join request failed: {} - {}", status, body);
    }

    let join_response: NodeJoinResponse = response
        .json()
        .await
        .context("Failed to parse join response")?;

    info!(
        node_id = %join_response.node_id,
        raft_node_id = join_response.raft_node_id,
        overlay_ip = %join_response.overlay_ip,
        "Received join response"
    );

    // 6. Save node configuration
    let node_config = NodeConfig {
        node_id: join_response.node_id.clone(),
        raft_node_id: join_response.raft_node_id,
        advertise_addr: advertise_addr.clone(),
        api_port: 8080, // Default for workers
        raft_port,
        overlay_port,
        overlay_cidr: token_data.overlay_cidr.clone(),
        wireguard_private_key: private_key,
        wireguard_public_key: public_key.clone(),
        is_leader: false,
        created_at: current_timestamp(),
    };
    save_node_config(&data_dir, &node_config).await?;

    // 7. Configure WireGuard with peers
    println!("  Configuring overlay network...");
    // TODO: Actually configure WireGuard interface with peers
    for peer in &join_response.peers {
        info!(
            peer_id = %peer.node_id,
            peer_addr = %peer.advertise_addr,
            "Added peer"
        );
    }

    // 8. Print success message
    println!();
    println!("Successfully joined cluster!");
    println!();
    println!("Node ID:        {}", join_response.node_id);
    println!("Raft Node ID:   {}", join_response.raft_node_id);
    println!("Overlay IP:     {}", join_response.overlay_ip);
    println!("Mode:           {}", mode);
    if let Some(svcs) = services {
        println!("Services:       {}", svcs.join(", "));
    }
    println!("Peers:          {}", join_response.peers.len());
    println!();
    println!("Start the agent with 'zlayer serve' to begin processing workloads.");

    Ok(())
}

/// List all nodes in the cluster
#[cfg(feature = "node")]
async fn handle_node_list(output: String) -> Result<()> {
    use std::time::Duration;

    // Try to load local node config to get API endpoint
    let data_dir = PathBuf::from("/var/lib/zlayer");
    let node_config = match load_node_config(&data_dir).await {
        Ok(config) => config,
        Err(_) => {
            anyhow::bail!(
                "Node not initialized. Run 'zlayer node init' or 'zlayer node join' first."
            );
        }
    };

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    // Fetch node list from API
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    let response = client
        .get(format!("http://{}/api/v1/cluster/nodes", api_endpoint))
        .send()
        .await;

    let nodes: Vec<NodeStatus> = match response {
        Ok(resp) if resp.status().is_success() => {
            resp.json().await.unwrap_or_else(|_| {
                // Return at least the local node if we can't parse
                vec![NodeStatus {
                    id: node_config.node_id.clone(),
                    address: format!("{}:{}", node_config.advertise_addr, node_config.api_port),
                    status: "unknown".to_string(),
                    mode: if node_config.is_leader {
                        "leader".to_string()
                    } else {
                        "worker".to_string()
                    },
                    services: vec![],
                    is_leader: node_config.is_leader,
                }]
            })
        }
        _ => {
            // API not available, show local node info
            warn!("Could not connect to cluster API. Showing local node only.");
            vec![NodeStatus {
                id: node_config.node_id.clone(),
                address: format!("{}:{}", node_config.advertise_addr, node_config.api_port),
                status: "local".to_string(),
                mode: if node_config.is_leader {
                    "leader".to_string()
                } else {
                    "worker".to_string()
                },
                services: vec![],
                is_leader: node_config.is_leader,
            }]
        }
    };

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&nodes)?);
    } else {
        // Table format
        println!(
            "{:<36} {:<20} {:<10} {:<10} {:<6} SERVICES",
            "NODE ID", "ADDRESS", "STATUS", "MODE", "LEADER"
        );
        println!("{}", "-".repeat(100));

        for node in nodes {
            let services = if node.services.is_empty() {
                "-".to_string()
            } else {
                let s = node.services.join(", ");
                if s.len() > 20 {
                    format!("{}...", &s[..17])
                } else {
                    s
                }
            };
            let leader_marker = if node.is_leader { "*" } else { "" };
            println!(
                "{:<36} {:<20} {:<10} {:<10} {:<6} {}",
                node.id, node.address, node.status, node.mode, leader_marker, services
            );
        }
    }

    Ok(())
}

/// Show detailed status of a node
#[cfg(feature = "node")]
async fn handle_node_status(node_id: Option<String>) -> Result<()> {
    let data_dir = PathBuf::from("/var/lib/zlayer");
    let node_config = match load_node_config(&data_dir).await {
        Ok(config) => config,
        Err(_) => {
            anyhow::bail!(
                "Node not initialized. Run 'zlayer node init' or 'zlayer node join' first."
            );
        }
    };

    // If no node_id specified, show this node
    let target_id = node_id.unwrap_or(node_config.node_id.clone());

    if target_id == node_config.node_id {
        // Show local node detailed status
        println!("Node Status");
        println!("{}", "=".repeat(50));
        println!();
        println!("Node ID:            {}", node_config.node_id);
        println!("Raft Node ID:       {}", node_config.raft_node_id);
        println!(
            "Role:               {}",
            if node_config.is_leader {
                "Leader"
            } else {
                "Worker"
            }
        );
        println!("Created At:         {}", node_config.created_at);
        println!();
        println!("Network Configuration:");
        println!("  Advertise Address: {}", node_config.advertise_addr);
        println!("  API Port:          {}", node_config.api_port);
        println!("  Raft Port:         {}", node_config.raft_port);
        println!("  Overlay Port:      {}", node_config.overlay_port);
        println!("  Overlay CIDR:      {}", node_config.overlay_cidr);
        println!();
        println!("WireGuard:");
        println!("  Public Key:        {}", node_config.wireguard_public_key);
        println!();
        println!("Endpoints:");
        println!(
            "  API:   http://{}:{}",
            node_config.advertise_addr, node_config.api_port
        );
        println!(
            "  Raft:  {}:{}",
            node_config.advertise_addr, node_config.raft_port
        );
        println!(
            "  WG:    {}:{}/udp",
            node_config.advertise_addr, node_config.overlay_port
        );
    } else {
        // Fetch remote node status via API
        use std::time::Duration;
        let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to create HTTP client")?;

        let response = client
            .get(format!(
                "http://{}/api/v1/cluster/nodes/{}",
                api_endpoint, target_id
            ))
            .send()
            .await
            .context("Failed to fetch node status")?;

        if !response.status().is_success() {
            anyhow::bail!("Node '{}' not found in cluster", target_id);
        }

        let status: NodeStatus = response
            .json()
            .await
            .context("Failed to parse node status")?;

        println!("Node Status");
        println!("{}", "=".repeat(50));
        println!();
        println!("Node ID:    {}", status.id);
        println!("Address:    {}", status.address);
        println!("Status:     {}", status.status);
        println!("Mode:       {}", status.mode);
        println!(
            "Leader:     {}",
            if status.is_leader { "Yes" } else { "No" }
        );
        if !status.services.is_empty() {
            println!("Services:   {}", status.services.join(", "));
        }
    }

    Ok(())
}

/// Remove a node from the cluster
#[cfg(feature = "node")]
async fn handle_node_remove(node_id: String, force: bool) -> Result<()> {
    use std::time::Duration;

    let data_dir = PathBuf::from("/var/lib/zlayer");
    let node_config = match load_node_config(&data_dir).await {
        Ok(config) => config,
        Err(_) => {
            anyhow::bail!(
                "Node not initialized. Run 'zlayer node init' or 'zlayer node join' first."
            );
        }
    };

    // Cannot remove yourself
    if node_id == node_config.node_id {
        anyhow::bail!(
            "Cannot remove the current node. To leave the cluster, stop the node and remove its data directory."
        );
    }

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    println!("Removing node '{}' from cluster...", node_id);
    if force {
        warn!("Force removal enabled - services will not be migrated");
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let mut url = format!("http://{}/api/v1/cluster/nodes/{}", api_endpoint, node_id);
    if force {
        url.push_str("?force=true");
    }

    let response = client
        .delete(&url)
        .send()
        .await
        .context("Failed to send remove request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to remove node: {} - {}", status, body);
    }

    println!("Node '{}' removed successfully.", node_id);
    if !force {
        println!("Services have been migrated to other nodes.");
    }

    Ok(())
}

/// Set node resource mode
#[cfg(feature = "node")]
async fn handle_node_set_mode(
    node_id: String,
    mode: String,
    services: Option<Vec<String>>,
) -> Result<()> {
    use std::time::Duration;

    // Validate mode
    let valid_modes = ["full", "dedicated", "replicate"];
    if !valid_modes.contains(&mode.as_str()) {
        anyhow::bail!(
            "Invalid mode '{}'. Valid modes are: {}",
            mode,
            valid_modes.join(", ")
        );
    }

    // Validate mode-service combination
    if (mode == "dedicated" || mode == "replicate") && services.is_none() {
        anyhow::bail!("Mode '{}' requires --services to be specified", mode);
    }

    let data_dir = PathBuf::from("/var/lib/zlayer");
    let node_config = match load_node_config(&data_dir).await {
        Ok(config) => config,
        Err(_) => {
            anyhow::bail!(
                "Node not initialized. Run 'zlayer node init' or 'zlayer node join' first."
            );
        }
    };

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    println!("Setting mode for node '{}'...", node_id);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    #[derive(Serialize)]
    struct SetModeRequest {
        mode: String,
        services: Option<Vec<String>>,
    }

    let request = SetModeRequest {
        mode: mode.clone(),
        services: services.clone(),
    };

    let response = client
        .put(format!(
            "http://{}/api/v1/cluster/nodes/{}/mode",
            api_endpoint, node_id
        ))
        .json(&request)
        .send()
        .await
        .context("Failed to send set-mode request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to set node mode: {} - {}", status, body);
    }

    println!("Node '{}' mode set to '{}'.", node_id, mode);
    if let Some(svcs) = services {
        println!("Services: {}", svcs.join(", "));
    }

    Ok(())
}

/// Add label to a node
#[cfg(feature = "node")]
async fn handle_node_label(node_id: String, label: String) -> Result<()> {
    use std::time::Duration;

    // Parse label
    let parts: Vec<&str> = label.splitn(2, '=').collect();
    if parts.len() != 2 {
        anyhow::bail!(
            "Invalid label format '{}'. Expected key=value format.",
            label
        );
    }
    let (key, value) = (parts[0], parts[1]);

    // Validate label key
    if key.is_empty() {
        anyhow::bail!("Label key cannot be empty");
    }

    let data_dir = PathBuf::from("/var/lib/zlayer");
    let node_config = match load_node_config(&data_dir).await {
        Ok(config) => config,
        Err(_) => {
            anyhow::bail!(
                "Node not initialized. Run 'zlayer node init' or 'zlayer node join' first."
            );
        }
    };

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    println!("Adding label to node '{}'...", node_id);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    #[derive(Serialize)]
    struct AddLabelRequest {
        key: String,
        value: String,
    }

    let request = AddLabelRequest {
        key: key.to_string(),
        value: value.to_string(),
    };

    let response = client
        .post(format!(
            "http://{}/api/v1/cluster/nodes/{}/labels",
            api_endpoint, node_id
        ))
        .json(&request)
        .send()
        .await
        .context("Failed to send label request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to add label: {} - {}", status, body);
    }

    println!("Label '{}={}' added to node '{}'.", key, value, node_id);

    Ok(())
}

/// Handle node generate-join-token command
fn handle_node_generate_join_token(
    deployment: String,
    api: String,
    service: Option<String>,
) -> Result<()> {
    use base64::Engine;

    let token_data = serde_json::json!({
        "deployment": deployment,
        "api_endpoint": api,
        "service": service,
    });

    let json = serde_json::to_string(&token_data)?;
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&json);

    println!("Join Token Generated");
    println!("====================\n");
    println!("Deployment: {}", deployment);
    println!("API: {}", api);
    if let Some(svc) = &service {
        println!("Service: {}", svc);
    }
    println!("\nToken:");
    println!("{}", token);
    println!("\nUsage:");
    println!("  zlayer node join <leader-addr> --token {}", token);

    Ok(())
}

/// Decode a base64url-encoded JSON string
fn decode_base64_json(input: &str) -> Result<serde_json::Value> {
    use base64::Engine;

    // JWT uses base64url encoding without padding
    // Try URL-safe first, then standard base64
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .or_else(|_| {
            // Add padding if needed and try again
            let padded = match input.len() % 4 {
                2 => format!("{}==", input),
                3 => format!("{}=", input),
                _ => input.to_string(),
            };
            base64::engine::general_purpose::URL_SAFE.decode(&padded)
        })
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(input))
        .context("Failed to decode base64")?;

    serde_json::from_slice(&decoded).context("Failed to parse JSON")
}

/// Handle token commands
fn handle_token(action: &TokenCommands) -> Result<()> {
    match action {
        TokenCommands::Create {
            subject,
            secret,
            hours,
            roles,
            quiet,
        } => {
            let subject = subject.clone();
            let secret = secret.clone();
            let hours = *hours;
            let roles = roles.clone();
            let quiet = *quiet;
            let secret = secret
                .or_else(|| std::env::var("ZLAYER_JWT_SECRET").ok())
                .unwrap_or_else(|| {
                    if !quiet {
                        warn!("Using default secret - tokens will only work with default server config");
                    }
                    "CHANGE_ME_IN_PRODUCTION".to_string()
                });

            let roles: Vec<String> = roles
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let expiry = Duration::from_secs(hours * 3600);

            let token = create_token(&secret, &subject, expiry, roles.clone())
                .context("Failed to create token")?;

            if quiet {
                println!("{}", token);
            } else {
                println!("Token created successfully!\n");
                println!("Subject: {}", subject);
                println!("Roles: {}", roles.join(", "));
                println!("Expires in: {} hours", hours);
                println!("\nToken:");
                println!("{}", token);
                println!("\nUsage:");
                println!(
                    "  curl -H 'Authorization: Bearer {}' http://localhost:8080/api/v1/deployments",
                    token
                );
            }
            Ok(())
        }

        TokenCommands::Decode { token, json } => {
            let token = token.clone();
            let json = *json;
            let parts: Vec<&str> = token.split('.').collect();
            if parts.len() != 3 {
                anyhow::bail!("Invalid JWT format: expected 3 parts separated by dots");
            }

            let header = decode_base64_json(parts[0]).context("Failed to decode token header")?;
            let claims = decode_base64_json(parts[1]).context("Failed to decode token payload")?;

            if json {
                let output = serde_json::json!({
                    "header": header,
                    "claims": claims,
                    "signature": parts[2]
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!("Token Header:");
                println!("{}", serde_json::to_string_pretty(&header)?);
                println!("\nToken Claims:");
                println!("{}", serde_json::to_string_pretty(&claims)?);

                if let Some(exp) = claims.get("exp").and_then(|v| v.as_u64()) {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    if exp < now {
                        println!("\n[!] Token is EXPIRED");
                    } else {
                        let remaining = exp - now;
                        let hours = remaining / 3600;
                        let mins = (remaining % 3600) / 60;
                        println!("\n[OK] Token expires in {}h {}m", hours, mins);
                    }
                }
            }
            Ok(())
        }

        TokenCommands::Info => {
            println!("ZLayer Token System");
            println!("===================\n");
            println!("Available Roles:");
            println!("  admin    - Full access to all operations");
            println!("  operator - Can scale services, view logs, manage deployments");
            println!("  deployer - Can create and update deployments");
            println!("  reader   - Read-only access to deployments and services");
            println!();
            println!("Token Format: JWT (HS256)");
            println!("Default Expiry: 24 hours");
            println!();
            println!("Environment Variables:");
            println!("  ZLAYER_JWT_SECRET - JWT signing secret (required for production)");
            println!("  ZLAYER_TOKEN      - Bearer token for API requests");
            Ok(())
        }
    }
}

/// Handle spec commands
async fn handle_spec(action: &SpecCommands) -> Result<()> {
    match action {
        SpecCommands::Dump { spec, format } => {
            let spec = spec.clone();
            let format = format.clone();
            let content = std::fs::read_to_string(&spec)
                .context("Failed to read specification file")?;

            let parsed_spec = spec::from_yaml_str(&content)
                .context("Failed to parse specification")?;

            match format.to_lowercase().as_str() {
                "json" => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&parsed_spec)
                            .context("Failed to serialize as JSON")?
                    );
                }
                _ => {
                    println!(
                        "{}",
                        serde_yaml::to_string(&parsed_spec)
                            .context("Failed to serialize as YAML")?
                    );
                }
            }
            Ok(())
        }
    }
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

    #[test]
    fn test_cli_build_command_minimal() {
        let cli = Cli::try_parse_from(["zlayer", "build"]).unwrap();

        match cli.command {
            Commands::Build {
                context,
                file,
                tags,
                runtime,
                runtime_auto,
                build_args,
                target,
                no_cache,
                push,
                no_tui,
                verbose_build,
            } => {
                assert_eq!(context, PathBuf::from("."));
                assert!(file.is_none());
                assert!(tags.is_empty());
                assert!(runtime.is_none());
                assert!(!runtime_auto);
                assert!(build_args.is_empty());
                assert!(target.is_none());
                assert!(!no_cache);
                assert!(!push);
                assert!(!no_tui);
                assert!(!verbose_build);
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_context() {
        let cli = Cli::try_parse_from(["zlayer", "build", "./my-app"]).unwrap();

        match cli.command {
            Commands::Build { context, .. } => {
                assert_eq!(context, PathBuf::from("./my-app"));
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_tags() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "build",
            "-t",
            "myapp:latest",
            "-t",
            "myapp:v1.0.0",
            ".",
        ])
        .unwrap();

        match cli.command {
            Commands::Build { tags, .. } => {
                assert_eq!(tags.len(), 2);
                assert_eq!(tags[0], "myapp:latest");
                assert_eq!(tags[1], "myapp:v1.0.0");
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_dockerfile() {
        let cli = Cli::try_parse_from(["zlayer", "build", "-f", "Dockerfile.prod", "."]).unwrap();

        match cli.command {
            Commands::Build { file, .. } => {
                assert_eq!(file, Some(PathBuf::from("Dockerfile.prod")));
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_runtime() {
        let cli = Cli::try_parse_from(["zlayer", "build", "--runtime", "node20", "."]).unwrap();

        match cli.command {
            Commands::Build { runtime, .. } => {
                assert_eq!(runtime, Some("node20".to_string()));
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_runtime_auto() {
        let cli = Cli::try_parse_from(["zlayer", "build", "--runtime-auto", "."]).unwrap();

        match cli.command {
            Commands::Build { runtime_auto, .. } => {
                assert!(runtime_auto);
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_build_args() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "build",
            "--build-arg",
            "VERSION=1.0.0",
            "--build-arg",
            "DEBUG=false",
            ".",
        ])
        .unwrap();

        match cli.command {
            Commands::Build { build_args, .. } => {
                assert_eq!(build_args.len(), 2);
                assert_eq!(build_args[0], "VERSION=1.0.0");
                assert_eq!(build_args[1], "DEBUG=false");
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_target() {
        let cli = Cli::try_parse_from(["zlayer", "build", "--target", "builder", "."]).unwrap();

        match cli.command {
            Commands::Build { target, .. } => {
                assert_eq!(target, Some("builder".to_string()));
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_flags() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "build",
            "--no-cache",
            "--push",
            "--no-tui",
            "--verbose-build",
            ".",
        ])
        .unwrap();

        match cli.command {
            Commands::Build {
                no_cache,
                push,
                no_tui,
                verbose_build,
                ..
            } => {
                assert!(no_cache);
                assert!(push);
                assert!(no_tui);
                assert!(verbose_build);
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "build",
            "-f",
            "Dockerfile.prod",
            "-t",
            "myapp:latest",
            "-t",
            "myapp:v1",
            "--runtime",
            "node20",
            "--build-arg",
            "VERSION=1.0",
            "--target",
            "production",
            "--no-cache",
            "--push",
            "--no-tui",
            "--verbose-build",
            "./my-project",
        ])
        .unwrap();

        match cli.command {
            Commands::Build {
                context,
                file,
                tags,
                runtime,
                build_args,
                target,
                no_cache,
                push,
                no_tui,
                verbose_build,
                ..
            } => {
                assert_eq!(context, PathBuf::from("./my-project"));
                assert_eq!(file, Some(PathBuf::from("Dockerfile.prod")));
                assert_eq!(tags.len(), 2);
                assert_eq!(runtime, Some("node20".to_string()));
                assert_eq!(build_args.len(), 1);
                assert_eq!(target, Some("production".to_string()));
                assert!(no_cache);
                assert!(push);
                assert!(no_tui);
                assert!(verbose_build);
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_runtimes_command() {
        let cli = Cli::try_parse_from(["zlayer", "runtimes"]).unwrap();
        assert!(matches!(cli.command, Commands::Runtimes));
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_cli_node_init_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "init", "--advertise-addr", "10.0.0.1"])
            .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Init {
                advertise_addr,
                api_port,
                raft_port,
                overlay_port,
                data_dir,
                overlay_cidr,
            }) => {
                assert_eq!(advertise_addr, "10.0.0.1");
                assert_eq!(api_port, 8080);
                assert_eq!(raft_port, 9000);
                assert_eq!(overlay_port, 51820);
                assert_eq!(data_dir, PathBuf::from("/var/lib/zlayer"));
                assert_eq!(overlay_cidr, "10.200.0.0/16");
            }
            _ => panic!("Expected Node Init command"),
        }
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_cli_node_init_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "init",
            "--advertise-addr",
            "192.168.1.100",
            "--api-port",
            "9090",
            "--raft-port",
            "9001",
            "--overlay-port",
            "51821",
            "--data-dir",
            "/custom/data",
            "--overlay-cidr",
            "10.100.0.0/16",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Init {
                advertise_addr,
                api_port,
                raft_port,
                overlay_port,
                data_dir,
                overlay_cidr,
            }) => {
                assert_eq!(advertise_addr, "192.168.1.100");
                assert_eq!(api_port, 9090);
                assert_eq!(raft_port, 9001);
                assert_eq!(overlay_port, 51821);
                assert_eq!(data_dir, PathBuf::from("/custom/data"));
                assert_eq!(overlay_cidr, "10.100.0.0/16");
            }
            _ => panic!("Expected Node Init command"),
        }
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_cli_node_join_command() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "join",
            "10.0.0.1:8080",
            "--token",
            "abc123",
            "--advertise-addr",
            "10.0.0.2",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Join {
                leader_addr,
                token,
                advertise_addr,
                mode,
                services,
            }) => {
                assert_eq!(leader_addr, "10.0.0.1:8080");
                assert_eq!(token, "abc123");
                assert_eq!(advertise_addr, "10.0.0.2");
                assert_eq!(mode, "full");
                assert!(services.is_none());
            }
            _ => panic!("Expected Node Join command"),
        }
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_cli_node_join_command_with_services() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "join",
            "10.0.0.1:8080",
            "--token",
            "abc123",
            "--advertise-addr",
            "10.0.0.2",
            "--mode",
            "replicate",
            "--services",
            "api",
            "--services",
            "web",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Join { mode, services, .. }) => {
                assert_eq!(mode, "replicate");
                assert_eq!(services, Some(vec!["api".to_string(), "web".to_string()]));
            }
            _ => panic!("Expected Node Join command"),
        }
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_cli_node_list_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "list"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::List { output }) => {
                assert_eq!(output, "table");
            }
            _ => panic!("Expected Node List command"),
        }
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_cli_node_list_command_json() {
        let cli = Cli::try_parse_from(["zlayer", "node", "list", "--output", "json"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::List { output }) => {
                assert_eq!(output, "json");
            }
            _ => panic!("Expected Node List command"),
        }
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_cli_node_status_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "status"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Status { node_id }) => {
                assert!(node_id.is_none());
            }
            _ => panic!("Expected Node Status command"),
        }
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_cli_node_status_command_with_id() {
        let cli = Cli::try_parse_from(["zlayer", "node", "status", "node-abc-123"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Status { node_id }) => {
                assert_eq!(node_id, Some("node-abc-123".to_string()));
            }
            _ => panic!("Expected Node Status command"),
        }
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_cli_node_remove_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "remove", "node-123"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Remove { node_id, force }) => {
                assert_eq!(node_id, "node-123");
                assert!(!force);
            }
            _ => panic!("Expected Node Remove command"),
        }
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_cli_node_remove_command_force() {
        let cli = Cli::try_parse_from(["zlayer", "node", "remove", "--force", "node-123"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Remove { node_id, force }) => {
                assert_eq!(node_id, "node-123");
                assert!(force);
            }
            _ => panic!("Expected Node Remove command"),
        }
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_cli_node_set_mode_command() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "set-mode",
            "node-123",
            "--mode",
            "dedicated",
            "--services",
            "api",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::SetMode {
                node_id,
                mode,
                services,
            }) => {
                assert_eq!(node_id, "node-123");
                assert_eq!(mode, "dedicated");
                assert_eq!(services, Some(vec!["api".to_string()]));
            }
            _ => panic!("Expected Node SetMode command"),
        }
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_cli_node_label_command() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "label",
            "node-123",
            "environment=production",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Label { node_id, label }) => {
                assert_eq!(node_id, "node-123");
                assert_eq!(label, "environment=production");
            }
            _ => panic!("Expected Node Label command"),
        }
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_generate_secure_token() {
        let token1 = super::generate_secure_token();
        let token2 = super::generate_secure_token();

        // Tokens should be different
        assert_ne!(token1, token2);

        // Tokens should be base64 encoded (43 chars for 32 bytes URL-safe no padding)
        assert_eq!(token1.len(), 43);
        assert_eq!(token2.len(), 43);
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_generate_node_id() {
        let id1 = super::generate_node_id();
        let id2 = super::generate_node_id();

        // IDs should be different
        assert_ne!(id1, id2);

        // IDs should be valid UUIDs (36 chars with dashes)
        assert_eq!(id1.len(), 36);
        assert_eq!(id2.len(), 36);
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_join_token_roundtrip() {
        let token = super::generate_join_token_data(
            "192.168.1.1",
            8080,
            9000,
            "test-public-key",
            "10.200.0.0/16",
        )
        .unwrap();

        let parsed = super::parse_cluster_join_token(&token).unwrap();

        assert_eq!(parsed.api_endpoint, "192.168.1.1:8080");
        assert_eq!(parsed.raft_endpoint, "192.168.1.1:9000");
        assert_eq!(parsed.leader_wg_pubkey, "test-public-key");
        assert_eq!(parsed.overlay_cidr, "10.200.0.0/16");
        assert!(!parsed.auth_secret.is_empty());
    }

    #[test]
    #[cfg(feature = "node")]
    fn test_parse_invalid_join_token() {
        // Invalid base64
        let result = super::parse_cluster_join_token("not-valid-base64!!!");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("base64"));

        // Valid base64 but invalid JSON
        let invalid_json = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            "not json",
        );
        let result = super::parse_cluster_join_token(&invalid_json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("JSON"));
    }
}
