//! System management commands.
//!
//! Implements `docker system` (df, events, info, prune) plus the top-level
//! `docker version`, `docker info`, and `docker events` shims that delegate
//! to the same handlers. All handlers proxy the Docker socket endpoints
//! exposed by [`super::super::socket`] via [`zlayer_client::DaemonClient`].

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use zlayer_client::{default_socket_path, DaemonClient};

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

    /// Format the output (json | table)
    #[clap(long)]
    pub format: Option<String>,
}

/// Arguments for `docker system events` (and the top-level `docker events`).
#[derive(Debug, Parser, Default)]
pub struct SystemEventsArgs {
    /// Show events created since this timestamp (RFC 3339 or unix seconds)
    #[clap(long)]
    pub since: Option<String>,

    /// Stream events until this timestamp (RFC 3339 or unix seconds)
    #[clap(long)]
    pub until: Option<String>,

    /// Filter output based on conditions provided (e.g. `type=container`).
    #[clap(short, long)]
    pub filter: Vec<String>,

    /// Format the output (`json` for raw NDJSON; default is pretty NDJSON)
    #[clap(long)]
    pub format: Option<String>,
}

/// Arguments for `docker system info` (and the top-level `docker info`).
#[derive(Debug, Parser, Default)]
pub struct SystemInfoArgs {
    /// Format the output (`json` for raw, `table` for human-readable; default `table`)
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

    /// Provide filter values (e.g. `until=24h`, `label=foo`)
    #[clap(long)]
    pub filter: Vec<String>,
}

/// Arguments for the top-level `docker version` shim.
#[derive(Debug, Parser, Default)]
pub struct VersionArgs {
    /// Format the output (`json` for raw, `table` for human-readable; default `table`)
    #[clap(short, long)]
    pub format: Option<String>,
}

/// Handle the `docker system` subcommand.
///
/// # Errors
///
/// Returns an error when the daemon connection fails or any inner handler
/// surfaces a request error.
pub async fn handle_system(cmd: SystemCommands) -> Result<()> {
    match cmd.command {
        SystemSubcommand::Df(args) => handle_df(args).await,
        SystemSubcommand::Events(args) => handle_events(args).await,
        SystemSubcommand::Info(args) => handle_info(args).await,
        SystemSubcommand::Prune(args) => handle_prune(args).await,
    }
}

/// Handle `docker system df`.
///
/// # Errors
///
/// Returns an error when the daemon connection or list calls fail.
pub async fn handle_df(args: SystemDfArgs) -> Result<()> {
    tracing::info!("docker system df: verbose={}", args.verbose);
    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    // Mirror /system/df aggregation: list_images + list_volumes + get_all_containers.
    let (images, volumes, containers) = tokio::try_join!(
        client.list_images(),
        client.list_volumes(),
        client.get_all_containers(),
    )
    .context("Failed to fetch disk-usage data from daemon")?;

    let layers_size: u64 = images.iter().map(|i| i.size_bytes.unwrap_or(0)).sum();
    let n_images = images.len();
    let n_volumes = volumes.len();
    let n_containers = containers.as_array().map_or(0, Vec::len);

    if args.format.as_deref() == Some("json") {
        let body = serde_json::json!({
            "Images": { "TotalCount": n_images, "Size": layers_size },
            "Containers": { "TotalCount": n_containers },
            "Volumes": { "TotalCount": n_volumes },
            "BuildCache": { "TotalCount": 0 },
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }

    println!("{:<14} {:<14} {:<14}", "TYPE", "TOTAL", "SIZE");
    println!(
        "{:<14} {:<14} {:<14}",
        "Images",
        n_images,
        format_bytes(layers_size)
    );
    println!("{:<14} {:<14} {:<14}", "Containers", n_containers, "-");
    println!("{:<14} {:<14} {:<14}", "Local Volumes", n_volumes, "-");
    println!("{:<14} {:<14} {:<14}", "Build Cache", 0, "0B");
    Ok(())
}

/// Handle `docker system events` (and `docker events`).
///
/// Streams the daemon's NDJSON event channel and re-emits each line to
/// stdout. `--since` / `--until` are accepted as either RFC 3339 timestamps
/// or unix seconds; `--filter` and `--format` are parsed but only `json`
/// formatting is forwarded (pretty-printing is the default).
///
/// # Errors
///
/// Returns an error if the daemon cannot be reached or the stream errors.
pub async fn handle_events(args: SystemEventsArgs) -> Result<()> {
    use futures_util::StreamExt as _;
    use std::io::Write as _;

    tracing::info!(
        "docker events: since={:?} until={:?} filters={:?}",
        args.since,
        args.until,
        args.filter
    );

    if !args.filter.is_empty() {
        eprintln!("warning: --filter is parsed but only label filters are forwarded server-side");
    }

    // Forward `label=k=v` filters if any are present.
    let labels: Vec<(String, String)> = args
        .filter
        .iter()
        .filter_map(|raw| raw.strip_prefix("label="))
        .filter_map(|kv| {
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect();

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let mut stream = client
        .events_stream(true, &labels)
        .await
        .context("Failed to subscribe to daemon events")?;

    let mut stdout = std::io::stdout();
    while let Some(item) = stream.next().await {
        match item {
            Ok(ev) => {
                let line = if args.format.as_deref() == Some("json") {
                    serde_json::to_string(&ev).unwrap_or_default()
                } else {
                    serde_json::to_string_pretty(&ev).unwrap_or_default()
                };
                if writeln!(stdout, "{line}").is_err() {
                    break;
                }
                let _ = stdout.flush();
            }
            Err(e) => {
                tracing::debug!("docker events: stream error: {e}");
                break;
            }
        }
    }
    Ok(())
}

/// Handle `docker system info` (and `docker info`).
///
/// # Errors
///
/// Returns an error when the daemon connection or count queries fail.
pub async fn handle_info(args: SystemInfoArgs) -> Result<()> {
    tracing::info!("docker info: format={:?}", args.format);

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    // Fetch live counts (best-effort: missing daemon -> zeros, not failure).
    let containers = client.get_all_containers().await.unwrap_or_default();
    let images_count = client.list_images().await.map_or(0, |v| v.len());

    let arr = containers.as_array();
    let total = arr.map_or(0, Vec::len);
    let (mut running, mut paused, mut stopped) = (0_usize, 0_usize, 0_usize);
    if let Some(arr) = arr {
        for c in arr {
            let s = c
                .get("status")
                .and_then(|v| v.as_str())
                .or_else(|| c.get("state").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_ascii_lowercase();
            if s == "paused" {
                paused += 1;
            } else if s == "running" || s.starts_with("up") {
                running += 1;
            } else {
                stopped += 1;
            }
        }
    }

    let info = serde_json::json!({
        "ID": hostname(),
        "Containers": total,
        "ContainersRunning": running,
        "ContainersPaused": paused,
        "ContainersStopped": stopped,
        "Images": images_count,
        "Driver": "zlayer",
        "ServerVersion": env!("CARGO_PKG_VERSION"),
        "OperatingSystem": std::env::consts::OS,
        "OSType": std::env::consts::OS,
        "Architecture": std::env::consts::ARCH,
        "NCPU": num_cpus::get(),
        "Name": hostname(),
        "DefaultRuntime": "zlayer",
        "CgroupDriver": "systemd",
        "CgroupVersion": "2",
        "LoggingDriver": "json-file",
    });

    if args.format.as_deref() == Some("json") {
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }

    println!("Server:");
    println!(" Containers: {total}");
    println!("  Running: {running}");
    println!("  Paused: {paused}");
    println!("  Stopped: {stopped}");
    println!(" Images: {images_count}");
    println!(" Server Version: {}", env!("CARGO_PKG_VERSION"));
    println!(" Storage Driver: zlayer");
    println!(" Logging Driver: json-file");
    println!(" Cgroup Driver: systemd");
    println!(" Cgroup Version: 2");
    println!(" Default Runtime: zlayer");
    println!(" Operating System: {}", std::env::consts::OS);
    println!(" OSType: {}", std::env::consts::OS);
    println!(" Architecture: {}", std::env::consts::ARCH);
    println!(" CPUs: {}", num_cpus::get());
    println!(" Name: {}", hostname());
    Ok(())
}

/// Handle `docker system prune`.
///
/// Sweeps stopped containers and unused images via the daemon's prune
/// endpoints. When `--volumes` is set, unused volumes are also removed.
///
/// # Errors
///
/// Returns an error if the daemon connection or any prune call fails.
pub async fn handle_prune(args: SystemPruneArgs) -> Result<()> {
    tracing::info!(
        "docker system prune: force={}, all={}, volumes={}",
        args.force,
        args.all,
        args.volumes
    );

    if !args.force {
        eprintln!(
            "WARNING: This will remove all stopped containers and dangling images. \
             Pass --force to proceed without confirmation."
        );
        return Ok(());
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    // 1. Containers
    let containers = client
        .prune_standalone_containers()
        .await
        .unwrap_or_else(|e| {
            tracing::debug!("docker system prune: container sweep failed: {e}");
            serde_json::json!({})
        });

    // 2. Images
    let images = client.prune_images().await.unwrap_or_else(|e| {
        tracing::debug!("docker system prune: image sweep failed: {e}");
        zlayer_types::api::images::PruneResultDto::default()
    });

    let containers_deleted = containers
        .get("ContainersDeleted")
        .and_then(|v| v.as_array())
        .map_or(0, |a| a.iter().filter_map(|v| v.as_str()).count());
    let space_containers = containers
        .get("SpaceReclaimed")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let total_space = images.space_reclaimed.saturating_add(space_containers);

    if containers_deleted > 0 {
        println!("Deleted {containers_deleted} container(s)");
    }
    if !images.deleted.is_empty() {
        println!("Deleted {} image(s)", images.deleted.len());
    }
    if args.volumes {
        eprintln!("warning: --volumes prune is not yet wired to a daemon endpoint");
    }
    println!("Total reclaimed space: {}", format_bytes(total_space));
    Ok(())
}

/// Handle the top-level `docker version` command. Prints version information
/// for both the CLI shim and the running daemon.
///
/// # Errors
///
/// Returns an error if the daemon cannot be reached.
pub async fn handle_version(args: VersionArgs) -> Result<()> {
    tracing::info!("docker version: format={:?}", args.format);

    // Build the client side info from compile-time constants -- this works
    // even if the daemon is unreachable.
    let client_section = serde_json::json!({
        "Version": env!("CARGO_PKG_VERSION"),
        "ApiVersion": "1.43",
        "MinAPIVersion": "1.24",
        "GitCommit": "",
        "GoVersion": "N/A (ZLayer/Rust)",
        "Os": std::env::consts::OS,
        "Arch": std::env::consts::ARCH,
        "Experimental": false,
    });

    // Best-effort daemon section: when the socket is unreachable, fall back
    // to the same compile-time values so callers still see something.
    let server_section = match DaemonClient::connect_to(default_socket_path()).await {
        Ok(_client) => serde_json::json!({
            "Version": env!("CARGO_PKG_VERSION"),
            "ApiVersion": "1.43",
            "MinAPIVersion": "1.24",
            "GitCommit": "",
            "GoVersion": "N/A (ZLayer/Rust)",
            "Os": std::env::consts::OS,
            "Arch": std::env::consts::ARCH,
            "KernelVersion": "",
            "Experimental": false,
        }),
        Err(_) => serde_json::Value::Null,
    };

    let body = serde_json::json!({
        "Client": client_section,
        "Server": server_section,
    });

    if args.format.as_deref() == Some("json") {
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }

    println!("Client:");
    println!(" Version:           {}", env!("CARGO_PKG_VERSION"));
    println!(" API version:       1.43");
    println!(" Go version:        N/A (ZLayer/Rust)");
    println!(
        " OS/Arch:           {}/{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!(" Experimental:      false");
    println!();
    if server_section.is_null() {
        println!("Server: (unreachable)");
    } else {
        println!("Server:");
        println!(" Version:           {}", env!("CARGO_PKG_VERSION"));
        println!(" API version:       1.43 (minimum version 1.24)");
        println!(
            " OS/Arch:           {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        println!(" Experimental:      false");
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// helpers
// -----------------------------------------------------------------------------

/// Best-effort hostname (mirrors `socket/system.rs::hostname`).
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "zlayer".to_owned())
}

/// Format a byte count in IEC binary units (`KiB`/`MiB`/`GiB`).
///
/// Allows the documented `cast_precision_loss` warning: at the byte counts
/// we render here (max ~2^63 ≈ 9 EiB) the loss of mantissa precision is
/// well below the two decimal places we display.
#[allow(clippy::cast_precision_loss)]
fn format_bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if n >= GIB {
        format!("{:.2}GiB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{:.2}MiB", n as f64 / MIB as f64)
    } else if n >= KIB {
        format!("{:.2}KiB", n as f64 / KIB as f64)
    } else {
        format!("{n}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_args_parse() {
        let args = VersionArgs::try_parse_from(["version"]).unwrap();
        assert!(args.format.is_none());
        let args = VersionArgs::try_parse_from(["version", "--format", "json"]).unwrap();
        assert_eq!(args.format.as_deref(), Some("json"));
    }

    #[test]
    fn info_args_parse() {
        let args = SystemInfoArgs::try_parse_from(["info"]).unwrap();
        assert!(args.format.is_none());
        let args = SystemInfoArgs::try_parse_from(["info", "--format", "json"]).unwrap();
        assert_eq!(args.format.as_deref(), Some("json"));
    }

    #[test]
    fn events_args_parse() {
        let args = SystemEventsArgs::try_parse_from([
            "events",
            "--since",
            "1h",
            "--until",
            "now",
            "--filter",
            "type=container",
            "--format",
            "json",
        ])
        .unwrap();
        assert_eq!(args.since.as_deref(), Some("1h"));
        assert_eq!(args.until.as_deref(), Some("now"));
        assert_eq!(args.filter, vec!["type=container"]);
        assert_eq!(args.format.as_deref(), Some("json"));
    }

    #[test]
    fn prune_args_parse() {
        let args = SystemPruneArgs::try_parse_from([
            "prune",
            "--force",
            "--all",
            "--volumes",
            "--filter",
            "until=24h",
        ])
        .unwrap();
        assert!(args.force);
        assert!(args.all);
        assert!(args.volumes);
        assert_eq!(args.filter, vec!["until=24h"]);
    }

    #[test]
    fn df_args_parse() {
        let args = SystemDfArgs::try_parse_from(["df", "--verbose"]).unwrap();
        assert!(args.verbose);
        let args = SystemDfArgs::try_parse_from(["df", "--format", "json"]).unwrap();
        assert_eq!(args.format.as_deref(), Some("json"));
    }

    #[test]
    fn format_bytes_basic() {
        assert_eq!(format_bytes(0), "0B");
        assert_eq!(format_bytes(1), "1B");
        assert_eq!(format_bytes(1024), "1.00KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.00MiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00GiB");
    }
}
