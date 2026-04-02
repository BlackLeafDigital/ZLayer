//! System management commands.

use clap::{Parser, Subcommand};

/// System management commands.
#[derive(Debug, Parser)]
pub struct SystemCommands {
    /// System subcommand
    #[clap(subcommand)]
    pub command: SystemSubcommand,
}

/// System subcommands.
#[derive(Debug, Subcommand)]
pub enum SystemSubcommand {
    /// Show Docker disk usage
    Df(SystemDfArgs),
    /// Get real time events from the server
    Events(SystemEventsArgs),
    /// Display system-wide information
    Info(SystemInfoArgs),
    /// Remove unused data
    Prune(SystemPruneArgs),
}

/// Arguments for `docker system df`.
#[derive(Debug, Parser)]
pub struct SystemDfArgs {
    /// Show detailed information on space usage
    #[clap(short, long)]
    pub verbose: bool,
}

/// Arguments for `docker system events`.
#[derive(Debug, Parser)]
pub struct SystemEventsArgs {
    /// Show events created since this timestamp
    #[clap(long)]
    pub since: Option<String>,

    /// Stream events until this timestamp
    #[clap(long)]
    pub until: Option<String>,

    /// Filter output based on conditions provided
    #[clap(short, long)]
    pub filter: Vec<String>,

    /// Format the output
    #[clap(long)]
    pub format: Option<String>,
}

/// Arguments for `docker system info`.
#[derive(Debug, Parser)]
pub struct SystemInfoArgs {
    /// Format the output
    #[clap(short, long)]
    pub format: Option<String>,
}

/// Arguments for `docker system prune`.
#[derive(Debug, Parser)]
pub struct SystemPruneArgs {
    /// Do not prompt for confirmation
    #[clap(short, long)]
    pub force: bool,

    /// Remove all unused images not just dangling ones
    #[clap(short, long)]
    pub all: bool,

    /// Prune volumes
    #[clap(long)]
    pub volumes: bool,
}

/// Handle the `docker system` subcommand.
///
/// # Errors
///
/// Currently infallible; will return errors once implemented.
#[allow(clippy::unused_async)]
pub async fn handle_system(cmd: SystemCommands) -> anyhow::Result<()> {
    match cmd.command {
        SystemSubcommand::Df(args) => {
            tracing::info!("docker system df: verbose={}", args.verbose);
            println!("TODO: docker system df");
        }
        SystemSubcommand::Events(args) => {
            tracing::info!("docker system events: since={:?}", args.since);
            println!("TODO: docker system events");
        }
        SystemSubcommand::Info(args) => {
            tracing::info!("docker system info: format={:?}", args.format);
            println!("TODO: docker system info");
        }
        SystemSubcommand::Prune(args) => {
            tracing::info!(
                "docker system prune: force={}, all={}",
                args.force,
                args.all
            );
            println!("TODO: docker system prune");
        }
    }
    Ok(())
}
