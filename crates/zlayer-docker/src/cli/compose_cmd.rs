//! Docker Compose command implementations.
//!
//! Implements the `zlayer docker compose` subcommand surface.  The CLI flags
//! and behaviours mirror Docker Compose v2 as closely as the underlying
//! `ZLayer` storage model allows:
//!
//! * `--project-name` / `-p` overrides the auto-derived project name (default
//!   = basename of the project working directory, or the top-level `name:`
//!   field in the merged compose file).
//! * `--project-directory` overrides the working directory used to resolve
//!   relative paths inside the compose file (default = the parent directory
//!   of the first `-f` file, or the current directory).
//! * `--file` / `-f` is repeatable; later files override earlier ones per the
//!   Compose spec merge rules ([`crate::compose::merge_compose_files`]).
//! * `--profile` is repeatable; profile names are merged with the
//!   `COMPOSE_PROFILES` env var per the Compose spec
//!   ([`crate::compose::profiles::select_active_profiles`]).
//! * `--env-file` is repeatable; layered after `<project_dir>/.env` and
//!   before the process environment ([`crate::compose::env_source`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use chrono::Utc;
use clap::{Parser, Subcommand};
use zlayer_api::{ComposeProject, ComposeProjectStorage};
use zlayer_client::{default_socket_path, DaemonClient};

use crate::compose::env_source::collect_env_sources;
use crate::compose::interpolate::interpolate_yaml_value;
use crate::compose::profiles::{filter_services_by_profile, select_active_profiles};
use crate::compose::{
    compose_to_deployment, merge_compose_files_to_value, resolve_extends, ComposeFile,
};
#[cfg(test)]
use zlayer_paths::ZLayerDirs;

// ---------------------------------------------------------------------------
// Subcommand definitions
// ---------------------------------------------------------------------------

/// Docker Compose commands.
#[derive(Debug, Parser)]
pub struct ComposeCommands {
    /// Project-level compose flags accepted *before* the subcommand, the way
    /// `docker compose [OPTIONS] SUBCOMMAND` (v2) and `docker-compose` (v1)
    /// expect them. These mirror the per-subcommand [`ComposeContextArgs`]
    /// (which remain `global = true` for the post-subcommand form), so
    /// `zlayer docker compose -f a.yml -p web config` parses identically to
    /// `zlayer docker compose config -f a.yml -p web`.
    #[clap(flatten)]
    pub ctx: ComposeContextArgs,

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
    /// List running compose projects (read from the daemon's project store).
    Ls(ComposeLsArgs),
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
    /// Force-stop service containers via signal
    Kill(ComposeKillArgs),
    /// Remove stopped service containers
    Rm(ComposeRmArgs),
    /// Pause service containers
    Pause(ComposePauseArgs),
    /// Unpause service containers
    Unpause(ComposeUnpauseArgs),
    /// Run a one-off command on a service
    Run(ComposeRunArgs),
    /// Show the resolved/interpolated compose configuration
    Config(ComposeConfigArgs),
    /// List images used by services
    Images(ComposeImagesArgs),
    /// Print the public port mapping for a service
    Port(ComposePortArgs),
    /// Display the running processes of services
    Top(ComposeTopArgs),
    /// Stream events from the project's containers
    Events(ComposeEventsArgs),
    /// Copy files/folders between a service container and the local filesystem
    Cp(ComposeCpArgs),
    /// Block until each service container has exited
    Wait(ComposeWaitArgs),
    /// Watch source files and rebuild/restart on change (not yet supported)
    Watch(ComposeWatchArgs),
    /// Attach the local stdio to a service container
    Attach(ComposeAttachArgs),
    /// Print the daemon version and the compose CLI version
    Version(ComposeVersionArgs),
    /// Compose alpha commands (experimental)
    #[clap(subcommand)]
    Alpha(ComposeAlphaSubcommand),
}

/// `docker compose alpha` subcommands.
#[derive(Debug, Subcommand)]
pub enum ComposeAlphaSubcommand {
    /// Publish a compose application (not yet supported).
    Publish(ComposeAlphaPublishArgs),
}

/// Common compose-context flags shared by every subcommand.
///
/// Flattened into each `*Args` struct so users can pass `-p`, `-f`,
/// `--profile`, etc. on every subcommand the way `docker compose` does.
#[derive(Debug, Clone, Parser, Default)]
pub struct ComposeContextArgs {
    /// Project name override. Defaults to the basename of the project
    /// directory (or the top-level `name:` field in the merged compose
    /// file). Sanitised to `ZLayer`'s deployment-name rules.
    #[clap(short = 'p', long = "project-name", global = true)]
    pub project_name: Option<String>,

    /// Project directory override. Relative `-f` paths and the optional
    /// `<project_dir>/.env` file are resolved against this directory.
    #[clap(long = "project-directory", global = true)]
    pub project_directory: Option<PathBuf>,

    /// One or more compose files. May be repeated. Later files override
    /// earlier ones per the Compose spec merge rules.
    #[clap(short = 'f', long = "file", global = true, num_args = 1)]
    pub files: Vec<PathBuf>,

    /// One or more `--profile` activations. Merged with the
    /// `COMPOSE_PROFILES` environment variable.
    #[clap(long = "profile", global = true, num_args = 1)]
    pub profiles: Vec<String>,

    /// One or more `--env-file` paths. Layered after `<project_dir>/.env`
    /// and before the process environment.
    #[clap(long = "env-file", global = true, num_args = 1)]
    pub env_files: Vec<PathBuf>,
}

/// Arguments for `docker compose up`.
#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
pub struct ComposeUpArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

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

    /// Stop all services if any container exits while in foreground mode.
    /// Mirrors `docker compose up --abort-on-container-exit`. Honoured only
    /// when `--detach` is NOT set; otherwise ignored.
    #[clap(long = "abort-on-container-exit")]
    pub abort_on_container_exit: bool,

    /// Like `--abort-on-container-exit`, but the final exit code is taken
    /// from the named service (rather than from whichever container exits
    /// first). Implies `--abort-on-container-exit`. Mirrors
    /// `docker compose up --exit-code-from`.
    #[clap(long = "exit-code-from")]
    pub exit_code_from: Option<String>,
}

/// Arguments for `docker compose down`.
#[derive(Debug, Parser)]
pub struct ComposeDownArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Remove named volumes
    #[clap(short = 'v', long)]
    pub volumes: bool,

    /// Remove images (all or local)
    #[clap(long)]
    pub rmi: Option<String>,
}

/// Arguments for `docker compose ls`.
#[derive(Debug, Parser, Default)]
pub struct ComposeLsArgs {
    /// Show all projects, including stopped ones (currently the default;
    /// reserved for future status filtering).
    #[clap(short = 'a', long)]
    pub all: bool,

    /// Only display the project names.
    #[clap(short, long)]
    pub quiet: bool,
}

/// Arguments for `docker compose build`.
#[derive(Debug, Parser)]
pub struct ComposeBuildArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

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
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to list (default: all)
    pub services: Vec<String>,

    /// Only display IDs
    #[clap(short, long)]
    pub quiet: bool,
}

/// Arguments for `docker compose logs`.
#[derive(Debug, Parser)]
pub struct ComposeLogsArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

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
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

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
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to pull (default: all)
    pub services: Vec<String>,
}

/// Arguments for `docker compose push`.
#[derive(Debug, Parser)]
pub struct ComposePushArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to push (default: all)
    pub services: Vec<String>,
}

/// Arguments for `docker compose restart`.
#[derive(Debug, Parser)]
pub struct ComposeRestartArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to restart (default: all)
    pub services: Vec<String>,

    /// Seconds to wait before killing
    #[clap(short, long, default_value = "10")]
    pub timeout: u32,
}

/// Arguments for `docker compose stop`.
#[derive(Debug, Parser)]
pub struct ComposeStopArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to stop (default: all)
    pub services: Vec<String>,

    /// Seconds to wait before killing
    #[clap(short, long, default_value = "10")]
    pub timeout: u32,
}

/// Arguments for `docker compose start`.
#[derive(Debug, Parser)]
pub struct ComposeStartArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to start (default: all)
    pub services: Vec<String>,
}

/// Arguments for `docker compose kill`.
#[derive(Debug, Parser)]
pub struct ComposeKillArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to kill (default: all)
    pub services: Vec<String>,

    /// Signal to send (default `SIGKILL`).
    #[clap(short = 's', long = "signal")]
    pub signal: Option<String>,
}

/// Arguments for `docker compose rm`.
#[derive(Debug, Parser)]
pub struct ComposeRmArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to remove (default: all)
    pub services: Vec<String>,

    /// Don't ask to confirm removal. (`-f` collides with the global
    /// `--file/-f`, so we expose only the long form.)
    #[clap(long = "force")]
    pub force: bool,

    /// Stop the containers, if required, before removing.
    #[clap(short = 's', long = "stop")]
    pub stop: bool,

    /// Remove any anonymous volumes attached to the containers.
    #[clap(short = 'v', long = "volumes")]
    pub volumes: bool,
}

/// Arguments for `docker compose pause`.
#[derive(Debug, Parser)]
pub struct ComposePauseArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to pause (default: all)
    pub services: Vec<String>,
}

/// Arguments for `docker compose unpause`.
#[derive(Debug, Parser)]
pub struct ComposeUnpauseArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to unpause (default: all)
    pub services: Vec<String>,
}

/// Arguments for `docker compose run`.
#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
pub struct ComposeRunArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Service name to run.
    pub service: String,

    /// Command to run inside the container (and its arguments).
    #[clap(trailing_var_arg = true)]
    pub command: Vec<String>,

    /// Keep STDIN open even if not attached.
    #[clap(short = 'i', long)]
    pub interactive: bool,

    /// Allocate a TTY.
    #[clap(short = 't', long)]
    pub tty: bool,

    /// Override the container name.
    #[clap(long)]
    pub name: Option<String>,

    /// Remove the container automatically when it exits.
    #[clap(long)]
    pub rm: bool,

    /// Run with the service's published ports enabled.
    #[clap(long = "service-ports")]
    pub service_ports: bool,
}

/// Arguments for `docker compose config`.
#[derive(Debug, Parser)]
pub struct ComposeConfigArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Print only the service names.
    #[clap(long = "services", id = "config_services")]
    pub services: bool,

    /// Print only the volume names.
    #[clap(long = "volumes", id = "config_volumes")]
    pub volumes: bool,

    /// Print only the profile names defined in the file. The id is namespaced
    /// because the global `--profile` (singular) flag is also exposed via
    /// [`ComposeContextArgs`] and the two would otherwise collide.
    #[clap(long = "profiles", id = "config_profiles")]
    pub profiles: bool,
}

/// Arguments for `docker compose images`.
#[derive(Debug, Parser)]
pub struct ComposeImagesArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to inspect (default: all).
    pub services: Vec<String>,
}

/// Arguments for `docker compose port`.
#[derive(Debug, Parser)]
pub struct ComposePortArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Service name.
    pub service: String,

    /// Container-side port to look up.
    pub private_port: u16,

    /// Protocol (tcp / udp).
    #[clap(long = "protocol", default_value = "tcp")]
    pub protocol: String,
}

/// Arguments for `docker compose top`.
#[derive(Debug, Parser)]
pub struct ComposeTopArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to inspect (default: all).
    pub services: Vec<String>,
}

/// Arguments for `docker compose events`.
#[derive(Debug, Parser)]
pub struct ComposeEventsArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Output events as JSON lines.
    #[clap(long)]
    pub json: bool,
}

/// Arguments for `docker compose cp`.
#[derive(Debug, Parser)]
pub struct ComposeCpArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Source path (`<service>:<path>` or local path).
    pub source: String,

    /// Destination path (`<service>:<path>` or local path).
    pub destination: String,
}

/// Arguments for `docker compose wait`.
#[derive(Debug, Parser)]
pub struct ComposeWaitArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Services to wait for (default: all).
    pub services: Vec<String>,
}

/// Arguments for `docker compose watch`.
#[derive(Debug, Parser)]
pub struct ComposeWatchArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,
}

/// Arguments for `docker compose attach`.
#[derive(Debug, Parser)]
pub struct ComposeAttachArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Service name to attach to.
    pub service: String,
}

/// Arguments for `docker compose version`.
#[derive(Debug, Parser, Default)]
pub struct ComposeVersionArgs {
    /// Print only the compose CLI version.
    #[clap(long)]
    pub short: bool,
}

/// Arguments for `docker compose alpha publish`.
#[derive(Debug, Parser)]
pub struct ComposeAlphaPublishArgs {
    #[clap(skip)]
    pub ctx: ComposeContextArgs,

    /// Repository to publish to.
    pub repository: Option<String>,
}

// ---------------------------------------------------------------------------
// Loaded project: the result of resolving + parsing + interpolating + filtering
// ---------------------------------------------------------------------------

/// A fully-loaded compose project.
///
/// Built by [`load_project`] from a [`ComposeContextArgs`].  Captures every
/// piece of state the per-subcommand handlers (and the persistence layer)
/// need to act on the project:
///
/// * The resolved project name.
/// * The resolved project working directory.
/// * The exact list of compose files that were merged (absolute paths).
/// * The `--profile` activations (CLI + env, deduped) used to filter services.
/// * The `--env-file` paths that were layered into interpolation.
/// * The merged + interpolated + profile-filtered [`ComposeFile`].
#[derive(Debug)]
pub struct LoadedProject {
    /// Final, sanitised project name (ZLayer-compatible).
    pub name: String,

    /// Resolved project directory (used for relative-path resolution and
    /// stored on the [`ComposeProject`] record).
    pub working_dir: PathBuf,

    /// Compose files in the order they were merged (absolute paths).
    pub files: Vec<PathBuf>,

    /// Active `--profile` flags (already merged with `COMPOSE_PROFILES`).
    pub profiles: Vec<String>,

    /// `--env-file` paths in order, before the process env layered on top.
    pub env_files: Vec<PathBuf>,

    /// Merged + interpolated + profile-filtered compose file.
    pub compose: ComposeFile,
}

impl LoadedProject {
    /// Names of the services that survived profile filtering, sorted for
    /// stable output (e.g. for `compose ls`).
    #[must_use]
    pub fn service_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.compose.services.keys().cloned().collect();
        names.sort();
        names
    }
}

/// Default compose file names (Docker-compatible discovery order).
const DEFAULT_COMPOSE_FILES: &[&str] = &[
    "docker-compose.yaml",
    "docker-compose.yml",
    "compose.yaml",
    "compose.yml",
];

/// Resolve the project directory from CLI flags + the first `-f` path.
///
/// Priority:
/// 1. `--project-directory` if set.
/// 2. The parent directory of the first explicit `-f` file, if any.
/// 3. The process current working directory.
fn resolve_project_dir(ctx: &ComposeContextArgs) -> anyhow::Result<PathBuf> {
    if let Some(dir) = &ctx.project_directory {
        return Ok(dir.clone());
    }
    if let Some(first) = ctx.files.first() {
        // If the user gave a relative path, make it absolute against cwd
        // before taking its parent so `-f compose.yaml` doesn't end up with
        // an empty parent.
        let abs = if first.is_absolute() {
            first.clone()
        } else {
            std::env::current_dir()
                .context("failed to read current directory")?
                .join(first)
        };
        if let Some(parent) = abs.parent() {
            return Ok(parent.to_path_buf());
        }
    }
    std::env::current_dir().context("failed to read current directory")
}

/// Resolve the list of compose files to merge.
///
/// If `ctx.files` is empty, search `project_dir` for the standard names in
/// the canonical Docker-compose discovery order. Relative files are joined
/// to `project_dir`; absolute files are kept as supplied.
fn resolve_compose_files(
    project_dir: &Path,
    ctx: &ComposeContextArgs,
) -> anyhow::Result<Vec<PathBuf>> {
    if !ctx.files.is_empty() {
        let resolved = crate::compose::resolve_paths(project_dir, &ctx.files);
        for p in &resolved {
            if !p.exists() {
                anyhow::bail!("compose file not found: {}", p.display());
            }
        }
        return Ok(resolved);
    }
    for name in DEFAULT_COMPOSE_FILES {
        let candidate = project_dir.join(name);
        if candidate.exists() {
            return Ok(vec![candidate]);
        }
    }
    anyhow::bail!(
        "no compose file found in {}; tried: {}",
        project_dir.display(),
        DEFAULT_COMPOSE_FILES.join(", "),
    );
}

/// Sanitize a string so it is a valid `ZLayer` deployment name.
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

/// Resolve the project name from the CLI flag, the merged compose file's
/// `name:` field, or the project directory basename.
///
/// Priority: `--project-name` -> `compose.name` -> directory basename ->
/// `"compose"`.
fn derive_project_name(
    ctx: &ComposeContextArgs,
    project_dir: &Path,
    compose: &ComposeFile,
) -> String {
    if let Some(name) = ctx.project_name.as_ref().filter(|n| !n.is_empty()) {
        return sanitize_project_name(name);
    }
    if let Some(name) = compose.name.as_ref().filter(|n| !n.is_empty()) {
        return sanitize_project_name(name);
    }
    if let Some(dir_name) = project_dir.file_name().and_then(|s| s.to_str()) {
        if !dir_name.is_empty() {
            return sanitize_project_name(dir_name);
        }
    }
    "compose".to_string()
}

/// Apply a `--profile` filter to the in-place service map.
fn apply_profile_filter(
    compose: &mut ComposeFile,
    env: &HashMap<String, String>,
    cli_profiles: &[String],
) {
    let active = select_active_profiles(cli_profiles, env);
    let kept: HashMap<String, _> = filter_services_by_profile(&compose.services, &active)
        .into_iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    compose.services = kept;
}

/// Load and fully resolve a compose project from a [`ComposeContextArgs`].
///
/// This is the entry point shared by `up`, `down`, `ps`, `logs`, etc.  It
/// produces a [`LoadedProject`] containing all of:
/// * resolved project directory and name,
/// * the absolute list of compose files merged,
/// * the active profile set (CLI + `COMPOSE_PROFILES`),
/// * the env-files layered into interpolation,
/// * the merged + interpolated + filtered [`ComposeFile`].
///
/// # Errors
///
/// Returns an error if any file cannot be read/parsed/merged, or if env-file
/// loading or variable interpolation fails.
pub fn load_project(ctx: &ComposeContextArgs) -> anyhow::Result<LoadedProject> {
    let project_dir = resolve_project_dir(ctx)?;
    let files = resolve_compose_files(&project_dir, ctx)?;

    // Layered env. Process env wins; --env-file files in order; <dir>/.env last-resort.
    let env = collect_env_sources(&project_dir, &ctx.env_files)
        .context("failed to collect env sources")?;

    // 1. Merge files into a single RAW YAML value (no typed deserialization yet).
    let base = files.first().and_then(|p| p.parent());
    let mut value = merge_compose_files_to_value(&files)
        .map_err(|e| anyhow::anyhow!(e.to_string()))
        .with_context(|| format!("failed to merge compose files: {files:?}"))?;

    // 2. Interpolate ${VAR...} on the raw value, BEFORE the typed deserializers
    //    run. This is load-bearing: the short-port parser splits on `:`, so an
    //    un-interpolated `${PORT:-3080}:3000` would be truncated at the `:` in
    //    `:-`, leaving a brace-less `${PORT` that fails interpolation.
    interpolate_yaml_value(&mut value, &env)
        .context("failed to interpolate compose variables")?;

    // 3. Deserialize into the typed ComposeFile, then resolve `extends:`.
    let mut compose: ComposeFile =
        serde_yaml::from_value(value).context("failed to parse interpolated compose file")?;
    resolve_extends(&mut compose, base)
        .map_err(|e| anyhow::anyhow!(e.to_string()))
        .context("failed to resolve compose extends")?;

    // 4. Apply --profile filter.
    apply_profile_filter(&mut compose, &env, &ctx.profiles);

    // 4. Resolve final project name.
    let name = derive_project_name(ctx, &project_dir, &compose);

    Ok(LoadedProject {
        name,
        working_dir: project_dir,
        files,
        profiles: ctx.profiles.clone(),
        env_files: ctx.env_files.clone(),
        compose,
    })
}

/// Connect to the local zlayer daemon.
async fn connect_daemon() -> anyhow::Result<DaemonClient> {
    DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")
}

// ---------------------------------------------------------------------------
// Subcommand dispatch
// ---------------------------------------------------------------------------

/// Handle the `docker compose` subcommand.
///
/// # Errors
///
/// Returns an error if the compose file cannot be read, parsed, or converted,
/// or if the daemon request fails.
pub async fn handle_compose(cmd: ComposeCommands) -> anyhow::Result<()> {
    // Project-level flags (`-f`, `-p`, `--project-directory`, `--env-file`,
    // `--profile`) are `global = true` and flattened on the parent
    // [`ComposeCommands`], so clap collects them whether they appear *before*
    // or *after* the subcommand (Docker Compose v2 and v1 both allow the
    // pre-subcommand form). The per-subcommand `ctx` fields are `#[clap(skip)]`
    // placeholders; we merge the parsed parent `ctx` into each here so the
    // existing handler bodies (which read `args.ctx`) keep working unchanged.
    let ctx = cmd.ctx;
    // Inject the parsed project-level `ctx` into the chosen subcommand's
    // skipped placeholder, then dispatch. `dispatch!` keeps this terse so the
    // handler bodies can keep reading `args.ctx` unchanged.
    macro_rules! dispatch {
        ($args:ident, $handler:ident) => {{
            let mut args = $args;
            args.ctx = ctx;
            $handler(args).await
        }};
    }
    match cmd.command {
        ComposeSubcommand::Up(a) => dispatch!(a, handle_up),
        ComposeSubcommand::Down(a) => dispatch!(a, handle_down),
        ComposeSubcommand::Ls(a) => handle_ls(a).await,
        ComposeSubcommand::Build(a) => dispatch!(a, handle_build),
        ComposeSubcommand::Ps(a) => dispatch!(a, handle_ps),
        ComposeSubcommand::Logs(a) => dispatch!(a, handle_logs),
        ComposeSubcommand::Exec(a) => dispatch!(a, handle_exec),
        ComposeSubcommand::Pull(a) => dispatch!(a, handle_pull),
        ComposeSubcommand::Push(a) => dispatch!(a, handle_push),
        ComposeSubcommand::Restart(a) => dispatch!(a, handle_restart),
        ComposeSubcommand::Stop(a) => dispatch!(a, handle_stop),
        ComposeSubcommand::Start(a) => dispatch!(a, handle_start),
        ComposeSubcommand::Kill(a) => dispatch!(a, handle_kill),
        ComposeSubcommand::Rm(a) => dispatch!(a, handle_rm),
        ComposeSubcommand::Pause(a) => dispatch!(a, handle_pause),
        ComposeSubcommand::Unpause(a) => dispatch!(a, handle_unpause),
        ComposeSubcommand::Run(a) => dispatch!(a, handle_run),
        ComposeSubcommand::Config(a) => dispatch!(a, handle_config),
        ComposeSubcommand::Images(a) => dispatch!(a, handle_images),
        ComposeSubcommand::Port(a) => dispatch!(a, handle_port),
        ComposeSubcommand::Top(a) => dispatch!(a, handle_top),
        ComposeSubcommand::Events(a) => dispatch!(a, handle_events),
        ComposeSubcommand::Cp(a) => dispatch!(a, handle_cp),
        ComposeSubcommand::Wait(a) => dispatch!(a, handle_wait),
        ComposeSubcommand::Watch(a) => dispatch!(a, handle_watch),
        ComposeSubcommand::Attach(a) => dispatch!(a, handle_attach),
        ComposeSubcommand::Version(a) => handle_version(a).await,
        ComposeSubcommand::Alpha(sub) => match sub {
            ComposeAlphaSubcommand::Publish(a) => dispatch!(a, handle_alpha_publish),
        },
    }
}

/// `docker compose up` — convert the compose file to a `DeploymentSpec` and
/// submit it to the zlayer daemon, then record a [`ComposeProject`] entry
/// into the supplied storage so a future `compose down`/`compose ls` can see
/// it.
async fn handle_up(args: ComposeUpArgs) -> anyhow::Result<()> {
    if !args.services.is_empty() {
        eprintln!(
            "warning: service filtering on `compose up` is not yet supported; deploying all services"
        );
    }
    if args.force_recreate {
        eprintln!("warning: --force-recreate is not yet supported; ignoring");
    }

    let project = load_project(&args.ctx)?;

    // 1. Optional build pass before deploying.
    if args.build {
        let plans = crate::compose::build::plan_builds(
            &project.name,
            &project.compose,
            &project.working_dir,
            None,
            false,
            false,
        )
        .with_context(|| format!("failed to plan builds for project '{}'", project.name))?;
        if plans.is_empty() {
            tracing::info!(project = %project.name, "compose up --build: no buildable services");
        } else {
            execute_build_plans(&plans).await?;
        }
    }

    tracing::info!(
        project = %project.name,
        files = ?project.files,
        profiles = ?project.profiles,
        env_files = ?project.env_files,
        "docker compose up: converting compose to DeploymentSpec",
    );

    let spec = compose_to_deployment(&project.compose, &project.name)
        .with_context(|| format!("failed to convert compose project '{}'", project.name))?;
    let yaml =
        serde_yaml::to_string(&spec).context("failed to serialize DeploymentSpec to YAML")?;

    let client = Arc::new(connect_daemon().await?);
    let resp = client
        .create_deployment(&yaml)
        .await
        .context("failed to create deployment on daemon")?;

    let deployed_name = resp
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(project.name.as_str())
        .to_string();
    println!("Deployment '{deployed_name}' created");

    // 2. If detached, we're done.
    if args.detach {
        return Ok(());
    }

    // 3. Foreground mode: stream multiplexed logs and honour
    //    --abort-on-container-exit / --exit-code-from.
    let abort_on_exit = args.abort_on_container_exit || args.exit_code_from.is_some();
    let exit = run_foreground_up(
        client.clone(),
        &deployed_name,
        abort_on_exit,
        args.exit_code_from.as_deref(),
    )
    .await?;
    match exit {
        ForegroundExit::NoContainers => {
            eprintln!(
                "(no containers found for compose project '{deployed_name}'; deployment may still be scheduling)",
            );
            Ok(())
        }
        ForegroundExit::Code(0) => Ok(()),
        ForegroundExit::Code(code) => {
            // Non-zero exit propagates to the shell. Use std::process::exit
            // (not anyhow) so the caller sees the literal exit code rather
            // than a generic "Error: 1" line.
            std::process::exit(code);
        }
    }
}

/// `docker compose down` — delete the deployment derived from the compose
/// file (and the optional persisted [`ComposeProject`] record).
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

    let project = load_project(&args.ctx)?;

    tracing::info!(project = %project.name, "docker compose down: deleting deployment");

    let client = connect_daemon().await?;
    client
        .delete_deployment(&project.name)
        .await
        .with_context(|| format!("failed to delete deployment '{}'", project.name))?;

    println!("Deployment '{}' removed", project.name);
    Ok(())
}

/// `docker compose ls` — list compose projects recorded by the daemon.
///
/// Until a `/api/v1/compose-projects` REST surface lands, the CLI cannot read
/// the daemon's persistent compose-project store.  We surface a clear notice
/// rather than guess, so users aren't misled about why their project list
/// is empty.
#[allow(clippy::unused_async, clippy::unnecessary_wraps)]
async fn handle_ls(_args: ComposeLsArgs) -> anyhow::Result<()> {
    println!(
        "{:<28} {:<10} {:<40} {:<30}",
        "NAME", "STATUS", "CONFIG FILES", "WORKING DIR"
    );
    eprintln!(
        "(compose ls: daemon-side compose-project listing API is not yet exposed; \
         records persisted via `compose up` are stored but not yet queryable from the CLI)"
    );
    Ok(())
}

/// `docker compose ps` — list containers belonging to the compose project's
/// deployment, rendered as a Docker-compose-style table.
async fn handle_ps(args: ComposePsArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
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
        .filter(|c| container_in_deployment(c, &project.name))
        .filter(|c| match &service_filter {
            Some(services) => services
                .iter()
                .any(|s| container_matches_service(c, s, &project.name)),
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
        eprintln!(
            "(no containers found for compose project '{}')",
            project.name
        );
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

    let project = load_project(&args.ctx)?;
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
        if !container_in_deployment(c, &project.name) {
            continue;
        }
        let svc = container_service(c, &project.name).unwrap_or_else(|| "?".to_string());
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
        eprintln!("(no logs found for compose project '{}')", project.name);
    }

    Ok(())
}

/// `docker compose pull` — iterate over services and request the daemon to
/// pull each image. Services using `build:` without `image:` are skipped.
async fn handle_pull(args: ComposePullArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
    let service_filter: Option<Vec<String>> = if args.services.is_empty() {
        None
    } else {
        Some(args.services.clone())
    };

    let client = connect_daemon().await?;
    let mut pulled = 0usize;
    let mut errors = 0usize;

    for (svc_name, svc) in &project.compose.services {
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
            "compose pull finished with {errors} error(s) ({pulled} successful) in project '{}'",
            project.name
        );
    }
    Ok(())
}

/// `docker compose build` — plan a build for every service in the project
/// that ships a `build:` directive, then execute each plan via
/// [`zlayer_builder::ImageBuilder`]. Each produced image is tagged
/// `<project>-<service>:latest` so a follow-up `compose up` can resolve it
/// without an extra `--image` indirection.
async fn handle_build(args: ComposeBuildArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
    let filter: Option<&[String]> = if args.services.is_empty() {
        None
    } else {
        Some(args.services.as_slice())
    };
    let plans = crate::compose::build::plan_builds(
        &project.name,
        &project.compose,
        &project.working_dir,
        filter,
        args.no_cache,
        args.pull,
    )
    .with_context(|| format!("failed to plan builds for project '{}'", project.name))?;

    if plans.is_empty() {
        eprintln!(
            "(no buildable services in compose project '{}'; nothing to do)",
            project.name,
        );
        return Ok(());
    }

    execute_build_plans(&plans).await
}

/// Execute a sequence of [`crate::compose::build::BuildPlan`]s with the
/// `zlayer-builder` backend. Each plan produces one image, tagged with the
/// canonical `<project>-<service>:latest` (and any user-supplied `tags:`).
///
/// The `build` cargo feature is on by default for `zlayer-docker`. When
/// it's been disabled (e.g. for a stripped-down build), this function
/// surfaces an actionable error rather than silently returning success.
#[cfg(feature = "build")]
async fn execute_build_plans(plans: &[crate::compose::build::BuildPlan]) -> anyhow::Result<()> {
    use zlayer_builder::{ImageBuilder, PullBaseMode};

    for plan in plans {
        eprintln!(
            "[+] Building {svc} ({ctx})",
            svc = plan.service,
            ctx = plan.context.display(),
        );

        let mut builder = ImageBuilder::new(&plan.context).await.with_context(|| {
            format!(
                "failed to initialise image builder for service '{}'",
                plan.service
            )
        })?;
        if let Some(df) = plan.dockerfile.as_ref() {
            builder = builder.dockerfile(df);
        }
        if !plan.args.is_empty() {
            builder = builder.build_args(plan.args.clone());
        }
        if let Some(target) = plan.target.as_ref() {
            builder = builder.target(target.clone());
        }
        for cf in &plan.cache_from {
            builder = builder.cache_from(cf.clone());
        }
        for tag in &plan.tags {
            builder = builder.tag(tag.clone());
        }
        if plan.no_cache {
            builder = builder.no_cache();
        }
        if plan.pull {
            builder = builder.pull(PullBaseMode::Always);
        }

        let built = builder
            .build()
            .await
            .with_context(|| format!("failed to build service '{}'", plan.service))?;
        eprintln!(
            "[+] Built {svc} -> {tag} (id={id})",
            svc = plan.service,
            tag = plan.primary_tag(),
            id = built.image_id,
        );
    }
    Ok(())
}

/// Stub used when the `build` feature is disabled at compile time. Returns
/// an error pointing the caller at the missing feature so they can rebuild
/// with `--features build` (the default).
#[cfg(not(feature = "build"))]
async fn execute_build_plans(plans: &[crate::compose::build::BuildPlan]) -> anyhow::Result<()> {
    if plans.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "compose build: zlayer-docker was compiled without the `build` feature; \
         rebuild with `--features build` to enable image building from compose"
    )
}

/// Drive a foreground `compose up`: poll the daemon for containers in this
/// deployment, fan out one log-streaming task per container with a
/// colourised service prefix, and apply
/// `--abort-on-container-exit` / `--exit-code-from` semantics.
///
/// Returns once every running container has exited (default), or once the
/// abort condition fires (in which case the remaining containers are
/// stopped via [`zlayer_client::DaemonClient::stop_container`]).
async fn run_foreground_up(
    client: Arc<DaemonClient>,
    project: &str,
    abort_on_exit: bool,
    exit_code_from: Option<&str>,
) -> anyhow::Result<ForegroundExit> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::mpsc;

    // Discover the deployment's containers. The deployment was just
    // submitted; the scheduler may not have placed every replica yet, so
    // poll for up to ~10 seconds.
    let containers =
        wait_for_containers(&client, project, std::time::Duration::from_secs(10)).await?;
    if containers.is_empty() {
        return Ok(ForegroundExit::NoContainers);
    }

    let pad_width = containers
        .iter()
        .map(|(svc, _)| svc.chars().count())
        .max()
        .unwrap_or(0);
    let colorize = is_terminal_stdout();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let (exit_tx, mut exit_rx) = mpsc::channel::<ContainerExit>(containers.len());

    // Spawn one log-streaming task per container.
    let mut handles = Vec::with_capacity(containers.len());
    for (service, container_id) in &containers {
        let svc = service.clone();
        let id = container_id.clone();
        let client = client.clone();
        let stop_flag = stop_flag.clone();
        let exit_tx = exit_tx.clone();
        handles.push(tokio::spawn(async move {
            stream_one_service_logs(&client, &svc, &id, pad_width, colorize, stop_flag, exit_tx)
                .await;
        }));
    }
    drop(exit_tx);

    // Collect exits as they arrive; trip the abort flag when appropriate.
    let mut observed_exits = Vec::new();
    while let Some(exit) = exit_rx.recv().await {
        observed_exits.push(exit.clone());
        if should_abort_after_exit(&exit, exit_code_from, abort_on_exit) {
            stop_flag.store(true, Ordering::SeqCst);
            // Best-effort stop of every still-running container so the
            // remaining log-streaming tasks unblock quickly.
            for (_, cid) in &containers {
                if cid != &exit.container_id {
                    let _ = client.stop_container(cid, Some(10)).await;
                }
            }
            break;
        }
    }

    // Drain any exits that arrive after we tripped the abort.
    while let Ok(exit) = exit_rx.try_recv() {
        observed_exits.push(exit);
    }
    for h in handles {
        let _ = h.await;
    }

    let code = decide_foreground_exit_code(&observed_exits, exit_code_from, abort_on_exit);
    Ok(ForegroundExit::Code(code))
}

/// Poll the daemon for `(service, container_id)` pairs belonging to
/// `project` until at least one shows up or `deadline` elapses.
async fn wait_for_containers(
    client: &DaemonClient,
    project: &str,
    deadline: std::time::Duration,
) -> anyhow::Result<Vec<(String, String)>> {
    let start = std::time::Instant::now();
    loop {
        let containers = client
            .get_all_containers()
            .await
            .context("failed to fetch containers from daemon")?;
        let arr = containers.as_array().cloned().unwrap_or_default();
        let mut out = Vec::new();
        for c in &arr {
            if !container_in_deployment(c, project) {
                continue;
            }
            let svc = container_service(c, project).unwrap_or_else(|| "?".to_string());
            let Some(id) = c.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            out.push((svc, id.to_string()));
        }
        if !out.is_empty() {
            // Stable ordering for deterministic prefix-padding width.
            out.sort();
            return Ok(out);
        }
        if start.elapsed() >= deadline {
            return Ok(Vec::new());
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// Stream one container's logs to stdout with a Docker-compose-style
/// prefix, then wait for the container to exit and post the exit code on
/// `exit_tx`.
async fn stream_one_service_logs(
    client: &DaemonClient,
    service: &str,
    container_id: &str,
    pad_width: usize,
    colorize: bool,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    exit_tx: tokio::sync::mpsc::Sender<ContainerExit>,
) {
    use futures_util::StreamExt as _;
    use std::sync::atomic::Ordering;

    let stream = client
        .stream_container_logs(
            container_id,
            true,  // follow
            None,  // tail
            None,  // since
            None,  // until
            false, // timestamps
            true,  // stdout
            true,  // stderr
            true,  // format_raw
        )
        .await;
    let mut stream = match stream {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "{}",
                format_log_line(
                    service,
                    &format!("<failed to attach to logs: {e}>"),
                    pad_width,
                    colorize,
                ),
            );
            // Still wait for the container so we can report its exit code.
            let exit_code = wait_container_exit_code(client, container_id).await;
            let _ = exit_tx
                .send(ContainerExit {
                    service: service.to_string(),
                    container_id: container_id.to_string(),
                    exit_code,
                })
                .await;
            return;
        }
    };

    let mut leftover = String::new();
    while let Some(chunk) = stream.next().await {
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }
        let Ok(bytes) = chunk else { break };
        // Strip Docker stdcopy framing best-effort, then split on newlines.
        let frames = decode_stdcopy_chunks(&bytes);
        for payload in frames {
            leftover.push_str(&String::from_utf8_lossy(payload));
            while let Some(idx) = leftover.find('\n') {
                let line = leftover[..idx].to_string();
                leftover.drain(..=idx);
                println!("{}", format_log_line(service, &line, pad_width, colorize));
            }
        }
    }
    if !leftover.is_empty() {
        println!(
            "{}",
            format_log_line(service, &leftover, pad_width, colorize)
        );
    }

    let exit_code = wait_container_exit_code(client, container_id).await;
    let _ = exit_tx
        .send(ContainerExit {
            service: service.to_string(),
            container_id: container_id.to_string(),
            exit_code,
        })
        .await;
}

/// Wait for `container_id` to exit and return its numeric exit code.
/// Falls back to `1` when the daemon returns an error envelope (so the
/// caller still observes a "non-zero" exit and can propagate it).
async fn wait_container_exit_code(client: &DaemonClient, container_id: &str) -> i32 {
    match client.wait_container(container_id, None).await {
        Ok(resp) => {
            if resp.error.is_some() {
                1
            } else {
                i32::try_from(resp.status_code).unwrap_or(1)
            }
        }
        Err(_) => 1,
    }
}

/// Best-effort isatty check on stdout — gates ANSI colour escapes off when
/// the output is being piped to a file. Uses [`std::io::IsTerminal`]
/// (stable since 1.70) so no extra crate or `unsafe` block is required.
fn is_terminal_stdout() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// `docker compose push` — not yet implemented.
#[allow(clippy::unused_async, clippy::unnecessary_wraps)]
async fn handle_push(_args: ComposePushArgs) -> anyhow::Result<()> {
    eprintln!(
        "compose push: not yet implemented. Use `zlayer push` or your registry's tooling for now."
    );
    Ok(())
}

/// `docker compose exec` — interactive exec against the first replica of a service.
async fn handle_exec(args: ComposeExecArgs) -> anyhow::Result<()> {
    use zlayer_client::ExecOptions;

    if args.command.is_empty() {
        anyhow::bail!(
            "compose exec requires a command: zlayer docker compose exec <service> <cmd> [args...]",
        );
    }

    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let id = first_container_id_for_service(&client, &project.name, &args.service).await?;

    if args.detach {
        eprintln!("warning: --detach is not yet supported; running attached");
    }

    let opts = ExecOptions {
        command: args.command.clone(),
        env: Vec::new(),
        working_dir: None,
        user: None,
        privileged: false,
        tty: !args.no_tty,
        attach_stdin: !args.no_tty,
        attach_stdout: true,
        attach_stderr: true,
    };

    run_exec_session(&client, &id, opts).await
}

/// `docker compose restart` — restart the containers belonging to a project.
async fn handle_restart(args: ComposeRestartArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let ids = container_ids_for_services(
        &client,
        &project.name,
        if args.services.is_empty() {
            None
        } else {
            Some(&args.services)
        },
    )
    .await?;
    let timeout = u64::from(args.timeout);
    let mut errors = 0usize;
    for id in &ids {
        match client.restart_container(id, Some(timeout)).await {
            Ok(()) => println!("Restarted {id}"),
            Err(e) => {
                eprintln!("Error restarting {id}: {e}");
                errors += 1;
            }
        }
    }
    if errors > 0 {
        anyhow::bail!("compose restart finished with {errors} error(s)");
    }
    Ok(())
}

/// `docker compose stop` — stop the containers belonging to a project.
async fn handle_stop(args: ComposeStopArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let ids = container_ids_for_services(
        &client,
        &project.name,
        if args.services.is_empty() {
            None
        } else {
            Some(&args.services)
        },
    )
    .await?;
    let timeout = u64::from(args.timeout);
    let mut errors = 0usize;
    for id in &ids {
        match client.stop_container(id, Some(timeout)).await {
            Ok(()) => println!("Stopped {id}"),
            Err(e) => {
                eprintln!("Error stopping {id}: {e}");
                errors += 1;
            }
        }
    }
    if errors > 0 {
        anyhow::bail!("compose stop finished with {errors} error(s)");
    }
    Ok(())
}

/// `docker compose start` — start (already-created) containers for a project.
async fn handle_start(args: ComposeStartArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let ids = container_ids_for_services(
        &client,
        &project.name,
        if args.services.is_empty() {
            None
        } else {
            Some(&args.services)
        },
    )
    .await?;
    let mut errors = 0usize;
    for id in &ids {
        match client.start_container(id).await {
            Ok(()) => println!("Started {id}"),
            Err(e) => {
                eprintln!("Error starting {id}: {e}");
                errors += 1;
            }
        }
    }
    if errors > 0 {
        anyhow::bail!("compose start finished with {errors} error(s)");
    }
    Ok(())
}

/// `docker compose kill` — send a signal to a project's containers.
async fn handle_kill(args: ComposeKillArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let ids = container_ids_for_services(
        &client,
        &project.name,
        if args.services.is_empty() {
            None
        } else {
            Some(&args.services)
        },
    )
    .await?;
    let mut errors = 0usize;
    for id in &ids {
        match client.kill_container(id, args.signal.as_deref()).await {
            Ok(()) => println!("Killed {id}"),
            Err(e) => {
                eprintln!("Error killing {id}: {e}");
                errors += 1;
            }
        }
    }
    if errors > 0 {
        anyhow::bail!("compose kill finished with {errors} error(s)");
    }
    Ok(())
}

/// `docker compose rm` — remove stopped containers from a project.
async fn handle_rm(args: ComposeRmArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let ids = container_ids_for_services(
        &client,
        &project.name,
        if args.services.is_empty() {
            None
        } else {
            Some(&args.services)
        },
    )
    .await?;
    if args.volumes {
        eprintln!(
            "warning: -v/--volumes is recorded but anonymous-volume cleanup is handled by deployment teardown",
        );
    }
    let mut errors = 0usize;
    for id in &ids {
        if args.stop {
            if let Err(e) = client.stop_container(id, Some(10)).await {
                eprintln!("Error stopping {id} prior to remove: {e}");
            }
        }
        match client.delete_container(id, args.force).await {
            Ok(()) => println!("Removed {id}"),
            Err(e) => {
                eprintln!("Error removing {id}: {e}");
                errors += 1;
            }
        }
    }
    if errors > 0 {
        anyhow::bail!("compose rm finished with {errors} error(s)");
    }
    Ok(())
}

/// `docker compose pause` — pause a project's containers.
async fn handle_pause(args: ComposePauseArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let ids = container_ids_for_services(
        &client,
        &project.name,
        if args.services.is_empty() {
            None
        } else {
            Some(&args.services)
        },
    )
    .await?;
    let mut errors = 0usize;
    for id in &ids {
        match client.pause_container(id).await {
            Ok(()) => println!("Paused {id}"),
            Err(e) => {
                eprintln!("Error pausing {id}: {e}");
                errors += 1;
            }
        }
    }
    if errors > 0 {
        anyhow::bail!("compose pause finished with {errors} error(s)");
    }
    Ok(())
}

/// `docker compose unpause` — unpause a project's containers.
async fn handle_unpause(args: ComposeUnpauseArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let ids = container_ids_for_services(
        &client,
        &project.name,
        if args.services.is_empty() {
            None
        } else {
            Some(&args.services)
        },
    )
    .await?;
    let mut errors = 0usize;
    for id in &ids {
        match client.unpause_container(id).await {
            Ok(()) => println!("Unpaused {id}"),
            Err(e) => {
                eprintln!("Error unpausing {id}: {e}");
                errors += 1;
            }
        }
    }
    if errors > 0 {
        anyhow::bail!("compose unpause finished with {errors} error(s)");
    }
    Ok(())
}

/// `docker compose run` — create + start a one-off container from a service's image.
async fn handle_run(args: ComposeRunArgs) -> anyhow::Result<()> {
    use zlayer_types::api::containers::CreateContainerRequest;

    let project = load_project(&args.ctx)?;
    let svc = project.compose.services.get(&args.service).ok_or_else(|| {
        anyhow::anyhow!(
            "service '{}' not found in compose project '{}'",
            args.service,
            project.name,
        )
    })?;
    let Some(image) = svc.image.clone() else {
        anyhow::bail!(
            "service '{}' has no `image:` (compose run requires a built image; build it first or set image:)",
            args.service,
        );
    };

    // Compose's default is to NOT publish service ports for `run`. When the
    // user opts in we surface the same caveat as `docker compose` (the
    // ports come from the service entry; we leave the typed request empty).
    if args.service_ports {
        eprintln!(
            "warning: --service-ports is recorded but port forwarding is determined by the service's compose entry",
        );
    }

    let mut request = CreateContainerRequest {
        image: image.clone(),
        name: args.name.clone(),
        ..CreateContainerRequest::default()
    };
    if !args.command.is_empty() {
        request.command = Some(args.command.clone());
    }
    request.labels.insert(
        "com.docker.compose.project".to_string(),
        project.name.clone(),
    );
    request.labels.insert(
        "com.docker.compose.service".to_string(),
        args.service.clone(),
    );
    request
        .labels
        .insert("com.docker.compose.oneoff".to_string(), "True".to_string());
    if args.rm {
        request.lifecycle.delete_on_exit = true;
    }

    let client = connect_daemon().await?;
    let resp = client
        .create_container(request)
        .await
        .with_context(|| format!("failed to create one-off container for '{}'", args.service))?;

    if args.tty || args.interactive {
        eprintln!(
            "(started {} — note: stdin/PTY forwarding for `compose run` is not yet wired through; container output stream will follow)",
            short_id(&resp.id),
        );
    } else {
        println!("Started {}", short_id(&resp.id));
    }
    Ok(())
}

/// `docker compose config` — print the resolved compose YAML.
#[allow(clippy::unused_async)]
async fn handle_config(args: ComposeConfigArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;

    if args.services {
        for name in project.service_names() {
            println!("{name}");
        }
        return Ok(());
    }
    if args.volumes {
        let mut names: Vec<&String> = project.compose.volumes.keys().collect();
        names.sort();
        for name in names {
            println!("{name}");
        }
        return Ok(());
    }
    if args.profiles {
        let mut names = std::collections::BTreeSet::new();
        for svc in project.compose.services.values() {
            for p in &svc.profiles {
                names.insert(p.clone());
            }
        }
        for name in names {
            println!("{name}");
        }
        return Ok(());
    }

    let yaml = serde_yaml::to_string(&project.compose)
        .context("failed to serialize resolved compose to YAML")?;
    print!("{yaml}");
    Ok(())
}

/// `docker compose images` — list images in use by service containers.
async fn handle_images(args: ComposeImagesArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
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

    println!(
        "{:<28} {:<35} {:<15} {:<20} {:<10}",
        "CONTAINER", "REPOSITORY", "TAG", "IMAGE_ID", "SIZE"
    );

    for c in arr {
        if !container_in_deployment(c, &project.name) {
            continue;
        }
        if let Some(ref filter) = service_filter {
            if !container_service(c, &project.name)
                .as_deref()
                .is_some_and(|svc| filter.iter().any(|f| f == svc))
            {
                continue;
            }
        }
        let name = c
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| c.get("id").and_then(|v| v.as_str()))
            .unwrap_or("-");
        let image_ref = c
            .get("image")
            .and_then(|v| v.as_str())
            .or_else(|| c.get("Image").and_then(|v| v.as_str()))
            .unwrap_or("-");
        let (repo, tag) = split_image_ref(image_ref);
        let image_id = c
            .get("image_id")
            .and_then(|v| v.as_str())
            .or_else(|| c.get("ImageID").and_then(|v| v.as_str()))
            .unwrap_or("-");
        let image_id_short = if image_id.len() > 12 {
            &image_id[..12]
        } else {
            image_id
        };
        let size = c
            .get("size")
            .and_then(serde_json::Value::as_u64)
            .map_or_else(|| "-".to_string(), format_size_human);
        println!("{name:<28} {repo:<35} {tag:<15} {image_id_short:<20} {size:<10}");
    }
    Ok(())
}

/// `docker compose port` — print the host-side mapping for a service+port.
async fn handle_port(args: ComposePortArgs) -> anyhow::Result<()> {
    let proto = args.protocol.to_lowercase();
    if proto != "tcp" && proto != "udp" {
        anyhow::bail!("--protocol must be 'tcp' or 'udp', got '{}'", args.protocol);
    }

    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let id = first_container_id_for_service(&client, &project.name, &args.service).await?;
    let body = client
        .container_port(&id)
        .await
        .with_context(|| format!("failed to fetch port map for '{id}'"))?;

    let key = format!("{}/{}", args.private_port, proto);
    let Some(map) = body.get("Ports").and_then(|v| v.as_object()) else {
        eprintln!("(no ports map returned for container '{id}')");
        return Ok(());
    };
    let Some(bindings) = map.get(&key).and_then(|v| v.as_array()) else {
        eprintln!("(no host binding for {key} on '{id}')");
        return Ok(());
    };
    for binding in bindings {
        let host_ip = binding
            .get("HostIp")
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0.0");
        let host_port = binding
            .get("HostPort")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        println!("{host_ip}:{host_port}");
    }
    Ok(())
}

/// `docker compose top` — list processes inside each service container.
async fn handle_top(args: ComposeTopArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let ids = container_ids_for_services(
        &client,
        &project.name,
        if args.services.is_empty() {
            None
        } else {
            Some(&args.services)
        },
    )
    .await?;

    if ids.is_empty() {
        eprintln!(
            "(no running containers found for compose project '{}')",
            project.name,
        );
        return Ok(());
    }

    let mut errors = 0usize;
    for id in &ids {
        match client.top_container(id, None).await {
            Ok(body) => {
                println!("== {id} ==");
                let titles = body
                    .get("Titles")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|t| t.as_str())
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let processes = body
                    .get("Processes")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|row| row.as_array())
                            .map(|row| {
                                row.iter()
                                    .filter_map(|cell| cell.as_str())
                                    .map(str::to_string)
                                    .collect::<Vec<_>>()
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if titles.is_empty() {
                    println!("{}", serde_json::to_string_pretty(&body)?);
                } else {
                    println!("{}", titles.join("\t"));
                    for row in &processes {
                        println!("{}", row.join("\t"));
                    }
                }
            }
            Err(e) => {
                eprintln!("Error fetching top for {id}: {e}");
                errors += 1;
            }
        }
    }
    if errors > 0 {
        anyhow::bail!("compose top finished with {errors} error(s)");
    }
    Ok(())
}

/// `docker compose events` — stream events filtered by project label.
async fn handle_events(args: ComposeEventsArgs) -> anyhow::Result<()> {
    use futures_util::StreamExt as _;

    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let filters = vec![(
        "com.docker.compose.project".to_string(),
        project.name.clone(),
    )];

    let mut stream = client
        .events_stream(true, &filters)
        .await
        .context("failed to open events stream")?;

    while let Some(item) = stream.next().await {
        match item {
            Ok(event) => {
                if args.json {
                    match serde_json::to_string(&event) {
                        Ok(line) => println!("{line}"),
                        Err(e) => eprintln!("(failed to encode event as JSON: {e})"),
                    }
                } else {
                    println!(
                        "{} {} {}",
                        event.at().to_rfc3339(),
                        event.resource(),
                        event.action(),
                    );
                }
            }
            Err(e) => {
                eprintln!("(events stream error: {e})");
                break;
            }
        }
    }
    Ok(())
}

/// `docker compose cp` — wraps `docker cp` and resolves `<service>:<path>`.
async fn handle_cp(args: ComposeCpArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;

    let resolved_src = resolve_cp_endpoint(&client, &project.name, &args.source).await?;
    let resolved_dst = resolve_cp_endpoint(&client, &project.name, &args.destination).await?;

    crate::cli::cp::handle_cp(crate::cli::cp::CpArgs {
        source: resolved_src,
        destination: resolved_dst,
    })
    .await
}

/// `docker compose wait` — block until each service container exits.
async fn handle_wait(args: ComposeWaitArgs) -> anyhow::Result<()> {
    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let ids = container_ids_for_services(
        &client,
        &project.name,
        if args.services.is_empty() {
            None
        } else {
            Some(&args.services)
        },
    )
    .await?;

    let mut last_status: i64 = 0;
    for id in &ids {
        match client.wait_container(id, None).await {
            Ok(resp) => {
                println!("{id} {}", resp.status_code);
                last_status = resp.status_code;
            }
            Err(e) => {
                eprintln!("Error waiting on {id}: {e}");
                last_status = 125;
            }
        }
    }
    if last_status != 0 {
        // Docker exits with a clamped 8-bit status when the wait result
        // overflows. Mirror that to keep our exit codes shell-friendly.
        let clamped = i32::try_from(last_status).unwrap_or(125);
        std::process::exit(clamped);
    }
    Ok(())
}

/// `docker compose watch` — not yet implemented.
#[allow(clippy::unused_async, clippy::needless_pass_by_value)]
async fn handle_watch(_args: ComposeWatchArgs) -> anyhow::Result<()> {
    anyhow::bail!("compose watch is not yet supported")
}

/// `docker compose attach` — attach to a service container's stdio.
async fn handle_attach(args: ComposeAttachArgs) -> anyhow::Result<()> {
    use futures_util::StreamExt as _;
    use std::io::Write as _;

    let project = load_project(&args.ctx)?;
    let client = connect_daemon().await?;
    let id = first_container_id_for_service(&client, &project.name, &args.service).await?;

    let mut stream = client
        .stream_container_logs(
            &id, true,  // follow
            None,  // tail
            None,  // since
            None,  // until
            false, // timestamps
            true,  // stdout
            true,  // stderr
            true,  // format_raw (stdcopy framing)
        )
        .await
        .context("failed to open log stream for attach")?;

    eprintln!(
        "(attached to compose project '{}' service '{}' container {} — Ctrl-C to detach)",
        project.name,
        args.service,
        short_id(&id),
    );

    let mut stdout = std::io::stdout();
    loop {
        tokio::select! {
            res = tokio::signal::ctrl_c() => {
                res.context("failed to install Ctrl-C handler")?;
                break;
            }
            chunk = stream.next() => match chunk {
                Some(Ok(bytes)) => {
                    for payload in decode_stdcopy_chunks(&bytes) {
                        if stdout.write_all(payload).is_err() {
                            return Ok(());
                        }
                    }
                    let _ = stdout.flush();
                }
                Some(Err(e)) => {
                    eprintln!("attach stream error: {e}");
                    break;
                }
                None => break,
            }
        }
    }
    Ok(())
}

/// `docker compose version` — print daemon + compose CLI version.
async fn handle_version(args: ComposeVersionArgs) -> anyhow::Result<()> {
    let cli_version = env!("CARGO_PKG_VERSION");
    if args.short {
        println!("{cli_version}");
        return Ok(());
    }

    let client = connect_daemon().await;
    let daemon_state = match client {
        Ok(c) => match c.health_check().await {
            Ok(true) => "running",
            Ok(false) => "unreachable",
            Err(_) => "error",
        },
        Err(_) => "unreachable",
    };

    println!("Docker Compose (zlayer) version v{cli_version}");
    println!("Daemon: {daemon_state}");
    Ok(())
}

/// `docker compose alpha publish` — not yet supported.
#[allow(clippy::unused_async, clippy::needless_pass_by_value)]
async fn handle_alpha_publish(_args: ComposeAlphaPublishArgs) -> anyhow::Result<()> {
    anyhow::bail!("compose alpha publish is not yet supported")
}

// ---------------------------------------------------------------------------
// Daemon helpers shared across the lifecycle handlers
// ---------------------------------------------------------------------------

/// Best-effort decoder for Docker's stdcopy framing (mirrors the helper in
/// `cli::run`). Each frame is `[stream:u8, 0,0,0, BE_u32(len), payload...]`.
fn decode_stdcopy_chunks(buf: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut cursor = 0;
    let mut looked_like_frame = false;

    while cursor + 8 <= buf.len() {
        let stream = buf[cursor];
        if stream > 2 || buf[cursor + 1] != 0 || buf[cursor + 2] != 0 || buf[cursor + 3] != 0 {
            break;
        }
        let len = u32::from_be_bytes([
            buf[cursor + 4],
            buf[cursor + 5],
            buf[cursor + 6],
            buf[cursor + 7],
        ]) as usize;
        let frame_end = cursor + 8 + len;
        if frame_end > buf.len() {
            break;
        }
        looked_like_frame = true;
        out.push(&buf[cursor + 8..frame_end]);
        cursor = frame_end;
    }
    if !looked_like_frame {
        return vec![buf];
    }
    out
}

/// Truncate an id to its 12-char short form (no panics on shorter inputs).
fn short_id(id: &str) -> &str {
    if id.len() > 12 {
        &id[..12]
    } else {
        id
    }
}

/// Split an OCI image reference into `(repository, tag)` for tabular output.
fn split_image_ref(image: &str) -> (String, String) {
    if image == "-" || image.is_empty() {
        return ("-".to_string(), "-".to_string());
    }
    // Strip optional digest first (`@sha256:...`).
    let (head, _digest) = image.split_once('@').unwrap_or((image, ""));
    // Tag is the segment after the *last* colon, but only when that segment
    // contains no slash (a slash means it's part of a registry port, not a tag).
    if let Some(idx) = head.rfind(':') {
        let after = &head[idx + 1..];
        if !after.contains('/') {
            return (head[..idx].to_string(), after.to_string());
        }
    }
    (head.to_string(), "latest".to_string())
}

/// Format a byte count as a Docker-style human-readable size.
///
/// We render the size to one decimal at the appropriate SI/IEC tier. The
/// `f64` cast is intentional: byte sizes above 2^53 would need IEEE-754
/// extended precision, which doesn't matter for image sizes (max plausible
/// size is small TB-class). The `cast_precision_loss` lint is suppressed
/// because we accept the precision tradeoff for human-readable output.
#[allow(clippy::cast_precision_loss)]
fn format_size_human(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes}B")
    }
}

/// Return every container id belonging to the named compose project,
/// optionally filtered to a list of service names.
async fn container_ids_for_services(
    client: &DaemonClient,
    project: &str,
    services: Option<&[String]>,
) -> anyhow::Result<Vec<String>> {
    let containers = client
        .get_all_containers()
        .await
        .context("failed to fetch containers from daemon")?;
    let Some(arr) = containers.as_array() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for c in arr {
        if !container_in_deployment(c, project) {
            continue;
        }
        if let Some(filter) = services {
            let svc_name = container_service(c, project);
            if !svc_name
                .as_deref()
                .is_some_and(|s| filter.iter().any(|f| f == s))
            {
                continue;
            }
        }
        if let Some(id) = c.get("id").and_then(|v| v.as_str()) {
            out.push(id.to_string());
        }
    }
    Ok(out)
}

/// Look up the first (lowest-replica-index) container id for a service in a
/// compose project.
async fn first_container_id_for_service(
    client: &DaemonClient,
    project: &str,
    service: &str,
) -> anyhow::Result<String> {
    let services = vec![service.to_string()];
    let ids = container_ids_for_services(client, project, Some(&services)).await?;
    ids.into_iter().next().ok_or_else(|| {
        anyhow::anyhow!(
            "no running container found for compose project '{project}' service '{service}'",
        )
    })
}

/// Drive an interactive `start_exec_pty` session: streams binary frames
/// to stdout and forwards local stdin to the daemon. On daemon close,
/// returns the exit code (or `Ok(())` when missing).
async fn run_exec_session(
    client: &DaemonClient,
    container_id: &str,
    opts: zlayer_client::ExecOptions,
) -> anyhow::Result<()> {
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use futures_util::StreamExt as _;
    use std::io::Write as _;

    let tty = opts.tty;
    let exec_id = client
        .create_exec(container_id, opts)
        .await
        .with_context(|| format!("failed to create exec on container '{container_id}'"))?;

    let mut conn = client
        .start_exec_pty(&exec_id, tty)
        .await
        .with_context(|| format!("failed to start exec '{exec_id}'"))?;

    if tty {
        enable_raw_mode().context("failed to enable raw mode on local TTY")?;
    }

    let mut stdout = std::io::stdout();
    let exit_code: Option<i32> = loop {
        tokio::select! {
            chunk = conn.reader.next() => match chunk {
                Some(Ok(bytes)) => {
                    let chunks: Vec<&[u8]> = if tty {
                        vec![&bytes]
                    } else {
                        decode_stdcopy_chunks(&bytes)
                    };
                    for payload in chunks {
                        let _ = stdout.write_all(payload);
                    }
                    let _ = stdout.flush();
                }
                Some(Err(e)) => {
                    if tty { let _ = disable_raw_mode(); }
                    return Err(anyhow::anyhow!("exec stream error: {e}"));
                }
                None => break None,
            },
            res = &mut conn.exit => {
                break res?;
            }
        }
    };

    if tty {
        let _ = disable_raw_mode();
        let _ = stdout.write_all(b"\r\n");
        let _ = stdout.flush();
    }

    if let Some(code) = exit_code {
        if code != 0 {
            std::process::exit(code);
        }
    }
    Ok(())
}

/// Resolve a `compose cp` endpoint by translating `<service>:<path>` into
/// `<container_id>:<path>`. Local paths are returned unchanged.
async fn resolve_cp_endpoint(
    client: &DaemonClient,
    project: &str,
    raw: &str,
) -> anyhow::Result<String> {
    if let Some((maybe_service, rest)) = raw.split_once(':') {
        let looks_like_service = !(maybe_service.is_empty()
            || maybe_service.contains('/')
            || maybe_service.contains('\\')
            || (maybe_service.len() == 1
                && maybe_service.chars().all(|c| c.is_ascii_alphabetic())));
        if looks_like_service {
            let id = first_container_id_for_service(client, project, maybe_service).await?;
            return Ok(format!("{id}:{rest}"));
        }
    }
    Ok(raw.to_string())
}

// ---------------------------------------------------------------------------
// Storage hook: testable persistence used to back compose up/down/ls
// ---------------------------------------------------------------------------

/// Persist a [`ComposeProject`] for a successful `compose up`.
///
/// Separated from [`handle_up`] so unit tests can drive it against an
/// [`zlayer_api::InMemoryComposeProjectStorage`] without spinning up a
/// daemon or HTTP layer.
///
/// # Errors
///
/// Propagates any [`zlayer_api::storage::StorageError`] from the upsert.
pub async fn persist_compose_project(
    storage: &Arc<dyn ComposeProjectStorage>,
    project: &LoadedProject,
    container_ids: Vec<String>,
) -> anyhow::Result<()> {
    let record = ComposeProject {
        name: project.name.clone(),
        working_dir: project.working_dir.clone(),
        files: project.files.clone(),
        profiles: project.profiles.clone(),
        env_files: project.env_files.clone(),
        services: project.service_names(),
        container_ids,
        created_at: Utc::now(),
    };
    storage
        .upsert(record)
        .await
        .context("failed to upsert compose project")?;
    Ok(())
}

/// List compose projects from a [`ComposeProjectStorage`] backend.
///
/// # Errors
///
/// Propagates any storage error.
pub async fn list_compose_projects(
    storage: &Arc<dyn ComposeProjectStorage>,
) -> anyhow::Result<Vec<ComposeProject>> {
    let mut all = storage
        .list()
        .await
        .context("failed to list compose projects")?;
    all.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(all)
}

/// Delete a compose project record from storage.
///
/// # Errors
///
/// Propagates any storage error.
pub async fn delete_compose_project(
    storage: &Arc<dyn ComposeProjectStorage>,
    name: &str,
) -> anyhow::Result<bool> {
    storage
        .delete(name)
        .await
        .context("failed to delete compose project")
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

// ---------------------------------------------------------------------------
// Foreground `compose up` helpers — log multiplexing + abort coordination
// ---------------------------------------------------------------------------

/// Fixed ANSI palette used to colourise per-service log prefixes when
/// `compose up` runs in the foreground. Mirrors the colours `docker compose`
/// emits (cyan, yellow, green, magenta, blue, red, ...). Picked
/// deterministically from the service name so re-running `compose up` paints
/// each service the same colour every time.
const FOREGROUND_LOG_COLORS: &[&str] = &[
    "\x1b[36m", // cyan
    "\x1b[33m", // yellow
    "\x1b[32m", // green
    "\x1b[35m", // magenta
    "\x1b[34m", // blue
    "\x1b[31m", // red
    "\x1b[96m", // bright cyan
    "\x1b[93m", // bright yellow
];
const ANSI_RESET: &str = "\x1b[0m";

/// Pick a stable colour for a service name. The returned index addresses
/// [`FOREGROUND_LOG_COLORS`] modulo its length, so the mapping is
/// deterministic across runs and unit-testable without any global state.
#[must_use]
pub fn service_color_index(service: &str) -> usize {
    // FNV-1a 32 — small, dependency-free, and stable enough for a palette
    // selection that just has to be repeatable. 32-bit FNV fits losslessly
    // into `usize` on every platform we target without an `as` cast.
    let mut hash: u32 = 0x811c_9dc5;
    for b in service.as_bytes() {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    (hash as usize) % FOREGROUND_LOG_COLORS.len()
}

/// Format one log line with a Docker-compose-style colourised service prefix.
///
/// Output shape: `<color><svc-padded><reset> | <line>`. The padding width is
/// supplied by the caller so every service in the multiplex can align on the
/// same column (mirrors `docker compose`'s `printer.go`).
///
/// `colorize: false` produces plain `"<svc-padded> | <line>"` so callers
/// targeting non-tty output skip the ANSI escapes entirely.
#[must_use]
pub fn format_log_line(service: &str, line: &str, pad_width: usize, colorize: bool) -> String {
    let trimmed = line.strip_suffix('\n').unwrap_or(line);
    let pad = pad_width.max(service.chars().count());
    let padded = format!("{service:<pad$}");
    if colorize {
        let color = FOREGROUND_LOG_COLORS[service_color_index(service)];
        format!("{color}{padded}{ANSI_RESET} | {trimmed}")
    } else {
        format!("{padded} | {trimmed}")
    }
}

/// Outcome of a foreground `compose up` session.
///
/// Drives the process exit code: `Ok(code)` is propagated to
/// `std::process::exit`, while `Err` is treated as an internal failure
/// (transport error, control-channel poisoned, ...) and surfaces as an
/// `anyhow::Error` to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForegroundExit {
    /// All services exited (or the user pressed Ctrl-C). Carries the
    /// numeric exit code chosen by `decide_foreground_exit_code`.
    Code(i32),
    /// No containers ever showed up — informational signal so the caller
    /// can print a different message before exiting cleanly.
    NoContainers,
}

/// One container's exit observation, captured by the foreground driver as
/// services finish. The driver hands a [`Vec`] of these to
/// [`decide_foreground_exit_code`] when deciding what to surface to the
/// shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerExit {
    /// Compose service the container belonged to.
    pub service: String,
    /// Container id, useful for diagnostics.
    pub container_id: String,
    /// Numeric exit code reported by the runtime (or 137 / `128+SIGKILL`
    /// when forcibly stopped).
    pub exit_code: i32,
}

/// Choose the foreground exit code, matching `docker compose`'s rules:
///
/// * `--exit-code-from <svc>` wins: the matching service's container is
///   located by exit-time priority (first one to exit). When no such
///   container exited, fall back to `0` and warn — Compose's behaviour.
/// * `--abort-on-container-exit` (without `--exit-code-from`) returns the
///   first observed container's exit code.
/// * Otherwise, returns `0` once every service has exited cleanly, or the
///   highest observed non-zero exit code if any service failed.
#[must_use]
pub fn decide_foreground_exit_code(
    exits: &[ContainerExit],
    exit_code_from: Option<&str>,
    abort_on_exit: bool,
) -> i32 {
    if let Some(svc) = exit_code_from {
        if let Some(exit) = exits.iter().find(|e| e.service == svc) {
            return exit.exit_code;
        }
        // No container with the named service exited — propagate 0,
        // matching docker compose v2.
        return 0;
    }
    if abort_on_exit {
        return exits.first().map_or(0, |e| e.exit_code);
    }
    // Default: any non-zero exit propagates the highest observed code.
    exits.iter().map(|e| e.exit_code).max().unwrap_or(0)
}

/// Decide whether the foreground driver should signal "stop everything"
/// after observing `exit`. Pulled out so the abort logic can be unit-tested
/// without owning a tokio runtime.
///
/// Returns `true` when:
/// * `exit_code_from = Some(svc)` and `exit.service == svc` (the named
///   service has exited; we have our final code), OR
/// * `abort_on_exit = true` (any container exit triggers a global stop).
///
/// Returns `false` otherwise (we keep streaming logs from the rest of the
/// services and let them run to completion).
#[must_use]
pub fn should_abort_after_exit(
    exit: &ContainerExit,
    exit_code_from: Option<&str>,
    abort_on_exit: bool,
) -> bool {
    if let Some(svc) = exit_code_from {
        return exit.service == svc;
    }
    abort_on_exit
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zlayer_api::InMemoryComposeProjectStorage;

    fn write_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

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

    // ----------------------------------------------------------------------
    // load_project — multi-file merge, project-name override, profile filter
    // ----------------------------------------------------------------------

    #[test]
    fn load_project_merges_two_files_in_order() {
        // Two compose files: the second overrides the first's image and adds a service.
        let dir = ZLayerDirs::system_default()
            .scratch_dir("load-project-merges-two-files-in-order-")
            .unwrap();
        let _ = write_file(
            dir.path(),
            "base.yaml",
            r"
services:
  web:
    image: nginx:1.24
  db:
    image: postgres:15
",
        );
        let _ = write_file(
            dir.path(),
            "override.yaml",
            r"
services:
  web:
    image: nginx:1.25
  cache:
    image: redis:7
",
        );

        let ctx = ComposeContextArgs {
            project_directory: Some(dir.path().to_path_buf()),
            files: vec![PathBuf::from("base.yaml"), PathBuf::from("override.yaml")],
            ..Default::default()
        };
        let project = load_project(&ctx).unwrap();

        assert_eq!(
            project
                .compose
                .services
                .get("web")
                .and_then(|s| s.image.as_deref()),
            Some("nginx:1.25"),
            "later file overrides earlier",
        );
        assert_eq!(
            project
                .compose
                .services
                .get("db")
                .and_then(|s| s.image.as_deref()),
            Some("postgres:15"),
            "earlier-only entries are preserved",
        );
        assert!(
            project.compose.services.contains_key("cache"),
            "later-only entries are added",
        );
        assert_eq!(project.files.len(), 2);
    }

    #[test]
    fn load_project_project_name_flag_overrides_default() {
        // Default would derive from the directory basename. Confirm that
        // `--project-name` wins and is sanitised.
        let dir = ZLayerDirs::system_default()
            .scratch_dir("load-project-project-name-flag-overrides-default-")
            .unwrap();
        write_file(
            dir.path(),
            "compose.yaml",
            r"
services:
  web:
    image: nginx
",
        );

        // Without override: uses sanitised dir name.
        let ctx_default = ComposeContextArgs {
            project_directory: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let proj_default = load_project(&ctx_default).unwrap();
        assert_ne!(proj_default.name, "Custom_Override");

        // With override: explicit name wins (and is sanitised to lowercase).
        let ctx_named = ComposeContextArgs {
            project_directory: Some(dir.path().to_path_buf()),
            project_name: Some("Custom_Override".to_string()),
            ..Default::default()
        };
        let proj_named = load_project(&ctx_named).unwrap();
        assert_eq!(proj_named.name, "custom-override");
    }

    #[test]
    fn load_project_profile_filter_drops_inactive_services() {
        // `web` has no profiles (always active); `debug` requires the
        // `debug` profile.  Confirm filtering happens during load.
        let dir = ZLayerDirs::system_default()
            .scratch_dir("load-project-profile-filter-drops-inactive-services-")
            .unwrap();
        write_file(
            dir.path(),
            "compose.yaml",
            r"
services:
  web:
    image: nginx
  debug:
    image: alpine
    profiles: ['debug']
",
        );

        // No profiles active: only `web` survives.
        let ctx_none = ComposeContextArgs {
            project_directory: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        let proj_none = load_project(&ctx_none).unwrap();
        assert!(proj_none.compose.services.contains_key("web"));
        assert!(!proj_none.compose.services.contains_key("debug"));

        // `debug` profile active: both survive.
        let ctx_debug = ComposeContextArgs {
            project_directory: Some(dir.path().to_path_buf()),
            profiles: vec!["debug".to_string()],
            ..Default::default()
        };
        let proj_debug = load_project(&ctx_debug).unwrap();
        assert!(proj_debug.compose.services.contains_key("web"));
        assert!(proj_debug.compose.services.contains_key("debug"));
    }

    // ----------------------------------------------------------------------
    // Persistence (mocked storage) — up persists, ls reads, down deletes.
    // ----------------------------------------------------------------------

    fn make_loaded(name: &str, dir: &Path) -> LoadedProject {
        LoadedProject {
            name: name.to_string(),
            working_dir: dir.to_path_buf(),
            files: vec![dir.join("compose.yaml")],
            profiles: vec!["dev".to_string()],
            env_files: vec![dir.join(".env")],
            compose: ComposeFile {
                version: None,
                name: None,
                include: Vec::new(),
                services: HashMap::new(),
                volumes: HashMap::new(),
                networks: HashMap::new(),
                secrets: HashMap::new(),
                configs: HashMap::new(),
                extensions: HashMap::new(),
            },
        }
    }

    #[tokio::test]
    async fn persist_lists_and_deletes_via_storage() {
        let storage: Arc<dyn ComposeProjectStorage> =
            Arc::new(InMemoryComposeProjectStorage::new());
        let dir = ZLayerDirs::system_default()
            .scratch_dir("persist-lists-and-deletes-via-storage-")
            .unwrap();

        // 1. compose up persists a record.
        let project = make_loaded("alpha", dir.path());
        persist_compose_project(
            &storage,
            &project,
            vec!["alpha-web-0".to_string(), "alpha-db-0".to_string()],
        )
        .await
        .expect("upsert");

        // 2. compose ls reads it.
        let listed = list_compose_projects(&storage).await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "alpha");
        assert_eq!(listed[0].profiles, vec!["dev".to_string()]);
        assert_eq!(
            listed[0].container_ids,
            vec!["alpha-web-0".to_string(), "alpha-db-0".to_string()]
        );
        assert_eq!(listed[0].working_dir, dir.path());

        // 3. compose down deletes it.
        let removed = delete_compose_project(&storage, "alpha")
            .await
            .expect("delete");
        assert!(removed, "first delete reports a row was removed");

        let listed_after = list_compose_projects(&storage).await.expect("list");
        assert!(listed_after.is_empty());

        // 4. compose down on a missing project is a no-op (returns false).
        let removed_again = delete_compose_project(&storage, "alpha")
            .await
            .expect("delete");
        assert!(!removed_again);
    }

    #[tokio::test]
    async fn persist_overwrites_existing_record() {
        let storage: Arc<dyn ComposeProjectStorage> =
            Arc::new(InMemoryComposeProjectStorage::new());
        let dir = ZLayerDirs::system_default()
            .scratch_dir("persist-overwrites-existing-record-")
            .unwrap();
        let project = make_loaded("beta", dir.path());

        persist_compose_project(&storage, &project, vec!["beta-web-0".to_string()])
            .await
            .expect("first upsert");
        persist_compose_project(&storage, &project, vec!["beta-web-1".to_string()])
            .await
            .expect("second upsert");

        let listed = list_compose_projects(&storage).await.expect("list");
        assert_eq!(listed.len(), 1, "upsert overwrites, not appends");
        assert_eq!(listed[0].container_ids, vec!["beta-web-1".to_string()]);
    }

    // ----------------------------------------------------------------------
    // 6.5.1-14: lifecycle + inspection subcommand parse tests.
    //
    // Each subcommand's clap surface is parsed end-to-end so a regression
    // (e.g. an accidental rename of `--protocol` or a missing flag) fails
    // here rather than at user-call-time.
    // ----------------------------------------------------------------------

    fn parse_compose<I, T>(args: I) -> ComposeCommands
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        ComposeCommands::try_parse_from(
            std::iter::once::<std::ffi::OsString>("compose".into())
                .chain(args.into_iter().map(Into::into)),
        )
        .unwrap()
    }

    #[test]
    fn parses_stop_subcommand() {
        let cmd = parse_compose(["stop", "-p", "myproj", "web", "db"]);
        // Project-level flags land on the parent `ctx` (they are `global` and
        // flattened on `ComposeCommands`), not on the subcommand's skipped one.
        assert_eq!(cmd.ctx.project_name.as_deref(), Some("myproj"));
        match cmd.command {
            ComposeSubcommand::Stop(args) => {
                assert_eq!(args.services, vec!["web", "db"]);
                assert_eq!(args.timeout, 10);
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_start_subcommand() {
        let cmd = parse_compose(["start", "-p", "myproj"]);
        assert_eq!(cmd.ctx.project_name.as_deref(), Some("myproj"));
        match cmd.command {
            ComposeSubcommand::Start(args) => {
                assert!(args.services.is_empty());
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    /// Regression: Docker Compose accepts project-level flags *before* the
    /// subcommand (`docker compose -f X -p Y config`). clap must collect those
    /// leading flags into the parent `ctx` rather than rejecting them as
    /// "unexpected argument", and the value must match the post-subcommand form.
    #[test]
    fn parses_pre_subcommand_project_flags() {
        // Leading -f / -p / --project-directory / --env-file / --profile.
        let cmd = parse_compose([
            "-f",
            "foo.yml",
            "-p",
            "myproj",
            "--project-directory",
            "/srv/app",
            "--env-file",
            ".env.prod",
            "--profile",
            "debug",
            "config",
            "--services",
        ]);
        assert_eq!(cmd.ctx.files, vec![PathBuf::from("foo.yml")]);
        assert_eq!(cmd.ctx.project_name.as_deref(), Some("myproj"));
        assert_eq!(
            cmd.ctx.project_directory.as_deref(),
            Some(Path::new("/srv/app"))
        );
        assert_eq!(cmd.ctx.env_files, vec![PathBuf::from(".env.prod")]);
        assert_eq!(cmd.ctx.profiles, vec!["debug".to_string()]);
        assert!(matches!(cmd.command, ComposeSubcommand::Config(_)));
    }

    /// The pre-subcommand and post-subcommand forms must parse identically:
    /// `compose -f X config` == `compose config -f X`. Repeated `-f` flags on
    /// the *same* side of the subcommand collect in order.
    #[test]
    fn pre_and_post_subcommand_flags_are_equivalent() {
        let pre = parse_compose(["-f", "a.yml", "-f", "b.yml", "config"]);
        let post = parse_compose(["config", "-f", "a.yml", "-f", "b.yml"]);
        let expected = vec![PathBuf::from("a.yml"), PathBuf::from("b.yml")];
        assert_eq!(pre.ctx.files, expected);
        assert_eq!(post.ctx.files, expected);
    }

    #[test]
    fn parses_restart_subcommand_with_timeout() {
        let cmd = parse_compose(["restart", "-p", "p", "-t", "30", "web"]);
        match cmd.command {
            ComposeSubcommand::Restart(args) => {
                assert_eq!(args.timeout, 30);
                assert_eq!(args.services, vec!["web"]);
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_kill_subcommand_with_signal() {
        let cmd = parse_compose(["kill", "-p", "p", "-s", "SIGTERM", "web"]);
        match cmd.command {
            ComposeSubcommand::Kill(args) => {
                assert_eq!(args.signal.as_deref(), Some("SIGTERM"));
                assert_eq!(args.services, vec!["web"]);
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_rm_subcommand_with_flags() {
        let cmd = parse_compose(["rm", "-p", "p", "--force", "-s", "-v", "web"]);
        match cmd.command {
            ComposeSubcommand::Rm(args) => {
                assert!(args.force);
                assert!(args.stop);
                assert!(args.volumes);
                assert_eq!(args.services, vec!["web"]);
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_pause_and_unpause_subcommands() {
        let cmd_pause = parse_compose(["pause", "-p", "p", "web"]);
        match cmd_pause.command {
            ComposeSubcommand::Pause(args) => assert_eq!(args.services, vec!["web"]),
            other => panic!("unexpected subcommand: {other:?}"),
        }
        let cmd_unpause = parse_compose(["unpause", "-p", "p", "web"]);
        match cmd_unpause.command {
            ComposeSubcommand::Unpause(args) => assert_eq!(args.services, vec!["web"]),
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_run_subcommand_with_flags() {
        let cmd = parse_compose([
            "run", "-p", "p", "-i", "-t", "--rm", "--name", "x", "web", "echo", "hi",
        ]);
        match cmd.command {
            ComposeSubcommand::Run(args) => {
                assert!(args.interactive);
                assert!(args.tty);
                assert!(args.rm);
                assert_eq!(args.name.as_deref(), Some("x"));
                assert_eq!(args.service, "web");
                assert_eq!(args.command, vec!["echo", "hi"]);
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_config_subcommand_filter_flags() {
        let cmd = parse_compose(["config", "-p", "p", "--services"]);
        match cmd.command {
            ComposeSubcommand::Config(args) => {
                assert!(args.services);
                assert!(!args.volumes);
                assert!(!args.profiles);
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_images_subcommand() {
        let cmd = parse_compose(["images", "-p", "p"]);
        match cmd.command {
            ComposeSubcommand::Images(_) => {}
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_port_subcommand() {
        let cmd = parse_compose(["port", "-p", "p", "web", "80", "--protocol", "tcp"]);
        match cmd.command {
            ComposeSubcommand::Port(args) => {
                assert_eq!(args.service, "web");
                assert_eq!(args.private_port, 80);
                assert_eq!(args.protocol, "tcp");
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_top_subcommand() {
        let cmd = parse_compose(["top", "-p", "p", "web"]);
        match cmd.command {
            ComposeSubcommand::Top(args) => assert_eq!(args.services, vec!["web"]),
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_events_subcommand_json_flag() {
        let cmd = parse_compose(["events", "-p", "p", "--json"]);
        match cmd.command {
            ComposeSubcommand::Events(args) => assert!(args.json),
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_cp_subcommand() {
        let cmd = parse_compose(["cp", "-p", "p", "web:/etc/hosts", "./hosts"]);
        match cmd.command {
            ComposeSubcommand::Cp(args) => {
                assert_eq!(args.source, "web:/etc/hosts");
                assert_eq!(args.destination, "./hosts");
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_wait_subcommand() {
        let cmd = parse_compose(["wait", "-p", "p"]);
        match cmd.command {
            ComposeSubcommand::Wait(args) => assert!(args.services.is_empty()),
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_attach_subcommand() {
        let cmd = parse_compose(["attach", "-p", "p", "web"]);
        match cmd.command {
            ComposeSubcommand::Attach(args) => assert_eq!(args.service, "web"),
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_version_subcommand() {
        let cmd = parse_compose(["version", "--short"]);
        match cmd.command {
            ComposeSubcommand::Version(args) => assert!(args.short),
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn parses_alpha_publish_subcommand() {
        let cmd = parse_compose(["alpha", "publish", "ghcr.io/foo/app"]);
        match cmd.command {
            ComposeSubcommand::Alpha(ComposeAlphaSubcommand::Publish(args)) => {
                assert_eq!(args.repository.as_deref(), Some("ghcr.io/foo/app"));
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[tokio::test]
    async fn watch_returns_unsupported_error() {
        let result = handle_watch(ComposeWatchArgs {
            ctx: ComposeContextArgs::default(),
        })
        .await;
        let err = result.expect_err("watch must error");
        assert!(err.to_string().contains("not yet supported"));
    }

    #[tokio::test]
    async fn alpha_publish_returns_unsupported_error() {
        let result = handle_alpha_publish(ComposeAlphaPublishArgs {
            ctx: ComposeContextArgs::default(),
            repository: None,
        })
        .await;
        let err = result.expect_err("alpha publish must error");
        assert!(err.to_string().contains("not yet supported"));
    }

    #[test]
    fn split_image_ref_handles_tags_and_registries() {
        assert_eq!(
            split_image_ref("nginx"),
            ("nginx".to_string(), "latest".to_string()),
        );
        assert_eq!(
            split_image_ref("nginx:1.25"),
            ("nginx".to_string(), "1.25".to_string()),
        );
        assert_eq!(
            split_image_ref("ghcr.io/foo/bar:v1"),
            ("ghcr.io/foo/bar".to_string(), "v1".to_string()),
        );
        assert_eq!(
            split_image_ref("registry:5000/foo/bar"),
            ("registry:5000/foo/bar".to_string(), "latest".to_string()),
            "port-style colon must NOT be parsed as a tag",
        );
        assert_eq!(
            split_image_ref("nginx:1.25@sha256:deadbeef"),
            ("nginx".to_string(), "1.25".to_string()),
            "digest is stripped before tag detection",
        );
    }

    #[test]
    fn format_size_human_picks_correct_tier() {
        assert_eq!(format_size_human(0), "0B");
        assert_eq!(format_size_human(512), "512B");
        assert_eq!(format_size_human(2048), "2.0KB");
        assert_eq!(format_size_human(5 * 1024 * 1024), "5.0MB");
        assert_eq!(format_size_human(3 * 1024 * 1024 * 1024), "3.0GB");
    }

    #[test]
    fn short_id_truncates_long_ids() {
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(short_id("abcdefghijklmnop"), "abcdefghijkl");
    }

    #[test]
    fn decode_stdcopy_chunks_passes_through_raw_payload() {
        let raw = b"plain bytes";
        let chunks = decode_stdcopy_chunks(raw);
        assert_eq!(chunks, vec![raw.as_slice()]);
    }

    #[test]
    fn decode_stdcopy_chunks_decodes_framed_payload() {
        // Frame layout: stream (1=stdout), zeroes, BE length, payload.
        let mut frame = Vec::new();
        frame.extend_from_slice(&[1, 0, 0, 0]);
        frame.extend_from_slice(&5u32.to_be_bytes());
        frame.extend_from_slice(b"hello");
        let chunks = decode_stdcopy_chunks(&frame);
        assert_eq!(chunks, vec![b"hello".as_slice()]);
    }

    // ----------------------------------------------------------------------
    // Task 6.4: `compose build` planning + `compose up --build` argument
    // surface. Execution itself is exercised end-to-end by the project's
    // image-build integration suite (which spins up buildah); the planning
    // layer is unit-tested here so the wiring stays correct.
    // ----------------------------------------------------------------------

    #[test]
    fn build_subcommand_parses_no_cache_and_pull_flags() {
        let cmd = parse_compose(["build", "-p", "p", "--no-cache", "--pull", "web"]);
        match cmd.command {
            ComposeSubcommand::Build(args) => {
                assert!(args.no_cache);
                assert!(args.pull);
                assert_eq!(args.services, vec!["web"]);
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn build_up_with_build_flag_parses() {
        let cmd = parse_compose(["up", "-p", "p", "--build", "-d"]);
        match cmd.command {
            ComposeSubcommand::Up(args) => {
                assert!(args.build);
                assert!(args.detach);
                assert!(!args.abort_on_container_exit);
                assert!(args.exit_code_from.is_none());
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
    }

    #[test]
    fn build_plan_filters_to_named_services_only() {
        // Wires through the planner to confirm `compose build SERVICE`
        // narrows the plan list (mirrors what `handle_build` does).
        use crate::compose::build::plan_builds;
        let yaml = "
services:
  web:
    build: ./web
  db:
    image: postgres:16
  worker:
    build: ./worker
";
        let compose = crate::compose::parse_compose(yaml).unwrap();
        let working = std::path::PathBuf::from("/tmp/proj");
        let filter = vec!["worker".to_string()];
        let plans = plan_builds("p", &compose, &working, Some(&filter), false, false).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].service, "worker");
        assert_eq!(plans[0].primary_tag(), "p-worker:latest");
    }

    // ----------------------------------------------------------------------
    // Task 6.6.1-2: foreground multiplexed log prefix + abort-on-exit /
    // exit-code-from semantics. The streaming + tokio plumbing is driven
    // through pure helper functions (`format_log_line`,
    // `decide_foreground_exit_code`, `should_abort_after_exit`) so the
    // logic can be unit-tested without spinning up an actual daemon.
    // ----------------------------------------------------------------------

    #[test]
    fn log_format_uncolorised_line_aligns_padding_and_strips_trailing_newline() {
        let out = format_log_line("web", "hello world\n", 8, false);
        // pad_width=8 with svc "web" -> "web     " then " | <line>".
        assert_eq!(out, "web      | hello world");
    }

    #[test]
    fn log_format_colorised_line_wraps_padded_prefix_in_ansi() {
        let out = format_log_line("api", "ready", 4, true);
        // The colour bytes are deterministic for a given service name.
        let expected_color = FOREGROUND_LOG_COLORS[service_color_index("api")];
        assert!(
            out.starts_with(expected_color),
            "colorised line must start with the service's ANSI color: got {out:?}",
        );
        assert!(out.contains(ANSI_RESET), "must close the ANSI sequence");
        assert!(out.ends_with(" | ready"));
    }

    #[test]
    fn log_format_pad_width_grows_to_service_name_when_shorter() {
        // pad_width smaller than svc.len() should not truncate the name.
        let out = format_log_line("very-long-svc", "ok", 3, false);
        assert!(out.starts_with("very-long-svc | "));
    }

    #[test]
    fn log_format_service_color_is_stable_across_calls() {
        // Same service must always pick the same colour index (so a single
        // run paints it consistently across re-attached log streams).
        assert_eq!(service_color_index("web"), service_color_index("web"));
        assert_eq!(service_color_index("db"), service_color_index("db"));
    }

    #[test]
    fn abort_on_container_exit_returns_first_observed_exit_code() {
        // Two services, "api" exits first with code 7, "db" exits later with
        // code 0. With --abort-on-container-exit, the foreground driver
        // surfaces 7 (the *first* exit) and stops the rest.
        let exits = vec![
            ContainerExit {
                service: "api".to_string(),
                container_id: "c1".to_string(),
                exit_code: 7,
            },
            ContainerExit {
                service: "db".to_string(),
                container_id: "c2".to_string(),
                exit_code: 0,
            },
        ];
        let code = decide_foreground_exit_code(&exits, None, true);
        assert_eq!(code, 7, "abort-on-container-exit returns the first exit");

        // And the abort decision fires on *any* exit when abort_on_exit=true.
        assert!(should_abort_after_exit(&exits[0], None, true));
        assert!(should_abort_after_exit(&exits[1], None, true));
    }

    #[test]
    fn abort_on_exit_kills_all_when_first_service_dies() {
        // Mock 2 services. Kill the first one (exit code 137 = SIGKILL).
        // Confirm the abort flag fires for that exit, AND the foreground
        // exit code is the killed container's code (not the still-running
        // peer's eventual 0).
        let killed = ContainerExit {
            service: "victim".to_string(),
            container_id: "vct".to_string(),
            exit_code: 137,
        };
        assert!(
            should_abort_after_exit(&killed, None, true),
            "abort-on-container-exit must trigger when any container dies",
        );
        // Even if the second service later exits 0, the first observed
        // exit (137) wins, matching docker compose v2 behaviour.
        let exits = vec![
            killed,
            ContainerExit {
                service: "peer".to_string(),
                container_id: "pee".to_string(),
                exit_code: 0,
            },
        ];
        assert_eq!(decide_foreground_exit_code(&exits, None, true), 137);
    }

    #[test]
    fn exit_code_from_picks_named_service_exit() {
        // Three services exit in order; only the named one's code matters.
        let exits = vec![
            ContainerExit {
                service: "noise1".to_string(),
                container_id: "x1".to_string(),
                exit_code: 5,
            },
            ContainerExit {
                service: "target".to_string(),
                container_id: "x2".to_string(),
                exit_code: 42,
            },
            ContainerExit {
                service: "noise2".to_string(),
                container_id: "x3".to_string(),
                exit_code: 99,
            },
        ];
        let code = decide_foreground_exit_code(&exits, Some("target"), true);
        assert_eq!(code, 42, "--exit-code-from picks the named service's code");

        // Abort decision: only fires once the named service has exited.
        assert!(!should_abort_after_exit(&exits[0], Some("target"), true));
        assert!(should_abort_after_exit(&exits[1], Some("target"), true));
        assert!(!should_abort_after_exit(&exits[2], Some("target"), true));
    }

    #[test]
    fn exit_code_from_returns_zero_when_named_service_never_exited() {
        // Compose v2 returns 0 when --exit-code-from names a service that
        // didn't exit during the foreground session (e.g. user Ctrl-C'd
        // before it terminated).
        let exits = vec![ContainerExit {
            service: "other".to_string(),
            container_id: "x1".to_string(),
            exit_code: 17,
        }];
        let code = decide_foreground_exit_code(&exits, Some("missing"), true);
        assert_eq!(code, 0, "--exit-code-from with no matching exit returns 0");
    }

    #[test]
    fn default_exit_propagates_highest_non_zero_code() {
        // Without --abort-on-container-exit and without --exit-code-from,
        // a normal `compose up` waits for every container, then surfaces
        // the highest non-zero exit code seen. All-zero -> 0.
        let exits = vec![
            ContainerExit {
                service: "a".to_string(),
                container_id: "ca".to_string(),
                exit_code: 0,
            },
            ContainerExit {
                service: "b".to_string(),
                container_id: "cb".to_string(),
                exit_code: 13,
            },
            ContainerExit {
                service: "c".to_string(),
                container_id: "cc".to_string(),
                exit_code: 2,
            },
        ];
        assert_eq!(decide_foreground_exit_code(&exits, None, false), 13);

        // All-zero stays zero.
        let zero = vec![ContainerExit {
            service: "ok".to_string(),
            container_id: "co".to_string(),
            exit_code: 0,
        }];
        assert_eq!(decide_foreground_exit_code(&zero, None, false), 0);
    }
}
