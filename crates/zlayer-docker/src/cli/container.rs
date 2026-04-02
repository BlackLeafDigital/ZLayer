//! Docker container management commands.

use clap::Parser;

/// Arguments for `docker ps`.
#[derive(Debug, Parser)]
pub struct PsArgs {
    /// Show all containers (default shows just running)
    #[clap(short, long)]
    pub all: bool,

    /// Only display container IDs
    #[clap(short, long)]
    pub quiet: bool,

    /// Filter output based on conditions provided
    #[clap(long)]
    pub filter: Vec<String>,

    /// Format output using a custom template
    #[clap(long)]
    pub format: Option<String>,
}

/// Arguments for `docker stop`.
#[derive(Debug, Parser)]
pub struct StopArgs {
    /// Container name(s) or ID(s)
    pub containers: Vec<String>,

    /// Seconds to wait before killing the container
    #[clap(short, long)]
    pub time: Option<u32>,
}

/// Arguments for `docker kill`.
#[derive(Debug, Parser)]
pub struct KillArgs {
    /// Container name(s) or ID(s)
    pub containers: Vec<String>,

    /// Signal to send to the container
    #[clap(short, long)]
    pub signal: Option<String>,
}

/// Arguments for `docker start`.
#[derive(Debug, Parser)]
pub struct StartArgs {
    /// Container name(s) or ID(s)
    pub containers: Vec<String>,
}

/// Arguments for `docker restart`.
#[derive(Debug, Parser)]
pub struct RestartArgs {
    /// Container name(s) or ID(s)
    pub containers: Vec<String>,

    /// Seconds to wait before killing the container
    #[clap(short, long)]
    pub time: Option<u32>,
}

/// Arguments for `docker rm`.
#[derive(Debug, Parser)]
pub struct RmArgs {
    /// Container name(s) or ID(s)
    pub containers: Vec<String>,

    /// Force the removal of a running container
    #[clap(short, long)]
    pub force: bool,

    /// Remove anonymous volumes associated with the container
    #[clap(short, long)]
    pub volumes: bool,
}

/// Arguments for `docker exec`.
#[derive(Debug, Parser)]
pub struct ExecArgs {
    /// Container name or ID
    pub container: String,

    /// Keep STDIN open even if not attached
    #[clap(short, long)]
    pub interactive: bool,

    /// Allocate a pseudo-TTY
    #[clap(short, long)]
    pub tty: bool,

    /// Username or UID
    #[clap(short, long)]
    pub user: Option<String>,

    /// Set environment variables
    #[clap(short, long = "env")]
    pub env: Vec<String>,

    /// Working directory inside the container
    #[clap(short, long)]
    pub workdir: Option<String>,

    /// Command and arguments to execute
    #[clap(trailing_var_arg = true)]
    pub command: Vec<String>,
}

/// Arguments for `docker logs`.
#[derive(Debug, Parser)]
pub struct LogsArgs {
    /// Container name or ID
    pub container: String,

    /// Follow log output
    #[clap(short, long)]
    pub follow: bool,

    /// Number of lines to show from the end of the logs
    #[clap(long)]
    pub tail: Option<String>,

    /// Show logs since timestamp or relative duration
    #[clap(long)]
    pub since: Option<String>,

    /// Show timestamps
    #[clap(long)]
    pub timestamps: bool,
}

/// Arguments for `docker inspect`.
#[derive(Debug, Parser)]
pub struct InspectArgs {
    /// Container or image name(s) or ID(s)
    pub names: Vec<String>,

    /// Format output using a custom template
    #[clap(long)]
    pub format: Option<String>,
}

/// Arguments for `docker cp`.
#[derive(Debug, Parser)]
pub struct CpArgs {
    /// Source path (container:path or local path)
    pub source: String,

    /// Destination path (container:path or local path)
    pub destination: String,
}

/// Arguments for `docker stats`.
#[derive(Debug, Parser)]
pub struct StatsArgs {
    /// Container name(s) or ID(s) (shows all if empty)
    pub containers: Vec<String>,

    /// Show all containers (default shows just running)
    #[clap(short, long)]
    pub all: bool,

    /// Disable streaming stats and only pull the first result
    #[clap(long)]
    pub no_stream: bool,
}

/// Handle the `docker ps` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_ps(args: PsArgs) -> anyhow::Result<()> {
    tracing::info!("docker ps: all={}, quiet={}", args.all, args.quiet);
    println!("zlayer docker ps");
    anyhow::bail!("docker ps is not yet implemented — use 'zlayer ps'")
}

/// Handle the `docker stop` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_stop(args: StopArgs) -> anyhow::Result<()> {
    tracing::info!("docker stop: containers={:?}", args.containers);
    println!("zlayer docker stop: {:?}", args.containers);
    anyhow::bail!("docker stop is not yet implemented — use 'zlayer stop'")
}

/// Handle the `docker kill` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_kill(args: KillArgs) -> anyhow::Result<()> {
    tracing::info!("docker kill: containers={:?}", args.containers);
    println!("zlayer docker kill: {:?}", args.containers);
    anyhow::bail!("docker kill is not yet implemented — use 'zlayer kill'")
}

/// Handle the `docker start` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_start(args: StartArgs) -> anyhow::Result<()> {
    tracing::info!("docker start: containers={:?}", args.containers);
    println!("zlayer docker start: {:?}", args.containers);
    anyhow::bail!("docker start is not yet implemented — use 'zlayer start'")
}

/// Handle the `docker restart` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_restart(args: RestartArgs) -> anyhow::Result<()> {
    tracing::info!("docker restart: containers={:?}", args.containers);
    println!("zlayer docker restart: {:?}", args.containers);
    anyhow::bail!("docker restart is not yet implemented — use 'zlayer restart'")
}

/// Handle the `docker rm` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_rm(args: RmArgs) -> anyhow::Result<()> {
    tracing::info!(
        "docker rm: containers={:?}, force={}",
        args.containers,
        args.force
    );
    println!("zlayer docker rm: {:?}", args.containers);
    anyhow::bail!("docker rm is not yet implemented — use 'zlayer rm'")
}

/// Handle the `docker exec` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_exec(args: ExecArgs) -> anyhow::Result<()> {
    tracing::info!("docker exec: container={}", args.container);
    println!("zlayer docker exec: {}", args.container);
    if !args.command.is_empty() {
        println!("  command: {:?}", args.command);
    }
    anyhow::bail!("docker exec is not yet implemented — use 'zlayer exec'")
}

/// Handle the `docker logs` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_logs(args: LogsArgs) -> anyhow::Result<()> {
    tracing::info!(
        "docker logs: container={}, follow={}",
        args.container,
        args.follow
    );
    println!("zlayer docker logs: {}", args.container);
    anyhow::bail!("docker logs is not yet implemented — use 'zlayer logs'")
}

/// Handle the `docker inspect` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_inspect(args: InspectArgs) -> anyhow::Result<()> {
    tracing::info!("docker inspect: names={:?}", args.names);
    println!("zlayer docker inspect: {:?}", args.names);
    anyhow::bail!("docker inspect is not yet implemented — use 'zlayer inspect'")
}

/// Handle the `docker cp` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_cp(args: CpArgs) -> anyhow::Result<()> {
    tracing::info!("docker cp: {} -> {}", args.source, args.destination);
    println!("zlayer docker cp: {} -> {}", args.source, args.destination);
    anyhow::bail!("docker cp is not yet implemented — use 'zlayer cp'")
}

/// Handle the `docker stats` command.
///
/// # Errors
///
/// Always returns an error; this command is not yet implemented.
#[allow(clippy::unused_async)]
pub async fn handle_stats(args: StatsArgs) -> anyhow::Result<()> {
    tracing::info!(
        "docker stats: containers={:?}, all={}",
        args.containers,
        args.all
    );
    println!("zlayer docker stats");
    anyhow::bail!("docker stats is not yet implemented — use 'zlayer stats'")
}
