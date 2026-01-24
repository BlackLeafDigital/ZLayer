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
#[cfg(feature = "deploy")]
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
    let _runtime = agent::create_runtime(runtime_config)
        .await
        .context("Failed to create container runtime")?;

    info!("Runtime created successfully");

    // Print deployment plan
    print_deployment_plan(&spec);

    // For now, just print what would be deployed
    // Future: Actually deploy the services using ServiceManager
    info!("Deployment would proceed with the above services");
    info!("(Full deployment implementation pending ServiceManager integration)");

    // Demonstrate runtime is working by showing its type
    match cli.runtime {
        RuntimeType::Mock => {
            info!("Using mock runtime - containers will be simulated");
        }
        RuntimeType::Youki => {
            info!(
                state_dir = %cli.state_dir.display(),
                "Using Youki runtime"
            );
        }
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
#[derive(Debug, serde::Deserialize)]
struct JoinInfo {
    /// Deployment identifier/key
    #[serde(alias = "key")]
    deployment: String,
    /// API endpoint to contact
    api_endpoint: String,
    /// Optional service name
    service: Option<String>,
}

/// Parse a join token
#[cfg(feature = "join")]
fn parse_join_token(token: &str) -> Result<JoinInfo> {
    use base64::Engine;

    // Try to decode as base64
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(token))
        .context("Invalid join token: not valid base64")?;

    // Parse as JSON
    let info: JoinInfo =
        serde_json::from_slice(&decoded).context("Invalid join token: not valid JSON")?;

    Ok(info)
}

/// Join an existing deployment
#[cfg(feature = "join")]
async fn join(
    _cli: &Cli,
    token: &str,
    spec_dir: Option<&str>,
    service: Option<&str>,
    replicas: u32,
) -> Result<()> {
    info!(
        token_len = token.len(),
        service = ?service,
        replicas = replicas,
        "Joining deployment"
    );

    // Parse the join token
    // Token format: base64(json({ "key": "deployment-key", "api": "http://...", "service": "svc" }))
    let join_info = parse_join_token(token)?;

    println!("Joining deployment: {}", join_info.deployment);
    println!("API endpoint: {}", join_info.api_endpoint);

    let target_service = service.or(join_info.service.as_deref());
    if let Some(svc) = target_service {
        println!("Service: {}", svc);
    }
    println!("Replicas: {}", replicas);

    // Spec directory
    let spec_base = spec_dir.map(String::from).unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{}/.zlayer/deployments", home)
    });
    let spec_path = format!("{}/{}/spec.yaml", spec_base, join_info.deployment);

    println!("\nLooking for spec at: {}", spec_path);

    // Check if spec exists locally, if not fetch from API
    if !std::path::Path::new(&spec_path).exists() {
        println!("Spec not found locally, fetching from API...");
        // TODO: Fetch spec from join_info.api_endpoint
        println!("[Spec fetching not yet implemented]");
        return Ok(());
    }

    // Load and validate spec
    let spec_content = std::fs::read_to_string(&spec_path).context("Failed to read spec file")?;
    let spec: DeploymentSpec =
        serde_yaml::from_str(&spec_content).context("Failed to parse spec")?;

    println!("Loaded deployment: {}", spec.deployment);

    // Find the service in the spec
    let service_name = target_service
        .ok_or_else(|| anyhow::anyhow!("No service specified and token doesn't include one"))?;

    let service_spec = spec
        .services
        .get(service_name)
        .ok_or_else(|| anyhow::anyhow!("Service '{}' not found in deployment", service_name))?;

    println!("\nService configuration:");
    println!("  Image: {}", service_spec.image.name);
    println!("  Type: {:?}", service_spec.rtype);

    // TODO: Actually start the service
    // 1. Create ServiceManager from agent crate
    // 2. Scale service to requested replicas
    // 3. Register with scheduler for load balancing
    println!("\n[Service startup not yet implemented]");
    println!("Would start {} replica(s) of '{}'", replicas, service_name);

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
            "deployment": "my-app",
            "api_endpoint": "http://localhost:8080",
            "service": "api"
        });

        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&info).unwrap());

        let parsed = super::parse_join_token(&token).unwrap();
        assert_eq!(parsed.deployment, "my-app");
        assert_eq!(parsed.api_endpoint, "http://localhost:8080");
        assert_eq!(parsed.service, Some("api".to_string()));
    }

    #[test]
    #[cfg(feature = "join")]
    fn test_parse_join_token_minimal() {
        use base64::Engine;

        let info = serde_json::json!({
            "key": "my-deploy",
            "api_endpoint": "http://api.example.com"
        });

        let token =
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_string(&info).unwrap());

        let parsed = super::parse_join_token(&token).unwrap();
        assert_eq!(parsed.deployment, "my-deploy");
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
