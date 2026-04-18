//! Docker container management commands.

use anyhow::Context;
use clap::Parser;
use zlayer_client::{default_socket_path, DaemonClient};

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
/// Lists containers known to the daemon. When `--quiet` is set, emits only
/// short container IDs; otherwise prints a Docker-style table.
///
/// # Errors
///
/// Returns an error if the daemon cannot be reached or the response cannot
/// be parsed.
pub async fn handle_ps(args: PsArgs) -> anyhow::Result<()> {
    tracing::info!("docker ps: all={}, quiet={}", args.all, args.quiet);

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let containers = client
        .get_all_containers()
        .await
        .context("Failed to fetch containers from daemon")?;

    let Some(arr) = containers.as_array() else {
        if args.format.as_deref() == Some("json") {
            println!("{}", serde_json::to_string_pretty(&containers)?);
        } else {
            println!(
                "{}",
                serde_json::to_string_pretty(&containers)
                    .unwrap_or_else(|_| containers.to_string())
            );
        }
        return Ok(());
    };

    // Apply --all filter: by default show only "running" containers.
    let rows: Vec<&serde_json::Value> = arr
        .iter()
        .filter(|c| {
            if args.all {
                return true;
            }
            let status = c
                .get("status")
                .and_then(|v| v.as_str())
                .or_else(|| c.get("state").and_then(|v| v.as_str()))
                .unwrap_or("");
            status.eq_ignore_ascii_case("running") || status.starts_with("Up")
        })
        .collect();

    if args.format.as_deref() == Some("json") {
        let out: Vec<&serde_json::Value> = rows.clone();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    if args.quiet {
        for c in &rows {
            if let Some(id) = c.get("id").and_then(|v| v.as_str()) {
                let short = if id.len() > 12 { &id[..12] } else { id };
                println!("{short}");
            }
        }
        return Ok(());
    }

    println!(
        "{:<14} {:<30} {:<20} {:<20} {:<20}",
        "CONTAINER ID", "IMAGE", "STATUS", "NAMES", "PORTS"
    );

    for c in &rows {
        let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let short_id = if id.len() > 12 { &id[..12] } else { id };
        let image = c.get("image").and_then(|v| v.as_str()).unwrap_or("-");
        let status = c
            .get("status")
            .and_then(|v| v.as_str())
            .or_else(|| c.get("state").and_then(|v| v.as_str()))
            .unwrap_or("-");
        let name = c
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| c.get("names").and_then(|v| v.as_str()))
            .unwrap_or("-");
        let ports = format_ports(c);
        println!("{short_id:<14} {image:<30} {status:<20} {name:<20} {ports:<20}");
    }

    if rows.is_empty() && !arr.is_empty() {
        // We filtered everything out -- hint the user.
        eprintln!("(no running containers; pass --all to include stopped)");
    }

    Ok(())
}

/// Render a JSON container's port mapping as a Docker-style string.
fn format_ports(c: &serde_json::Value) -> String {
    let Some(ports) = c.get("ports").and_then(|v| v.as_array()) else {
        return "-".to_string();
    };
    if ports.is_empty() {
        return "-".to_string();
    }
    let mut rendered: Vec<String> = Vec::with_capacity(ports.len());
    for p in ports {
        let container_port = p
            .get("container_port")
            .or_else(|| p.get("port"))
            .and_then(serde_json::Value::as_u64);
        let host_port = p.get("host_port").and_then(serde_json::Value::as_u64);
        let proto = p.get("protocol").and_then(|v| v.as_str()).unwrap_or("tcp");
        match (host_port, container_port) {
            (Some(h), Some(cp)) => rendered.push(format!("0.0.0.0:{h}->{cp}/{proto}")),
            (None, Some(cp)) => rendered.push(format!("{cp}/{proto}")),
            _ => {}
        }
    }
    if rendered.is_empty() {
        "-".to_string()
    } else {
        rendered.join(", ")
    }
}

/// Handle the `docker stop` command.
///
/// Sends a graceful stop request to each named container, then echoes
/// the container id on success (matching Docker CLI conventions).
///
/// # Errors
///
/// Returns an error if the daemon cannot be reached, or propagates the
/// first failure from the daemon after attempting all targets.
pub async fn handle_stop(args: StopArgs) -> anyhow::Result<()> {
    tracing::info!("docker stop: containers={:?}", args.containers);

    if args.containers.is_empty() {
        anyhow::bail!("at least one container name or ID is required");
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let timeout = args.time.map(u64::from);
    let mut first_err: Option<anyhow::Error> = None;
    for id in &args.containers {
        match client.stop_container(id, timeout).await {
            Ok(()) => println!("{id}"),
            Err(e) => {
                eprintln!("Error response from daemon: {e}");
                if first_err.is_none() {
                    first_err = Some(e.context(format!("Failed to stop container '{id}'")));
                }
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    Ok(())
}

/// Handle the `docker kill` command.
///
/// Sends a signal (default `SIGKILL`) to each named container and echoes
/// its id on success.
///
/// # Errors
///
/// Returns the first daemon error after attempting all targets.
pub async fn handle_kill(args: KillArgs) -> anyhow::Result<()> {
    tracing::info!("docker kill: containers={:?}", args.containers);

    if args.containers.is_empty() {
        anyhow::bail!("at least one container name or ID is required");
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let signal = args.signal.as_deref();
    let mut first_err: Option<anyhow::Error> = None;
    for id in &args.containers {
        match client.kill_container(id, signal).await {
            Ok(()) => println!("{id}"),
            Err(e) => {
                eprintln!("Error response from daemon: {e}");
                if first_err.is_none() {
                    first_err = Some(e.context(format!("Failed to kill container '{id}'")));
                }
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    Ok(())
}

/// Handle the `docker start` command.
///
/// # Errors
///
/// Returns the first daemon error after attempting all targets.
pub async fn handle_start(args: StartArgs) -> anyhow::Result<()> {
    tracing::info!("docker start: containers={:?}", args.containers);

    if args.containers.is_empty() {
        anyhow::bail!("at least one container name or ID is required");
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let mut first_err: Option<anyhow::Error> = None;
    for id in &args.containers {
        match client.start_container(id).await {
            Ok(()) => println!("{id}"),
            Err(e) => {
                eprintln!("Error response from daemon: {e}");
                if first_err.is_none() {
                    first_err = Some(e.context(format!("Failed to start container '{id}'")));
                }
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    Ok(())
}

/// Handle the `docker restart` command.
///
/// # Errors
///
/// Returns the first daemon error after attempting all targets.
pub async fn handle_restart(args: RestartArgs) -> anyhow::Result<()> {
    tracing::info!("docker restart: containers={:?}", args.containers);

    if args.containers.is_empty() {
        anyhow::bail!("at least one container name or ID is required");
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let timeout = args.time.map(u64::from);
    let mut first_err: Option<anyhow::Error> = None;
    for id in &args.containers {
        match client.restart_container(id, timeout).await {
            Ok(()) => println!("{id}"),
            Err(e) => {
                eprintln!("Error response from daemon: {e}");
                if first_err.is_none() {
                    first_err = Some(e.context(format!("Failed to restart container '{id}'")));
                }
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    Ok(())
}

/// Handle the `docker rm` command.
///
/// Removes each named container via the daemon. The `--volumes` flag is
/// currently accepted but ignored (daemon removes anonymous volumes as
/// part of normal container deletion).
///
/// # Errors
///
/// Returns the first daemon error after attempting all targets.
pub async fn handle_rm(args: RmArgs) -> anyhow::Result<()> {
    tracing::info!(
        "docker rm: containers={:?}, force={}",
        args.containers,
        args.force
    );

    if args.containers.is_empty() {
        anyhow::bail!("at least one container name or ID is required");
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let mut first_err: Option<anyhow::Error> = None;
    for id in &args.containers {
        match client.delete_container(id, args.force).await {
            Ok(()) => println!("{id}"),
            Err(e) => {
                eprintln!("Error response from daemon: {e}");
                if first_err.is_none() {
                    first_err = Some(e.context(format!("Failed to remove container '{id}'")));
                }
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    Ok(())
}

/// Handle the `docker exec` command.
///
/// Executes a command inside a running container and streams stdout/stderr
/// back to the caller. Exits the process with the command's exit code on
/// non-zero.
///
/// Note: the daemon's container exec endpoint is non-interactive. `-i`,
/// `-t`, `-u`, `-e`, and `-w` flags are parsed for Docker CLI compatibility
/// but not forwarded to the daemon (which runs the command to completion
/// and buffers output).
///
/// # Errors
///
/// Returns an error if the daemon cannot be reached, the container is not
/// running, or the command could not be started. If the command runs but
/// exits non-zero, the process exits with that code (no error returned).
pub async fn handle_exec(args: ExecArgs) -> anyhow::Result<()> {
    tracing::info!("docker exec: container={}", args.container);

    if args.command.is_empty() {
        anyhow::bail!("exec requires a command: zlayer docker exec <container> <cmd> [args...]");
    }
    if args.interactive || args.tty {
        eprintln!(
            "warning: -i/-t are not yet supported for container exec; running non-interactively"
        );
    }
    if args.user.is_some() || !args.env.is_empty() || args.workdir.is_some() {
        eprintln!("warning: -u/-e/-w are not yet supported for container exec; ignoring");
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let resp = client
        .exec_in_container(&args.container, args.command.clone())
        .await
        .with_context(|| format!("Failed to exec in container '{}'", args.container))?;

    if !resp.stdout.is_empty() {
        print!("{}", resp.stdout);
    }
    if !resp.stderr.is_empty() {
        eprint!("{}", resp.stderr);
    }
    if resp.exit_code != 0 {
        std::process::exit(resp.exit_code);
    }
    Ok(())
}

/// Handle the `docker logs` command.
///
/// Fetches container logs from the daemon. `--follow` is accepted for
/// compatibility but falls back to a one-shot fetch because the
/// container-level endpoint does not yet stream; `--since` and
/// `--timestamps` are similarly parsed but not forwarded.
///
/// # Errors
///
/// Returns an error if the daemon cannot be reached or the request fails.
pub async fn handle_logs(args: LogsArgs) -> anyhow::Result<()> {
    tracing::info!(
        "docker logs: container={}, follow={}",
        args.container,
        args.follow
    );

    if args.follow {
        eprintln!("warning: --follow is not yet supported for container logs; falling back to one-shot fetch");
    }
    if args.since.is_some() {
        eprintln!("warning: --since is not yet supported; ignoring");
    }
    if args.timestamps {
        eprintln!("warning: --timestamps is not yet supported; ignoring");
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    // Parse --tail: "all" / "0" means "no limit", anything else is a u32.
    let tail = match args.tail.as_deref() {
        None | Some("all" | "0") => None,
        Some(s) => Some(
            s.parse::<u32>()
                .with_context(|| format!("invalid --tail value: '{s}'"))?,
        ),
    };

    let logs = client
        .get_container_logs(&args.container, tail)
        .await
        .with_context(|| format!("Failed to fetch logs for container '{}'", args.container))?;

    print!("{logs}");
    if !logs.ends_with('\n') {
        println!();
    }
    Ok(())
}

/// Handle the `docker inspect` command.
///
/// Fetches detailed container information as a JSON array (matching the
/// Docker CLI output shape) for each named target. The `--format` flag is
/// parsed for compatibility but not yet applied.
///
/// # Errors
///
/// Returns an error if the daemon cannot be reached or any of the names
/// cannot be resolved.
pub async fn handle_inspect(args: InspectArgs) -> anyhow::Result<()> {
    tracing::info!("docker inspect: names={:?}", args.names);

    if args.names.is_empty() {
        anyhow::bail!("at least one container name or ID is required");
    }
    if args.format.is_some() {
        eprintln!("warning: --format is not yet supported; emitting full JSON");
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let mut results: Vec<serde_json::Value> = Vec::with_capacity(args.names.len());
    let mut first_err: Option<anyhow::Error> = None;
    for name in &args.names {
        match client.get_container(name).await {
            Ok(v) => results.push(v),
            Err(e) => {
                eprintln!("Error: No such container: {name}");
                if first_err.is_none() {
                    first_err = Some(e.context(format!("Failed to inspect '{name}'")));
                }
            }
        }
    }

    if !results.is_empty() {
        println!("{}", serde_json::to_string_pretty(&results)?);
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    Ok(())
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
/// Prints a one-shot snapshot of container resource usage. Streaming mode
/// (`--no-stream=false`, the default for Docker CLI) is not yet wired; this
/// always behaves as if `--no-stream` were passed.
///
/// # Errors
///
/// Returns an error if the daemon cannot be reached.
pub async fn handle_stats(args: StatsArgs) -> anyhow::Result<()> {
    tracing::info!(
        "docker stats: containers={:?}, all={}",
        args.containers,
        args.all
    );
    if !args.no_stream {
        eprintln!("warning: live streaming is not supported; showing a single snapshot");
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    // Figure out which containers to show. If none were passed, enumerate them.
    let targets: Vec<String> = if args.containers.is_empty() {
        let all = client
            .get_all_containers()
            .await
            .context("Failed to list containers")?;
        all.as_array()
            .map(|arr| {
                arr.iter()
                    .filter(|c| {
                        if args.all {
                            return true;
                        }
                        let status = c
                            .get("status")
                            .and_then(|v| v.as_str())
                            .or_else(|| c.get("state").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        status.eq_ignore_ascii_case("running") || status.starts_with("Up")
                    })
                    .filter_map(|c| c.get("id").and_then(|v| v.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        args.containers.clone()
    };

    if targets.is_empty() {
        println!("(no containers)");
        return Ok(());
    }

    let mut snapshots: Vec<serde_json::Value> = Vec::with_capacity(targets.len());
    for id in &targets {
        match client.get_container_stats(id).await {
            Ok(v) => snapshots.push(serde_json::json!({ "id": id, "stats": v })),
            Err(e) => eprintln!("warning: failed to fetch stats for {id}: {e}"),
        }
    }
    println!("{}", serde_json::to_string_pretty(&snapshots)?);
    Ok(())
}
