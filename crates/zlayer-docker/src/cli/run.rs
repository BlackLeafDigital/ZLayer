//! `docker run` command implementation.

use clap::Parser;

/// Arguments for `docker run`.
#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunArgs {
    /// Run container in background and print container ID
    #[clap(short, long)]
    pub detach: bool,

    /// Keep STDIN open even if not attached
    #[clap(short = 'i', long)]
    pub interactive: bool,

    /// Allocate a pseudo-TTY
    #[clap(short, long)]
    pub tty: bool,

    /// Publish a container's port(s) to the host
    #[clap(short = 'p', long = "publish")]
    pub ports: Vec<String>,

    /// Bind mount a volume
    #[clap(short, long = "volume")]
    pub volumes: Vec<String>,

    /// Set environment variables
    #[clap(short, long = "env")]
    pub env: Vec<String>,

    /// Read in a file of environment variables
    #[clap(long)]
    pub env_file: Vec<String>,

    /// Assign a name to the container
    #[clap(long)]
    pub name: Option<String>,

    /// Automatically remove the container when it exits
    #[clap(long)]
    pub rm: bool,

    /// Connect a container to a network
    #[clap(long)]
    pub network: Option<String>,

    /// Restart policy to apply when a container exits
    #[clap(long)]
    pub restart: Option<String>,

    /// Memory limit
    #[clap(short, long)]
    pub memory: Option<String>,

    /// Number of CPUs
    #[clap(long)]
    pub cpus: Option<String>,

    /// Overwrite the default ENTRYPOINT of the image
    #[clap(long)]
    pub entrypoint: Option<String>,

    /// Working directory inside the container
    #[clap(short = 'w', long)]
    pub workdir: Option<String>,

    /// Add Linux capabilities
    #[clap(long)]
    pub cap_add: Vec<String>,

    /// Drop Linux capabilities
    #[clap(long)]
    pub cap_drop: Vec<String>,

    /// Give extended privileges to this container
    #[clap(long)]
    pub privileged: bool,

    /// Username or UID
    #[clap(short, long)]
    pub user: Option<String>,

    /// Container host name
    #[clap(short = 'H', long)]
    pub hostname: Option<String>,

    /// Set meta data on a container
    #[clap(short, long = "label")]
    pub labels: Vec<String>,

    /// Run an init inside the container
    #[clap(long)]
    pub init: bool,

    /// Set the target platform
    #[clap(long)]
    pub platform: Option<String>,

    /// Image to run
    pub image: String,

    /// Command to run in the container
    #[clap(trailing_var_arg = true)]
    pub command: Vec<String>,
}

/// Handle the `docker run` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_run(args: RunArgs) -> anyhow::Result<()> {
    tracing::info!("docker run: image={}, detach={}", args.image, args.detach);
    println!("zlayer docker run: {}", args.image);
    if !args.command.is_empty() {
        println!("  command: {:?}", args.command);
    }
    anyhow::bail!("docker run is not yet implemented — use 'zlayer deploy' with a spec file")
}
