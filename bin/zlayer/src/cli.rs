use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Return the platform-appropriate default data directory for ZLayer.
///
/// - macOS: `~/.local/share/zlayer` (user-writable without root)
/// - Linux: `/var/lib/zlayer` (traditional, typically runs as root)
pub(crate) fn default_data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home).join(".local/share/zlayer")
        } else {
            PathBuf::from("/var/lib/zlayer")
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PathBuf::from("/var/lib/zlayer")
    }
}

/// Return the platform-appropriate default runtime directory.
///
/// - macOS: `{data_dir}/run` (under the user data dir)
/// - Linux: `/var/run/zlayer`
pub(crate) fn default_run_dir(data_dir: &std::path::Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        data_dir.join("run")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = data_dir;
        PathBuf::from("/var/run/zlayer")
    }
}

/// Return the platform-appropriate default log directory.
///
/// - macOS: `{data_dir}/logs` (under the user data dir)
/// - Linux: `/var/log/zlayer`
pub(crate) fn default_log_dir(data_dir: &std::path::Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        data_dir.join("logs")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = data_dir;
        PathBuf::from("/var/log/zlayer")
    }
}

/// Return the platform-appropriate default socket path.
pub(crate) fn default_socket_path(data_dir: &std::path::Path) -> String {
    #[cfg(target_os = "macos")]
    {
        default_run_dir(data_dir)
            .join("zlayer.sock")
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = data_dir;
        "/var/run/zlayer.sock".to_string()
    }
}

/// ZLayer container orchestration platform
#[derive(Parser)]
#[command(name = "zlayer")]
#[command(version, about = "ZLayer container orchestration platform")]
#[command(propagate_version = true)]
pub(crate) struct Cli {
    /// Container runtime to use
    #[arg(long, default_value = "auto", value_enum)]
    pub(crate) runtime: RuntimeType,

    /// Root data directory for all ZLayer state (databases, secrets, containers).
    ///
    /// On macOS defaults to ~/.local/share/zlayer (no root required).
    /// On Linux defaults to /var/lib/zlayer.
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

/// CLI subcommands
#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Launch interactive TUI
    Tui {
        /// Build context directory
        #[arg(short = 'c', long)]
        context: Option<PathBuf>,
    },

    /// Deploy services from a spec file
    Deploy {
        /// Path to deployment spec (auto-discovers *.zlayer.yml in current directory)
        spec_path: Option<PathBuf>,

        /// Dry run - parse and validate but don't actually deploy
        #[arg(long)]
        dry_run: bool,
    },

    /// Join an existing deployment
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
    },

    /// Start the API server
    Serve {
        /// Bind address (e.g., 0.0.0.0:3669)
        #[arg(long, default_value = "0.0.0.0:3669")]
        bind: String,

        /// JWT secret for authentication (can also be set via ZLAYER_JWT_SECRET env var)
        #[arg(long, env = "ZLAYER_JWT_SECRET")]
        jwt_secret: Option<String>,

        /// Disable Swagger UI
        #[arg(long)]
        no_swagger: bool,

        /// Run as background daemon
        #[arg(long)]
        daemon: bool,

        /// Unix socket path for CLI communication.
        /// Defaults to {run-dir}/zlayer.sock.
        #[arg(long)]
        socket: Option<String>,
    },

    /// Show runtime status
    Status,

    /// Validate a spec file without deploying
    Validate {
        /// Path to deployment spec (auto-discovers *.zlayer.yml in current directory)
        spec_path: Option<PathBuf>,
    },

    /// Stream logs from a service
    Logs {
        /// Deployment name
        #[arg(long)]
        deployment: String,

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
    Stop {
        /// Deployment name
        deployment: String,

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

    /// Deploy and start services (like docker compose up)
    ///
    /// Auto-discovers *.zlayer.yml in current directory if no spec path given.
    /// Runs in foreground by default, streaming logs and waiting for Ctrl+C.
    /// Use -b/--background to deploy and return immediately.
    ///
    /// Examples:
    ///   zlayer up
    ///   zlayer up myapp.zlayer.yml
    ///   zlayer up -b
    #[command(verbatim_doc_comment)]
    Up {
        /// Path to deployment spec (auto-discovers *.zlayer.yml if not given)
        spec_path: Option<PathBuf>,
    },

    /// Stop all services in a deployment (like docker compose down)
    ///
    /// Auto-discovers deployment name from *.zlayer.yml if not given.
    ///
    /// Examples:
    ///   zlayer down
    ///   zlayer down my-deployment
    #[command(verbatim_doc_comment)]
    Down {
        /// Deployment name (auto-discovers from *.zlayer.yml if not given)
        deployment: Option<String>,
    },

    /// Build a container image from a Dockerfile
    ///
    /// Examples:
    ///   zlayer build .
    ///   zlayer build -t myapp:latest .
    ///   zlayer build --runtime node20 -t myapp:v1 ./my-node-app
    ///   zlayer build --runtime-auto -t myapp:latest .
    ///   zlayer build -f Dockerfile.prod --target production -t myapp:prod .
    #[command(verbatim_doc_comment)]
    Build {
        /// Build context directory
        #[arg(default_value = ".")]
        context: PathBuf,

        /// Dockerfile path (default: Dockerfile in context)
        #[arg(short = 'f', long)]
        file: Option<PathBuf>,

        /// ZImagefile path (alternative to Dockerfile)
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

        /// Push to registry after build
        #[arg(long)]
        push: bool,

        /// Disable TUI (plain output for CI)
        #[arg(long)]
        no_tui: bool,

        /// Verbose output (show all build output)
        #[arg(long)]
        verbose_build: bool,
    },

    /// List available runtime templates
    ///
    /// Shows all pre-built Dockerfile templates for common development
    /// environments. These can be used with `zlayer build --runtime <name>`.
    Runtimes,

    /// Token management commands
    ///
    /// Create, decode, and inspect JWT tokens for API authentication.
    #[command(subcommand)]
    Token(TokenCommands),

    /// Specification inspection commands
    ///
    /// Validate, dump, and inspect deployment specifications.
    #[command(subcommand)]
    Spec(SpecCommands),

    /// Manage cluster nodes
    #[command(subcommand)]
    Node(NodeCommands),

    /// Export an image to a tar file (OCI Image Layout)
    ///
    /// Exports a locally stored image to an OCI Image Layout tar archive
    /// that can be imported on another system or loaded into Docker.
    ///
    /// Examples:
    ///   zlayer export myapp:latest -o myapp.tar
    ///   zlayer export myapp:v1.0 -o myapp.tar.gz --gzip
    ///   zlayer export myapp@sha256:abc123... -o myapp.tar
    #[command(verbatim_doc_comment)]
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
    #[command(verbatim_doc_comment)]
    Import {
        /// Input tar file path
        input: PathBuf,

        /// Tag to apply to imported image
        #[arg(short, long)]
        tag: Option<String>,
    },

    /// WASM build, export, and management commands
    #[command(subcommand)]
    Wasm(WasmCommands),

    /// Tunnel management commands
    ///
    /// Create, manage, and connect tunnels for secure access to services.
    #[command(subcommand)]
    Tunnel(TunnelCommands),

    /// ZLayer Manager commands
    ///
    /// Initialize, configure, and manage the ZLayer Manager web UI.
    #[command(subcommand)]
    Manager(ManagerCommands),

    /// Pull an image from a registry
    ///
    /// Examples:
    ///   zlayer pull zachhandley/zlayer-node:latest
    ///   zlayer pull ghcr.io/blackleafdigital/zlayer-manager:0.9.6
    #[command(verbatim_doc_comment)]
    Pull {
        /// Image reference (e.g., zachhandley/zlayer-node:latest)
        image: String,
    },

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
    #[command(verbatim_doc_comment)]
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
    #[command(verbatim_doc_comment)]
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
    #[command(verbatim_doc_comment)]
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
    ///   zlayer tunnel connect --server wss://tunnel.example.com --token tun_xxx --service ssh:22:2222
    ///   zlayer tunnel connect --server wss://tunnel.example.com --token tun_xxx --service postgres:5432
    #[command(verbatim_doc_comment)]
    Connect {
        /// Tunnel server URL (e.g., wss://tunnel.example.com/tunnel/v1)
        #[arg(long)]
        server: String,

        /// Authentication token
        #[arg(long)]
        token: String,

        /// Service to expose (format: name:local_port[:remote_port], can be repeated)
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

/// ZLayer Manager subcommands
#[derive(Subcommand, Debug)]
pub(crate) enum ManagerCommands {
    /// Initialize zlayer-manager deployment
    ///
    /// Creates a deployment spec file for zlayer-manager and optionally
    /// deploys it immediately.
    ///
    /// Examples:
    ///   zlayer manager init
    ///   zlayer manager init --port 8080
    ///   zlayer manager init --deploy
    #[command(verbatim_doc_comment)]
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

        /// ZLayer version to use (default: latest)
        #[arg(long)]
        version: Option<String>,
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

        /// Overlay network port (WireGuard protocol)
        #[arg(long, default_value = "51820")]
        overlay_port: u16,

        /// Data directory (defaults to platform default, see --data-dir)
        #[arg(long)]
        data_dir: Option<PathBuf>,

        /// Overlay network CIDR
        #[arg(long, default_value = "10.200.0.0/16")]
        overlay_cidr: String,
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
    },

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
    ///   zlayer node generate-join-token my-deploy -a http://10.0.0.1:3669
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

        /// Data directory (where node_config.json lives, defaults to platform default)
        #[arg(long)]
        data_dir: Option<PathBuf>,
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

        /// JWT secret (or use ZLAYER_JWT_SECRET env var)
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
            "/tmp/zlayer-test.sock",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Serve {
                bind,
                jwt_secret,
                no_swagger,
                daemon,
                socket,
            }) => {
                assert_eq!(bind, "0.0.0.0:3000");
                assert_eq!(jwt_secret, Some("test-secret".to_string()));
                assert!(no_swagger);
                assert!(daemon);
                assert_eq!(socket, Some("/tmp/zlayer-test.sock".to_string()));
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
                assert_eq!(deployment, "my-deployment");
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
            }) => {
                assert!(file.is_none());
                assert!(set.is_empty());
                assert!(!push);
                assert!(fail_fast);
                assert!(!no_tui);
                assert!(only.is_none());
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
            Some(Commands::Up { spec_path }) => {
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
}
