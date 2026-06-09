//! Docker CLI compatibility layer.
//!
//! This module defines a [`clap`] [`DockerCommands`] enum that mirrors Docker CLI subcommands
//! and a [`handle_docker_command`] dispatcher that routes each command to the appropriate handler.

pub mod compose_cmd;
pub mod config;
pub mod container;
pub mod cp;
pub mod daemon_reconfig;
pub mod env_profile;
#[cfg(windows)]
pub mod env_profile_windows;
pub mod image;
pub mod image_extras;
pub mod install;
pub mod network;
pub mod node;
pub mod pause;
pub mod port;
pub mod prune;
pub mod rename;
pub mod run;
pub mod secret;
pub mod service;
pub mod sock_symlink;
pub mod swarm;
pub mod system;
pub mod top;
pub mod unpause;
pub mod update;
pub mod volume;

use clap::Subcommand;

// Re-export the `run` arg structs at the `cli` module root so external
// crates (e.g. `bin/zlayer`) can name `zlayer_docker::cli::RunArgs` when
// constructing a container-run request for the top-level `zlayer run`.
pub use run::{
    RunArgs, RunLifecycleArgs, RunMiscArgs, RunNetworkArgs, RunResourcesArgs, RunRuntimeArgs,
    RunSecurityArgs, RunVolumesArgs,
};

/// Docker-compatible CLI commands.
#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum DockerCommands {
    /// Create and run a new container
    Run(Box<run::RunArgs>),

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

    /// Rename a container
    Rename(rename::RenameArgs),

    /// Pause all processes within one or more containers
    Pause(pause::PauseArgs),

    /// Unpause all processes within one or more containers
    Unpause(unpause::UnpauseArgs),

    /// Update configuration of one or more containers
    Update(update::UpdateArgs),

    /// Display the running processes of a container
    Top(top::TopArgs),

    /// List port mappings or a specific mapping for the container
    Port(port::PortArgs),

    /// Remove all stopped containers
    #[clap(name = "container-prune")]
    Prune(prune::PruneArgs),

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
    Cp(cp::CpArgs),

    /// Log in to a registry
    Login(image::LoginArgs),

    /// Log out from a registry
    Logout(image::LogoutArgs),

    /// Show the history of an image
    History(image_extras::HistoryArgs),

    /// Search the registry for images
    Search(image_extras::SearchArgs),

    /// Save one or more images to a tar archive
    Save(image_extras::SaveArgs),

    /// Load images from a tar archive
    Load(image_extras::LoadArgs),

    /// Import a tar archive as a new image
    Import(image_extras::ImportArgs),

    /// Export a container's filesystem as a tar archive
    Export(image_extras::ExportArgs),

    /// Create a new image from a container's changes
    Commit(image_extras::CommitArgs),

    /// Display a live stream of container resource usage statistics
    Stats(container::StatsArgs),

    /// Show the Docker version information
    Version(system::VersionArgs),

    /// Display system-wide information
    Info(system::SystemInfoArgs),

    /// Get real time events from the server
    Events(system::SystemEventsArgs),

    /// Manage containers (alias group)
    #[clap(subcommand)]
    Container(ContainerSubcommand),

    /// Manage images (alias group)
    #[clap(subcommand)]
    Image(ImageSubcommand),

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

    /// Install the Docker compatibility shim: configures the daemon to
    /// expose the Docker Engine API socket, installs `docker` and
    /// `docker-compose` shims on PATH, and optionally symlinks
    /// `/var/run/docker.sock`.
    Install(install::InstallArgs),

    /// Remove the Docker compatibility shim installed by `install`.
    Uninstall(install::UninstallArgs),

    /// Manage a swarm — redirected to `ZLayer` native cluster commands
    Swarm {
        #[command(subcommand)]
        cmd: swarm::SwarmCommand,
    },

    /// Manage swarm nodes — redirected to `ZLayer` native cluster commands
    Node {
        #[command(subcommand)]
        cmd: node::NodeCommand,
    },

    /// Manage services — redirected to `zlayer deploy`
    Service {
        #[command(subcommand)]
        cmd: service::ServiceCommand,
    },

    /// Manage Docker secrets — redirected to `zlayer secret`
    Secret {
        #[command(subcommand)]
        cmd: secret::SecretCommand,
    },

    /// Manage Docker configs — redirected to `zlayer variable`
    Config {
        #[command(subcommand)]
        cmd: config::ConfigCommand,
    },
}

/// Subcommands of `docker container <verb>`.
///
/// Each variant is an alias for the matching top-level Docker container
/// subcommand. `docker container ls` is therefore identical to `docker ps`,
/// `docker container rm` identical to `docker rm`, etc.
#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum ContainerSubcommand {
    /// List containers (alias of `docker ps`)
    #[clap(alias = "list", alias = "ps")]
    Ls(container::PsArgs),
    /// Run a new container (alias of `docker run`)
    Run(Box<run::RunArgs>),
    /// Stop one or more running containers (alias of `docker stop`)
    Stop(container::StopArgs),
    /// Kill one or more running containers (alias of `docker kill`)
    Kill(container::KillArgs),
    /// Start one or more stopped containers (alias of `docker start`)
    Start(container::StartArgs),
    /// Restart one or more containers (alias of `docker restart`)
    Restart(container::RestartArgs),
    /// Remove one or more containers (alias of `docker rm`)
    Rm(container::RmArgs),
    /// Pause all processes within one or more containers
    Pause(pause::PauseArgs),
    /// Unpause all processes within one or more containers
    Unpause(unpause::UnpauseArgs),
    /// Update configuration of one or more containers
    Update(update::UpdateArgs),
    /// Display the running processes of a container
    Top(top::TopArgs),
    /// List port mappings or a specific mapping for the container
    Port(port::PortArgs),
    /// Rename a container
    Rename(rename::RenameArgs),
    /// Execute a command in a running container
    Exec(container::ExecArgs),
    /// Fetch the logs of a container
    Logs(container::LogsArgs),
    /// Inspect a container
    Inspect(container::InspectArgs),
    /// Live stream of container resource usage statistics
    Stats(container::StatsArgs),
    /// Copy files between a container and the local filesystem
    Cp(cp::CpArgs),
    /// Remove all stopped containers
    Prune(prune::PruneArgs),
    /// Export a container's filesystem as a tar archive
    Export(image_extras::ExportArgs),
    /// Create a new image from a container's changes
    Commit(image_extras::CommitArgs),
}

/// Subcommands of `docker image <verb>`.
///
/// Each variant is an alias for the matching top-level Docker image
/// subcommand. `docker image ls` is therefore identical to `docker images`,
/// `docker image rm` identical to `docker rmi`, etc.
#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum ImageSubcommand {
    /// List images (alias of `docker images`)
    #[clap(alias = "list")]
    Ls(image::ImagesArgs),
    /// Pull an image (alias of `docker pull`)
    Pull(image::PullArgs),
    /// Push an image (alias of `docker push`)
    Push(image::PushArgs),
    /// Build an image from a Dockerfile (alias of `docker build`)
    Build(image::BuildArgs),
    /// Tag an image (alias of `docker tag`)
    Tag(image::TagArgs),
    /// Remove an image (alias of `docker rmi`)
    #[clap(alias = "remove")]
    Rm(image::RmiArgs),
    /// Show the history of an image
    History(image_extras::HistoryArgs),
    /// Search the registry for images
    Search(image_extras::SearchArgs),
    /// Save one or more images to a tar archive
    Save(image_extras::SaveArgs),
    /// Load images from a tar archive
    Load(image_extras::LoadArgs),
    /// Import a tar archive as a new image
    Import(image_extras::ImportArgs),
    /// Inspect an image
    Inspect(container::InspectArgs),
}

/// Dispatch a parsed [`DockerCommands`] to the appropriate handler.
///
/// # Errors
///
/// Returns an error from the individual command handler.
#[allow(clippy::too_many_lines)]
pub async fn handle_docker_command(cmd: DockerCommands) -> anyhow::Result<()> {
    match cmd {
        DockerCommands::Run(args) => run::handle_run(*args).await,
        DockerCommands::Ps(args) => container::handle_ps(args).await,
        DockerCommands::Stop(args) => container::handle_stop(args).await,
        DockerCommands::Kill(args) => container::handle_kill(args).await,
        DockerCommands::Rm(args) => container::handle_rm(args).await,
        DockerCommands::Start(args) => container::handle_start(args).await,
        DockerCommands::Restart(args) => container::handle_restart(args).await,
        DockerCommands::Rename(args) => rename::handle_rename(args).await,
        DockerCommands::Pause(args) => pause::handle_pause(args).await,
        DockerCommands::Unpause(args) => unpause::handle_unpause(args).await,
        DockerCommands::Update(args) => update::handle_update(args).await,
        DockerCommands::Top(args) => top::handle_top(args).await,
        DockerCommands::Port(args) => port::handle_port(args).await,
        DockerCommands::Prune(args) => prune::handle_prune(args).await,
        DockerCommands::Exec(args) => container::handle_exec(args).await,
        DockerCommands::Logs(args) => container::handle_logs(args).await,
        DockerCommands::Build(args) => image::handle_build(args).await,
        DockerCommands::Pull(args) => image::handle_pull(args).await,
        DockerCommands::Push(args) => image::handle_push(args).await,
        DockerCommands::Images(args) => image::handle_images(args).await,
        DockerCommands::Rmi(args) => image::handle_rmi(args).await,
        DockerCommands::Tag(args) => image::handle_tag(args).await,
        DockerCommands::Inspect(args) => container::handle_inspect(args).await,
        DockerCommands::Cp(args) => cp::handle_cp(args).await,
        DockerCommands::Login(args) => image::handle_login(args).await,
        DockerCommands::Logout(args) => image::handle_logout(args).await,
        DockerCommands::History(args) => image_extras::handle_history(args).await,
        DockerCommands::Search(args) => image_extras::handle_search(args).await,
        DockerCommands::Save(args) => image_extras::handle_save(args).await,
        DockerCommands::Load(args) => image_extras::handle_load(args).await,
        DockerCommands::Import(args) => image_extras::handle_import(args).await,
        DockerCommands::Export(args) => image_extras::handle_export(args).await,
        DockerCommands::Commit(args) => image_extras::handle_commit(args).await,
        DockerCommands::Stats(args) => container::handle_stats(args).await,
        DockerCommands::Version(args) => system::handle_version(args).await,
        DockerCommands::Info(args) => system::handle_info(args).await,
        DockerCommands::Events(args) => system::handle_events(args).await,
        DockerCommands::Container(sub) => handle_container_group(sub).await,
        DockerCommands::Image(sub) => handle_image_group(sub).await,
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
        DockerCommands::Install(args) => install::handle_install(args).await,
        DockerCommands::Uninstall(args) => install::handle_uninstall(args).await,
        DockerCommands::Swarm { cmd } => swarm::handle(&cmd),
        DockerCommands::Node { cmd } => node::handle(&cmd),
        DockerCommands::Service { cmd } => service::handle(&cmd),
        DockerCommands::Secret { cmd } => secret::handle(&cmd),
        DockerCommands::Config { cmd } => config::handle(&cmd),
    }
}

/// Dispatch `docker container <verb>` to the matching top-level handler.
async fn handle_container_group(sub: ContainerSubcommand) -> anyhow::Result<()> {
    match sub {
        ContainerSubcommand::Ls(args) => container::handle_ps(args).await,
        ContainerSubcommand::Run(args) => run::handle_run(*args).await,
        ContainerSubcommand::Stop(args) => container::handle_stop(args).await,
        ContainerSubcommand::Kill(args) => container::handle_kill(args).await,
        ContainerSubcommand::Start(args) => container::handle_start(args).await,
        ContainerSubcommand::Restart(args) => container::handle_restart(args).await,
        ContainerSubcommand::Rm(args) => container::handle_rm(args).await,
        ContainerSubcommand::Pause(args) => pause::handle_pause(args).await,
        ContainerSubcommand::Unpause(args) => unpause::handle_unpause(args).await,
        ContainerSubcommand::Update(args) => update::handle_update(args).await,
        ContainerSubcommand::Top(args) => top::handle_top(args).await,
        ContainerSubcommand::Port(args) => port::handle_port(args).await,
        ContainerSubcommand::Rename(args) => rename::handle_rename(args).await,
        ContainerSubcommand::Exec(args) => container::handle_exec(args).await,
        ContainerSubcommand::Logs(args) => container::handle_logs(args).await,
        ContainerSubcommand::Inspect(args) => container::handle_inspect(args).await,
        ContainerSubcommand::Stats(args) => container::handle_stats(args).await,
        ContainerSubcommand::Cp(args) => cp::handle_cp(args).await,
        ContainerSubcommand::Prune(args) => prune::handle_prune(args).await,
        ContainerSubcommand::Export(args) => image_extras::handle_export(args).await,
        ContainerSubcommand::Commit(args) => image_extras::handle_commit(args).await,
    }
}

/// Dispatch `docker image <verb>` to the matching top-level handler.
async fn handle_image_group(sub: ImageSubcommand) -> anyhow::Result<()> {
    match sub {
        ImageSubcommand::Ls(args) => image::handle_images(args).await,
        ImageSubcommand::Pull(args) => image::handle_pull(args).await,
        ImageSubcommand::Push(args) => image::handle_push(args).await,
        ImageSubcommand::Build(args) => image::handle_build(args).await,
        ImageSubcommand::Tag(args) => image::handle_tag(args).await,
        ImageSubcommand::Rm(args) => image::handle_rmi(args).await,
        ImageSubcommand::History(args) => image_extras::handle_history(args).await,
        ImageSubcommand::Search(args) => image_extras::handle_search(args).await,
        ImageSubcommand::Save(args) => image_extras::handle_save(args).await,
        ImageSubcommand::Load(args) => image_extras::handle_load(args).await,
        ImageSubcommand::Import(args) => image_extras::handle_import(args).await,
        ImageSubcommand::Inspect(args) => container::handle_inspect(args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Wrapper used to invoke the subcommand parser in tests, mirroring how
    /// the main `zlayer` CLI nests `DockerCommands` under a subcommand.
    #[derive(Debug, Parser)]
    struct TestCli {
        #[clap(subcommand)]
        cmd: DockerCommands,
    }

    fn parse_ok(argv: &[&str]) -> DockerCommands {
        TestCli::try_parse_from(argv)
            .expect("CLI should parse successfully")
            .cmd
    }

    #[test]
    fn parses_top_level_version() {
        let cmd = parse_ok(&["zlayer-docker", "version"]);
        assert!(matches!(cmd, DockerCommands::Version(_)));
    }

    #[test]
    fn parses_top_level_info() {
        let cmd = parse_ok(&["zlayer-docker", "info", "--format", "json"]);
        match cmd {
            DockerCommands::Info(args) => assert_eq!(args.format.as_deref(), Some("json")),
            other => panic!("expected Info, got {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_events() {
        let cmd = parse_ok(&["zlayer-docker", "events", "--since", "1h"]);
        match cmd {
            DockerCommands::Events(args) => assert_eq!(args.since.as_deref(), Some("1h")),
            other => panic!("expected Events, got {other:?}"),
        }
    }

    #[test]
    fn parses_container_group_ls_alias() {
        // `docker container ls` should hit the Ls variant (alias of ps).
        let cmd = parse_ok(&["zlayer-docker", "container", "ls"]);
        match cmd {
            DockerCommands::Container(ContainerSubcommand::Ls(_)) => {}
            other => panic!("expected Container::Ls, got {other:?}"),
        }
        // `docker container ps` is the same alias.
        let cmd = parse_ok(&["zlayer-docker", "container", "ps"]);
        match cmd {
            DockerCommands::Container(ContainerSubcommand::Ls(_)) => {}
            other => panic!("expected Container::Ls (via ps alias), got {other:?}"),
        }
    }

    #[test]
    fn parses_container_group_rm() {
        let cmd = parse_ok(&["zlayer-docker", "container", "rm", "foo"]);
        match cmd {
            DockerCommands::Container(ContainerSubcommand::Rm(args)) => {
                assert_eq!(args.containers, vec!["foo"]);
            }
            other => panic!("expected Container::Rm, got {other:?}"),
        }
    }

    #[test]
    fn parses_image_group_ls_alias() {
        let cmd = parse_ok(&["zlayer-docker", "image", "ls"]);
        match cmd {
            DockerCommands::Image(ImageSubcommand::Ls(_)) => {}
            other => panic!("expected Image::Ls, got {other:?}"),
        }
    }

    #[test]
    fn parses_image_group_rm() {
        let cmd = parse_ok(&["zlayer-docker", "image", "rm", "nginx:latest"]);
        match cmd {
            DockerCommands::Image(ImageSubcommand::Rm(args)) => {
                assert_eq!(args.images, vec!["nginx:latest"]);
            }
            other => panic!("expected Image::Rm, got {other:?}"),
        }
    }

    #[test]
    fn parses_volume_group_ls() {
        let cmd = parse_ok(&["zlayer-docker", "volume", "ls"]);
        match cmd {
            DockerCommands::Volume(volume::VolumeSubcommand::Ls(_)) => {}
            other => panic!("expected Volume::Ls, got {other:?}"),
        }
    }

    #[test]
    fn parses_network_group_ls() {
        let cmd = parse_ok(&["zlayer-docker", "network", "ls"]);
        match cmd {
            DockerCommands::Network(network::NetworkSubcommand::Ls(_)) => {}
            other => panic!("expected Network::Ls, got {other:?}"),
        }
    }

    #[test]
    fn parses_system_prune() {
        let cmd = parse_ok(&["zlayer-docker", "system", "prune", "--force", "--all"]);
        match cmd {
            DockerCommands::System(system::SystemSubcommand::Prune(args)) => {
                assert!(args.force);
                assert!(args.all);
            }
            other => panic!("expected System::Prune, got {other:?}"),
        }
    }
}
