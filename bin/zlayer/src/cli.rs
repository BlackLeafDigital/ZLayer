use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::ui::consent::ConsentMode;

/// Return the platform-appropriate default data directory for `ZLayer`.
///
/// - macOS: `~/.zlayer`
/// - Linux (root): `/var/lib/zlayer`
/// - Linux (user): `~/.zlayer`
/// - Windows: `%LOCALAPPDATA%\ZLayer` or `C:\ProgramData\ZLayer`
pub(crate) fn default_data_dir() -> PathBuf {
    zlayer_paths::ZLayerDirs::detect_data_dir()
}

/// Return the platform-appropriate default runtime directory.
///
/// When `data_dir` matches the platform default data directory, returns the
/// system-default run dir (e.g. `/var/run/zlayer` on Linux). Otherwise
/// derives `{data_dir}/run` so callers passing `--data-dir /tmp/foo` get an
/// isolated runtime directory and don't collide with a system install.
pub(crate) fn default_run_dir(data_dir: &std::path::Path) -> PathBuf {
    zlayer_paths::ZLayerDirs::default_run_dir_for(data_dir)
}

/// Return the platform-appropriate default log directory.
///
/// When `data_dir` matches the platform default data directory, returns the
/// system-default log dir (e.g. `/var/log/zlayer` on Linux). Otherwise
/// derives `{data_dir}/logs` so callers passing `--data-dir /tmp/foo` get
/// fully isolated logs and don't collide with a system install.
pub(crate) fn default_log_dir(data_dir: &std::path::Path) -> PathBuf {
    zlayer_paths::ZLayerDirs::default_log_dir_for(data_dir)
}

/// Return the platform-appropriate default socket path.
///
/// When `data_dir` matches the platform default data directory, returns the
/// system-default socket path (e.g. `/var/run/zlayer.sock` on Linux).
/// Otherwise derives `{data_dir}/run/zlayer.sock`. On Windows always returns
/// the TCP loopback endpoint since Unix domain sockets have limited support.
pub(crate) fn default_socket_path(data_dir: &std::path::Path) -> String {
    zlayer_paths::ZLayerDirs::default_socket_path_for(data_dir)
}

/// `ZLayer` container orchestration platform
#[derive(Parser)]
#[command(name = "zlayer")]
#[command(version, about = "ZLayer container orchestration platform")]
#[command(propagate_version = true)]
#[command(after_help = "\
COMMAND GROUPS:
  Lifecycle:    up, down, deploy, join, ps, status, exec, logs, stop
  Building:     build, pipeline, runtimes
  Registry:     pull, export, import
  Cluster:      node, serve, tunnel, manager
  Inspection:   validate, token, spec, wasm
  Auth:         auth bootstrap, auth login, auth logout, auth whoami
  Users:        user ls, user create, user set-role, user set-password, user delete
  Projects:     project ls, project create, project show, project update, project delete
  Credentials:  credential registry ls/add/delete, credential git ls/add/delete
  Interface:    tui

Run 'zlayer <command> --help' for details on a specific command.")]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Cli {
    /// Container runtime to use
    #[arg(long, default_value = "auto", value_enum)]
    pub(crate) runtime: RuntimeType,

    /// Root data directory for all `ZLayer` state (databases, secrets, containers).
    ///
    /// On macOS defaults to ~/.zlayer.
    /// On Linux defaults to /var/lib/zlayer (root) or ~/.zlayer (user).
    /// Other directories (logs, run, containers) are derived from this unless
    /// individually overridden.
    #[arg(long, env = "ZLAYER_DATA_DIR")]
    pub(crate) data_dir: Option<PathBuf>,

    /// State directory for container runtime data.
    /// Defaults to {data-dir}/containers.
    #[arg(long)]
    pub(crate) state_dir: Option<PathBuf>,

    /// Enable verbose logging
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub(crate) verbose: u8,

    /// Run in background mode (used with 'up' command)
    #[arg(short = 'b', long = "background", global = true)]
    pub(crate) background: bool,

    /// Detach after services are running (used with 'up' and 'deploy' commands)
    ///
    /// Shows full deployment progress, waits for services to stabilize,
    /// then automatically exits without waiting for Ctrl+C.
    /// Unlike -b/--background, this waits for services to be confirmed running.
    #[arg(short = 'd', long = "detach", global = true)]
    pub(crate) detach: bool,

    /// Disable interactive TUI (use plain text output)
    #[arg(long, global = true)]
    pub(crate) no_tui: bool,

    /// Use host networking instead of overlay (containers share host network namespace)
    ///
    /// When set, containers will not get their own network namespace and will share
    /// the host's network stack. This bypasses the overlay networking requirement.
    /// Equivalent to Docker's --network host mode.
    #[arg(long, global = true)]
    pub(crate) host_network: bool,

    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

impl Cli {
    /// Resolve the effective root data directory.
    pub(crate) fn effective_data_dir(&self) -> PathBuf {
        self.data_dir.clone().unwrap_or_else(default_data_dir)
    }

    /// Resolve the effective state directory (container runtime state).
    pub(crate) fn effective_state_dir(&self) -> PathBuf {
        self.state_dir
            .clone()
            .unwrap_or_else(|| self.effective_data_dir().join("containers"))
    }

    /// Resolve the effective log directory.
    pub(crate) fn effective_log_dir(&self) -> PathBuf {
        default_log_dir(&self.effective_data_dir())
    }

    /// Resolve the effective runtime (run) directory.
    #[allow(dead_code)] // used on Linux in cfg(not(target_os = "macos")) blocks
    pub(crate) fn effective_run_dir(&self) -> PathBuf {
        default_run_dir(&self.effective_data_dir())
    }

    /// Resolve the effective socket path.
    pub(crate) fn effective_socket_path(&self) -> String {
        if let Some(Commands::Serve {
            socket: Some(s), ..
        }) = &self.command
        {
            return s.clone();
        }
        default_socket_path(&self.effective_data_dir())
    }
}

/// Runtime type selection
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum RuntimeType {
    /// Auto-detect the best available runtime (youki > docker > error)
    Auto,
    /// Docker runtime (requires running Docker daemon)
    #[cfg(feature = "docker")]
    Docker,
    /// Youki runtime for production deployments (Linux only)
    #[cfg(target_os = "linux")]
    Youki,
    /// macOS sandbox runtime (uses Apple sandbox framework)
    #[cfg(target_os = "macos")]
    MacSandbox,
    /// macOS VM runtime (uses Virtualization.framework)
    #[cfg(target_os = "macos")]
    MacVm,
}

/// Shared flag group controlling WSL2 auto-install consent.
///
/// Flattened into every subcommand that may trigger WSL2 bootstrap
/// (`node init`, `node join`, `join <url>`, `daemon install`). The flag
/// resolves to a [`ConsentMode`] that the handler forwards to
/// `crate::ui::consent::wsl2_install_consent`.
///
/// Precedence: explicit `--install-wsl <value>` on the CLI beats
/// `ZLAYER_INSTALL_WSL`, which beats the default (`ask`).
#[derive(Debug, Clone, Copy, Args)]
pub(crate) struct InstallWslArgs {
    /// Consent policy for auto-installing WSL2 on Windows when it's missing.
    ///
    /// - `ask` (default): interactive Y/n prompt on the controlling terminal.
    /// - `yes`: auto-accept (UAC prompt still appears).
    /// - `no`: never install; hard-fail if WSL2 isn't already present.
    ///
    /// Only consulted on Windows — ignored on other platforms.
    #[arg(
        long = "install-wsl",
        value_enum,
        default_value_t = ConsentMode::Ask,
        env = "ZLAYER_INSTALL_WSL",
        global = false,
    )]
    pub(crate) install_wsl: ConsentMode,
}

impl InstallWslArgs {
    /// Extract the underlying consent mode.
    ///
    /// Consumed by the Windows-gated handlers in
    /// `commands::{node, join, daemon}`; kept unconditionally public so the
    /// (currently in-flight) H-1/H-2/H-3 wiring doesn't need a separate
    /// feature-gate dance.
    #[must_use]
    #[allow(dead_code)] // wired in H-1/H-2/H-3 (see task G-6 notes)
    pub(crate) fn mode(self) -> ConsentMode {
        self.install_wsl
    }
}

/// CLI subcommands
#[derive(Subcommand)]
pub(crate) enum Commands {
    // ── Lifecycle ─────────────────────────────────────────────────────
    /// Deploy and start services (like docker compose up)
    ///
    /// Auto-discovers *.zlayer.yml in current directory if no spec path given.
    /// Runs in foreground by default, streaming logs and waiting for Ctrl+C.
    /// Use -b/--background to deploy and return immediately.
    /// Use -d/--detach to wait for services to stabilize, then exit.
    /// Use --pull to force-pull all images, or --no-pull to skip pulling entirely.
    ///
    /// Examples:
    ///   zlayer up
    ///   zlayer up myapp.zlayer.yml
    ///   zlayer up -b
    ///   zlayer up -d
    ///   zlayer up --pull
    ///   zlayer up --no-pull
    #[command(verbatim_doc_comment, display_order = 1)]
    Up {
        /// Path to deployment spec (auto-discovers *.zlayer.yml if not given)
        spec_path: Option<PathBuf>,

        /// Force pull all images before deploying (overrides per-service `pull_policy`)
        #[arg(long, conflicts_with = "no_pull")]
        pull: bool,

        /// Skip pulling images even if a per-tag default would pull
        #[arg(long = "no-pull", conflicts_with = "pull")]
        no_pull: bool,
    },

    /// Stop all services in a deployment (like docker compose down)
    ///
    /// Auto-discovers deployment name from *.zlayer.yml if not given.
    ///
    /// Examples:
    ///   zlayer down
    ///   zlayer down my-deployment
    #[command(verbatim_doc_comment, display_order = 2)]
    Down {
        /// Deployment name (auto-discovers from *.zlayer.yml if not given)
        deployment: Option<String>,
    },

    /// Deploy services from a spec file
    #[command(display_order = 3)]
    Deploy {
        /// Path to deployment spec (auto-discovers *.zlayer.yml in current directory)
        spec_path: Option<PathBuf>,

        /// Dry run - parse and validate but don't actually deploy
        #[arg(long)]
        dry_run: bool,
    },

    /// Join an existing deployment
    #[command(display_order = 4)]
    Join {
        /// Join token (contains deployment key and service info)
        token: String,

        /// Override the spec directory
        #[arg(long)]
        spec_dir: Option<String>,

        /// Service to run (if token doesn't specify)
        #[arg(short, long)]
        service: Option<String>,

        /// Number of replicas to run on this node
        #[arg(short, long, default_value = "1")]
        replicas: u32,

        /// WSL2 auto-install consent (Windows only).
        #[command(flatten)]
        install_wsl: InstallWslArgs,
    },

    /// List running deployments, services, and containers
    ///
    /// Shows a summary of all deployments and their services. Use --containers
    /// to also show individual container replicas.
    ///
    /// Examples:
    ///   zlayer ps
    ///   zlayer ps --deployment my-app
    ///   zlayer ps --containers
    ///   zlayer ps --format json
    #[command(verbatim_doc_comment, display_order = 5)]
    Ps {
        /// Filter by deployment name
        #[arg(long)]
        deployment: Option<String>,
        /// Show individual containers
        #[arg(long, short = 'c')]
        containers: bool,
        /// Output format: table, json, yaml
        #[arg(long, default_value = "table")]
        format: String,
    },

    /// Show runtime status
    #[command(display_order = 6)]
    Status,

    /// Execute a command in a running service container
    ///
    /// Runs a command inside a container belonging to the specified service.
    /// If only one deployment exists, the deployment flag can be omitted.
    /// By default targets the first healthy replica; use --replica to pick one.
    ///
    /// Examples:
    ///   zlayer exec web -- ls -la
    ///   zlayer exec --deployment my-app web -- /bin/sh
    ///   zlayer exec --replica 2 api -- cat /etc/hosts
    #[command(verbatim_doc_comment, display_order = 7)]
    Exec {
        /// Service name
        service: String,
        /// Deployment name (auto-detected if only one exists)
        #[arg(long)]
        deployment: Option<String>,
        /// Target a specific replica number
        #[arg(long, short = 'r')]
        replica: Option<u32>,
        /// Command and arguments to execute
        #[arg(last = true)]
        cmd: Vec<String>,
    },

    /// Stream logs from a service
    #[command(display_order = 8)]
    Logs {
        /// Deployment name (optional — auto-resolves from service name when omitted)
        #[arg(long)]
        deployment: Option<String>,

        /// Service name
        service: String,

        /// Number of lines to show
        #[arg(short = 'n', long, default_value = "100")]
        lines: u32,

        /// Follow log output
        #[arg(short, long)]
        follow: bool,

        /// Filter by instance ID
        #[arg(short, long)]
        instance: Option<String>,
    },

    /// Stop a deployment or service
    #[command(display_order = 9)]
    Stop {
        /// Deployment name (optional — auto-resolves when only one deployment exists, else prompts)
        deployment: Option<String>,

        /// Service name (optional, stops all if not specified)
        #[arg(short, long)]
        service: Option<String>,

        /// Force immediate shutdown (no graceful period)
        #[arg(short, long)]
        force: bool,

        /// Timeout for graceful shutdown in seconds
        #[arg(short, long, default_value = "30")]
        timeout: u64,
    },

    // ── Building ──────────────────────────────────────────────────────
    /// Build a container image from a Dockerfile
    ///
    /// Examples:
    ///   zlayer build .
    ///   zlayer build -t myapp:latest .
    ///   zlayer build --runtime node20 -t myapp:v1 ./my-node-app
    ///   zlayer build --runtime-auto -t myapp:latest .
    ///   zlayer build -f Dockerfile.prod --target production -t myapp:prod .
    #[command(verbatim_doc_comment, display_order = 10)]
    Build {
        /// Build context directory
        #[arg(default_value = ".")]
        context: PathBuf,

        /// Dockerfile path (default: Dockerfile in context)
        #[arg(short = 'f', long)]
        file: Option<PathBuf>,

        /// `ZImagefile` path (alternative to Dockerfile)
        #[arg(long = "zimagefile", short = 'z')]
        zimagefile: Option<PathBuf>,

        /// Image tag (can be specified multiple times)
        #[arg(short = 't', long = "tag")]
        tags: Vec<String>,

        /// Use runtime template instead of Dockerfile
        #[arg(long, value_name = "RUNTIME")]
        runtime: Option<String>,

        /// Auto-detect runtime from project files
        #[arg(long)]
        runtime_auto: bool,

        /// Build argument (KEY=VALUE, can be specified multiple times)
        #[arg(long = "build-arg")]
        build_args: Vec<String>,

        /// Target stage for multi-stage builds
        #[arg(long)]
        target: Option<String>,

        /// Disable layer caching
        #[arg(long)]
        no_cache: bool,

        /// Base-image pull strategy (newer|always|never). Default: newer.
        ///
        /// - `newer` - Pull only if the registry has a newer version (fast + correct)
        /// - `always` - Always pull, even if a local copy exists
        /// - `never`  - Use whatever is in local storage (offline builds)
        #[arg(long, value_parser = ["newer", "always", "never"], default_value = "newer")]
        pull: String,

        /// Shortcut for `--pull=never` (offline builds).
        #[arg(long)]
        no_pull: bool,

        /// Push to registry after build
        #[arg(long)]
        push: bool,

        /// Disable TUI (plain output for CI)
        #[arg(long)]
        no_tui: bool,

        /// Verbose output (show all build output)
        #[arg(long)]
        verbose_build: bool,

        /// Target platform for the build (e.g. `linux`, `linux/amd64`, `windows`, `windows/amd64`).
        ///
        /// Selects the OS portion of the image being built and routes to the
        /// matching builder backend (buildah on Linux, HCS on Windows). When
        /// omitted, the OS is inferred from the `ZImagefile`'s `os:` or
        /// `platform:` field, falling back to Linux.
        #[arg(long, value_name = "PLATFORM")]
        platform: Option<String>,

        /// Ignore any existing `zlayer-bottles.lock` next to the spec and
        /// force fresh resolution of every macOS Homebrew bottle. The lockfile
        /// is rewritten from scratch. Mirrors `cargo update` semantics.
        /// On non-macOS platforms this flag is accepted but has no effect.
        #[arg(long)]
        update_bottles: bool,
    },

    /// Build multiple images from a pipeline manifest
    ///
    /// Reads a ZPipeline.yaml (or zlayer-pipeline.yaml) and builds all
    /// images defined in it, respecting dependency order.
    ///
    /// Examples:
    ///   zlayer pipeline
    ///   zlayer pipeline -f my-pipeline.yaml
    ///   zlayer pipeline --set VERSION=1.0 --push
    ///   zlayer pipeline --only web,api
    #[command(verbatim_doc_comment, display_order = 11)]
    Pipeline {
        /// Path to pipeline file (defaults to ZPipeline.yaml or zlayer-pipeline.yaml)
        #[arg(short = 'f', long = "file")]
        file: Option<PathBuf>,

        /// Set pipeline variables (KEY=VALUE, repeatable)
        #[arg(long = "set")]
        set: Vec<String>,

        /// Push all images after successful builds
        #[arg(long)]
        push: bool,

        /// Stop on first build failure (default: true)
        #[arg(long, default_value = "true")]
        fail_fast: bool,

        /// Disable TUI progress display
        #[arg(long)]
        no_tui: bool,

        /// Only build specific images (comma-separated)
        #[arg(long)]
        only: Option<String>,

        /// Override platforms for all images (comma-separated, e.g., "linux/amd64,linux/arm64")
        #[arg(long)]
        platform: Option<String>,
    },

    /// List available runtime templates
    ///
    /// Shows all pre-built Dockerfile templates for common development
    /// environments. These can be used with `zlayer build --runtime <name>`.
    #[command(display_order = 12)]
    Runtimes,

    // ── Registry ──────────────────────────────────────────────────────
    /// Pull an image from a registry
    ///
    /// Examples:
    ///   zlayer pull zachhandley/zlayer-node:latest
    ///   zlayer pull ghcr.io/blackleafdigital/zlayer-manager:0.9.6
    #[command(verbatim_doc_comment, display_order = 20)]
    Pull {
        /// Image reference. If omitted, pulls every image from the auto-discovered spec.
        image: Option<String>,
    },

    /// Export an image to a tar file (OCI Image Layout)
    ///
    /// Exports a locally stored image to an OCI Image Layout tar archive
    /// that can be imported on another system or loaded into Docker.
    ///
    /// Examples:
    ///   zlayer export myapp:latest -o myapp.tar
    ///   zlayer export myapp:v1.0 -o myapp.tar.gz --gzip
    ///   zlayer export myapp@sha256:abc123... -o myapp.tar
    #[command(verbatim_doc_comment, display_order = 21)]
    Export {
        /// Image reference (name:tag or name@digest)
        image: String,

        /// Output file path (.tar or .tar.gz)
        #[arg(short, long)]
        output: PathBuf,

        /// Compress output with gzip
        #[arg(long)]
        gzip: bool,
    },

    /// Import an image from a tar file
    ///
    /// Imports an OCI Image Layout tar archive into the local registry.
    /// The archive can be created by `zlayer export` or `docker save`.
    ///
    /// Examples:
    ///   zlayer import myapp.tar
    ///   zlayer import myapp.tar.gz -t myapp:imported
    ///   zlayer import /path/to/image.tar --tag myapp:v1.0
    ///   zlayer import <https://forge.example.com/.../myapp-oci.tar>
    ///   zlayer import <https://forge.example.com/.../myapp-oci.tar> -u user -p token
    #[command(verbatim_doc_comment, display_order = 22)]
    Import {
        /// Input: either a local tar file path or an http(s):// URL to an OCI
        /// tar archive. URL fetches support optional HTTP Basic auth via
        /// `--username` / `--password`.
        input: String,

        /// Tag to apply to imported image
        #[arg(short, long)]
        tag: Option<String>,

        /// HTTP Basic auth username (URL input only)
        #[arg(short = 'u', long)]
        username: Option<String>,

        /// HTTP Basic auth password (URL input only)
        #[arg(short = 'p', long)]
        password: Option<String>,
    },

    // ── Cluster & Infrastructure ──────────────────────────────────────
    /// Manage cluster nodes
    #[command(subcommand, display_order = 30)]
    Node(NodeCommands),

    /// Cluster-level administration commands.
    ///
    /// In v0.12.0 these commands were promoted from `zlayer node <…>` to
    /// their own top-level subgroup. The original `zlayer node <…>`
    /// invocations still work and dispatch to the same handlers (with a
    /// deprecation note in their `--help` output, see Wave 5B.5).
    #[command(subcommand, display_order = 30)]
    Cluster(ClusterCommands),

    /// Start the API server
    #[command(display_order = 31)]
    Serve {
        /// Bind address (e.g., 0.0.0.0:3669)
        #[arg(long, default_value = "0.0.0.0:3669")]
        bind: String,

        /// JWT secret for authentication (can also be set via `ZLAYER_JWT_SECRET` env var)
        #[arg(long, env = "ZLAYER_JWT_SECRET")]
        jwt_secret: Option<String>,

        /// Disable Swagger UI
        #[arg(long)]
        no_swagger: bool,

        /// Run as background daemon
        #[arg(long)]
        daemon: bool,

        /// Run as a Windows Service under SCM (internal flag, set by the
        /// Service Control Manager when it spawns the binary). Hidden from
        /// normal help because it has no effect when launched from a console.
        /// Errors on non-Windows targets.
        #[arg(long, hide = true)]
        service: bool,

        /// Unix socket path for CLI communication.
        /// Defaults to {run-dir}/zlayer.sock.
        #[arg(long)]
        socket: Option<String>,

        /// Deployment name used to scope overlay network interfaces, socket
        /// paths, and per-deployment state. Defaults to "zlayer". Set this to
        /// a unique value when running a second daemon alongside a system
        /// install to avoid collisions on kernel-level resources (link names
        /// like `zl-<name>-g`, `WireGuard` UAPI sockets under
        /// `/var/run/wireguard/`).
        #[arg(long, env = "ZLAYER_DEPLOYMENT_NAME", default_value = "zlayer")]
        deployment_name: String,

        /// UDP port for `WireGuard` overlay traffic. Defaults to
        /// `zlayer_core::DEFAULT_WG_PORT` (51420). Override when running a
        /// second daemon alongside a system install to avoid UDP port
        /// collision. CLI flag takes precedence over `node_config.json`'s
        /// `overlay_port` field (used by the `node init` flow).
        #[arg(long, env = "ZLAYER_WG_PORT")]
        wg_port: Option<u16>,

        /// TCP/UDP port for the overlay DNS server (binds on 127.0.0.1).
        /// Defaults to `zlayer_overlay::DEFAULT_DNS_PORT` (15353). Override
        /// when running a second daemon to avoid port collision.
        #[arg(long, env = "ZLAYER_DNS_PORT")]
        dns_port: Option<u16>,

        /// Override the WSL2 vhdSize cap (GiB). Defaults to ~80% of free host disk.
        /// Only meaningful on Windows.
        #[cfg(all(target_os = "windows", feature = "wsl"))]
        #[arg(long, value_name = "GB")]
        vhd_gb: Option<u64>,

        /// Enable Docker API socket emulation
        #[cfg(feature = "docker-compat")]
        #[arg(long)]
        docker_socket: bool,

        /// Docker API socket path
        #[cfg(feature = "docker-compat")]
        #[arg(long, default_value_t = zlayer_paths::ZLayerDirs::default_docker_socket_path())]
        docker_socket_path: String,

        /// Disable NAT traversal (STUN/TURN). Default is enabled.
        #[clap(long)]
        no_nat: bool,

        /// Override STUN servers (repeatable). Format: host:port.
        #[clap(long = "stun-server")]
        stun_servers: Vec<String>,

        /// Override TURN/relay servers (repeatable). Format: host:port.
        #[clap(long = "turn-server")]
        turn_servers: Vec<String>,

        /// Bind address for the built-in relay server (format: host:port).
        /// If unset, no relay server is started locally.
        #[clap(long)]
        relay_server_bind: Option<String>,

        /// Bind address for the tunnel WebSocket control server (format:
        /// host:port). Defaults to 0.0.0.0:3679 when unset.
        #[clap(long, env = "ZLAYER_TUNNEL_BIND")]
        tunnel_bind: Option<String>,

        /// Path to a TLS certificate (PEM) for the tunnel server.
        /// Used when terminating TLS at the tunnel listener (future
        /// enhancement); currently logged at startup for diagnostics.
        #[clap(long, env = "ZLAYER_TUNNEL_TLS_CERT")]
        tunnel_tls_cert: Option<std::path::PathBuf>,

        /// Path to a TLS private key (PEM) for the tunnel server.
        #[clap(long, env = "ZLAYER_TUNNEL_TLS_KEY")]
        tunnel_tls_key: Option<std::path::PathBuf>,

        /// Enable TLS on the daemon API listener by loading a static
        /// certificate from disk. Requires `--api-tls-key`.
        ///
        /// When set, the daemon API binds HTTPS instead of HTTP. The
        /// cert+key are loaded once at startup. Use `--api-tls-acme` for
        /// hot-reloading ACME-managed certs instead.
        #[clap(
            long,
            value_name = "PATH",
            env = "ZLAYER_API_TLS_CERT",
            requires = "api_tls_key",
            conflicts_with = "api_tls_acme"
        )]
        api_tls_cert: Option<std::path::PathBuf>,

        /// Private key paired with `--api-tls-cert`. PEM-encoded.
        #[clap(
            long,
            value_name = "PATH",
            env = "ZLAYER_API_TLS_KEY",
            requires = "api_tls_cert",
            conflicts_with = "api_tls_acme"
        )]
        api_tls_key: Option<std::path::PathBuf>,

        /// Enable TLS on the daemon API listener using the proxy's
        /// ACME-capable `CertManager`. The same cert pool the reverse proxy
        /// uses for L7 routes will be served on the daemon API. Mutually
        /// exclusive with `--api-tls-cert`/`--api-tls-key`.
        #[clap(
            long,
            env = "ZLAYER_API_TLS_ACME",
            conflicts_with_all = ["api_tls_cert", "api_tls_key"]
        )]
        api_tls_acme: bool,

        /// Disable the daemon-side tunnel server entirely. Endpoints under
        /// `POST /api/v1/tunnels/access/sessions` will return 503.
        #[clap(long, env = "ZLAYER_DISABLE_TUNNEL_SERVER")]
        no_tunnel_server: bool,

        /// On clean shutdown, exit with code 75 (`EX_TEMPFAIL`) instead of 0, so
        /// a supervisor (systemd `Restart=on-failure`, runit, etc.) respawns
        /// the daemon. Used by `zlayer self-update`-driven cluster upgrades.
        #[arg(long, env = "ZLAYER_RESTART_ON_EXIT")]
        restart_on_exit: bool,

        /// Wipe `{data_dir}/join_secret` at daemon startup so the HMAC key for
        /// HS256-JWT join tokens is regenerated.
        ///
        /// Use this if you suspect the symmetric secret has leaked. Existing
        /// HS256 tokens will fail to validate after vacuum (which is the point),
        /// but new tokens can be issued immediately and the cluster keeps running.
        /// Ed25519-signed tokens are unaffected.
        #[clap(long, env = "ZLAYER_VACUUM_SECRETS")]
        vacuum_secrets: bool,
    },

    /// Manage the zlayer background daemon (systemd on Linux, launchd on
    /// macOS, SCM on Windows).
    #[command(subcommand, display_order = 32)]
    Daemon(DaemonAction),

    /// Windows-only maintenance commands (WSL2 distro management)
    #[cfg(all(target_os = "windows", feature = "wsl"))]
    #[command(subcommand, display_order = 32)]
    Windows(WindowsCommands),

    /// Tunnel management commands
    ///
    /// Create, manage, and connect tunnels for secure access to services.
    #[command(subcommand, display_order = 33)]
    Tunnel(TunnelCommands),

    /// `ZLayer` Manager commands
    ///
    /// Initialize, configure, and manage the `ZLayer` Manager web UI.
    #[command(subcommand, display_order = 34)]
    Manager(ManagerCommands),

    /// Image management commands
    ///
    /// List, remove, and prune cached images in the daemon's image storage.
    /// Works against whichever runtime the daemon is using (Youki, Docker, ...).
    #[command(subcommand, display_order = 35)]
    Image(ImageCommands),

    /// Container management commands
    ///
    /// List, inspect, remove, and view logs/stats of containers managed by
    /// the daemon.
    #[command(subcommand, display_order = 36)]
    Container(ContainerCommands),

    /// System-wide maintenance commands
    ///
    /// High-level maintenance operations: prune dangling resources, inspect
    /// daemon state, etc.
    #[command(subcommand, display_order = 37)]
    System(SystemCommands),

    /// Secrets management commands
    ///
    /// Create, list, inspect, and remove secrets stored by the daemon.
    /// Secret values are encrypted at rest and never exposed through
    /// listing or inspection.
    #[command(subcommand, display_order = 38)]
    Secret(SecretCommands),

    /// Run a local command with secrets injected as env vars.
    ///
    /// Resolution order (later wins on collision):
    ///   1. Parent env (inherited)
    ///   2. `global` env secrets (auto-merged unless --no-global)
    ///   3. Each --merge <slug> env in order
    ///   4. --env <slug> secrets (highest priority)
    ///
    /// Examples:
    ///   zlayer run --env dev -- pnpm run dev
    ///   zlayer run --env staging --no-global -- bash ./deploy.sh
    ///   zlayer run --env prod --merge global --merge baseline -- ./bin/server
    ///   zlayer run --env dev --dry-run         # masked preview
    ///   zlayer run --env dev --dry-run --unmask  # plaintext (admin)
    #[command(verbatim_doc_comment, display_order = 39)]
    Run {
        /// Primary environment to resolve secrets from (id or name).
        #[arg(long)]
        env: String,

        /// Skip the implicit `global` env merge. Off by default.
        #[arg(long, default_value_t = false)]
        no_global: bool,

        /// Additional env(s) to merge under the primary env. Left-to-right
        /// order: later flags win over earlier ones (but the primary --env
        /// always wins).
        #[arg(long = "merge", value_name = "SLUG")]
        merge: Vec<String>,

        /// Project id used to resolve non-UUID env names.
        #[arg(long)]
        project: Option<String>,

        /// Print the resolved env instead of spawning. Values are masked
        /// as `***` unless --unmask is also passed.
        #[arg(long, default_value_t = false)]
        dry_run: bool,

        /// Reveal plaintext values in --dry-run output (admin only).
        #[arg(long, default_value_t = false, requires = "dry_run")]
        unmask: bool,

        /// Command and arguments to run. Use `--` to separate from zlayer args.
        /// Example: `zlayer run --env dev -- pnpm run dev`
        #[arg(trailing_var_arg = true, required = true, num_args = 1..)]
        command: Vec<String>,
    },

    /// Environment management commands
    ///
    /// Manage deployment/runtime environments (e.g. `dev`, `staging`,
    /// `prod`). Each environment is an isolated namespace for secrets and,
    /// eventually, deployments. Optionally belongs to a project.
    #[command(subcommand, display_order = 38)]
    Env(EnvCommands),

    /// Network management commands
    ///
    /// List, create, inspect, and remove networks. Also show overlay
    /// network status, peers, and DNS entries.
    #[command(subcommand, display_order = 39)]
    Network(NetworkCommands),

    /// Variable management commands
    ///
    /// Manage plaintext key-value variables for template substitution in
    /// deployment specs. Variables are NOT encrypted (unlike secrets).
    /// They can be global or project-scoped.
    #[command(subcommand, display_order = 39)]
    Variable(VariableCommands),

    /// Task management commands
    ///
    /// Manage named runnable scripts that can be executed on demand.
    /// Tasks can be global or project-scoped.
    #[command(subcommand, display_order = 39)]
    Task(TaskCommands),

    /// Workflow management commands
    ///
    /// Manage named DAGs of steps that compose tasks, project builds,
    /// deploys, and sync applies. Steps run sequentially.
    #[command(subcommand, display_order = 39)]
    Workflow(WorkflowCommands),

    /// Notifier management commands
    ///
    /// Manage notification channels (Slack, Discord, webhook, SMTP)
    /// that fire alerts when triggered.
    #[command(subcommand, display_order = 39)]
    Notifier(NotifierCommands),

    /// Volume management commands
    ///
    /// List and remove named volumes stored by the daemon.
    #[command(subcommand, display_order = 40)]
    Volume(VolumeCommands),

    // ── Inspection & Configuration ────────────────────────────────────
    /// Validate a spec file without deploying
    #[command(display_order = 41)]
    Validate {
        /// Path to deployment spec (auto-discovers *.zlayer.yml in current directory)
        spec_path: Option<PathBuf>,
    },

    /// Token management commands
    ///
    /// Create, decode, and inspect JWT tokens for API authentication.
    #[command(subcommand, display_order = 41)]
    Token(TokenCommands),

    /// Authentication commands
    ///
    /// Log in to a `ZLayer` server, check status, or log out.
    #[command(subcommand, display_order = 41)]
    Auth(AuthCommands),

    /// User management commands
    ///
    /// Admin-only operations: create, list, update, and delete user accounts.
    #[command(subcommand, display_order = 43)]
    User(UserCommands),

    /// Group management commands
    ///
    /// Admin-only CRUD for user groups and membership management.
    #[command(subcommand, display_order = 44)]
    Group(GroupCommands),

    /// Permission management commands
    ///
    /// Admin-only: grant, list, and revoke resource-level permissions.
    #[command(subcommand, display_order = 44)]
    Permission(PermissionCommands),

    /// Audit log commands
    ///
    /// Admin-only: query the audit trail.
    #[command(subcommand, display_order = 44)]
    Audit(AuditCommands),

    /// Project management commands
    ///
    /// Create, list, update, and delete projects. Projects group
    /// deployments, credentials, and build configuration together.
    #[command(subcommand, display_order = 44)]
    Project(ProjectCommands),

    /// Credential management commands
    ///
    /// Store and manage registry and git credentials. Secrets are
    /// encrypted at rest; listing only exposes metadata.
    #[command(subcommand, display_order = 45)]
    Credential(CredentialCommands),

    /// `GitOps` sync management commands
    ///
    /// Manage sync resources that point at git directories containing
    /// `ZLayer` resource YAMLs. Supports diff and apply (dry-run in v1).
    #[command(subcommand, display_order = 46, name = "sync")]
    Sync(SyncCommands),

    /// Specification inspection commands
    ///
    /// Validate, dump, and inspect deployment specifications.
    #[command(subcommand, display_order = 42)]
    Spec(SpecCommands),

    /// Job and cron job management commands
    ///
    /// List, trigger, and check the status of jobs and cron jobs.
    #[command(subcommand, display_order = 42)]
    Job(JobCommands),

    /// WASM build, export, and management commands
    #[command(subcommand, display_order = 43)]
    Wasm(WasmCommands),

    // ── Docker Compatibility ──────────────────────────────────────────
    /// Docker-compatible commands (run, ps, compose, etc.)
    ///
    /// Provides Docker CLI compatibility so you can use familiar Docker
    /// commands through `ZLayer`.
    ///
    /// Examples:
    ///   zlayer docker ps
    ///   zlayer docker compose up -f docker-compose.yaml
    ///   zlayer docker run --rm -it alpine sh
    #[cfg(feature = "docker-compat")]
    #[command(subcommand, verbatim_doc_comment, display_order = 45)]
    Docker(Box<zlayer_docker::DockerCommands>),

    // ── Maintenance ───────────────────────────────────────────────────
    /// Replace the current zlayer binary with a newer release.
    ///
    /// Downloads the requested release tarball from GitHub, optionally
    /// verifies its SHA-256, and atomically swaps the running binary on
    /// disk. On Unix the swap is in-place (rename onto a running binary
    /// is safe); Windows requires a manual restart after the new file is
    /// staged next to the current one.
    #[command(name = "self-update", display_order = 49)]
    SelfUpdate {
        /// Target version (e.g. v0.12.0). Defaults to the latest GitHub release.
        #[arg(long)]
        version: Option<String>,

        /// Skip the confirmation prompt and apply immediately.
        #[arg(short = 'y', long)]
        yes: bool,

        /// Re-exec the new binary in place of the current process after install.
        #[arg(long)]
        restart: bool,

        /// GitHub repo (owner/name). Override for testing.
        #[arg(long, default_value = "BlackLeafDigital/ZLayer", hide = true)]
        repo: String,
    },

    // ── Interface ─────────────────────────────────────────────────────
    /// Launch interactive TUI
    #[command(display_order = 50)]
    Tui {
        /// Build context directory
        #[arg(short = 'c', long)]
        context: Option<PathBuf>,
    },

    /// Generate shell completion script
    ///
    /// Writes a completion script for the requested shell to stdout. Pipe
    /// the output into the shell's completion directory. Examples:
    ///   `zlayer completions bash > /etc/bash_completion.d/zlayer`
    ///   `zlayer completions zsh  > "${fpath[1]}/_zlayer"`
    ///   `zlayer completions fish > ~/.config/fish/completions/zlayer.fish`
    #[command(display_order = 51, verbatim_doc_comment)]
    Completions {
        /// Target shell (bash, zsh, fish, powershell, elvish)
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

/// Windows-only maintenance commands. Currently exposes VHDX compaction;
/// future subcommands will cover `.wslconfig` management and distro ops.
#[cfg(all(target_os = "windows", feature = "wsl"))]
#[derive(Debug, clap::Subcommand)]
pub enum WindowsCommands {
    /// Compact the `ZLayer` WSL2 distro's `ext4.vhdx` to reclaim freed space to the host.
    Compact {
        /// Skip the graceful daemon-stop wait; go straight to `wsl --shutdown`.
        #[arg(long)]
        force: bool,
    },
}

/// All flags accepted by `daemon install`. Boxed inside [`DaemonAction::Install`]
/// to keep the enum small (`clippy::large_enum_variant`).
#[derive(Args, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct InstallArgs {
    /// Don't start the service after installing
    #[arg(long)]
    pub(crate) no_start: bool,

    /// API bind address for the daemon
    #[arg(long, default_value = "0.0.0.0:3669")]
    pub(crate) bind: String,

    /// JWT secret for API authentication
    #[arg(long, env = "ZLAYER_JWT_SECRET")]
    pub(crate) jwt_secret: Option<String>,

    /// Disable Swagger UI
    #[arg(long)]
    pub(crate) no_swagger: bool,

    /// Enable Docker API socket emulation at /var/run/docker.sock
    #[cfg(feature = "docker-compat")]
    #[arg(long)]
    pub(crate) docker_socket: bool,

    /// WSL2 auto-install consent (Windows only — parsed here for
    /// forward-compatibility when `daemon install` lands on Windows).
    #[command(flatten)]
    pub(crate) install_wsl: InstallWslArgs,

    /// Admin email for the management UI (also accepts `ZLAYER_BOOTSTRAP_EMAIL` env).
    /// If unset and stdin is a TTY, install prompts unless `--no-admin-prompt`.
    #[clap(long, env = "ZLAYER_BOOTSTRAP_EMAIL")]
    pub(crate) admin_email: Option<String>,

    /// Admin password (cleartext). Prefer --admin-password-file in production.
    #[clap(long, hide_env_values = true)]
    pub(crate) admin_password: Option<String>,

    /// Path to a file containing the admin password (preferred over --admin-password).
    #[clap(long)]
    pub(crate) admin_password_file: Option<std::path::PathBuf>,

    /// Skip the interactive admin prompt entirely.
    #[clap(long)]
    pub(crate) no_admin_prompt: bool,

    /// Disable NAT traversal (STUN/TURN). Default is enabled.
    #[clap(long)]
    pub(crate) no_nat: bool,

    /// Override STUN servers (repeatable). Format: host:port.
    #[clap(long = "stun-server")]
    pub(crate) stun_servers: Vec<String>,

    /// Override TURN/relay servers (repeatable). Format: host:port.
    #[clap(long = "turn-server")]
    pub(crate) turn_servers: Vec<String>,

    /// Bind address for the built-in relay server (format: host:port).
    /// If unset, no relay server is started locally.
    #[clap(long)]
    pub(crate) relay_server_bind: Option<String>,

    /// Bind address for the tunnel WebSocket control server (format:
    /// host:port). Defaults to 0.0.0.0:3679 when unset.
    #[clap(long, env = "ZLAYER_TUNNEL_BIND")]
    pub(crate) tunnel_bind: Option<String>,

    /// Path to a TLS certificate (PEM) for the tunnel server.
    #[clap(long, env = "ZLAYER_TUNNEL_TLS_CERT")]
    pub(crate) tunnel_tls_cert: Option<std::path::PathBuf>,

    /// Path to a TLS private key (PEM) for the tunnel server.
    #[clap(long, env = "ZLAYER_TUNNEL_TLS_KEY")]
    pub(crate) tunnel_tls_key: Option<std::path::PathBuf>,

    /// Disable the daemon-side tunnel server entirely.
    #[clap(long, env = "ZLAYER_DISABLE_TUNNEL_SERVER")]
    pub(crate) no_tunnel_server: bool,
}

/// Daemon lifecycle actions
#[derive(Subcommand)]
pub(crate) enum DaemonAction {
    /// Install zlayer as a system service (launchd on macOS, systemd on Linux)
    Install(Box<InstallArgs>),

    /// Uninstall the zlayer system service
    Uninstall,

    /// Start the daemon service
    Start,

    /// Stop the daemon service
    Stop,

    /// Restart the daemon service
    Restart,

    /// Show daemon and service status
    Status,

    /// Reset daemon state (wipes Raft storage for clean reinit)
    Reset {
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },

    /// Resume deployments from a snapshot file written by `daemon install`.
    ///
    /// `daemon install` snapshots the running deployments before restarting the
    /// daemon and, on success, replays the snapshot to re-scale services that
    /// ended up stopped. If the install fails partway, or the auto-restore
    /// only handled some services, the snapshot file is retained at
    /// `<data_dir>/.install-snapshot-<unix-ts>.json` and can be replayed
    /// manually with this command.
    ResumeFromSnapshot {
        /// Path to the snapshot JSON file produced by `daemon install`.
        path: std::path::PathBuf,
    },

    /// Migrate the on-disk data directory layout to the current version's
    /// expected layout. Idempotent — safe to run repeatedly. The daemon also
    /// runs this on every boot, so this is primarily a manual escape hatch.
    Migrate {
        /// Override the data directory. Defaults to the top-level --data-dir
        /// (or its platform default if unset).
        #[arg(long)]
        data_dir: Option<std::path::PathBuf>,
        /// Report what would be migrated without making any changes.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Tunnel management subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum TunnelCommands {
    /// Create a new tunnel token
    ///
    /// Generates an authentication token that can be used to establish a tunnel.
    ///
    /// Examples:
    ///   zlayer tunnel create --name my-tunnel
    ///   zlayer tunnel create --name db-access --services postgres,redis
    #[command(verbatim_doc_comment)]
    Create {
        /// Name for this tunnel (for identification)
        #[arg(short, long)]
        name: String,

        /// Services this tunnel is allowed to expose (comma-separated)
        #[arg(short, long)]
        services: Option<String>,

        /// Token time-to-live in hours (default: 24)
        #[arg(long, default_value = "24")]
        ttl_hours: u64,
    },

    /// List all tunnel tokens
    List {
        /// Output format (table or json)
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Revoke a tunnel token
    Revoke {
        /// Tunnel ID to revoke
        id: String,
    },

    /// Connect to a tunnel server as a client
    ///
    /// Establishes a connection to a tunnel server and exposes local services.
    ///
    /// Examples:
    ///   zlayer tunnel connect --server <wss://tunnel.example.com> --token `tun_xxx` --service ssh:22:2222
    ///   zlayer tunnel connect --server <wss://tunnel.example.com> --token `tun_xxx` --service postgres:5432
    #[command(verbatim_doc_comment)]
    Connect {
        /// Tunnel server URL (e.g., <wss://tunnel.example.com/tunnel/v1>)
        #[arg(long)]
        server: String,

        /// Authentication token
        #[arg(long)]
        token: String,

        /// Service to expose (format: `name:local_port`[:`remote_port`], can be repeated)
        #[arg(short, long = "service")]
        services: Vec<String>,
    },

    /// Add a node-to-node tunnel
    ///
    /// Creates a tunnel between two nodes in the cluster.
    ///
    /// Examples:
    ///   zlayer tunnel add db-tunnel --from node1 --to node2 --local-port 5432 --remote-port 5432
    ///   zlayer tunnel add web-proxy --from edge --to backend --local-port 80 --remote-port 8080 --expose public
    #[command(verbatim_doc_comment)]
    Add {
        /// Tunnel name
        name: String,

        /// Source node ID
        #[arg(long)]
        from: String,

        /// Destination node ID
        #[arg(long)]
        to: String,

        /// Local port on source node
        #[arg(long)]
        local_port: u16,

        /// Remote port on destination node
        #[arg(long)]
        remote_port: u16,

        /// Exposure level: public or internal
        #[arg(long, default_value = "internal")]
        expose: String,
    },

    /// Remove a node-to-node tunnel
    Remove {
        /// Tunnel name
        name: String,
    },

    /// Show tunnel status
    Status {
        /// Tunnel ID (optional, shows all if not specified)
        id: Option<String>,
    },

    /// Request temporary access to a tunneled service
    ///
    /// Creates a local proxy for accessing a remote service through a tunnel.
    ///
    /// Examples:
    ///   zlayer tunnel access postgres.service.zlayer --local-port 15432
    ///   zlayer tunnel access ssh.service.zlayer --ttl 1h
    #[command(verbatim_doc_comment)]
    Access {
        /// Service endpoint to access
        endpoint: String,

        /// Local port for the proxy (default: auto-assign)
        #[arg(long)]
        local_port: Option<u16>,

        /// Time-to-live for the access (e.g., 1h, 30m)
        #[arg(long, default_value = "1h")]
        ttl: String,
    },
}

/// Image management subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum ImageCommands {
    /// List cached images known to the daemon.
    ///
    /// Examples:
    ///   zlayer image ls
    #[command(visible_alias = "list", verbatim_doc_comment)]
    Ls {
        /// Output format (table or json)
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Remove a cached image from the daemon.
    ///
    /// Examples:
    ///   zlayer image rm zachhandley/zlayer-manager:latest
    ///   zlayer image rm ubuntu:22.04 --force
    #[command(visible_alias = "remove", verbatim_doc_comment)]
    Rm {
        /// Image reference to remove (e.g. `myregistry/myimage:tag`).
        image: String,

        /// Force removal even if the image is referenced by a container.
        #[arg(long)]
        force: bool,
    },

    /// Push an image to a remote registry.
    ///
    /// Examples:
    ///   zlayer image push myregistry/myimage:tag
    ///   zlayer image push myregistry/myimage:tag --username user --password pass
    #[command(verbatim_doc_comment)]
    Push {
        /// Image reference to push (e.g. `myregistry/myimage:tag`).
        image: String,

        /// Registry username.
        #[arg(long)]
        username: Option<String>,

        /// Registry password.
        #[arg(long)]
        password: Option<String>,
    },

    /// Inspect an image (show manifest and configuration details).
    ///
    /// Examples:
    ///   zlayer image inspect ubuntu:22.04
    ///   zlayer image inspect zachhandley/zlayer-manager:latest
    #[command(verbatim_doc_comment)]
    Inspect {
        /// Image reference to inspect.
        image: String,
    },
}

/// Container management subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum ContainerCommands {
    /// List all containers managed by the daemon.
    ///
    /// Examples:
    ///   zlayer container ls
    ///   zlayer container ls --output json
    #[command(visible_alias = "list", verbatim_doc_comment)]
    Ls {
        /// Output format (table or json)
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Inspect a container (detailed JSON output).
    ///
    /// Examples:
    ///   zlayer container inspect abc123
    #[command(verbatim_doc_comment)]
    Inspect {
        /// Container ID
        id: String,
    },

    /// Remove a container.
    ///
    /// Examples:
    ///   zlayer container rm abc123
    ///   zlayer container rm abc123 --force
    #[command(visible_alias = "remove", verbatim_doc_comment)]
    Rm {
        /// Container ID
        id: String,

        /// Force removal even if the container is running.
        #[arg(long)]
        force: bool,
    },

    /// View container logs.
    ///
    /// Examples:
    ///   zlayer container logs abc123
    ///   zlayer container logs abc123 --tail 100
    #[command(verbatim_doc_comment)]
    Logs {
        /// Container ID
        id: String,

        /// Number of most recent log lines to return.
        #[arg(long)]
        tail: Option<u32>,
    },

    /// View container resource statistics.
    ///
    /// Examples:
    ///   zlayer container stats abc123
    #[command(verbatim_doc_comment)]
    Stats {
        /// Container ID
        id: String,
    },
}

/// System-wide maintenance subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum SystemCommands {
    /// Prune dangling / unused cached images.
    ///
    /// Removes any cached image blob that is not referenced by a currently
    /// cached manifest. Safe to run on a long-lived daemon to reclaim disk
    /// space.
    ///
    /// Examples:
    ///   zlayer system prune
    ///   zlayer system prune --yes
    #[command(verbatim_doc_comment)]
    Prune {
        /// Skip the confirmation prompt.
        #[arg(long, short)]
        yes: bool,
    },
}

/// Secrets management subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum SecretCommands {
    /// List secrets. Pass `--env <id|name>` to list secrets in a specific
    /// environment; without `--env`, lists all secrets in the default scope.
    #[command(visible_alias = "list")]
    Ls {
        /// Output format (table or json)
        #[arg(long, default_value = "table")]
        output: String,
        /// Environment id or name to scope the listing to.
        #[arg(long)]
        env: Option<String>,
        /// Project id used to resolve a non-UUID environment name.
        #[arg(long)]
        project: Option<String>,
    },
    /// Create a new secret. Exactly one of `--value`, `--from-stdin`,
    /// `--from-file`, or `--random` must be provided.
    Create {
        /// Secret name
        name: String,
        /// Inline value. Exposed on argv — prefer `--from-file` / `--from-stdin`
        /// for anything sensitive.
        #[arg(long, group = "value_source")]
        value: Option<String>,
        /// Read the value from stdin (no newline trim beyond a single trailing newline).
        #[arg(long, group = "value_source")]
        from_stdin: bool,
        /// Read the value from a file (trailing newline trimmed).
        #[arg(long, group = "value_source", value_name = "PATH")]
        from_file: Option<PathBuf>,
        /// Generate a random alphanumeric value and store it. Prints the
        /// generated value once to stdout after creation.
        #[arg(long, group = "value_source")]
        random: bool,
        /// Length of the generated random value (requires `--random`).
        #[arg(long, default_value_t = 32)]
        random_length: usize,
        /// Environment id or name to scope the secret to.
        #[arg(long)]
        env: Option<String>,
        /// Project id used to resolve a non-UUID environment name.
        #[arg(long)]
        project: Option<String>,
    },
    /// Get secret metadata. Pass `--reveal` together with `--env` to obtain
    /// the plaintext value (admin only).
    Get {
        /// Secret name
        name: String,
        /// Environment id or name to scope the lookup to.
        #[arg(long)]
        env: Option<String>,
        /// Project id used to resolve a non-UUID environment name.
        #[arg(long)]
        project: Option<String>,
        /// Include the plaintext value in the output (requires admin and
        /// `--env`).
        #[arg(long, default_value_t = false)]
        reveal: bool,
    },
    /// Remove a secret.
    #[command(visible_alias = "remove")]
    Rm {
        /// Secret name
        name: String,
        /// Environment id or name to scope the removal to.
        #[arg(long)]
        env: Option<String>,
        /// Project id used to resolve a non-UUID environment name.
        #[arg(long)]
        project: Option<String>,
    },
    /// Set (create-or-update) a secret. Exactly one of `--value`,
    /// `--from-stdin`, `--from-file`, or `--random` is required.
    Set {
        /// Secret name.
        name: String,
        /// Inline value. Prefer `--from-file` / `--from-stdin` for sensitive data.
        #[arg(long, group = "value_source_set")]
        value: Option<String>,
        /// Read the value from stdin (no newline trim beyond a single trailing newline).
        #[arg(long, group = "value_source_set")]
        from_stdin: bool,
        /// Read the value from a file (trailing newline trimmed).
        #[arg(long, group = "value_source_set", value_name = "PATH")]
        from_file: Option<PathBuf>,
        /// Generate a random alphanumeric value and store it. Prints the
        /// generated value once to stdout after setting.
        #[arg(long, group = "value_source_set")]
        random: bool,
        /// Length of the generated random value (requires `--random`).
        #[arg(long, default_value_t = 32)]
        random_length: usize,
        /// Environment id or name to store the secret under.
        #[arg(long)]
        env: String,
        /// Project id used to resolve a non-UUID environment name.
        #[arg(long)]
        project: Option<String>,
    },
    /// Rotate an existing secret: overwrite with a new value and return the
    /// version before+after. Fails if the secret does not exist.
    Rotate {
        /// Secret name.
        name: String,
        /// Inline value. Prefer `--from-file` / `--from-stdin` for sensitive data.
        #[arg(long, group = "value_source_rotate")]
        value: Option<String>,
        /// Read the value from stdin (no newline trim beyond a single trailing newline).
        #[arg(long, group = "value_source_rotate")]
        from_stdin: bool,
        /// Read the value from a file (trailing newline trimmed).
        #[arg(long, group = "value_source_rotate", value_name = "PATH")]
        from_file: Option<PathBuf>,
        /// Generate a random alphanumeric value and store it. Prints the
        /// generated value once to stdout after rotation.
        #[arg(long, group = "value_source_rotate")]
        random: bool,
        /// Length of the generated random value (requires `--random`).
        #[arg(long, default_value_t = 32)]
        random_length: usize,
        /// Environment id or name.
        #[arg(long)]
        env: String,
        /// Project id used to resolve a non-UUID environment name.
        #[arg(long)]
        project: Option<String>,
    },
    /// Unset (delete) a secret by name. Requires `--env`.
    Unset {
        /// Secret name
        name: String,
        /// Environment id or name to scope the removal to.
        #[arg(long)]
        env: String,
        /// Project id used to resolve a non-UUID environment name.
        #[arg(long)]
        project: Option<String>,
    },
    /// Bulk-import secrets from a `.env`-style file into an environment.
    Import {
        /// Path to a `.env`-format file (`KEY=VALUE` per line).
        #[arg(long)]
        file: PathBuf,
        /// Environment id or name to import into.
        #[arg(long)]
        env: String,
        /// Project id used to resolve a non-UUID environment name.
        #[arg(long)]
        project: Option<String>,
    },
    /// Export secrets from an environment (admin only; reveals plaintext
    /// values).
    Export {
        /// Environment id or name to export from.
        #[arg(long)]
        env: String,
        /// Output format: `env` for `KEY=VALUE` lines, `json` for a map.
        #[arg(long, default_value = "env")]
        format: String,
        /// Project id used to resolve a non-UUID environment name.
        #[arg(long)]
        project: Option<String>,
    },

    /// Grant a user access to an environment's secrets.
    ///
    /// Wraps `POST /api/v1/permissions` with `resource_kind = "environment"`.
    /// Admin only.
    ///
    /// Examples:
    ///   zlayer secret grant alice@example.com dev read
    ///   zlayer secret grant u-1234 prod write --project p-1
    #[command(verbatim_doc_comment)]
    Grant {
        /// User email or id.
        user: String,
        /// Environment id or name.
        env: String,
        /// Access level: `read`, `execute`, or `write` (case-insensitive).
        level: String,
        /// Project id used to resolve a non-UUID env name.
        #[arg(long)]
        project: Option<String>,
    },

    /// Revoke a user's access to an environment's secrets.
    ///
    /// Resolves the grant row by listing the user's permissions and filtering
    /// on `resource_kind = "environment"` + `resource_id = <env_id>`, then
    /// calls `DELETE /api/v1/permissions/{id}`.
    ///
    /// Examples:
    ///   zlayer secret revoke alice@example.com dev
    ///   zlayer secret revoke u-1234 prod --project p-1
    #[command(verbatim_doc_comment)]
    Revoke {
        /// User email or id.
        user: String,
        /// Environment id or name.
        env: String,
        /// Project id used to resolve a non-UUID env name.
        #[arg(long)]
        project: Option<String>,
    },

    /// List every grant on a given environment.
    ///
    /// Wraps `GET /api/v1/permissions/by-resource?kind=environment&id=<env>`.
    ///
    /// Examples:
    ///   zlayer secret permissions dev
    ///   zlayer secret permissions prod --project p-1 --output json
    #[command(verbatim_doc_comment)]
    Permissions {
        /// Environment id or name.
        env: String,
        /// Project id used to resolve a non-UUID env name.
        #[arg(long)]
        project: Option<String>,
        /// Output format: `table` or `json`.
        #[arg(long, default_value = "table")]
        output: String,
    },
}

/// Environment management subcommands.
///
/// An environment is a named, isolated namespace for secrets (and, in
/// future phases, deployments). Environments are either global
/// (`project_id = None`) or owned by a specific project.
#[derive(Subcommand, Debug)]
pub(crate) enum EnvCommands {
    /// List environments.
    #[command(visible_alias = "list")]
    Ls {
        /// Filter by project id. When omitted, only global environments are
        /// listed; pass `*` to list every environment across projects and
        /// globals.
        #[arg(long)]
        project: Option<String>,
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Create a new environment.
    Create {
        /// Environment name (e.g. "dev", "prod").
        name: String,
        /// Owning project id. Omit for a global environment.
        #[arg(long)]
        project: Option<String>,
        /// Free-form description shown in the UI.
        #[arg(long)]
        description: Option<String>,
    },

    /// Show one environment by id.
    Show {
        /// Environment id.
        id: String,
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "json")]
        output: String,
    },

    /// Update an environment's name and/or description.
    Update {
        /// Environment id.
        id: String,
        /// New display name.
        #[arg(long)]
        name: Option<String>,
        /// New description. Pass an empty string to clear.
        #[arg(long)]
        description: Option<String>,
    },

    /// Delete an environment. Fails if it still contains secrets.
    Delete {
        /// Environment id.
        id: String,
        /// Skip the confirmation prompt.
        #[arg(long, short)]
        yes: bool,
    },
}

/// Variable management subcommands.
///
/// Variables are plaintext key-value pairs for template substitution in
/// deployment specs. Unlike secrets, variable values are NOT encrypted
/// and are fully visible in API responses. They can be global
/// (`scope = None`) or project-scoped.
#[derive(Subcommand, Debug)]
pub(crate) enum VariableCommands {
    /// List variables.
    #[command(visible_alias = "ls")]
    List {
        /// Filter by scope (project id). When omitted, only global variables
        /// are listed.
        #[arg(long)]
        scope: Option<String>,
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Set a variable (create or update by name+scope).
    Set {
        /// Variable name (e.g. `APP_VERSION`).
        name: String,
        /// Variable value.
        value: String,
        /// Scope (project id). Omit for a global variable.
        #[arg(long)]
        scope: Option<String>,
    },

    /// Get a variable's value by name.
    Get {
        /// Variable name.
        name: String,
        /// Scope (project id). Omit for a global variable.
        #[arg(long)]
        scope: Option<String>,
    },

    /// Unset (delete) a variable by name+scope.
    Unset {
        /// Variable name.
        name: String,
        /// Scope (project id). Omit for a global variable.
        #[arg(long)]
        scope: Option<String>,
    },
}

/// Task management subcommands.
///
/// Tasks are named runnable scripts that can be executed on demand.
/// When run, the task body is executed as a subprocess and stdout/stderr
/// are captured.
#[derive(Subcommand, Debug)]
pub(crate) enum TaskCommands {
    /// List tasks.
    #[command(visible_alias = "ls")]
    List {
        /// Filter by project id. When omitted, all tasks are listed.
        #[arg(long)]
        project: Option<String>,
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Create a new task.
    Create {
        /// Task name.
        #[arg(long)]
        name: String,
        /// Script type.
        #[arg(long, default_value = "bash")]
        kind: String,
        /// The script/command body.
        #[arg(long)]
        body: String,
        /// Project id to scope the task to.
        #[arg(long)]
        project: Option<String>,
    },

    /// Execute a task synchronously.
    Run {
        /// Task id.
        id: String,
    },

    /// Show the last run's output for a task.
    Logs {
        /// Task id.
        id: String,
    },

    /// Delete a task.
    Delete {
        /// Task id.
        id: String,
        /// Skip confirmation prompt.
        #[arg(short, long)]
        yes: bool,
    },
}

/// Workflow management subcommands.
///
/// Workflows are named DAGs of steps that compose tasks, project builds,
/// deploys, and sync applies. Steps execute sequentially; if a step fails,
/// the optional `on_failure` handler runs before aborting.
#[derive(Subcommand, Debug)]
pub(crate) enum WorkflowCommands {
    /// List workflows.
    #[command(visible_alias = "ls")]
    List {
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Create a new workflow.
    Create {
        /// Workflow name.
        #[arg(long)]
        name: String,
        /// Steps as inline JSON array.
        #[arg(long)]
        steps: String,
        /// Project id scope.
        #[arg(long)]
        project: Option<String>,
    },

    /// Execute a workflow synchronously.
    Run {
        /// Workflow id.
        id: String,
    },

    /// Show the last run's step results for a workflow.
    Logs {
        /// Workflow id.
        id: String,
    },

    /// Delete a workflow.
    Delete {
        /// Workflow id.
        id: String,
        /// Skip confirmation prompt.
        #[arg(short, long)]
        yes: bool,
    },
}

/// Project management subcommands.
///
/// Notifier management subcommands.
///
/// Notifiers are named notification channels that send alerts to Slack,
/// Discord, generic webhooks, or SMTP endpoints when triggered.
#[derive(Subcommand, Debug)]
pub(crate) enum NotifierCommands {
    /// List notifiers.
    #[command(visible_alias = "ls")]
    List {
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Create a new notifier.
    Create {
        /// Notifier name.
        #[arg(long)]
        name: String,
        /// Notification channel kind (slack, discord, webhook, smtp).
        #[arg(long)]
        kind: String,
        /// Webhook URL (required for slack and discord).
        #[arg(long)]
        webhook_url: Option<String>,
        /// Target URL (required for generic webhook).
        #[arg(long)]
        url: Option<String>,
    },

    /// Send a test notification.
    Test {
        /// Notifier id.
        id: String,
    },

    /// Delete a notifier.
    Delete {
        /// Notifier id.
        id: String,
        /// Skip confirmation prompt.
        #[arg(short, long)]
        yes: bool,
    },
}

/// Projects group deployments, credentials, environments, and build
/// configuration under a single entity.
#[derive(Subcommand, Debug)]
pub(crate) enum ProjectCommands {
    /// List all projects.
    #[command(visible_alias = "list")]
    Ls {
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Create a new project.
    Create {
        /// Project name.
        name: String,

        /// Git repository URL.
        #[arg(long)]
        git_url: Option<String>,

        /// Git branch (defaults to the repo's default branch).
        #[arg(long)]
        git_branch: Option<String>,

        /// Build kind.
        #[arg(long, value_enum)]
        build_kind: Option<CliBuildKind>,

        /// Relative path to the build context within the repo.
        #[arg(long)]
        build_path: Option<String>,

        /// Free-form description.
        #[arg(long)]
        description: Option<String>,

        /// Registry credential id to attach.
        #[arg(long)]
        registry_credential: Option<String>,

        /// Git credential id to attach.
        #[arg(long)]
        git_credential: Option<String>,

        /// Default environment id to assign.
        #[arg(long)]
        default_env: Option<String>,
    },

    /// Show one project by id.
    Show {
        /// Project id.
        id: String,
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "json")]
        output: String,
    },

    /// Update project fields (partial).
    Update {
        /// Project id.
        id: String,

        /// New display name.
        #[arg(long)]
        name: Option<String>,

        /// New description. Pass an empty string to clear.
        #[arg(long)]
        description: Option<String>,

        /// New git URL.
        #[arg(long)]
        git_url: Option<String>,

        /// New git branch.
        #[arg(long)]
        git_branch: Option<String>,

        /// New build kind.
        #[arg(long, value_enum)]
        build_kind: Option<CliBuildKind>,

        /// New build path.
        #[arg(long)]
        build_path: Option<String>,

        /// New registry credential id.
        #[arg(long)]
        registry_credential: Option<String>,

        /// New git credential id.
        #[arg(long)]
        git_credential: Option<String>,

        /// New default environment id.
        #[arg(long)]
        default_env: Option<String>,
    },

    /// Delete a project.
    Delete {
        /// Project id.
        id: String,

        /// Skip the confirmation prompt.
        #[arg(long, short)]
        yes: bool,
    },

    /// Link a deployment to a project.
    LinkDeployment {
        /// Project id.
        id: String,
        /// Deployment name.
        deployment: String,
    },

    /// Unlink a deployment from a project.
    UnlinkDeployment {
        /// Project id.
        id: String,
        /// Deployment name.
        deployment: String,
    },

    /// List deployments linked to a project.
    ListDeployments {
        /// Project id.
        id: String,
    },

    /// Clone or fast-forward pull the project's git repository on the
    /// daemon. Uses the project's `git_credential_id` for authentication
    /// when set; falls back to anonymous otherwise.
    Pull {
        /// Project id.
        id: String,
    },

    /// Enable or disable automatic deploy on new commits.
    AutoDeploy {
        /// Project id.
        id: String,
        /// Set auto-deploy on or off.
        #[arg(long)]
        enabled: bool,
    },

    /// Set (or clear) the git polling interval for a project.
    ///
    /// When set, the daemon periodically checks the remote for new
    /// commits and pulls them automatically.
    PollInterval {
        /// Project id.
        id: String,
        /// Polling interval in seconds.  Pass 0 to disable polling.
        #[arg(long)]
        seconds: u64,
    },

    /// Manage the webhook configuration for a project.
    ///
    /// Each project can have a webhook secret used by git hosts (GitHub,
    /// Gitea, Forgejo, GitLab) to authenticate push events.
    #[command(subcommand)]
    Webhook(WebhookCommands),
}

/// Webhook management subcommands for projects.
#[derive(Subcommand, Debug)]
pub(crate) enum WebhookCommands {
    /// Show the webhook URL and secret for a project.
    ///
    /// Generates the secret on first call if it does not exist yet.
    Show {
        /// Project id.
        id: String,
    },

    /// Rotate (regenerate) the webhook secret for a project.
    ///
    /// Admin only. After rotation the old secret is invalidated and the
    /// git host must be reconfigured with the new value.
    Rotate {
        /// Project id.
        id: String,
    },
}

/// CLI-friendly build kind. Sent as a lowercase string in the JSON body.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum CliBuildKind {
    Dockerfile,
    Compose,
    Zimagefile,
    Spec,
}

impl std::fmt::Display for CliBuildKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Dockerfile => "dockerfile",
            Self::Compose => "compose",
            Self::Zimagefile => "zimagefile",
            Self::Spec => "spec",
        };
        f.write_str(s)
    }
}

/// Credential management subcommands.
///
/// Registry and git credentials are stored encrypted at rest. Only
/// metadata (id, registry, username, kind) is exposed through listing.
#[derive(Subcommand, Debug)]
pub(crate) enum CredentialCommands {
    /// Registry credential subcommands.
    #[command(subcommand)]
    Registry(RegistryCredentialCommands),
    /// Git credential subcommands.
    #[command(subcommand)]
    Git(GitCredentialCommands),
}

/// Registry credential subcommands.
#[derive(Subcommand, Debug)]
pub(crate) enum RegistryCredentialCommands {
    /// List registry credentials.
    #[command(visible_alias = "list")]
    Ls {
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "table")]
        output: String,
    },
    /// Add a registry credential.
    Add {
        /// Container registry URL (e.g. `ghcr.io`, `docker.io`).
        #[arg(long)]
        registry: String,

        /// Registry username.
        #[arg(long)]
        username: String,

        /// Registry password or token (prompted if omitted).
        #[arg(long)]
        password: Option<String>,

        /// Authentication type.
        #[arg(long, value_enum, default_value_t = CliRegistryAuthType::Basic)]
        auth_type: CliRegistryAuthType,
    },
    /// Delete a registry credential.
    Delete {
        /// Credential id.
        id: String,

        /// Skip the confirmation prompt.
        #[arg(long, short)]
        yes: bool,
    },
}

/// Git credential subcommands.
#[derive(Subcommand, Debug)]
pub(crate) enum GitCredentialCommands {
    /// List git credentials.
    #[command(visible_alias = "list")]
    Ls {
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "table")]
        output: String,
    },
    /// Add a git credential (PAT or SSH key).
    Add {
        /// Credential display name.
        #[arg(long)]
        name: String,

        /// Credential value (PAT string or SSH private key; prompted if omitted).
        #[arg(long)]
        value: Option<String>,

        /// Credential kind.
        #[arg(long, value_enum)]
        kind: CliGitCredentialKind,
    },
    /// Delete a git credential.
    Delete {
        /// Credential id.
        id: String,

        /// Skip the confirmation prompt.
        #[arg(long, short)]
        yes: bool,
    },
}

/// CLI-friendly registry auth type.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum CliRegistryAuthType {
    Basic,
    Token,
}

impl std::fmt::Display for CliRegistryAuthType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Basic => "basic",
            Self::Token => "token",
        };
        f.write_str(s)
    }
}

/// CLI-friendly git credential kind.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum CliGitCredentialKind {
    Pat,
    SshKey,
}

impl std::fmt::Display for CliGitCredentialKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pat => "pat",
            Self::SshKey => "ssh-key",
        };
        f.write_str(s)
    }
}

/// Sync management subcommands.
///
/// Syncs point at git directories containing `ZLayer` resource YAMLs and
/// support diff / apply operations for `GitOps` workflows.
#[derive(Subcommand, Debug)]
pub(crate) enum SyncCommands {
    /// List all syncs.
    #[command(visible_alias = "list")]
    Ls {
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Create a new sync.
    Create {
        /// Display name for this sync.
        #[arg(long)]
        name: String,

        /// Linked project id.
        #[arg(long)]
        project: Option<String>,

        /// Path within the project's checkout to scan for resource YAMLs.
        #[arg(long)]
        path: String,

        /// Automatically apply on pull.
        #[arg(long)]
        auto_apply: bool,
    },

    /// Compute a diff for a sync (what would change on apply).
    Diff {
        /// Sync id.
        id: String,
    },

    /// Apply a sync (dry-run in v1).
    Apply {
        /// Sync id.
        id: String,
    },

    /// Delete a sync.
    Delete {
        /// Sync id.
        id: String,

        /// Skip confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// Network management subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum NetworkCommands {
    /// List all networks
    #[command(visible_alias = "list")]
    Ls {
        /// Output format (table or json)
        #[arg(long, default_value = "table")]
        output: String,
    },
    /// Inspect a network
    Inspect {
        /// Network name
        name: String,
    },
    /// Create a network
    Create {
        /// Network name
        name: String,
    },
    /// Remove a network
    #[command(visible_alias = "remove")]
    Rm {
        /// Network name
        name: String,
    },
    /// Show overlay network status
    Status,
    /// List overlay peers
    Peers,
    /// Show DNS entries
    Dns,
}

/// Volume management subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum VolumeCommands {
    /// List all volumes
    ///
    /// Examples:
    ///   zlayer volume ls
    ///   zlayer volume ls --output json
    #[command(visible_alias = "list", verbatim_doc_comment)]
    Ls {
        /// Output format (table or json)
        #[arg(long, default_value = "table")]
        output: String,
    },
    /// Remove a volume
    ///
    /// Examples:
    ///   zlayer volume rm my-volume
    ///   zlayer volume rm my-volume --force
    #[command(visible_alias = "remove", verbatim_doc_comment)]
    Rm {
        /// Volume name
        name: String,
        /// Force removal even if the volume is non-empty
        #[arg(long)]
        force: bool,
    },
}

/// Job management subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum JobCommands {
    /// List all jobs
    #[command(visible_alias = "list")]
    Ls {
        /// Output format (table or json)
        #[arg(long, default_value = "table")]
        output: String,
    },
    /// Trigger a job
    Trigger {
        /// Job name
        job: String,
        /// Deployment name (optional — auto-resolves when unambiguous)
        #[arg(long)]
        deployment: Option<String>,
    },
    /// Get job status
    Status {
        /// Job name
        job: String,
        /// Deployment name (optional — auto-resolves when unambiguous)
        #[arg(long)]
        deployment: Option<String>,
    },
    /// Cron job management
    #[command(subcommand)]
    Cron(CronCommands),
}

/// Cron job subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum CronCommands {
    /// List all cron jobs
    #[command(visible_alias = "list")]
    Ls {
        /// Output format (table or json)
        #[arg(long, default_value = "table")]
        output: String,
    },
    /// Get cron job status
    Status {
        /// Cron job name
        cron: String,
        /// Deployment name (optional — auto-resolves when unambiguous)
        #[arg(long)]
        deployment: Option<String>,
    },
}

/// `ZLayer` Manager subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum ManagerCommands {
    /// Initialize zlayer-manager deployment
    ///
    /// Creates a deployment spec file for zlayer-manager and optionally
    /// deploys it immediately. When run interactively, prompts for an admin
    /// email + password, stores the password in the daemon's `bootstrap`
    /// environment via the secrets API, and emits a spec that references the
    /// secret via `$secret://bootstrap/ZLAYER_BOOTSTRAP_PASSWORD`.
    ///
    /// Examples:
    ///   zlayer manager init
    ///   zlayer manager init --port 8080
    ///   zlayer manager init --deploy
    ///   zlayer manager init --email admin@example.com --random
    ///   zlayer manager init --env-file /run/secrets/manager.env
    ///   zlayer manager init --no-bootstrap
    #[command(verbatim_doc_comment, disable_version_flag = true)]
    Init {
        /// Output directory for spec file (default: current directory)
        #[arg(short, long, default_value = ".")]
        output: PathBuf,

        /// Port to expose manager on (default: 3000)
        #[arg(short, long, default_value = "3000")]
        port: u16,

        /// Deploy immediately after creating spec
        #[arg(long)]
        deploy: bool,

        /// `ZLayer` version to use (default: latest)
        #[arg(long)]
        version: Option<String>,

        /// Admin email (interactive prompt if missing, unless --no-prompt is set).
        #[arg(long)]
        email: Option<String>,

        /// Inline admin password. Exposed on argv — prefer --password-file or --random.
        #[arg(long, group = "bootstrap_pw")]
        password: Option<String>,

        /// Read the admin password from a file.
        #[arg(long, group = "bootstrap_pw", value_name = "PATH")]
        password_file: Option<PathBuf>,

        /// Generate a random 32-char alphanumeric admin password. Printed once.
        #[arg(long, group = "bootstrap_pw", default_value_t = false)]
        random: bool,

        /// Skip the interactive prompt. When set without --password/--password-file/--random,
        /// the spec is emitted WITHOUT a bootstrap block (equivalent to --no-bootstrap).
        #[arg(long, default_value_t = false)]
        no_prompt: bool,

        /// Emit the spec with `ZLAYER_BOOTSTRAP_PASSWORD_FILE` pointing at the
        /// given file instead of generating+storing a password. Skips all
        /// prompts and secret-store writes.
        #[arg(long, value_name = "PATH")]
        env_file: Option<PathBuf>,

        /// Emit the legacy spec with commented-out bootstrap env examples.
        /// Skip prompts + secret-store writes. Mutually exclusive with --env-file.
        #[arg(long, default_value_t = false)]
        no_bootstrap: bool,

        /// Overwrite an existing bootstrap password. By default, if a
        /// `ZLAYER_BOOTSTRAP_PASSWORD` is already stored in the `bootstrap`
        /// environment, it is reused and the spec is rewritten without
        /// prompting. Pass --force to rotate the stored password.
        #[arg(long, default_value_t = false)]
        force: bool,
    },

    /// Show manager status
    Status,

    /// Stop the manager
    Stop {
        /// Force stop without graceful shutdown
        #[arg(short, long)]
        force: bool,
    },
}

/// WASM management subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum WasmCommands {
    /// Build WASM from source code
    ///
    /// Compiles source code to a WebAssembly binary using the appropriate
    /// compiler for the detected or specified language.
    ///
    /// Examples:
    ///   zlayer wasm build .
    ///   zlayer wasm build --language rust ./my-rust-app
    ///   zlayer wasm build --target wasip2 -o output.wasm .
    #[command(verbatim_doc_comment)]
    Build {
        /// Path to the source directory
        context: PathBuf,

        /// Language (auto-detected if not specified)
        #[arg(short, long)]
        language: Option<String>,

        /// WASI target (preview1 or preview2)
        #[arg(short, long, default_value = "preview2")]
        target: String,

        /// Output file path
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Optimize the output (release mode)
        #[arg(long)]
        optimize: bool,

        /// WIT directory path
        #[arg(long)]
        wit: Option<PathBuf>,
    },

    /// Export WASM binary as OCI artifact
    ///
    /// Creates an OCI-compliant artifact from a WASM binary that can be
    /// pushed to any OCI registry (ghcr.io, Docker Hub, etc).
    ///
    /// Examples:
    ///   zlayer wasm export ./app.wasm --name myapp:v1
    ///   zlayer wasm export ./app.wasm --name ghcr.io/myorg/myapp:latest
    #[command(verbatim_doc_comment)]
    Export {
        /// Path to the WASM file
        wasm_file: PathBuf,

        /// Name/tag for the artifact (e.g., "myapp:v1" or "ghcr.io/org/app:tag")
        #[arg(short, long)]
        name: String,

        /// Output directory for OCI artifact (default: current directory)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Push WASM artifact to registry
    ///
    /// Pushes a WASM binary to an OCI registry as a WASM artifact.
    ///
    /// Examples:
    ///   zlayer wasm push ./app.wasm ghcr.io/myorg/myapp:v1
    ///   zlayer wasm push ./app.wasm --username user --password pass registry.example.com/app:v1
    #[command(verbatim_doc_comment)]
    Push {
        /// Path to the WASM file
        wasm_file: PathBuf,

        /// Registry reference (e.g., ghcr.io/org/name:tag)
        reference: String,

        /// Registry username
        #[arg(short, long)]
        username: Option<String>,

        /// Registry password
        #[arg(short, long)]
        password: Option<String>,
    },

    /// Validate a WASM binary
    ///
    /// Checks that a file is a valid WebAssembly binary by verifying
    /// its magic bytes and structure.
    ///
    /// Examples:
    ///   zlayer wasm validate ./app.wasm
    #[command(verbatim_doc_comment)]
    Validate {
        /// Path to the WASM file
        wasm_file: PathBuf,
    },

    /// Show information about a WASM binary
    ///
    /// Displays detailed information about a WASM binary including
    /// WASI version, size, and whether it's a component or core module.
    ///
    /// Examples:
    ///   zlayer wasm info ./app.wasm
    #[command(verbatim_doc_comment)]
    Info {
        /// Path to the WASM file
        wasm_file: PathBuf,
    },
}

/// Node management subcommands
#[derive(Subcommand)]
pub(crate) enum NodeCommands {
    /// Initialize this node as cluster leader
    ///
    /// This starts the control plane, overlay network, and API server.
    /// Other nodes can join using the token printed on success.
    ///
    /// Examples:
    ///   zlayer node init --advertise-addr 10.0.0.1
    ///   zlayer node init --advertise-addr $(curl -s ifconfig.me) --api-port 9090
    #[command(verbatim_doc_comment)]
    Init {
        /// Public IP address for other nodes to connect to
        #[arg(long, required = true)]
        advertise_addr: String,

        /// API server port
        #[arg(long, default_value = "3669")]
        api_port: u16,

        /// Raft consensus port
        #[arg(long, default_value = "9000")]
        raft_port: u16,

        /// Overlay network port (`WireGuard` protocol)
        #[arg(long, default_value_t = zlayer_core::DEFAULT_WG_PORT)]
        overlay_port: u16,

        /// Data directory (defaults to platform default, see --data-dir)
        #[arg(long)]
        data_dir: Option<PathBuf>,

        /// Overlay network CIDR
        #[arg(long, default_value = "10.200.0.0/16")]
        overlay_cidr: String,

        /// WSL2 auto-install consent (Windows only).
        #[command(flatten)]
        install_wsl: InstallWslArgs,
    },

    /// Join an existing cluster as a worker node
    ///
    /// Examples:
    ///   zlayer node join 10.0.0.1:3669 --token <TOKEN> --advertise-addr 10.0.0.2
    #[command(verbatim_doc_comment)]
    Join {
        /// Leader address (host:port)
        leader_addr: String,

        /// Join token from leader
        #[arg(long, required = true)]
        token: String,

        /// This node's public IP address
        #[arg(long, required = true)]
        advertise_addr: String,

        /// Node mode: full (all resources) or replicate (specific services)
        #[arg(long, default_value = "full")]
        mode: String,

        /// Services to replicate (only with --mode replicate)
        #[arg(long)]
        services: Option<Vec<String>>,

        /// API server port
        #[arg(long, default_value = "3669")]
        api_port: u16,

        /// Raft consensus port
        #[arg(long, default_value = "9000")]
        raft_port: u16,

        /// Overlay network port (`WireGuard` protocol)
        #[arg(long, default_value_t = zlayer_core::DEFAULT_WG_PORT)]
        overlay_port: u16,

        /// WSL2 auto-install consent (Windows only).
        #[command(flatten)]
        install_wsl: InstallWslArgs,
    },

    /// [deprecated: use `zlayer cluster list` — `zlayer node list` will be removed in v0.13.0]
    ///
    /// List all nodes in the cluster
    #[command(verbatim_doc_comment)]
    List {
        /// Output format (table or json)
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// [deprecated: use `zlayer cluster status` — `zlayer node status` will be removed in v0.13.0]
    ///
    /// Show detailed status of a node
    #[command(verbatim_doc_comment)]
    Status {
        /// Node ID (default: this node)
        node_id: Option<String>,
    },

    /// [deprecated: use `zlayer cluster remove` — `zlayer node remove` will be removed in v0.13.0]
    ///
    /// Remove a node from the cluster
    #[command(verbatim_doc_comment)]
    Remove {
        /// Node ID to remove
        node_id: String,

        /// Force removal without migrating services
        #[arg(long)]
        force: bool,
    },

    /// [deprecated: use `zlayer cluster set-mode` — `zlayer node set-mode` will be removed in v0.13.0]
    ///
    /// Set node resource mode
    #[command(verbatim_doc_comment)]
    SetMode {
        /// Node ID
        node_id: String,

        /// Mode: full, dedicated, or replicate
        #[arg(long)]
        mode: String,

        /// Services for dedicated/replicate mode
        #[arg(long)]
        services: Option<Vec<String>>,
    },

    /// [deprecated: use `zlayer cluster label` — `zlayer node label` will be removed in v0.13.0]
    ///
    /// Add label to a node
    #[command(verbatim_doc_comment)]
    Label {
        /// Node ID
        node_id: String,

        /// Label in key=value format
        label: String,
    },

    /// Generate a join token for worker nodes
    ///
    /// Creates a base64-encoded token that workers can use to join this cluster.
    /// If no API endpoint is provided, reads it from the node config.
    /// If the node hasn't been initialized, auto-initializes with defaults.
    /// If no deployment name is provided, tries to discover it from .zlayer.yml.
    ///
    /// Examples:
    ///   zlayer node generate-join-token
    ///   zlayer node generate-join-token my-deploy
    ///   zlayer node generate-join-token my-deploy -a <http://10.0.0.1:3669>
    #[command(verbatim_doc_comment)]
    GenerateJoinToken {
        /// Deployment name/key (auto-discovered from .zlayer.yml if not given)
        deployment: Option<String>,

        /// API endpoint URL (inferred from node config if not given)
        #[arg(short, long)]
        api: Option<String>,

        /// Service name (optional)
        #[arg(short, long)]
        service: Option<String>,

        /// Data directory (where `node_config.json` lives, defaults to platform default)
        #[arg(long)]
        data_dir: Option<PathBuf>,

        /// Token expiration as a duration from now. Defaults to 24h.
        ///
        /// Accepts humantime syntax: `1h`, `30m`, `24h`, `7d`, `1week`.
        /// The signed token's `exp` claim is set to `now() + ttl`. The HS256
        /// JWT's `exp` claim is set identically. (The legacy plaintext format
        /// was removed in v0.13.0.)
        #[arg(long, value_name = "DURATION", default_value = "24h")]
        ttl: humantime::Duration,
    },

    /// [deprecated: use `zlayer cluster upgrade` — `zlayer node upgrade` will be removed in v0.13.0]
    ///
    /// Roll a new zlayer version across every node in the cluster.
    ///
    /// Followers self-update one at a time (deterministic, ascending
    /// `node_id` order). After the follower walk, the leader self-upgrades
    /// last: its daemon exits with code 75 and the OS supervisor
    /// (launchd / systemd) respawns it on the new binary. Pass
    /// `--skip-leader` to keep the legacy behaviour of leaving the
    /// leader untouched.
    #[command(name = "upgrade", verbatim_doc_comment)]
    Upgrade {
        /// Target version (e.g. v0.12.0). Defaults to the latest GitHub release.
        #[arg(long)]
        version: Option<String>,
        /// Pause between followers (after each comes back healthy) to let
        /// observability soak. Default 30s.
        #[arg(long, default_value = "30")]
        cooldown_secs: u64,
        /// Abort the rollout if any follower fails to come back healthy.
        #[arg(long)]
        strict: bool,
        /// Skip the confirmation prompt and apply immediately.
        #[arg(short = 'y', long)]
        yes: bool,
        /// Don't auto-upgrade the leader. Use when the operator wants to
        /// upgrade the leader manually (e.g. to time a deliberate
        /// failover window).
        #[arg(long)]
        skip_leader: bool,
    },

    /// [deprecated: use `zlayer cluster force-leader` — `zlayer node force-leader` will be removed in v0.13.0]
    ///
    /// Force this node to become cluster leader (disaster recovery)
    ///
    /// Use when the original leader is permanently lost in a 2-node cluster.
    /// The surviving learner node will take over with preserved state.
    /// Requires daemon restart after execution.
    ///
    /// Examples:
    ///   zlayer node force-leader
    ///   zlayer node force-leader --api-addr 127.0.0.1:3669
    #[command(verbatim_doc_comment)]
    ForceLeader {
        /// API address of this node (host:port)
        #[arg(long, default_value = "127.0.0.1:3669")]
        api_addr: String,
    },

    /// [deprecated: use `zlayer cluster rotate-signing-key` — `zlayer node rotate-signing-key` will be removed in v0.13.0]
    ///
    /// Rotate the cluster signing keypair (alias for `zlayer cluster rotate-signing-key`).
    ///
    /// If this node is a worker, the daemon forwards the request to the leader.
    /// If this node is a leader, rotates locally and writes the new keystore.
    ///
    /// The previous active key is moved into a grace window (default 7d,
    /// configurable via --grace) where it continues to verify in-flight
    /// join tokens.
    #[command(name = "rotate-signing-key", verbatim_doc_comment)]
    RotateSigningKey {
        /// Grace duration before the previous active key is purged.
        /// Humantime syntax (`24h`, `7d`).
        #[arg(long, value_name = "DURATION")]
        grace: Option<humantime::Duration>,
    },
}

/// Cluster-level administration subcommands.
///
/// In v0.12.0 these were promoted out of `zlayer node <…>` into a dedicated
/// `zlayer cluster <…>` namespace. Every variant here corresponds 1:1 to a
/// `NodeCommands` variant of the same name and delegates to the same
/// underlying handler (see `commands/cluster.rs`). `RotateSigningKey` is the
/// lone exception: it is brand new with the Ed25519 join-token work and has
/// no `NodeCommands` equivalent. Wave 5B.3 wires its dedicated handler.
#[derive(Subcommand)]
pub(crate) enum ClusterCommands {
    /// List all nodes in the cluster
    List {
        /// Output format (table or json)
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Show detailed status of a node
    Status {
        /// Node ID (default: this node)
        node_id: Option<String>,
    },

    /// Remove a node from the cluster
    Remove {
        /// Node ID to remove
        node_id: String,

        /// Force removal without migrating services
        #[arg(long)]
        force: bool,
    },

    /// Set node resource mode
    SetMode {
        /// Node ID
        node_id: String,

        /// Mode: full, dedicated, or replicate
        #[arg(long)]
        mode: String,

        /// Services for dedicated/replicate mode
        #[arg(long)]
        services: Option<Vec<String>>,
    },

    /// Add label to a node
    Label {
        /// Node ID
        node_id: String,

        /// Label in key=value format
        label: String,
    },

    /// Force a specific node to become cluster leader (disaster recovery)
    #[command(verbatim_doc_comment)]
    ForceLeader {
        /// API address of this node (host:port)
        #[arg(long, default_value = "127.0.0.1:3669")]
        api_addr: String,
    },

    /// Roll a new zlayer version across every node in the cluster.
    #[command(name = "upgrade")]
    Upgrade {
        /// Target version (e.g. v0.12.0). Defaults to the latest GitHub release.
        #[arg(long)]
        version: Option<String>,
        /// Pause between followers (after each comes back healthy) to let
        /// observability soak. Default 30s.
        #[arg(long, default_value = "30")]
        cooldown_secs: u64,
        /// Abort the rollout if any follower fails to come back healthy.
        #[arg(long)]
        strict: bool,
        /// Skip the confirmation prompt and apply immediately.
        #[arg(short = 'y', long)]
        yes: bool,
        /// Don't auto-upgrade the leader.
        #[arg(long)]
        skip_leader: bool,
    },

    /// Rotate the cluster's Ed25519 signing keypair.
    ///
    /// Generates a fresh keypair, promotes it to the active slot, and demotes
    /// the previous key into a verification-only grace window. Workers can
    /// still validate tokens minted with the old key during the grace period
    /// — useful for staggered rollouts. Wave 5B.3 wires the dedicated
    /// handler; this scaffolding only reserves the CLI surface.
    #[command(name = "rotate-signing-key", verbatim_doc_comment)]
    RotateSigningKey {
        /// How long the previous active key continues verifying tokens.
        /// Humantime syntax (`24h`, `7d`). Defaults to 7d.
        #[arg(long, value_name = "DURATION")]
        grace: Option<humantime::Duration>,
    },

    /// Revoke a previously-issued cluster join token.
    ///
    /// Adds the token to the cluster-wide Raft-replicated revocation
    /// list so every node rejects subsequent uses. Accepts either the
    /// raw token b64 envelope OR a pre-computed 64-char lowercase hex
    /// SHA-256 of one. The actual token bytes never enter replicated
    /// state — only the hash leaves the local node.
    ///
    /// Entries auto-prune at the token's natural expiry (or `now()+24h`
    /// when only a hash was supplied) so the list stays bounded.
    #[command(name = "revoke-token", verbatim_doc_comment)]
    RevokeToken {
        /// Raw token (b64 envelope from `zlayer node generate-join-token`)
        /// OR lowercase hex SHA-256 of one.
        token_or_hash: String,
        /// Optional human-readable reason recorded server-side for audit.
        #[arg(long)]
        reason: Option<String>,
    },

    /// List currently-active token revocations.
    ///
    /// Returns the un-expired revocations replicated through Raft on
    /// this node. Entries auto-prune at apply time so the listing only
    /// shows revocations that are still load-bearing for validation.
    #[command(name = "list-revocations", verbatim_doc_comment)]
    ListRevocations {},

    /// Manage cluster trust bundles for federation.
    ///
    /// Subcommands let operators export this cluster's own trust
    /// bundle (for sharing out-of-band with peer clusters) and
    /// manage imports of foreign-cluster bundles so this cluster
    /// validates their signed join tokens.
    #[command(subcommand, name = "trust-bundle")]
    TrustBundle(TrustBundleCommands),

    /// Begin the HS256 -> Ed25519-JWT migration.
    ///
    /// Sets the cluster JWT algorithm policy to `both` so new tokens
    /// minted as `EdDSA` work while in-flight HS256 tokens stay valid.
    /// Operators run this BEFORE re-issuing tokens.
    #[command(name = "migrate-jwt-to-eddsa", verbatim_doc_comment)]
    MigrateJwtToEddsa {
        /// Reserved for future use. The `--grace` knob will become
        /// meaningful when the migration command also issues a fresh
        /// signing-key rotation; for now it's accepted and recorded
        /// for symmetry with the eventual flow.
        #[arg(long, value_name = "DURATION")]
        grace: Option<humantime::Duration>,
    },

    /// Complete the HS256 -> Ed25519-JWT migration.
    ///
    /// Flips the cluster JWT algorithm policy to `eddsa`. HS256-JWT
    /// tokens are rejected after this completes. With
    /// `--vacuum-secret`, also wipes `{data_dir}/join_secret` on every
    /// node via a separate Raft op so the symmetric secret is gone.
    #[command(name = "decommission-hs256", verbatim_doc_comment)]
    DecommissionHs256 {
        /// If set, also schedule a cluster-wide wipe of
        /// `{data_dir}/join_secret`.
        #[arg(long)]
        vacuum_secret: bool,
    },

    /// Show the cluster JWT algorithm status.
    #[command(name = "jwt-status", verbatim_doc_comment)]
    JwtStatus {},
}

/// Subcommands for `zlayer cluster trust-bundle <...>`.
#[derive(clap::Subcommand)]
pub(crate) enum TrustBundleCommands {
    /// Print this cluster's trust bundle as JSON for out-of-band
    /// transport to a peer cluster.
    Export {
        /// Optional file path to write the bundle to. If omitted,
        /// the JSON is printed to stdout.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    /// Import a foreign cluster's trust bundle from a file or URL.
    Import {
        /// Path to a JSON file containing a `TrustBundle`, OR an
        /// `https://` URL to fetch (over TLS).
        source: String,
        /// Optional source-URL annotation recorded server-side for
        /// audit; defaults to the supplied URL when `source` is a URL.
        #[arg(long)]
        source_url: Option<String>,
    },
    /// List all currently-trusted foreign-cluster bundles.
    List {},
    /// Remove a previously-imported trust bundle.
    Remove {
        /// The `cluster_domain` of the bundle to forget.
        cluster_domain: String,
    },
}

/// Token management subcommands
#[derive(Subcommand)]
pub(crate) enum TokenCommands {
    /// Create a new JWT token
    Create {
        /// Subject (user ID or API key name)
        #[arg(short, long, default_value = "dev")]
        subject: String,

        /// JWT secret (or use `ZLAYER_JWT_SECRET` env var)
        #[arg(long)]
        secret: Option<String>,

        /// Token validity in hours
        #[arg(short = 'H', long, default_value = "24")]
        hours: u64,

        /// Roles to grant (comma-separated)
        #[arg(short, long, default_value = "admin")]
        roles: String,

        /// Output only the token (for scripting)
        #[arg(long)]
        quiet: bool,
    },

    /// Decode and display a JWT token
    Decode {
        /// The JWT token to decode
        token: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// List token capabilities and available roles
    Info,

    /// Show admin API credentials
    Show,
}

/// Authentication subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum AuthCommands {
    /// Create the first admin user (first-run setup).
    ///
    /// Fails with 409 if any user already exists on the daemon.
    ///
    /// Examples:
    ///   zlayer auth bootstrap --email admin@local --password hunter2
    ///   zlayer auth bootstrap --email admin@local      # password prompted
    #[command(verbatim_doc_comment)]
    Bootstrap {
        /// Email address of the admin user.
        #[arg(long)]
        email: String,

        /// Password (prompted interactively if omitted).
        #[arg(long)]
        password: Option<String>,

        /// Optional display name (defaults to email).
        #[arg(long)]
        display_name: Option<String>,
    },

    /// Log in with email + password.
    ///
    /// Obtains a JWT from `POST /auth/token` and persists it to
    /// `~/.zlayer/session.json`. Subsequent commands run as this user.
    ///
    /// Examples:
    ///   zlayer auth login --email admin@local
    ///   zlayer auth login --email admin@local --password hunter2
    #[command(verbatim_doc_comment)]
    Login {
        /// Email address.
        #[arg(long)]
        email: String,

        /// Password (prompted interactively if omitted).
        #[arg(long)]
        password: Option<String>,
    },

    /// Log out -- delete the local session file and notify the server.
    Logout,

    /// Print the currently signed-in user.
    Whoami,
}

/// CLI-friendly user role. Converts into `zlayer_types::storage::UserRole`.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum CliUserRole {
    Admin,
    User,
}

impl From<CliUserRole> for zlayer_types::storage::UserRole {
    fn from(r: CliUserRole) -> Self {
        match r {
            CliUserRole::Admin => zlayer_types::storage::UserRole::Admin,
            CliUserRole::User => zlayer_types::storage::UserRole::User,
        }
    }
}

/// User management subcommands (admin-only).
#[derive(Subcommand, Debug)]
pub(crate) enum UserCommands {
    /// List all users.
    #[command(visible_alias = "list")]
    Ls {
        /// Output format (table or json).
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Create a new user.
    Create {
        /// Email address.
        #[arg(long)]
        email: String,

        /// Password (prompted if omitted).
        #[arg(long)]
        password: Option<String>,

        /// Role.
        #[arg(long, value_enum, default_value_t = CliUserRole::User)]
        role: CliUserRole,

        /// Display name (defaults to email).
        #[arg(long)]
        display_name: Option<String>,
    },

    /// Change a user's role.
    SetRole {
        /// User id.
        id: String,

        /// New role.
        #[arg(long, value_enum)]
        role: CliUserRole,
    },

    /// Set a user's password. Interactive by default; supply
    /// `--password`/`--password-file`/`--random` for non-interactive use.
    SetPassword {
        /// User id. Mutually exclusive with `--email`.
        id: Option<String>,

        /// User email (resolved to id via `list_users`). Mutually
        /// exclusive with the positional id.
        #[arg(long)]
        email: Option<String>,

        /// New password (inline). Mutually exclusive with
        /// `--password-file` and `--random`.
        #[arg(long, conflicts_with_all = ["password_file", "random"])]
        password: Option<String>,

        /// Path to a file containing the new password. Trailing
        /// newline/CR is trimmed. Mutually exclusive with `--password`
        /// and `--random`.
        #[arg(long, value_name = "PATH", conflicts_with_all = ["password", "random"])]
        password_file: Option<PathBuf>,

        /// Generate a random 32-char password and print it once on
        /// stdout. Mutually exclusive with `--password` and
        /// `--password-file`.
        #[arg(long, conflicts_with_all = ["password", "password_file"])]
        random: bool,

        /// Skip the "are you sure" confirmation prompt.
        #[arg(long)]
        no_confirm: bool,
    },

    /// Delete a user.
    Delete {
        /// User id.
        id: String,

        /// Skip the confirmation prompt.
        #[arg(long, short)]
        yes: bool,
    },
}

/// Group management subcommands.
#[derive(Subcommand, Debug)]
pub(crate) enum GroupCommands {
    /// List all groups.
    #[command(visible_alias = "ls")]
    List {
        /// Output format (table or json).
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Create a new group.
    Create {
        /// Group name.
        #[arg(long)]
        name: String,
    },

    /// Delete a group.
    Delete {
        /// Group id.
        id: String,

        /// Skip the confirmation prompt.
        #[arg(long, short)]
        yes: bool,
    },

    /// Member management.
    #[command(subcommand)]
    Member(GroupMemberCommands),
}

/// Group member management subcommands.
#[derive(Subcommand, Debug)]
pub(crate) enum GroupMemberCommands {
    /// Add a user to a group.
    Add {
        /// Group id.
        #[arg(long)]
        group: String,
        /// User id.
        #[arg(long)]
        user: String,
    },

    /// Remove a user from a group.
    Remove {
        /// Group id.
        #[arg(long)]
        group: String,
        /// User id.
        #[arg(long)]
        user: String,
    },
}

/// Permission management subcommands.
#[derive(Subcommand, Debug)]
pub(crate) enum PermissionCommands {
    /// List permissions for a user or group.
    #[command(visible_alias = "ls")]
    List {
        /// Filter by user id.
        #[arg(long)]
        user: Option<String>,
        /// Filter by group id.
        #[arg(long)]
        group: Option<String>,
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "table")]
        output: String,
    },

    /// Grant a permission.
    Grant {
        /// Subject kind: `user` or `group`.
        #[arg(long, value_enum)]
        subject_kind: CliSubjectKind,
        /// Subject id.
        #[arg(long)]
        subject: String,
        /// Resource kind (e.g. `deployment`, `project`, `secret`).
        #[arg(long)]
        resource_kind: String,
        /// Specific resource id (omit for wildcard).
        #[arg(long)]
        resource: Option<String>,
        /// Access level.
        #[arg(long, value_enum)]
        level: CliPermissionLevel,
    },

    /// Revoke a permission by id.
    Revoke {
        /// Permission id.
        id: String,
    },
}

/// CLI enum for subject kind.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum CliSubjectKind {
    User,
    Group,
}

impl From<CliSubjectKind> for zlayer_types::storage::SubjectKind {
    fn from(k: CliSubjectKind) -> Self {
        match k {
            CliSubjectKind::User => zlayer_types::storage::SubjectKind::User,
            CliSubjectKind::Group => zlayer_types::storage::SubjectKind::Group,
        }
    }
}

/// CLI enum for permission level.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum CliPermissionLevel {
    Read,
    Write,
    Execute,
}

impl From<CliPermissionLevel> for zlayer_types::storage::PermissionLevel {
    fn from(l: CliPermissionLevel) -> Self {
        match l {
            CliPermissionLevel::Read => zlayer_types::storage::PermissionLevel::Read,
            CliPermissionLevel::Write => zlayer_types::storage::PermissionLevel::Write,
            CliPermissionLevel::Execute => zlayer_types::storage::PermissionLevel::Execute,
        }
    }
}

/// Audit log subcommands.
#[derive(Subcommand, Debug)]
pub(crate) enum AuditCommands {
    /// Show recent audit log entries.
    Tail {
        /// Filter by user id.
        #[arg(long)]
        user: Option<String>,
        /// Filter by resource kind.
        #[arg(long)]
        resource: Option<String>,
        /// Maximum number of entries to return.
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output format (`table` or `json`).
        #[arg(long, default_value = "table")]
        output: String,
    },
}

/// Spec inspection subcommands
#[derive(Subcommand)]
pub(crate) enum SpecCommands {
    /// Dump the parsed specification
    Dump {
        /// Path to spec file
        spec: PathBuf,

        /// Output format (json, yaml)
        #[arg(short, long, default_value = "yaml")]
        format: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deploy mode controls whether the runtime stays in foreground or returns after deploying
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum DeployMode {
        /// Stay running in foreground, wait for Ctrl+C
        Foreground,
        /// Deploy and return immediately
        Background,
        /// Wait for services to stabilize, then automatically exit
        Detach,
    }

    #[test]
    fn test_cli_parsing() {
        // Test default runtime
        let cli = Cli::try_parse_from(["zlayer", "status"]).unwrap();
        assert!(matches!(cli.runtime, RuntimeType::Auto));
        assert!(matches!(cli.command, Some(Commands::Status)));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_cli_youki_runtime() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "--runtime",
            "youki",
            "--state-dir",
            "/custom/state",
            "status",
        ])
        .unwrap();

        assert!(matches!(cli.runtime, RuntimeType::Youki));
        assert_eq!(cli.state_dir, Some(PathBuf::from("/custom/state")));
    }

    #[test]
    fn test_cli_deploy_command() {
        let cli = Cli::try_parse_from(["zlayer", "deploy", "test-spec.yaml"]).unwrap();

        match cli.command {
            Some(Commands::Deploy { spec_path, dry_run }) => {
                assert_eq!(spec_path, Some(PathBuf::from("test-spec.yaml")));
                assert!(!dry_run);
            }
            _ => panic!("Expected Deploy command"),
        }
    }

    #[test]
    fn test_cli_deploy_command_no_spec() {
        let cli = Cli::try_parse_from(["zlayer", "deploy"]).unwrap();

        match cli.command {
            Some(Commands::Deploy { spec_path, dry_run }) => {
                assert!(spec_path.is_none());
                assert!(!dry_run);
            }
            _ => panic!("Expected Deploy command"),
        }
    }

    #[test]
    fn test_cli_deploy_dry_run() {
        let cli = Cli::try_parse_from(["zlayer", "deploy", "--dry-run", "test-spec.yaml"]).unwrap();

        match cli.command {
            Some(Commands::Deploy { dry_run, .. }) => {
                assert!(dry_run);
            }
            _ => panic!("Expected Deploy command"),
        }
    }

    #[test]
    fn test_cli_join_command() {
        let cli = Cli::try_parse_from(["zlayer", "join", "some-token"]).unwrap();

        match cli.command {
            Some(Commands::Join {
                token,
                spec_dir,
                service,
                replicas,
                ..
            }) => {
                assert_eq!(token, "some-token");
                assert!(spec_dir.is_none());
                assert!(service.is_none());
                assert_eq!(replicas, 1); // default
            }
            _ => panic!("Expected Join command"),
        }
    }

    #[test]
    fn test_cli_join_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "join",
            "--spec-dir",
            "/custom/specs",
            "--service",
            "web",
            "--replicas",
            "3",
            "my-join-token",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Join {
                token,
                spec_dir,
                service,
                replicas,
                ..
            }) => {
                assert_eq!(token, "my-join-token");
                assert_eq!(spec_dir, Some("/custom/specs".to_string()));
                assert_eq!(service, Some("web".to_string()));
                assert_eq!(replicas, 3);
            }
            _ => panic!("Expected Join command"),
        }
    }

    #[test]
    fn test_cli_join_command_short_flags() {
        let cli =
            Cli::try_parse_from(["zlayer", "join", "-s", "api", "-r", "5", "token123"]).unwrap();

        match cli.command {
            Some(Commands::Join {
                token,
                service,
                replicas,
                ..
            }) => {
                assert_eq!(token, "token123");
                assert_eq!(service, Some("api".to_string()));
                assert_eq!(replicas, 5);
            }
            _ => panic!("Expected Join command"),
        }
    }

    #[test]
    fn test_parse_join_token() {
        use base64::Engine;

        let info = serde_json::json!({
            "api_endpoint": "http://localhost:3669",
            "deployment": "my-app",
            "key": "secret-auth-key",
            "service": "api"
        });

        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&info).unwrap());

        let parsed = crate::commands::join::parse_join_token(&token).unwrap();
        assert_eq!(parsed.api_endpoint, "http://localhost:3669");
        assert_eq!(parsed.deployment, "my-app");
        assert_eq!(parsed.key, "secret-auth-key");
        assert_eq!(parsed.service, Some("api".to_string()));
    }

    #[test]
    fn test_parse_join_token_minimal() {
        use base64::Engine;

        let info = serde_json::json!({
            "api_endpoint": "http://api.example.com",
            "deployment": "my-deploy",
            "key": "auth-key"
        });

        let token =
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_string(&info).unwrap());

        let parsed = crate::commands::join::parse_join_token(&token).unwrap();
        assert_eq!(parsed.api_endpoint, "http://api.example.com");
        assert_eq!(parsed.deployment, "my-deploy");
        assert_eq!(parsed.key, "auth-key");
        assert!(parsed.service.is_none());
    }

    #[test]
    fn test_parse_join_token_invalid_base64() {
        let result = crate::commands::join::parse_join_token("not-valid-base64!!!");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not valid base64"));
    }

    #[test]
    fn test_parse_join_token_invalid_json() {
        use base64::Engine;

        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("not json");

        let result = crate::commands::join::parse_join_token(&token);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not valid JSON"));
    }

    #[test]
    fn test_cli_validate_command() {
        let cli = Cli::try_parse_from(["zlayer", "validate", "test-spec.yaml"]).unwrap();

        match cli.command {
            Some(Commands::Validate { spec_path }) => {
                assert_eq!(spec_path, Some(PathBuf::from("test-spec.yaml")));
            }
            _ => panic!("Expected Validate command"),
        }
    }

    #[test]
    fn test_cli_validate_command_no_spec() {
        let cli = Cli::try_parse_from(["zlayer", "validate"]).unwrap();

        match cli.command {
            Some(Commands::Validate { spec_path }) => {
                assert!(spec_path.is_none());
            }
            _ => panic!("Expected Validate command"),
        }
    }

    #[test]
    fn test_cli_verbose_levels() {
        let cli = Cli::try_parse_from(["zlayer", "status"]).unwrap();
        assert_eq!(cli.verbose, 0);

        let cli = Cli::try_parse_from(["zlayer", "-v", "status"]).unwrap();
        assert_eq!(cli.verbose, 1);

        let cli = Cli::try_parse_from(["zlayer", "-vv", "status"]).unwrap();
        assert_eq!(cli.verbose, 2);
    }

    #[test]
    fn test_build_runtime_config_auto() {
        let cli = Cli::try_parse_from(["zlayer", "status"]).unwrap();
        let config = crate::config::build_runtime_config(&cli);
        assert!(matches!(config, zlayer_agent::RuntimeConfig::Auto));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_build_runtime_config_youki() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "--runtime",
            "youki",
            "--state-dir",
            "/custom/state",
            "status",
        ])
        .unwrap();

        let config = crate::config::build_runtime_config(&cli);
        match config {
            zlayer_agent::RuntimeConfig::Youki(c) => {
                assert_eq!(c.state_dir, PathBuf::from("/custom/state"));
            }
            _ => panic!("Expected Youki config"),
        }
    }

    #[test]
    fn test_cli_serve_command_defaults() {
        let cli = Cli::try_parse_from(["zlayer", "serve"]).unwrap();

        match cli.command {
            Some(Commands::Serve {
                bind,
                jwt_secret,
                no_swagger,
                daemon,
                socket,
                ..
            }) => {
                assert_eq!(bind, "0.0.0.0:3669");
                assert!(jwt_secret.is_none());
                assert!(!no_swagger);
                assert!(!daemon);
                assert!(socket.is_none());
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_custom_bind() {
        let cli = Cli::try_parse_from(["zlayer", "serve", "--bind", "127.0.0.1:9090"]).unwrap();

        match cli.command {
            Some(Commands::Serve { bind, .. }) => {
                assert_eq!(bind, "127.0.0.1:9090");
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_jwt_secret() {
        let cli = Cli::try_parse_from(["zlayer", "serve", "--jwt-secret", "my-super-secret-key"])
            .unwrap();

        match cli.command {
            Some(Commands::Serve { jwt_secret, .. }) => {
                assert_eq!(jwt_secret, Some("my-super-secret-key".to_string()));
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_no_swagger() {
        let cli = Cli::try_parse_from(["zlayer", "serve", "--no-swagger"]).unwrap();

        match cli.command {
            Some(Commands::Serve { no_swagger, .. }) => {
                assert!(no_swagger);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "serve",
            "--bind",
            "0.0.0.0:3000",
            "--jwt-secret",
            "test-secret",
            "--no-swagger",
            "--daemon",
            "--socket",
            "/var/lib/zlayer-test.sock",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Serve {
                bind,
                jwt_secret,
                no_swagger,
                daemon,
                socket,
                ..
            }) => {
                assert_eq!(bind, "0.0.0.0:3000");
                assert_eq!(jwt_secret, Some("test-secret".to_string()));
                assert!(no_swagger);
                assert!(daemon);
                assert_eq!(socket, Some("/var/lib/zlayer-test.sock".to_string()));
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_daemon_flag() {
        let cli = Cli::try_parse_from(["zlayer", "serve", "--daemon"]).unwrap();

        match cli.command {
            Some(Commands::Serve { daemon, .. }) => {
                assert!(daemon);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_socket_flag() {
        let cli = Cli::try_parse_from(["zlayer", "serve", "--socket", "/tmp/custom.sock"]).unwrap();

        match cli.command {
            Some(Commands::Serve { socket, .. }) => {
                assert_eq!(socket, Some("/tmp/custom.sock".to_string()));
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_deployment_name_default() {
        let cli = Cli::try_parse_from(["zlayer", "serve"]).unwrap();
        match cli.command {
            Some(Commands::Serve {
                deployment_name, ..
            }) => {
                assert_eq!(deployment_name, "zlayer");
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_deployment_name_flag() {
        let cli = Cli::try_parse_from(["zlayer", "serve", "--deployment-name", "foo"]).unwrap();
        match cli.command {
            Some(Commands::Serve {
                deployment_name, ..
            }) => {
                assert_eq!(deployment_name, "foo");
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_wg_port_default() {
        let cli = Cli::try_parse_from(["zlayer", "serve"]).unwrap();
        match cli.command {
            Some(Commands::Serve { wg_port, .. }) => {
                assert_eq!(wg_port, None);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_wg_port_flag() {
        let cli = Cli::try_parse_from(["zlayer", "serve", "--wg-port", "51421"]).unwrap();
        match cli.command {
            Some(Commands::Serve { wg_port, .. }) => {
                assert_eq!(wg_port, Some(51421));
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_dns_port_default() {
        let cli = Cli::try_parse_from(["zlayer", "serve"]).unwrap();
        match cli.command {
            Some(Commands::Serve { dns_port, .. }) => {
                assert_eq!(dns_port, None);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_dns_port_flag() {
        let cli = Cli::try_parse_from(["zlayer", "serve", "--dns-port", "15354"]).unwrap();
        match cli.command {
            Some(Commands::Serve { dns_port, .. }) => {
                assert_eq!(dns_port, Some(15354));
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_accepts_api_tls_cert_and_key() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "serve",
            "--api-tls-cert",
            "/tmp/c.pem",
            "--api-tls-key",
            "/tmp/k.pem",
        ])
        .unwrap();
        match cli.command {
            Some(Commands::Serve {
                api_tls_cert,
                api_tls_key,
                api_tls_acme,
                ..
            }) => {
                assert_eq!(api_tls_cert, Some(std::path::PathBuf::from("/tmp/c.pem")));
                assert_eq!(api_tls_key, Some(std::path::PathBuf::from("/tmp/k.pem")));
                assert!(!api_tls_acme);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_rejects_api_tls_cert_without_key() {
        assert!(Cli::try_parse_from(["zlayer", "serve", "--api-tls-cert", "/tmp/c.pem"]).is_err());
    }

    #[test]
    fn test_cli_serve_rejects_api_tls_cert_with_acme() {
        assert!(Cli::try_parse_from([
            "zlayer",
            "serve",
            "--api-tls-cert",
            "/tmp/c.pem",
            "--api-tls-key",
            "/tmp/k.pem",
            "--api-tls-acme",
        ])
        .is_err());
    }

    #[test]
    fn vacuum_secrets_flag_parses() {
        // Wave 6 (v0.13.0): `--vacuum-secrets` opts a daemon into wiping
        // its on-disk HS256 join_secret on startup. Verify clap accepts
        // the flag and surfaces it on `Commands::Serve`.
        let cli = Cli::try_parse_from(["zlayer", "serve", "--vacuum-secrets"]).unwrap();
        match cli.command {
            Some(Commands::Serve { vacuum_secrets, .. }) => {
                assert!(
                    vacuum_secrets,
                    "--vacuum-secrets must parse to true when supplied"
                );
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn vacuum_secrets_defaults_to_false() {
        // Defensive: a fresh `zlayer serve` invocation must not silently
        // vacuum the secret. The flag has to be opted into explicitly.
        let cli = Cli::try_parse_from(["zlayer", "serve"]).unwrap();
        match cli.command {
            Some(Commands::Serve { vacuum_secrets, .. }) => {
                assert!(
                    !vacuum_secrets,
                    "--vacuum-secrets must default to false (preserve HS256 key across restart)"
                );
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_accepts_api_tls_acme_alone() {
        let cli = Cli::try_parse_from(["zlayer", "serve", "--api-tls-acme"]).unwrap();
        match cli.command {
            Some(Commands::Serve {
                api_tls_cert,
                api_tls_key,
                api_tls_acme,
                ..
            }) => {
                assert!(api_tls_cert.is_none());
                assert!(api_tls_key.is_none());
                assert!(api_tls_acme);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_daemon_and_socket() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "serve",
            "--daemon",
            "--socket",
            "/var/run/zlayer-custom.sock",
            "--bind",
            "127.0.0.1:3669",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Serve {
                bind,
                daemon,
                socket,
                ..
            }) => {
                assert_eq!(bind, "127.0.0.1:3669");
                assert!(daemon);
                assert_eq!(socket, Some("/var/run/zlayer-custom.sock".to_string()));
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_logs_command_minimal() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "logs",
            "--deployment",
            "my-deployment",
            "my-service",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Logs {
                deployment,
                service,
                lines,
                follow,
                instance,
            }) => {
                assert_eq!(deployment.as_deref(), Some("my-deployment"));
                assert_eq!(service, "my-service");
                assert_eq!(lines, 100); // default
                assert!(!follow);
                assert!(instance.is_none());
            }
            _ => panic!("Expected Logs command"),
        }
    }

    #[test]
    fn test_cli_no_subcommand_is_none() {
        let cli = Cli::try_parse_from(["zlayer"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_cli_tui_subcommand() {
        let cli = Cli::try_parse_from(["zlayer", "tui"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Tui { context: None })));
    }

    #[test]
    fn test_cli_tui_with_context() {
        let cli = Cli::try_parse_from(["zlayer", "tui", "-c", "/some/path"]).unwrap();
        match cli.command {
            Some(Commands::Tui { context }) => {
                assert_eq!(context, Some(PathBuf::from("/some/path")));
            }
            _ => panic!("Expected Tui command"),
        }
    }

    #[test]
    fn test_cli_pipeline_command() {
        let cli = Cli::try_parse_from(["zlayer", "pipeline"]).unwrap();
        match cli.command {
            Some(Commands::Pipeline {
                file,
                set,
                push,
                fail_fast,
                no_tui,
                only,
                platform,
            }) => {
                assert!(file.is_none());
                assert!(set.is_empty());
                assert!(!push);
                assert!(fail_fast);
                assert!(!no_tui);
                assert!(only.is_none());
                assert!(platform.is_none());
            }
            _ => panic!("Expected Pipeline command"),
        }
    }

    #[test]
    fn test_deploy_mode_variants() {
        assert_ne!(DeployMode::Foreground, DeployMode::Background);
        assert_ne!(DeployMode::Foreground, DeployMode::Detach);
        assert_ne!(DeployMode::Background, DeployMode::Detach);
    }

    #[test]
    fn test_cli_up_command_no_spec() {
        let cli = Cli::try_parse_from(["zlayer", "up"]).unwrap();

        match cli.command {
            Some(Commands::Up { spec_path, .. }) => {
                assert!(spec_path.is_none());
            }
            _ => panic!("Expected Up command"),
        }
        assert!(!cli.background);
    }

    #[test]
    fn test_cli_up_command_background() {
        let cli = Cli::try_parse_from(["zlayer", "up", "-b"]).unwrap();

        assert!(cli.background);
        assert!(matches!(cli.command, Some(Commands::Up { .. })));
    }

    #[test]
    fn test_cli_down_command_no_deployment() {
        let cli = Cli::try_parse_from(["zlayer", "down"]).unwrap();

        match cli.command {
            Some(Commands::Down { deployment }) => {
                assert!(deployment.is_none());
            }
            _ => panic!("Expected Down command"),
        }
    }

    #[test]
    fn test_cli_ps_command_defaults() {
        let cli = Cli::try_parse_from(["zlayer", "ps"]).unwrap();

        match cli.command {
            Some(Commands::Ps {
                deployment,
                containers,
                format,
            }) => {
                assert!(deployment.is_none());
                assert!(!containers);
                assert_eq!(format, "table");
            }
            _ => panic!("Expected Ps command"),
        }
    }

    // -----------------------------------------------------------------------
    // L-2: `zlayer build --platform` flag parses to Option<String> and the
    //      downstream code (bin/zlayer/src/commands/build.rs) converts it to
    //      `ImageOs`. We validate the CLI-side surface here; the string→ImageOs
    //      conversion has its own coverage in `backend::mod::tests`.
    // -----------------------------------------------------------------------

    #[test]
    fn test_cli_build_platform_flag_linux_amd64() {
        let cli =
            Cli::try_parse_from(["zlayer", "build", "--platform", "linux/amd64", "."]).unwrap();
        match cli.command {
            Some(Commands::Build { platform, .. }) => {
                assert_eq!(platform.as_deref(), Some("linux/amd64"));
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_platform_flag_windows() {
        let cli = Cli::try_parse_from(["zlayer", "build", "--platform", "windows", "."]).unwrap();
        match cli.command {
            Some(Commands::Build { platform, .. }) => {
                assert_eq!(platform.as_deref(), Some("windows"));
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_platform_flag_omitted_is_none() {
        let cli = Cli::try_parse_from(["zlayer", "build", "."]).unwrap();
        match cli.command {
            Some(Commands::Build { platform, .. }) => {
                assert!(platform.is_none());
            }
            _ => panic!("Expected Build command"),
        }
    }

    // ── Wave 5B.1+5B.2: `zlayer cluster <…>` scaffolding ────────────────
    //
    // These tests confirm:
    //   1. The new top-level `cluster` subgroup is reachable through clap.
    //   2. Specific subcommands parse into the expected `ClusterCommands`
    //      variants (including the new `RotateSigningKey` with `--grace`).
    //   3. The legacy `zlayer node <…>` invocations STILL parse to
    //      `Commands::Node(NodeCommands::…)` — i.e. we have not regressed
    //      the alias surface that ships through one more release.

    #[test]
    fn cli_recognizes_top_level_cluster_subcommand() {
        // `--help` causes try_parse_from to return Err with DisplayHelp.
        // We're checking the subcommand is REACHABLE in the clap tree,
        // not that help actually renders successfully here.
        let cli = Cli::try_parse_from(["zlayer", "cluster", "--help"]);
        match cli {
            Err(e) if matches!(e.kind(), clap::error::ErrorKind::DisplayHelp) => {}
            Err(e) => panic!("unexpected clap error: {e}"),
            Ok(_) => panic!("expected DisplayHelp"),
        }
    }

    #[test]
    fn cli_cluster_list_parses() {
        let cli = Cli::try_parse_from(["zlayer", "cluster", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Cluster(ClusterCommands::List { .. }))
        ));
    }

    #[test]
    fn cli_cluster_rotate_signing_key_parses_with_grace() {
        let cli =
            Cli::try_parse_from(["zlayer", "cluster", "rotate-signing-key", "--grace", "24h"])
                .unwrap();
        let Some(Commands::Cluster(ClusterCommands::RotateSigningKey { grace })) = cli.command
        else {
            panic!("expected Commands::Cluster(ClusterCommands::RotateSigningKey {{ .. }})")
        };
        assert_eq!(
            *grace.expect("--grace was provided"),
            std::time::Duration::from_secs(86_400)
        );
    }

    #[test]
    fn cli_cluster_rotate_signing_key_parses_without_grace() {
        // No --grace flag → field should be None; the actual default is
        // applied by the Wave 5B.3 handler, not by clap.
        let cli = Cli::try_parse_from(["zlayer", "cluster", "rotate-signing-key"]).unwrap();
        let Some(Commands::Cluster(ClusterCommands::RotateSigningKey { grace })) = cli.command
        else {
            panic!("expected Commands::Cluster(ClusterCommands::RotateSigningKey {{ .. }})")
        };
        assert!(grace.is_none());
    }

    #[test]
    fn cli_node_list_still_works_as_alias() {
        // Regression check: existing `zlayer node list` invocation must
        // still parse to NodeCommands::List unchanged.
        let cli = Cli::try_parse_from(["zlayer", "node", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Node(NodeCommands::List { .. }))
        ));
    }

    // ── Wave 5B.3: `zlayer node rotate-signing-key` alias ───────────────
    //
    // The dedicated `zlayer cluster rotate-signing-key` surface (covered by
    // the tests above) ships alongside an alias on `zlayer node …` so the
    // historic single-namespace muscle memory still works. These tests
    // confirm both invocations parse, with and without `--grace`.

    #[test]
    fn cli_node_rotate_signing_key_parses_with_grace() {
        let cli =
            Cli::try_parse_from(["zlayer", "node", "rotate-signing-key", "--grace", "1h"]).unwrap();
        let Some(Commands::Node(NodeCommands::RotateSigningKey { grace })) = cli.command else {
            panic!("expected Commands::Node(NodeCommands::RotateSigningKey {{ .. }})")
        };
        assert_eq!(
            *grace.expect("--grace was provided"),
            std::time::Duration::from_secs(3600)
        );
    }

    #[test]
    fn cli_node_rotate_signing_key_parses_without_grace() {
        // No --grace flag → field should be None; the actual default is
        // applied server-side by the rotate-signing-key handler, not by
        // clap. Mirrors the cluster-namespace test above.
        let cli = Cli::try_parse_from(["zlayer", "node", "rotate-signing-key"]).unwrap();
        let Some(Commands::Node(NodeCommands::RotateSigningKey { grace })) = cli.command else {
            panic!("expected Commands::Node(NodeCommands::RotateSigningKey {{ .. }})")
        };
        assert!(grace.is_none());
    }

    #[test]
    fn cli_cluster_rotate_signing_key_alias_still_parses() {
        // Sanity check that the original cluster-namespace surface is
        // unchanged by the Wave 5B.3 NodeCommands addition. Tested in the
        // 5B.1+5B.2 block above, repeated here to anchor the alias
        // contract.
        let cli = Cli::try_parse_from(["zlayer", "cluster", "rotate-signing-key", "--grace", "1h"])
            .unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Cluster(ClusterCommands::RotateSigningKey { .. }))
        ));
    }

    // ── Wave 5B.5: deprecation notices on promoted NodeCommands variants ──
    //
    // The eight variants that have been promoted into the `zlayer cluster
    // <…>` namespace (List, Status, Remove, SetMode, Label, ForceLeader,
    // Upgrade, RotateSigningKey) carry a docstring deprecation note that
    // surfaces in `zlayer node <variant> --help`. The three variants that
    // remain node-level (Init, Join, GenerateJoinToken) MUST NOT carry a
    // deprecation note. These tests pin both contracts.

    #[test]
    fn node_list_help_includes_deprecation_note() {
        use clap::CommandFactory;
        let cmd = Cli::command();
        let node_help = cmd
            .find_subcommand("node")
            .expect("`node` subcommand")
            .find_subcommand("list")
            .expect("`node list` subcommand");
        let long_about_or_about = node_help
            .get_long_about()
            .or_else(|| node_help.get_about())
            .expect("help text on `node list`")
            .to_string();
        assert!(
            long_about_or_about.contains("deprecated")
                && long_about_or_about.contains("zlayer cluster list"),
            "expected deprecation note in `zlayer node list --help`, got: {long_about_or_about}"
        );
    }

    #[test]
    fn node_init_help_does_not_mention_deprecation() {
        use clap::CommandFactory;
        let cmd = Cli::command();
        let node_help = cmd
            .find_subcommand("node")
            .expect("`node` subcommand")
            .find_subcommand("init")
            .expect("`node init` subcommand");
        let long_about_or_about = node_help
            .get_long_about()
            .or_else(|| node_help.get_about())
            .map(std::string::ToString::to_string)
            .unwrap_or_default();
        assert!(
            !long_about_or_about.to_lowercase().contains("deprecated"),
            "zlayer node init should NOT carry a deprecation note (it stays node-level), got: {long_about_or_about}"
        );
    }
}
