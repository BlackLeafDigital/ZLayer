//! Docker Compose command implementations.

use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand};
use zlayer_client::{default_socket_path, DaemonClient};

use crate::compose::{compose_to_deployment, parse_compose, ComposeFile};

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

    /// Compose file path
    #[clap(short, long)]
    pub file: Option<String>,
}

/// Arguments for `docker compose ps`.
#[derive(Debug, Parser)]
pub struct ComposePsArgs {
    /// Services to list (default: all)
    pub services: Vec<String>,

    /// Only display IDs
    #[clap(short, long)]
    pub quiet: bool,

    /// Compose file path
    #[clap(short, long)]
    pub file: Option<String>,
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

    /// Compose file path
    #[clap(short, long)]
    pub file: Option<String>,
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

    /// Compose file path
    #[clap(short, long)]
    pub file: Option<String>,
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

/// Default compose file names (Docker-compatible discovery order).
const DEFAULT_COMPOSE_FILES: &[&str] = &[
    "docker-compose.yaml",
    "docker-compose.yml",
    "compose.yaml",
    "compose.yml",
];

/// Resolve a compose file path from an optional `-f` flag.
///
/// If an explicit path is provided it is used as-is. Otherwise the current
/// working directory is searched for (in order): `docker-compose.yaml`,
/// `docker-compose.yml`, `compose.yaml`, `compose.yml`.
fn resolve_compose_file(explicit: Option<&str>) -> anyhow::Result<PathBuf> {
    if let Some(path) = explicit {
        let p = PathBuf::from(path);
        if !p.exists() {
            anyhow::bail!("compose file not found: {}", p.display());
        }
        return Ok(p);
    }

    let cwd = std::env::current_dir().context("failed to read current directory")?;
    for name in DEFAULT_COMPOSE_FILES {
        let candidate = cwd.join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    anyhow::bail!(
        "no compose file found in {}; tried: {}",
        cwd.display(),
        DEFAULT_COMPOSE_FILES.join(", "),
    )
}

/// Load and parse a compose file from disk.
fn load_compose(explicit: Option<&str>) -> anyhow::Result<(PathBuf, ComposeFile)> {
    let path = resolve_compose_file(explicit)?;
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read compose file {}", path.display()))?;
    let compose = parse_compose(&contents)
        .with_context(|| format!("failed to parse compose file {}", path.display()))?;
    Ok((path, compose))
}

/// Sanitize a directory basename so it is a valid `ZLayer` deployment name.
///
/// `ZLayer` requires 3-63 chars, `[a-z0-9-]`. We lowercase, replace invalid
/// runs with `-`, trim leading/trailing `-`, and pad short names.
fn sanitize_project_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_dash = true; // suppress leading '-'
    for ch in raw.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || c == '-' {
            if c == '-' && last_dash {
                continue;
            }
            out.push(c);
            last_dash = c == '-';
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    // Digits-only leading is fine for ZLayer, but empty / too short is not.
    if out.len() < 3 {
        out = format!("compose-{out}");
        while out.ends_with('-') {
            out.pop();
        }
        if out.len() < 3 {
            out = "compose".to_string();
        }
    }
    if out.len() > 63 {
        out.truncate(63);
        while out.ends_with('-') {
            out.pop();
        }
    }
    out
}

/// Derive the project/deployment name from a compose file.
///
/// Priority: `compose.name` → parent directory basename → `"compose"`.
fn project_name(path: &Path, compose: &ComposeFile) -> String {
    if let Some(name) = compose.name.as_ref().filter(|n| !n.is_empty()) {
        return sanitize_project_name(name);
    }
    if let Some(parent) = path.parent() {
        if let Some(dir_name) = parent.file_name().and_then(|s| s.to_str()) {
            if !dir_name.is_empty() {
                return sanitize_project_name(dir_name);
            }
        }
    }
    "compose".to_string()
}

/// Connect to the local zlayer daemon.
async fn connect_daemon() -> anyhow::Result<DaemonClient> {
    DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")
}

/// Handle the `docker compose` subcommand.
///
/// # Errors
///
/// Returns an error if the compose file cannot be read, parsed, or converted,
/// or if the daemon request fails.
pub async fn handle_compose(cmd: ComposeCommands) -> anyhow::Result<()> {
    match cmd.command {
        ComposeSubcommand::Up(args) => handle_up(args).await,
        ComposeSubcommand::Down(args) => handle_down(args).await,
        ComposeSubcommand::Build(args) => handle_build(args).await,
        ComposeSubcommand::Ps(args) => handle_ps(args).await,
        ComposeSubcommand::Logs(args) => handle_logs(args).await,
        ComposeSubcommand::Exec(args) => handle_exec(args).await,
        ComposeSubcommand::Pull(args) => handle_pull(args).await,
        ComposeSubcommand::Push(args) => handle_push(args).await,
        ComposeSubcommand::Restart(args) => handle_restart(args).await,
        ComposeSubcommand::Stop(args) => handle_stop(args).await,
        ComposeSubcommand::Start(args) => handle_start(args).await,
    }
}

/// `docker compose up` — convert the compose file to a `DeploymentSpec` and
/// submit it to the zlayer daemon.
async fn handle_up(args: ComposeUpArgs) -> anyhow::Result<()> {
    if !args.services.is_empty() {
        eprintln!(
            "warning: service filtering on `compose up` is not yet supported; deploying all services"
        );
    }
    if !args.detach {
        // v1: foreground streaming is not supported. Behave like --detach but warn.
        eprintln!("warning: foreground mode not yet supported; running in detached mode");
    }
    if args.build {
        eprintln!("warning: --build is not yet supported; deploying with existing images");
    }
    if args.force_recreate {
        eprintln!("warning: --force-recreate is not yet supported; ignoring");
    }

    let (path, compose) = load_compose(args.file.as_deref())?;
    let name = project_name(&path, &compose);

    tracing::info!(
        compose_file = %path.display(),
        deployment = %name,
        "docker compose up: converting compose to DeploymentSpec"
    );

    let spec = compose_to_deployment(&compose, &name)
        .with_context(|| format!("failed to convert compose file {}", path.display()))?;

    let yaml =
        serde_yaml::to_string(&spec).context("failed to serialize DeploymentSpec to YAML")?;

    let client = connect_daemon().await?;
    let resp = client
        .create_deployment(&yaml)
        .await
        .context("failed to create deployment on daemon")?;

    let deployed_name = resp
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(name.as_str());
    println!("Deployment '{deployed_name}' created");
    Ok(())
}

/// `docker compose down` — delete the deployment derived from the compose file.
async fn handle_down(args: ComposeDownArgs) -> anyhow::Result<()> {
    if args.volumes {
        eprintln!(
            "warning: -v/--volumes cleanup of anonymous volumes is handled by deployment deletion; \
             named volumes persist"
        );
    }
    if let Some(ref rmi) = args.rmi {
        eprintln!("warning: --rmi {rmi} is not yet supported; images are left in place");
    }

    let (path, compose) = load_compose(args.file.as_deref())?;
    let name = project_name(&path, &compose);

    tracing::info!(
        compose_file = %path.display(),
        deployment = %name,
        "docker compose down: deleting deployment"
    );

    let client = connect_daemon().await?;
    client
        .delete_deployment(&name)
        .await
        .with_context(|| format!("failed to delete deployment '{name}'"))?;

    println!("Deployment '{name}' removed");
    Ok(())
}

/// `docker compose ps` — list containers belonging to the compose project's
/// deployment, rendered as a Docker-compose-style table.
async fn handle_ps(args: ComposePsArgs) -> anyhow::Result<()> {
    let (path, compose) = load_compose(args.file.as_deref())?;
    let name = project_name(&path, &compose);
    let service_filter: Option<Vec<String>> = if args.services.is_empty() {
        None
    } else {
        Some(args.services.clone())
    };

    let client = connect_daemon().await?;
    let containers = client
        .get_all_containers()
        .await
        .context("failed to fetch containers from daemon")?;

    let Some(arr) = containers.as_array() else {
        eprintln!("no containers returned from daemon");
        return Ok(());
    };

    let rows: Vec<&serde_json::Value> = arr
        .iter()
        .filter(|c| container_in_deployment(c, &name))
        .filter(|c| match &service_filter {
            Some(services) => services
                .iter()
                .any(|s| container_matches_service(c, s, &name)),
            None => true,
        })
        .collect();

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
        "{:<28} {:<30} {:<15} {:<20}",
        "NAME", "COMMAND", "STATE", "PORTS"
    );

    for c in &rows {
        let name = c
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| c.get("id").and_then(|v| v.as_str()))
            .unwrap_or("-");
        let command = c
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let command_display = if command.is_empty() {
            "-".to_string()
        } else {
            truncate(&command, 28)
        };
        let state = c
            .get("state")
            .and_then(|v| v.as_str())
            .or_else(|| c.get("status").and_then(|v| v.as_str()))
            .unwrap_or("-");
        let ports = format_ports(c);
        println!("{name:<28} {command_display:<30} {state:<15} {ports:<20}");
    }

    if rows.is_empty() {
        eprintln!("(no containers found for compose project '{name}')");
    }

    Ok(())
}

/// `docker compose logs` — print container logs for every service in the
/// compose project, prefixed with `service_name | `.
async fn handle_logs(args: ComposeLogsArgs) -> anyhow::Result<()> {
    if args.follow {
        eprintln!("warning: --follow is not yet supported; printing current logs and exiting");
    }
    if args.timestamps {
        eprintln!("warning: --timestamps is not yet supported; ignoring");
    }
    let tail: Option<u32> = match args.tail.as_deref() {
        None | Some("all") => None,
        Some(s) => {
            if let Ok(n) = s.parse::<u32>() {
                Some(n)
            } else {
                eprintln!("warning: invalid --tail value '{s}'; ignoring");
                None
            }
        }
    };

    let (path, compose) = load_compose(args.file.as_deref())?;
    let deployment = project_name(&path, &compose);
    let service_filter: Option<Vec<String>> = if args.services.is_empty() {
        None
    } else {
        Some(args.services.clone())
    };

    let client = connect_daemon().await?;
    let containers = client
        .get_all_containers()
        .await
        .context("failed to fetch containers from daemon")?;

    let Some(arr) = containers.as_array() else {
        eprintln!("no containers returned from daemon");
        return Ok(());
    };

    let mut any = false;
    for c in arr {
        if !container_in_deployment(c, &deployment) {
            continue;
        }
        let svc = container_service(c, &deployment).unwrap_or_else(|| "?".to_string());
        if let Some(ref filter) = service_filter {
            if !filter.contains(&svc) {
                continue;
            }
        }
        let Some(id) = c.get("id").and_then(|v| v.as_str()) else {
            continue;
        };

        match client.get_container_logs(id, tail).await {
            Ok(logs) => {
                any = true;
                for line in logs.lines() {
                    println!("{svc} | {line}");
                }
            }
            Err(e) => {
                eprintln!("{svc} | <failed to fetch logs: {e}>");
            }
        }
    }

    if !any {
        eprintln!("(no logs found for compose project '{deployment}')");
    }

    Ok(())
}

/// `docker compose pull` — iterate over services and request the daemon to
/// pull each image. Services using `build:` without `image:` are skipped.
async fn handle_pull(args: ComposePullArgs) -> anyhow::Result<()> {
    let (path, compose) = load_compose(args.file.as_deref())?;
    let service_filter: Option<Vec<String>> = if args.services.is_empty() {
        None
    } else {
        Some(args.services.clone())
    };

    let client = connect_daemon().await?;
    let mut pulled = 0usize;
    let mut errors = 0usize;

    for (svc_name, svc) in &compose.services {
        if let Some(ref filter) = service_filter {
            if !filter.contains(svc_name) {
                continue;
            }
        }
        let Some(image) = svc.image.as_ref() else {
            eprintln!("skipping '{svc_name}': service has no 'image' field");
            continue;
        };
        print!("Pulling {svc_name} ({image})... ");
        match client.pull_image_from_server(image, None).await {
            Ok(_) => {
                println!("done");
                pulled += 1;
            }
            Err(e) => {
                println!("failed: {e}");
                errors += 1;
            }
        }
    }

    if errors > 0 {
        anyhow::bail!(
            "compose pull finished with {errors} error(s) ({pulled} successful) [from {}]",
            path.display()
        );
    }
    Ok(())
}

/// `docker compose build` — not yet implemented end-to-end.
#[allow(clippy::unused_async, clippy::unnecessary_wraps)]
async fn handle_build(_args: ComposeBuildArgs) -> anyhow::Result<()> {
    eprintln!(
        "compose build: not yet implemented. Run `zlayer build` against each service's build context, \
         or use `zlayer pipeline` for multi-image builds."
    );
    Ok(())
}

/// `docker compose push` — not yet implemented.
#[allow(clippy::unused_async, clippy::unnecessary_wraps)]
async fn handle_push(_args: ComposePushArgs) -> anyhow::Result<()> {
    eprintln!(
        "compose push: not yet implemented. Use `zlayer push` or your registry's tooling for now."
    );
    Ok(())
}

/// `docker compose exec` — not yet implemented.
#[allow(
    clippy::unused_async,
    clippy::unnecessary_wraps,
    clippy::needless_pass_by_value
)]
async fn handle_exec(args: ComposeExecArgs) -> anyhow::Result<()> {
    eprintln!(
        "compose exec: not yet implemented for service '{}'. Use `zlayer exec <deployment>/<service>` against \
         a running deployment instead.",
        args.service,
    );
    Ok(())
}

/// `docker compose restart` — not yet implemented.
#[allow(clippy::unused_async, clippy::unnecessary_wraps)]
async fn handle_restart(_args: ComposeRestartArgs) -> anyhow::Result<()> {
    eprintln!(
        "compose restart: not yet implemented. Re-run `zlayer docker compose up` to apply changes."
    );
    Ok(())
}

/// `docker compose stop` — not yet implemented.
#[allow(clippy::unused_async, clippy::unnecessary_wraps)]
async fn handle_stop(_args: ComposeStopArgs) -> anyhow::Result<()> {
    eprintln!("compose stop: not yet implemented. Use `zlayer docker compose down` to remove the deployment.");
    Ok(())
}

/// `docker compose start` — not yet implemented.
#[allow(clippy::unused_async, clippy::unnecessary_wraps)]
async fn handle_start(_args: ComposeStartArgs) -> anyhow::Result<()> {
    eprintln!("compose start: not yet implemented. Use `zlayer docker compose up` to create the deployment.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check whether a container JSON record belongs to the given compose-project
/// deployment. Matches either a `deployment` label or a name prefix of
/// `{deployment}-` / `{deployment}/`.
fn container_in_deployment(c: &serde_json::Value, deployment: &str) -> bool {
    if let Some(label) = c
        .get("labels")
        .and_then(|l| l.get("deployment"))
        .and_then(|v| v.as_str())
    {
        if label == deployment {
            return true;
        }
    }
    if let Some(label) = c
        .get("labels")
        .and_then(|l| l.get("zlayer.deployment"))
        .and_then(|v| v.as_str())
    {
        if label == deployment {
            return true;
        }
    }
    if let Some(name) = c.get("name").and_then(|v| v.as_str()) {
        let prefix = format!("{deployment}-");
        let slashed = format!("{deployment}/");
        if name.starts_with(&prefix) || name.starts_with(&slashed) || name == deployment {
            return true;
        }
    }
    false
}

/// Extract the service name associated with a container, if discoverable.
fn container_service(c: &serde_json::Value, deployment: &str) -> Option<String> {
    if let Some(svc) = c
        .get("labels")
        .and_then(|l| l.get("service"))
        .and_then(|v| v.as_str())
    {
        return Some(svc.to_string());
    }
    if let Some(svc) = c
        .get("labels")
        .and_then(|l| l.get("zlayer.service"))
        .and_then(|v| v.as_str())
    {
        return Some(svc.to_string());
    }
    let name = c.get("name").and_then(|v| v.as_str())?;
    let rest = name
        .strip_prefix(&format!("{deployment}-"))
        .or_else(|| name.strip_prefix(&format!("{deployment}/")))?;
    // Trim the trailing replica suffix (e.g., "web-0", "web-abc123").
    let svc = rest.rsplit_once('-').map_or(rest, |(svc, _)| svc);
    Some(svc.to_string())
}

/// Return true if a container matches a specific service name within a deployment.
fn container_matches_service(c: &serde_json::Value, service: &str, deployment: &str) -> bool {
    container_service(c, deployment).as_deref() == Some(service)
}

/// Render ports from a container JSON value as a Docker-style port list.
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

/// Truncate a string to `max` chars with a trailing ellipsis.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_basic() {
        assert_eq!(sanitize_project_name("MyProject"), "myproject");
        assert_eq!(sanitize_project_name("my_app"), "my-app");
        assert_eq!(sanitize_project_name("a"), "compose-a");
        assert_eq!(sanitize_project_name(""), "compose");
        assert_eq!(sanitize_project_name("--foo--"), "foo");
    }

    #[test]
    fn container_filter_matches_label() {
        let c = serde_json::json!({
            "id": "abc",
            "name": "web-0",
            "labels": {"deployment": "myapp"}
        });
        assert!(container_in_deployment(&c, "myapp"));
        assert!(!container_in_deployment(&c, "other"));
    }

    #[test]
    fn container_filter_matches_name_prefix() {
        let c = serde_json::json!({
            "id": "abc",
            "name": "myapp-web-0",
            "labels": {}
        });
        assert!(container_in_deployment(&c, "myapp"));
        assert!(!container_in_deployment(&c, "other"));
    }

    #[test]
    fn service_extracted_from_label() {
        let c = serde_json::json!({
            "name": "myapp-web-0",
            "labels": {"service": "web"}
        });
        assert_eq!(container_service(&c, "myapp").as_deref(), Some("web"),);
    }

    #[test]
    fn service_extracted_from_name() {
        let c = serde_json::json!({
            "name": "myapp-web-0",
            "labels": {}
        });
        assert_eq!(container_service(&c, "myapp").as_deref(), Some("web"),);
    }

    #[test]
    fn truncate_short_is_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_is_ellipsed() {
        let out = truncate("abcdefghij", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
    }
}
