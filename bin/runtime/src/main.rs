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

mod cli;
mod commands;
mod config;
mod util;

use anyhow::{Context, Result};
use clap::Parser;
use std::io::IsTerminal;

use cli::{Cli, Commands};
use zlayer_observability::{
    init_observability, LogFormat, LogLevel, LoggingConfig, ObservabilityConfig,
};

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
        Commands::Deploy { spec_path, dry_run } => {
            let path = util::discover_spec_path(spec_path.as_deref())?;
            commands::deploy::deploy(&cli, &path, *dry_run).await
        }
        #[cfg(feature = "join")]
        Commands::Join {
            token,
            spec_dir,
            service,
            replicas,
        } => {
            commands::join::join(
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
        } => commands::serve::serve(bind, jwt_secret.clone(), *no_swagger).await,
        Commands::Status => commands::lifecycle::status(&cli).await,
        Commands::Validate { spec_path } => {
            let path = util::discover_spec_path(spec_path.as_deref())?;
            commands::lifecycle::validate(&path).await
        }
        #[cfg(feature = "deploy")]
        Commands::Logs {
            deployment,
            service,
            lines,
            follow,
            instance,
        } => {
            commands::lifecycle::logs(deployment, service, *lines, *follow, instance.clone()).await
        }
        #[cfg(feature = "deploy")]
        Commands::Stop {
            deployment,
            service,
            force,
            timeout,
        } => commands::lifecycle::stop(deployment, service.clone(), *force, *timeout).await,
        #[cfg(feature = "deploy")]
        Commands::Up { spec_path } => {
            let path = util::discover_spec_path(spec_path.as_deref())?;
            commands::deploy::up(&cli, &path).await
        }
        #[cfg(feature = "deploy")]
        Commands::Down { deployment } => commands::deploy::down(deployment.clone()).await,
        Commands::Build {
            context,
            file,
            zimagefile,
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
            commands::build::handle_build(
                context.clone(),
                file.clone(),
                zimagefile.clone(),
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
        Commands::Runtimes => commands::build::handle_runtimes().await,
        Commands::Token(token_cmd) => commands::token::handle_token(token_cmd),
        Commands::Spec(spec_cmd) => commands::spec::handle_spec(spec_cmd).await,
        Commands::Export {
            image,
            output,
            gzip,
        } => commands::registry::handle_export(&cli, image, output, *gzip).await,
        Commands::Import { input, tag } => {
            commands::registry::handle_import(&cli, input, tag.clone()).await
        }
        Commands::Wasm(wasm_cmd) => commands::wasm::handle_wasm(&cli, wasm_cmd).await,
        Commands::Tunnel(tunnel_cmd) => commands::tunnel::handle_tunnel(&cli, tunnel_cmd).await,
        Commands::Manager(manager_cmd) => commands::manager::handle_manager(manager_cmd).await,
        Commands::Pull { image } => commands::registry::handle_pull(image).await,
        #[cfg(feature = "node")]
        Commands::Node(node_cmd) => commands::node::handle_node(node_cmd).await,
    }
}
