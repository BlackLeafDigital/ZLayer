//! Docker CLI compatibility layer.
//!
//! This module defines a [`clap`] [`DockerCommands`] enum that mirrors Docker CLI subcommands
//! and a [`handle_docker_command`] dispatcher that routes each command to the appropriate handler.

pub mod compose_cmd;
pub mod container;
pub mod image;
pub mod network;
pub mod run;
pub mod system;
pub mod volume;

use clap::Subcommand;

/// Docker-compatible CLI commands.
#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum DockerCommands {
    /// Create and run a new container
    Run(run::RunArgs),

    /// List containers
    Ps(container::PsArgs),

    /// Stop one or more running containers
    Stop(container::StopArgs),

    /// Kill one or more running containers
    Kill(container::KillArgs),

    /// Remove one or more containers
    Rm(container::RmArgs),

    /// Start one or more stopped containers
    Start(container::StartArgs),

    /// Restart one or more containers
    Restart(container::RestartArgs),

    /// Execute a command in a running container
    Exec(container::ExecArgs),

    /// Fetch the logs of a container
    Logs(container::LogsArgs),

    /// Build an image from a Dockerfile
    Build(image::BuildArgs),

    /// Download an image from a registry
    Pull(image::PullArgs),

    /// Upload an image to a registry
    Push(image::PushArgs),

    /// List images
    Images(image::ImagesArgs),

    /// Remove one or more images
    Rmi(image::RmiArgs),

    /// Create a tag `TARGET_IMAGE` that refers to `SOURCE_IMAGE`
    Tag(image::TagArgs),

    /// Return low-level information on Docker objects
    Inspect(container::InspectArgs),

    /// Copy files/folders between a container and the local filesystem
    Cp(container::CpArgs),

    /// Log in to a registry
    Login(image::LoginArgs),

    /// Log out from a registry
    Logout(image::LogoutArgs),

    /// Display a live stream of container resource usage statistics
    Stats(container::StatsArgs),

    /// Manage volumes
    #[clap(subcommand)]
    Volume(volume::VolumeSubcommand),

    /// Manage networks
    #[clap(subcommand)]
    Network(network::NetworkSubcommand),

    /// Docker Compose commands
    Compose(compose_cmd::ComposeCommands),

    /// Manage Docker
    #[clap(subcommand)]
    System(system::SystemSubcommand),
}

/// Dispatch a parsed [`DockerCommands`] to the appropriate handler.
///
/// # Errors
///
/// Returns an error from the individual command handler.
pub async fn handle_docker_command(cmd: DockerCommands) -> anyhow::Result<()> {
    match cmd {
        DockerCommands::Run(args) => run::handle_run(args).await,
        DockerCommands::Ps(args) => container::handle_ps(args).await,
        DockerCommands::Stop(args) => container::handle_stop(args).await,
        DockerCommands::Kill(args) => container::handle_kill(args).await,
        DockerCommands::Rm(args) => container::handle_rm(args).await,
        DockerCommands::Start(args) => container::handle_start(args).await,
        DockerCommands::Restart(args) => container::handle_restart(args).await,
        DockerCommands::Exec(args) => container::handle_exec(args).await,
        DockerCommands::Logs(args) => container::handle_logs(args).await,
        DockerCommands::Build(args) => image::handle_build(args).await,
        DockerCommands::Pull(args) => image::handle_pull(args).await,
        DockerCommands::Push(args) => image::handle_push(args).await,
        DockerCommands::Images(args) => image::handle_images(args).await,
        DockerCommands::Rmi(args) => image::handle_rmi(args).await,
        DockerCommands::Tag(args) => image::handle_tag(args).await,
        DockerCommands::Inspect(args) => container::handle_inspect(args).await,
        DockerCommands::Cp(args) => container::handle_cp(args).await,
        DockerCommands::Login(args) => image::handle_login(args).await,
        DockerCommands::Logout(args) => image::handle_logout(args).await,
        DockerCommands::Stats(args) => container::handle_stats(args).await,
        DockerCommands::Volume(sub) => {
            volume::handle_volume(volume::VolumeCommands { command: sub }).await
        }
        DockerCommands::Network(sub) => {
            network::handle_network(network::NetworkCommands { command: sub }).await
        }
        DockerCommands::Compose(cmd) => compose_cmd::handle_compose(cmd).await,
        DockerCommands::System(sub) => {
            system::handle_system(system::SystemCommands { command: sub }).await
        }
    }
}
