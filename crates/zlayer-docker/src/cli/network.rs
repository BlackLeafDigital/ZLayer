//! Network management commands.

use clap::{Parser, Subcommand};

/// Network management commands.
#[derive(Debug, Parser)]
pub struct NetworkCommands {
    /// Network subcommand
    #[clap(subcommand)]
    pub command: NetworkSubcommand,
}

/// Network subcommands.
#[derive(Debug, Subcommand)]
pub enum NetworkSubcommand {
    /// Create a network
    Create(NetworkCreateArgs),
    /// Display detailed information on one or more networks
    Inspect(NetworkInspectArgs),
    /// List networks
    Ls(NetworkLsArgs),
    /// Remove one or more networks
    Rm(NetworkRmArgs),
    /// Remove all unused networks
    Prune(NetworkPruneArgs),
    /// Connect a container to a network
    Connect(NetworkConnectArgs),
    /// Disconnect a container from a network
    Disconnect(NetworkDisconnectArgs),
}

/// Arguments for `docker network create`.
#[derive(Debug, Parser)]
pub struct NetworkCreateArgs {
    /// Network name
    pub name: String,

    /// Driver to manage the network
    #[clap(short, long, default_value = "bridge")]
    pub driver: String,

    /// Subnet in CIDR format
    #[clap(long)]
    pub subnet: Option<String>,

    /// IPv4 or IPv6 gateway for the subnet
    #[clap(long)]
    pub gateway: Option<String>,

    /// Set metadata on a network
    #[clap(long)]
    pub label: Vec<String>,

    /// Enable internal mode (restrict external access)
    #[clap(long)]
    pub internal: bool,
}

/// Arguments for `docker network inspect`.
#[derive(Debug, Parser)]
pub struct NetworkInspectArgs {
    /// Network name(s) or ID(s)
    pub networks: Vec<String>,

    /// Format the output
    #[clap(short, long)]
    pub format: Option<String>,
}

/// Arguments for `docker network ls`.
#[derive(Debug, Parser)]
pub struct NetworkLsArgs {
    /// Only display network IDs
    #[clap(short, long)]
    pub quiet: bool,

    /// Provide filter values
    #[clap(short, long)]
    pub filter: Vec<String>,
}

/// Arguments for `docker network rm`.
#[derive(Debug, Parser)]
pub struct NetworkRmArgs {
    /// Network name(s) or ID(s)
    pub networks: Vec<String>,

    /// Force the removal
    #[clap(short, long)]
    pub force: bool,
}

/// Arguments for `docker network prune`.
#[derive(Debug, Parser)]
pub struct NetworkPruneArgs {
    /// Do not prompt for confirmation
    #[clap(short, long)]
    pub force: bool,
}

/// Arguments for `docker network connect`.
#[derive(Debug, Parser)]
pub struct NetworkConnectArgs {
    /// Network name or ID
    pub network: String,

    /// Container name or ID
    pub container: String,

    /// IPv4 address
    #[clap(long)]
    pub ip: Option<String>,

    /// Add network-scoped alias for the container
    #[clap(long)]
    pub alias: Vec<String>,
}

/// Arguments for `docker network disconnect`.
#[derive(Debug, Parser)]
pub struct NetworkDisconnectArgs {
    /// Network name or ID
    pub network: String,

    /// Container name or ID
    pub container: String,

    /// Force the container to disconnect
    #[clap(short, long)]
    pub force: bool,
}

/// Handle the `docker network` subcommand.
///
/// # Errors
///
/// Currently infallible; will return errors once implemented.
#[allow(clippy::unused_async)]
pub async fn handle_network(cmd: NetworkCommands) -> anyhow::Result<()> {
    match cmd.command {
        NetworkSubcommand::Create(args) => {
            tracing::info!("docker network create: name={}", args.name);
            println!("TODO: docker network create {}", args.name);
        }
        NetworkSubcommand::Inspect(args) => {
            tracing::info!("docker network inspect: networks={:?}", args.networks);
            println!("TODO: docker network inspect");
        }
        NetworkSubcommand::Ls(args) => {
            tracing::info!("docker network ls: quiet={}", args.quiet);
            println!("TODO: docker network ls");
        }
        NetworkSubcommand::Rm(args) => {
            tracing::info!("docker network rm: networks={:?}", args.networks);
            println!("TODO: docker network rm");
        }
        NetworkSubcommand::Prune(args) => {
            tracing::info!("docker network prune: force={}", args.force);
            println!("TODO: docker network prune");
        }
        NetworkSubcommand::Connect(args) => {
            tracing::info!(
                "docker network connect: network={}, container={}",
                args.network,
                args.container
            );
            println!("TODO: docker network connect");
        }
        NetworkSubcommand::Disconnect(args) => {
            tracing::info!(
                "docker network disconnect: network={}, container={}",
                args.network,
                args.container
            );
            println!("TODO: docker network disconnect");
        }
    }
    Ok(())
}
