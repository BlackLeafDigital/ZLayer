//! ZLayer Developer Control Tool
//!
//! CLI for development and debugging of ZLayer deployments.
//!
//! # Usage
//!
//! ```bash
//! # Create a JWT token for API access
//! devctl token create --subject dev --hours 24 --roles admin
//!
//! # Decode and inspect a token
//! devctl token decode <token>
//!
//! # Validate a specification file
//! devctl validate ./deployment.yaml
//!
//! # Dump parsed spec as JSON
//! devctl dump-spec ./deployment.yaml --format json
//!
//! # Run a deployment locally for testing
//! devctl run spec.yaml --dry-run
//!
//! # Generate a join token
//! devctl generate join-token -d my-deploy -a http://localhost:8080
//! ```

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::time::Duration;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use api::create_token;

#[derive(Parser)]
#[command(name = "devctl")]
#[command(about = "ZLayer developer control tool")]
#[command(version)]
#[command(author = "ZLayer Team")]
struct Cli {
    /// Increase verbosity (-v for info, -vv for debug, -vvv for trace)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Token management commands
    Token {
        #[command(subcommand)]
        action: TokenCommands,
    },

    /// Validate a specification file
    Validate {
        /// Path to spec file
        spec: String,
    },

    /// Inspect a running deployment
    Inspect {
        /// Deployment name
        deployment: String,

        /// Output format (json, yaml, table)
        #[arg(short, long, default_value = "table")]
        format: String,
    },

    /// Dump the parsed specification
    DumpSpec {
        /// Path to spec file
        spec: String,

        /// Output format (json, yaml)
        #[arg(short, long, default_value = "yaml")]
        format: String,
    },

    /// Run a deployment locally for testing
    Run {
        /// Path to spec file
        spec: String,

        /// Only validate, don't actually run
        #[arg(long)]
        dry_run: bool,

        /// Port offset for services (for running multiple deployments)
        #[arg(long, default_value = "0")]
        port_offset: u16,

        /// Environment (dev, staging, prod)
        #[arg(short, long, default_value = "dev")]
        env: String,
    },

    /// Generate tokens and configuration
    Generate {
        #[command(subcommand)]
        what: GenerateCommands,
    },
}

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

#[derive(Subcommand)]
enum GenerateCommands {
    /// Generate a join token for a deployment
    JoinToken {
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

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Setup logging based on verbosity
    let filter = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter)),
        )
        .without_time()
        .init();

    match cli.command {
        Commands::Token { action } => handle_token(action),
        Commands::Validate { spec } => handle_validate(&spec),
        Commands::Inspect { deployment, format } => handle_inspect(&deployment, &format),
        Commands::DumpSpec { spec, format } => handle_dump_spec(&spec, &format),
        Commands::Run {
            spec,
            dry_run,
            port_offset,
            env,
        } => handle_run(&spec, dry_run, port_offset, &env),
        Commands::Generate { what } => handle_generate(what),
    }
}

fn handle_token(action: TokenCommands) -> Result<()> {
    match action {
        TokenCommands::Create {
            subject,
            secret,
            hours,
            roles,
            quiet,
        } => {
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
                println!("\nExport as environment variable:");
                println!("  export ZLAYER_TOKEN=\"{}\"", token);
            }

            Ok(())
        }

        TokenCommands::Decode { token, json } => {
            // JWT format: header.payload.signature
            let parts: Vec<&str> = token.split('.').collect();
            if parts.len() != 3 {
                anyhow::bail!("Invalid JWT format: expected 3 parts separated by dots");
            }

            // Decode header
            let header = decode_base64_json(parts[0]).context("Failed to decode token header")?;

            // Decode payload
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

                // Check expiration
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

                // Show subject
                if let Some(sub) = claims.get("sub").and_then(|v| v.as_str()) {
                    println!("Subject: {}", sub);
                }

                // Show roles
                if let Some(roles) = claims.get("roles").and_then(|v| v.as_array()) {
                    let role_strs: Vec<&str> = roles
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect();
                    println!("Roles: {}", role_strs.join(", "));
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
            println!();

            println!("Creating Tokens:");
            println!("  devctl token create --subject myuser --hours 48 --roles admin,operator");
            println!("  devctl token create --quiet  # Output only the token");
            println!();

            println!("Decoding Tokens:");
            println!("  devctl token decode <token>");
            println!("  devctl token decode <token> --json  # Machine-readable output");

            Ok(())
        }
    }
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

fn handle_validate(spec_path: &str) -> Result<()> {
    info!(path = spec_path, "Validating specification");

    let content =
        std::fs::read_to_string(spec_path).context("Failed to read specification file")?;

    // Parse and validate using the spec crate
    let spec = spec::from_yaml_str(&content).context("Specification validation failed")?;

    println!("[OK] Specification is valid!\n");
    println!("Deployment: {}", spec.deployment);
    println!("Version: {}", spec.version);
    println!("Services: {}", spec.services.len());

    for (name, svc) in &spec.services {
        let endpoint_count = svc.endpoints.len();
        let scale_info = match &svc.scale {
            spec::ScaleSpec::Fixed { replicas } => format!("fixed ({})", replicas),
            spec::ScaleSpec::Adaptive { min, max, .. } => format!("adaptive ({}-{})", min, max),
            spec::ScaleSpec::Manual => "manual".to_string(),
        };

        println!(
            "  - {} (image: {}, endpoints: {}, scale: {})",
            name, svc.image.name, endpoint_count, scale_info
        );
    }

    Ok(())
}

fn handle_inspect(deployment: &str, format: &str) -> Result<()> {
    info!(
        deployment = deployment,
        format = format,
        "Inspecting deployment"
    );

    // TODO: Connect to API and fetch deployment info
    // For now, print a placeholder message
    println!("Inspecting deployment: {}", deployment);
    println!("Output format: {}", format);
    println!();
    println!("[Not yet implemented - requires running API server]");
    println!();
    println!("To inspect a deployment, ensure the API server is running and try:");
    println!(
        "  curl -H 'Authorization: Bearer $ZLAYER_TOKEN' http://localhost:8080/api/v1/deployments/{}",
        deployment
    );

    Ok(())
}

fn handle_dump_spec(spec_path: &str, format: &str) -> Result<()> {
    info!(path = spec_path, format = format, "Dumping specification");

    let content =
        std::fs::read_to_string(spec_path).context("Failed to read specification file")?;

    // Parse using the spec crate
    let spec = spec::from_yaml_str(&content).context("Failed to parse specification")?;

    match format.to_lowercase().as_str() {
        "json" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&spec).context("Failed to serialize as JSON")?
            );
        }
        "yaml" | _ => {
            println!(
                "{}",
                serde_yaml::to_string(&spec).context("Failed to serialize as YAML")?
            );
        }
    }

    Ok(())
}

fn handle_run(spec_path: &str, dry_run: bool, port_offset: u16, env: &str) -> Result<()> {
    println!("ZLayer Local Runner");
    println!("==================\n");

    // Load and validate spec
    let content =
        std::fs::read_to_string(spec_path).context("Failed to read spec file")?;

    let spec = spec::from_yaml_str(&content).context("Specification validation failed")?;

    println!("Deployment: {}", spec.deployment);
    println!("Environment: {}", env);
    println!("Port offset: {}", port_offset);
    println!("Dry run: {}", dry_run);
    println!("\nServices to run:");

    for (name, svc) in &spec.services {
        println!("\n  {}:", name);
        println!("    Image: {}", svc.image.name);
        println!("    Type: {:?}", svc.rtype);

        // Show endpoints with port offset
        for ep in &svc.endpoints {
            let port = ep.port + port_offset;
            println!(
                "    Endpoint: {} ({}:{}, {})",
                ep.name,
                format!("{:?}", ep.protocol).to_lowercase(),
                port,
                match ep.expose {
                    spec::ExposeType::Public => "public",
                    spec::ExposeType::Internal => "internal",
                }
            );
        }

        // Show scale config
        match &svc.scale {
            spec::ScaleSpec::Fixed { replicas } => {
                println!("    Scale: fixed at {} replicas", replicas);
            }
            spec::ScaleSpec::Adaptive { min, max, .. } => {
                println!("    Scale: adaptive ({}-{} replicas)", min, max);
            }
            spec::ScaleSpec::Manual => {
                println!("    Scale: manual");
            }
        }

        // Show environment variables
        if !svc.env.is_empty() {
            println!("    Environment:");
            for (k, v) in &svc.env {
                // Mask potential secrets
                let display_value = if k.to_lowercase().contains("secret")
                    || k.to_lowercase().contains("password")
                    || k.to_lowercase().contains("key")
                {
                    "***".to_string()
                } else {
                    v.clone()
                };
                println!("      {}={}", k, display_value);
            }
        }
    }

    if dry_run {
        println!("\n[Dry run - no containers started]");
        return Ok(());
    }

    println!("\n[Local runner not yet fully implemented]");
    println!("To run containers manually:");

    for (name, svc) in &spec.services {
        let mut cmd = format!("docker run -d --name {}-{}", spec.deployment, name);

        // Add port mappings
        for ep in &svc.endpoints {
            let host_port = ep.port + port_offset;
            cmd.push_str(&format!(" -p {}:{}", host_port, ep.port));
        }

        // Add environment variables
        for (k, v) in &svc.env {
            cmd.push_str(&format!(" -e {}={}", k, v));
        }

        cmd.push_str(&format!(" {}", svc.image.name));

        println!("\n  {}", cmd);
    }

    Ok(())
}

fn handle_generate(what: GenerateCommands) -> Result<()> {
    match what {
        GenerateCommands::JoinToken {
            deployment,
            api,
            service,
        } => {
            use base64::Engine;

            let token_data = serde_json::json!({
                "deployment": deployment,
                "api_endpoint": api,
                "service": service,
            });

            let json = serde_json::to_string(&token_data)?;
            let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&json);

            println!("Join Token Generated");
            println!("===================\n");
            println!("Deployment: {}", deployment);
            println!("API: {}", api);
            if let Some(svc) = &service {
                println!("Service: {}", svc);
            }
            println!("\nToken:");
            println!("{}", token);
            println!("\nUsage:");
            println!("  zlayer join {}", token);
            if service.is_some() {
                println!("  # Service is pre-configured in token");
            } else {
                println!("  zlayer join {} --service <SERVICE_NAME>", token);
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
        // Test that CLI parses correctly
        let cli = Cli::try_parse_from(["devctl", "token", "info"]).unwrap();
        assert_eq!(cli.verbose, 0);
    }

    #[test]
    fn test_cli_verbose() {
        let cli = Cli::try_parse_from(["devctl", "-vv", "token", "info"]).unwrap();
        assert_eq!(cli.verbose, 2);
    }

    #[test]
    fn test_cli_verbose_triple() {
        let cli = Cli::try_parse_from(["devctl", "-vvv", "token", "info"]).unwrap();
        assert_eq!(cli.verbose, 3);
    }

    #[test]
    fn test_token_create_args() {
        let cli = Cli::try_parse_from([
            "devctl",
            "token",
            "create",
            "--subject",
            "testuser",
            "--hours",
            "48",
            "--roles",
            "admin,operator",
        ])
        .unwrap();

        match cli.command {
            Commands::Token {
                action: TokenCommands::Create { subject, hours, roles, .. },
            } => {
                assert_eq!(subject, "testuser");
                assert_eq!(hours, 48);
                assert_eq!(roles, "admin,operator");
            }
            _ => panic!("Expected Token Create command"),
        }
    }

    #[test]
    fn test_validate_args() {
        let cli = Cli::try_parse_from(["devctl", "validate", "./test.yaml"]).unwrap();

        match cli.command {
            Commands::Validate { spec } => {
                assert_eq!(spec, "./test.yaml");
            }
            _ => panic!("Expected Validate command"),
        }
    }

    #[test]
    fn test_dump_spec_args() {
        let cli =
            Cli::try_parse_from(["devctl", "dump-spec", "./test.yaml", "--format", "json"]).unwrap();

        match cli.command {
            Commands::DumpSpec { spec, format } => {
                assert_eq!(spec, "./test.yaml");
                assert_eq!(format, "json");
            }
            _ => panic!("Expected DumpSpec command"),
        }
    }

    #[test]
    fn test_decode_base64_json() {
        // Test decoding a simple base64url-encoded JSON
        // {"sub":"test","exp":9999999999}
        let encoded = "eyJzdWIiOiJ0ZXN0IiwiZXhwIjo5OTk5OTk5OTk5fQ";
        let decoded = decode_base64_json(encoded).unwrap();
        assert_eq!(decoded["sub"], "test");
        assert_eq!(decoded["exp"], 9999999999_u64);
    }

    #[test]
    fn test_decode_base64_json_with_padding() {
        // Same content but with different padding scenarios
        let encoded = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        let decoded = decode_base64_json(encoded).unwrap();
        assert_eq!(decoded["alg"], "HS256");
        assert_eq!(decoded["typ"], "JWT");
    }

    #[test]
    fn test_cli_run_command() {
        let cli = Cli::try_parse_from(["devctl", "run", "spec.yaml"]).unwrap();
        match cli.command {
            Commands::Run {
                spec,
                dry_run,
                port_offset,
                env,
            } => {
                assert_eq!(spec, "spec.yaml");
                assert!(!dry_run);
                assert_eq!(port_offset, 0);
                assert_eq!(env, "dev");
            }
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_cli_run_command_with_options() {
        let cli = Cli::try_parse_from([
            "devctl",
            "run",
            "deployment.yaml",
            "--dry-run",
            "--port-offset",
            "1000",
            "--env",
            "staging",
        ])
        .unwrap();
        match cli.command {
            Commands::Run {
                spec,
                dry_run,
                port_offset,
                env,
            } => {
                assert_eq!(spec, "deployment.yaml");
                assert!(dry_run);
                assert_eq!(port_offset, 1000);
                assert_eq!(env, "staging");
            }
            _ => panic!("Expected Run command"),
        }
    }

    #[test]
    fn test_cli_generate_join_token() {
        let cli = Cli::try_parse_from([
            "devctl",
            "generate",
            "join-token",
            "-d",
            "my-deploy",
            "-a",
            "http://localhost:8080",
            "-s",
            "api",
        ])
        .unwrap();
        match cli.command {
            Commands::Generate {
                what: GenerateCommands::JoinToken {
                    deployment,
                    api,
                    service,
                },
            } => {
                assert_eq!(deployment, "my-deploy");
                assert_eq!(api, "http://localhost:8080");
                assert_eq!(service, Some("api".to_string()));
            }
            _ => panic!("Expected Generate JoinToken command"),
        }
    }

    #[test]
    fn test_cli_generate_join_token_without_service() {
        let cli = Cli::try_parse_from([
            "devctl",
            "generate",
            "join-token",
            "-d",
            "my-deploy",
            "-a",
            "http://localhost:8080",
        ])
        .unwrap();
        match cli.command {
            Commands::Generate {
                what: GenerateCommands::JoinToken {
                    deployment,
                    api,
                    service,
                },
            } => {
                assert_eq!(deployment, "my-deploy");
                assert_eq!(api, "http://localhost:8080");
                assert_eq!(service, None);
            }
            _ => panic!("Expected Generate JoinToken command"),
        }
    }
}
