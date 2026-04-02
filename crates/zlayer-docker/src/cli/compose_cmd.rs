//! Docker Compose command implementations.

use clap::{Parser, Subcommand};

/// Docker Compose commands.
#[derive(Debug, Parser)]
pub struct ComposeCommands {
    /// Compose subcommand
    #[clap(subcommand)]
    pub command: ComposeSubcommand,
}

/// Docker Compose subcommands.
#[derive(Debug, Subcommand)]
pub enum ComposeSubcommand {
    /// Create and start containers
    Up(ComposeUpArgs),
    /// Stop and remove containers, networks
    Down(ComposeDownArgs),
    /// Build or rebuild services
    Build(ComposeBuildArgs),
    /// List containers
    Ps(ComposePsArgs),
    /// View output from containers
    Logs(ComposeLogsArgs),
    /// Execute a command in a running service container
    Exec(ComposeExecArgs),
    /// Pull service images
    Pull(ComposePullArgs),
    /// Push service images
    Push(ComposePushArgs),
    /// Restart service containers
    Restart(ComposeRestartArgs),
    /// Stop services
    Stop(ComposeStopArgs),
    /// Start services
    Start(ComposeStartArgs),
}

/// Arguments for `docker compose up`.
#[derive(Debug, Parser)]
pub struct ComposeUpArgs {
    /// Services to start (default: all)
    pub services: Vec<String>,

    /// Detached mode
    #[clap(short, long)]
    pub detach: bool,

    /// Build images before starting containers
    #[clap(long)]
    pub build: bool,

    /// Force recreate containers
    #[clap(long)]
    pub force_recreate: bool,

    /// Compose file path
    #[clap(short, long)]
    pub file: Option<String>,
}

/// Arguments for `docker compose down`.
#[derive(Debug, Parser)]
pub struct ComposeDownArgs {
    /// Remove named volumes
    #[clap(short = 'v', long)]
    pub volumes: bool,

    /// Remove images (all or local)
    #[clap(long)]
    pub rmi: Option<String>,

    /// Compose file path
    #[clap(short, long)]
    pub file: Option<String>,
}

/// Arguments for `docker compose build`.
#[derive(Debug, Parser)]
pub struct ComposeBuildArgs {
    /// Services to build (default: all)
    pub services: Vec<String>,

    /// Do not use cache when building
    #[clap(long)]
    pub no_cache: bool,

    /// Always attempt to pull a newer version
    #[clap(long)]
    pub pull: bool,
}

/// Arguments for `docker compose ps`.
#[derive(Debug, Parser)]
pub struct ComposePsArgs {
    /// Services to list (default: all)
    pub services: Vec<String>,

    /// Only display IDs
    #[clap(short, long)]
    pub quiet: bool,
}

/// Arguments for `docker compose logs`.
#[derive(Debug, Parser)]
pub struct ComposeLogsArgs {
    /// Services to show logs for (default: all)
    pub services: Vec<String>,

    /// Follow log output
    #[clap(short, long)]
    pub follow: bool,

    /// Show timestamps
    #[clap(short, long)]
    pub timestamps: bool,

    /// Number of lines from the end
    #[clap(long)]
    pub tail: Option<String>,
}

/// Arguments for `docker compose exec`.
#[derive(Debug, Parser)]
pub struct ComposeExecArgs {
    /// Service name
    pub service: String,

    /// Command and arguments
    pub command: Vec<String>,

    /// Detached mode
    #[clap(short, long)]
    pub detach: bool,

    /// Allocate a pseudo-TTY
    #[clap(short = 'T', long = "no-tty")]
    pub no_tty: bool,
}

/// Arguments for `docker compose pull`.
#[derive(Debug, Parser)]
pub struct ComposePullArgs {
    /// Services to pull (default: all)
    pub services: Vec<String>,
}

/// Arguments for `docker compose push`.
#[derive(Debug, Parser)]
pub struct ComposePushArgs {
    /// Services to push (default: all)
    pub services: Vec<String>,
}

/// Arguments for `docker compose restart`.
#[derive(Debug, Parser)]
pub struct ComposeRestartArgs {
    /// Services to restart (default: all)
    pub services: Vec<String>,

    /// Seconds to wait before killing
    #[clap(short, long, default_value = "10")]
    pub timeout: u32,
}

/// Arguments for `docker compose stop`.
#[derive(Debug, Parser)]
pub struct ComposeStopArgs {
    /// Services to stop (default: all)
    pub services: Vec<String>,

    /// Seconds to wait before killing
    #[clap(short, long, default_value = "10")]
    pub timeout: u32,
}

/// Arguments for `docker compose start`.
#[derive(Debug, Parser)]
pub struct ComposeStartArgs {
    /// Services to start (default: all)
    pub services: Vec<String>,
}

/// Handle the `docker compose` subcommand.
///
/// # Errors
///
/// Currently infallible; will return errors once implemented.
#[allow(clippy::unused_async)]
pub async fn handle_compose(cmd: ComposeCommands) -> anyhow::Result<()> {
    match cmd.command {
        ComposeSubcommand::Up(args) => {
            tracing::info!("docker compose up: services={:?}", args.services);
            println!("TODO: docker compose up");
        }
        ComposeSubcommand::Down(args) => {
            tracing::info!("docker compose down: volumes={}", args.volumes);
            println!("TODO: docker compose down");
        }
        ComposeSubcommand::Build(args) => {
            tracing::info!("docker compose build: services={:?}", args.services);
            println!("TODO: docker compose build");
        }
        ComposeSubcommand::Ps(args) => {
            tracing::info!("docker compose ps: services={:?}", args.services);
            println!("TODO: docker compose ps");
        }
        ComposeSubcommand::Logs(args) => {
            tracing::info!("docker compose logs: services={:?}", args.services);
            println!("TODO: docker compose logs");
        }
        ComposeSubcommand::Exec(args) => {
            tracing::info!("docker compose exec: service={}", args.service);
            println!("TODO: docker compose exec");
        }
        ComposeSubcommand::Pull(args) => {
            tracing::info!("docker compose pull: services={:?}", args.services);
            println!("TODO: docker compose pull");
        }
        ComposeSubcommand::Push(args) => {
            tracing::info!("docker compose push: services={:?}", args.services);
            println!("TODO: docker compose push");
        }
        ComposeSubcommand::Restart(args) => {
            tracing::info!("docker compose restart: services={:?}", args.services);
            println!("TODO: docker compose restart");
        }
        ComposeSubcommand::Stop(args) => {
            tracing::info!("docker compose stop: services={:?}", args.services);
            println!("TODO: docker compose stop");
        }
        ComposeSubcommand::Start(args) => {
            tracing::info!("docker compose start: services={:?}", args.services);
            println!("TODO: docker compose start");
        }
    }
    Ok(())
}
