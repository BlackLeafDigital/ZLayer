//! Volume management commands.

use clap::{Parser, Subcommand};

/// Volume management commands.
#[derive(Debug, Parser)]
pub struct VolumeCommands {
    /// Volume subcommand
    #[clap(subcommand)]
    pub command: VolumeSubcommand,
}

/// Volume subcommands.
#[derive(Debug, Subcommand)]
pub enum VolumeSubcommand {
    /// Create a volume
    Create(VolumeCreateArgs),
    /// Display detailed information on one or more volumes
    Inspect(VolumeInspectArgs),
    /// List volumes
    Ls(VolumeLsArgs),
    /// Remove one or more volumes
    Rm(VolumeRmArgs),
    /// Remove unused local volumes
    Prune(VolumePruneArgs),
}

/// Arguments for `docker volume create`.
#[derive(Debug, Parser)]
pub struct VolumeCreateArgs {
    /// Volume name
    pub name: Option<String>,

    /// Specify volume driver name
    #[clap(short, long, default_value = "local")]
    pub driver: String,

    /// Set metadata for a volume
    #[clap(long)]
    pub label: Vec<String>,

    /// Set driver specific options
    #[clap(short, long = "opt")]
    pub options: Vec<String>,
}

/// Arguments for `docker volume inspect`.
#[derive(Debug, Parser)]
pub struct VolumeInspectArgs {
    /// Volume name(s)
    pub volumes: Vec<String>,

    /// Format the output
    #[clap(short, long)]
    pub format: Option<String>,
}

/// Arguments for `docker volume ls`.
#[derive(Debug, Parser)]
pub struct VolumeLsArgs {
    /// Only display volume names
    #[clap(short, long)]
    pub quiet: bool,

    /// Provide filter values
    #[clap(short, long)]
    pub filter: Vec<String>,
}

/// Arguments for `docker volume rm`.
#[derive(Debug, Parser)]
pub struct VolumeRmArgs {
    /// Volume name(s)
    pub volumes: Vec<String>,

    /// Force the removal
    #[clap(short, long)]
    pub force: bool,
}

/// Arguments for `docker volume prune`.
#[derive(Debug, Parser)]
pub struct VolumePruneArgs {
    /// Do not prompt for confirmation
    #[clap(short, long)]
    pub force: bool,

    /// Remove all unused volumes, not just anonymous ones
    #[clap(short, long)]
    pub all: bool,
}

/// Handle the `docker volume` subcommand.
///
/// # Errors
///
/// Currently infallible; will return errors once implemented.
#[allow(clippy::unused_async)]
pub async fn handle_volume(cmd: VolumeCommands) -> anyhow::Result<()> {
    match cmd.command {
        VolumeSubcommand::Create(args) => {
            tracing::info!("docker volume create: name={:?}", args.name);
            println!("TODO: docker volume create");
        }
        VolumeSubcommand::Inspect(args) => {
            tracing::info!("docker volume inspect: volumes={:?}", args.volumes);
            println!("TODO: docker volume inspect");
        }
        VolumeSubcommand::Ls(args) => {
            tracing::info!("docker volume ls: quiet={}", args.quiet);
            println!("TODO: docker volume ls");
        }
        VolumeSubcommand::Rm(args) => {
            tracing::info!("docker volume rm: volumes={:?}", args.volumes);
            println!("TODO: docker volume rm");
        }
        VolumeSubcommand::Prune(args) => {
            tracing::info!("docker volume prune: force={}", args.force);
            println!("TODO: docker volume prune");
        }
    }
    Ok(())
}
