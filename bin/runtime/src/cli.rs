use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// ZLayer container orchestration runtime
#[derive(Parser)]
#[command(name = "zlayer-runtime")]
#[command(version, about = "ZLayer container orchestration runtime")]
#[command(propagate_version = true)]
pub(crate) struct Cli {
    /// Container runtime to use
    #[arg(long, default_value = "auto", value_enum)]
    pub(crate) runtime: RuntimeType,

    /// State directory for runtime data
    #[arg(long, default_value = "/var/lib/zlayer/containers")]
    pub(crate) state_dir: PathBuf,

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
    pub(crate) command: Commands,
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

        /// Unix socket path for CLI communication
        #[arg(long, default_value = "/var/run/zlayer.sock")]
        socket: String,
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

        /// Data directory
        #[arg(long, default_value = "/var/lib/zlayer")]
        data_dir: PathBuf,

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

        /// Data directory (where node_config.json lives)
        #[arg(long, default_value = "/var/lib/zlayer")]
        data_dir: PathBuf,
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
        let cli = Cli::try_parse_from(["zlayer-runtime", "status"]).unwrap();
        assert!(matches!(cli.runtime, RuntimeType::Auto));
        assert!(matches!(cli.command, Commands::Status));
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
        assert_eq!(cli.state_dir, PathBuf::from("/custom/state"));
    }

    #[test]
    fn test_cli_deploy_command() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "deploy", "test-spec.yaml"]).unwrap();

        match cli.command {
            Commands::Deploy { spec_path, dry_run } => {
                assert_eq!(spec_path, Some(PathBuf::from("test-spec.yaml")));
                assert!(!dry_run);
            }
            _ => panic!("Expected Deploy command"),
        }
    }

    #[test]
    fn test_cli_deploy_command_no_spec() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "deploy"]).unwrap();

        match cli.command {
            Commands::Deploy { spec_path, dry_run } => {
                assert!(spec_path.is_none());
                assert!(!dry_run);
            }
            _ => panic!("Expected Deploy command"),
        }
    }

    #[test]
    fn test_cli_deploy_dry_run() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "deploy", "--dry-run", "test-spec.yaml"])
            .unwrap();

        match cli.command {
            Commands::Deploy { dry_run, .. } => {
                assert!(dry_run);
            }
            _ => panic!("Expected Deploy command"),
        }
    }

    #[test]
    fn test_cli_join_command() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "join", "some-token"]).unwrap();

        match cli.command {
            Commands::Join {
                token,
                spec_dir,
                service,
                replicas,
            } => {
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
            Commands::Join {
                token,
                spec_dir,
                service,
                replicas,
            } => {
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
            Cli::try_parse_from(["zlayer-runtime", "join", "-s", "api", "-r", "5", "token123"])
                .unwrap();

        match cli.command {
            Commands::Join {
                token,
                service,
                replicas,
                ..
            } => {
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
        let cli = Cli::try_parse_from(["zlayer-runtime", "validate", "test-spec.yaml"]).unwrap();

        match cli.command {
            Commands::Validate { spec_path } => {
                assert_eq!(spec_path, Some(PathBuf::from("test-spec.yaml")));
            }
            _ => panic!("Expected Validate command"),
        }
    }

    #[test]
    fn test_cli_validate_command_no_spec() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "validate"]).unwrap();

        match cli.command {
            Commands::Validate { spec_path } => {
                assert!(spec_path.is_none());
            }
            _ => panic!("Expected Validate command"),
        }
    }

    #[test]
    fn test_cli_verbose_levels() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "status"]).unwrap();
        assert_eq!(cli.verbose, 0);

        let cli = Cli::try_parse_from(["zlayer-runtime", "-v", "status"]).unwrap();
        assert_eq!(cli.verbose, 1);

        let cli = Cli::try_parse_from(["zlayer-runtime", "-vv", "status"]).unwrap();
        assert_eq!(cli.verbose, 2);
    }

    #[test]
    fn test_build_runtime_config_auto() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "status"]).unwrap();
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
        let cli = Cli::try_parse_from(["zlayer-runtime", "serve"]).unwrap();

        match cli.command {
            Commands::Serve {
                bind,
                jwt_secret,
                no_swagger,
                daemon,
                socket,
            } => {
                assert_eq!(bind, "0.0.0.0:3669");
                assert!(jwt_secret.is_none());
                assert!(!no_swagger);
                assert!(!daemon);
                assert_eq!(socket, "/var/run/zlayer.sock");
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_custom_bind() {
        let cli =
            Cli::try_parse_from(["zlayer-runtime", "serve", "--bind", "127.0.0.1:9090"]).unwrap();

        match cli.command {
            Commands::Serve { bind, .. } => {
                assert_eq!(bind, "127.0.0.1:9090");
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_jwt_secret() {
        let cli = Cli::try_parse_from([
            "zlayer-runtime",
            "serve",
            "--jwt-secret",
            "my-super-secret-key",
        ])
        .unwrap();

        match cli.command {
            Commands::Serve { jwt_secret, .. } => {
                assert_eq!(jwt_secret, Some("my-super-secret-key".to_string()));
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_no_swagger() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "serve", "--no-swagger"]).unwrap();

        match cli.command {
            Commands::Serve { no_swagger, .. } => {
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
            Commands::Serve {
                bind,
                jwt_secret,
                no_swagger,
                daemon,
                socket,
            } => {
                assert_eq!(bind, "0.0.0.0:3000");
                assert_eq!(jwt_secret, Some("test-secret".to_string()));
                assert!(no_swagger);
                assert!(daemon);
                assert_eq!(socket, "/tmp/zlayer-test.sock");
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_daemon_flag() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "serve", "--daemon"]).unwrap();

        match cli.command {
            Commands::Serve { daemon, .. } => {
                assert!(daemon);
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_serve_command_socket_flag() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "serve", "--socket", "/tmp/custom.sock"])
            .unwrap();

        match cli.command {
            Commands::Serve { socket, .. } => {
                assert_eq!(socket, "/tmp/custom.sock");
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
            Commands::Serve {
                bind,
                daemon,
                socket,
                ..
            } => {
                assert_eq!(bind, "127.0.0.1:3669");
                assert!(daemon);
                assert_eq!(socket, "/var/run/zlayer-custom.sock");
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
            Commands::Logs {
                deployment,
                service,
                lines,
                follow,
                instance,
            } => {
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
    fn test_cli_logs_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "logs",
            "--deployment",
            "prod",
            "--lines",
            "50",
            "--follow",
            "--instance",
            "abc123",
            "web-server",
        ])
        .unwrap();

        match cli.command {
            Commands::Logs {
                deployment,
                service,
                lines,
                follow,
                instance,
            } => {
                assert_eq!(deployment, "prod");
                assert_eq!(service, "web-server");
                assert_eq!(lines, 50);
                assert!(follow);
                assert_eq!(instance, Some("abc123".to_string()));
            }
            _ => panic!("Expected Logs command"),
        }
    }

    #[test]
    fn test_cli_logs_command_short_flags() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "logs",
            "--deployment",
            "staging",
            "-n",
            "25",
            "-f",
            "-i",
            "inst-456",
            "api",
        ])
        .unwrap();

        match cli.command {
            Commands::Logs {
                deployment,
                service,
                lines,
                follow,
                instance,
            } => {
                assert_eq!(deployment, "staging");
                assert_eq!(service, "api");
                assert_eq!(lines, 25);
                assert!(follow);
                assert_eq!(instance, Some("inst-456".to_string()));
            }
            _ => panic!("Expected Logs command"),
        }
    }

    #[test]
    fn test_cli_stop_command_minimal() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "stop", "my-deployment"]).unwrap();

        match cli.command {
            Commands::Stop {
                deployment,
                service,
                force,
                timeout,
            } => {
                assert_eq!(deployment, "my-deployment");
                assert!(service.is_none());
                assert!(!force);
                assert_eq!(timeout, 30); // default
            }
            _ => panic!("Expected Stop command"),
        }
    }

    #[test]
    fn test_cli_stop_command_with_service() {
        let cli = Cli::try_parse_from([
            "zlayer-runtime",
            "stop",
            "--service",
            "web",
            "my-deployment",
        ])
        .unwrap();

        match cli.command {
            Commands::Stop {
                deployment,
                service,
                ..
            } => {
                assert_eq!(deployment, "my-deployment");
                assert_eq!(service, Some("web".to_string()));
            }
            _ => panic!("Expected Stop command"),
        }
    }

    #[test]
    fn test_cli_stop_command_force() {
        let cli =
            Cli::try_parse_from(["zlayer-runtime", "stop", "--force", "my-deployment"]).unwrap();

        match cli.command {
            Commands::Stop { force, .. } => {
                assert!(force);
            }
            _ => panic!("Expected Stop command"),
        }
    }

    #[test]
    fn test_cli_stop_command_timeout() {
        let cli =
            Cli::try_parse_from(["zlayer-runtime", "stop", "--timeout", "60", "my-deployment"])
                .unwrap();

        match cli.command {
            Commands::Stop { timeout, .. } => {
                assert_eq!(timeout, 60);
            }
            _ => panic!("Expected Stop command"),
        }
    }

    #[test]
    fn test_cli_stop_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "stop",
            "-s",
            "database",
            "-f",
            "-t",
            "15",
            "production",
        ])
        .unwrap();

        match cli.command {
            Commands::Stop {
                deployment,
                service,
                force,
                timeout,
            } => {
                assert_eq!(deployment, "production");
                assert_eq!(service, Some("database".to_string()));
                assert!(force);
                assert_eq!(timeout, 15);
            }
            _ => panic!("Expected Stop command"),
        }
    }

    #[test]
    fn test_cli_build_command_minimal() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "build"]).unwrap();

        match cli.command {
            Commands::Build {
                context,
                file,
                zimagefile,
                tags,
                runtime,
                runtime_auto,
                build_args,
                target,
                no_cache,
                push,
                no_tui,
                verbose_build,
            } => {
                assert_eq!(context, PathBuf::from("."));
                assert!(file.is_none());
                assert!(zimagefile.is_none());
                assert!(tags.is_empty());
                assert!(runtime.is_none());
                assert!(!runtime_auto);
                assert!(build_args.is_empty());
                assert!(target.is_none());
                assert!(!no_cache);
                assert!(!push);
                assert!(!no_tui);
                assert!(!verbose_build);
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_zimagefile() {
        let cli =
            Cli::try_parse_from(["zlayer-runtime", "build", "-z", "ZImagefile.yml", "."]).unwrap();

        match cli.command {
            Commands::Build { zimagefile, .. } => {
                assert_eq!(zimagefile, Some(PathBuf::from("ZImagefile.yml")));
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_context() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "build", "./my-app"]).unwrap();

        match cli.command {
            Commands::Build { context, .. } => {
                assert_eq!(context, PathBuf::from("./my-app"));
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_tags() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "build",
            "-t",
            "myapp:latest",
            "-t",
            "myapp:v1.0.0",
            ".",
        ])
        .unwrap();

        match cli.command {
            Commands::Build { tags, .. } => {
                assert_eq!(tags.len(), 2);
                assert_eq!(tags[0], "myapp:latest");
                assert_eq!(tags[1], "myapp:v1.0.0");
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_dockerfile() {
        let cli =
            Cli::try_parse_from(["zlayer-runtime", "build", "-f", "Dockerfile.prod", "."]).unwrap();

        match cli.command {
            Commands::Build { file, .. } => {
                assert_eq!(file, Some(PathBuf::from("Dockerfile.prod")));
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_runtime() {
        let cli =
            Cli::try_parse_from(["zlayer-runtime", "build", "--runtime", "node20", "."]).unwrap();

        match cli.command {
            Commands::Build { runtime, .. } => {
                assert_eq!(runtime, Some("node20".to_string()));
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_runtime_auto() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "build", "--runtime-auto", "."]).unwrap();

        match cli.command {
            Commands::Build { runtime_auto, .. } => {
                assert!(runtime_auto);
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_build_args() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "build",
            "--build-arg",
            "VERSION=1.0.0",
            "--build-arg",
            "DEBUG=false",
            ".",
        ])
        .unwrap();

        match cli.command {
            Commands::Build { build_args, .. } => {
                assert_eq!(build_args.len(), 2);
                assert_eq!(build_args[0], "VERSION=1.0.0");
                assert_eq!(build_args[1], "DEBUG=false");
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_target() {
        let cli =
            Cli::try_parse_from(["zlayer-runtime", "build", "--target", "builder", "."]).unwrap();

        match cli.command {
            Commands::Build { target, .. } => {
                assert_eq!(target, Some("builder".to_string()));
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_with_flags() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "build",
            "--no-cache",
            "--push",
            "--no-tui",
            "--verbose-build",
            ".",
        ])
        .unwrap();

        match cli.command {
            Commands::Build {
                no_cache,
                push,
                no_tui,
                verbose_build,
                ..
            } => {
                assert!(no_cache);
                assert!(push);
                assert!(no_tui);
                assert!(verbose_build);
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_build_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "build",
            "-f",
            "Dockerfile.prod",
            "-t",
            "myapp:latest",
            "-t",
            "myapp:v1",
            "--runtime",
            "node20",
            "--build-arg",
            "VERSION=1.0",
            "--target",
            "production",
            "--no-cache",
            "--push",
            "--no-tui",
            "--verbose-build",
            "./my-project",
        ])
        .unwrap();

        match cli.command {
            Commands::Build {
                context,
                file,
                tags,
                runtime,
                build_args,
                target,
                no_cache,
                push,
                no_tui,
                verbose_build,
                ..
            } => {
                assert_eq!(context, PathBuf::from("./my-project"));
                assert_eq!(file, Some(PathBuf::from("Dockerfile.prod")));
                assert_eq!(tags.len(), 2);
                assert_eq!(runtime, Some("node20".to_string()));
                assert_eq!(build_args.len(), 1);
                assert_eq!(target, Some("production".to_string()));
                assert!(no_cache);
                assert!(push);
                assert!(no_tui);
                assert!(verbose_build);
            }
            _ => panic!("Expected Build command"),
        }
    }

    #[test]
    fn test_cli_runtimes_command() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "runtimes"]).unwrap();
        assert!(matches!(cli.command, Commands::Runtimes));
    }

    #[test]
    fn test_cli_node_init_command() {
        let cli = Cli::try_parse_from([
            "zlayer-runtime",
            "node",
            "init",
            "--advertise-addr",
            "10.0.0.1",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Init {
                advertise_addr,
                api_port,
                raft_port,
                overlay_port,
                data_dir,
                overlay_cidr,
            }) => {
                assert_eq!(advertise_addr, "10.0.0.1");
                assert_eq!(api_port, 3669);
                assert_eq!(raft_port, 9000);
                assert_eq!(overlay_port, 51820);
                assert_eq!(data_dir, PathBuf::from("/var/lib/zlayer"));
                assert_eq!(overlay_cidr, "10.200.0.0/16");
            }
            _ => panic!("Expected Node Init command"),
        }
    }

    #[test]
    fn test_cli_node_init_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "init",
            "--advertise-addr",
            "192.168.1.100",
            "--api-port",
            "9090",
            "--raft-port",
            "9001",
            "--overlay-port",
            "51821",
            "--data-dir",
            "/custom/data",
            "--overlay-cidr",
            "10.100.0.0/16",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Init {
                advertise_addr,
                api_port,
                raft_port,
                overlay_port,
                data_dir,
                overlay_cidr,
            }) => {
                assert_eq!(advertise_addr, "192.168.1.100");
                assert_eq!(api_port, 9090);
                assert_eq!(raft_port, 9001);
                assert_eq!(overlay_port, 51821);
                assert_eq!(data_dir, PathBuf::from("/custom/data"));
                assert_eq!(overlay_cidr, "10.100.0.0/16");
            }
            _ => panic!("Expected Node Init command"),
        }
    }

    #[test]
    fn test_cli_node_join_command() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "join",
            "10.0.0.1:3669",
            "--token",
            "abc123",
            "--advertise-addr",
            "10.0.0.2",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Join {
                leader_addr,
                token,
                advertise_addr,
                mode,
                services,
            }) => {
                assert_eq!(leader_addr, "10.0.0.1:3669");
                assert_eq!(token, "abc123");
                assert_eq!(advertise_addr, "10.0.0.2");
                assert_eq!(mode, "full");
                assert!(services.is_none());
            }
            _ => panic!("Expected Node Join command"),
        }
    }

    #[test]
    fn test_cli_node_join_command_with_services() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "join",
            "10.0.0.1:3669",
            "--token",
            "abc123",
            "--advertise-addr",
            "10.0.0.2",
            "--mode",
            "replicate",
            "--services",
            "api",
            "--services",
            "web",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Join { mode, services, .. }) => {
                assert_eq!(mode, "replicate");
                assert_eq!(services, Some(vec!["api".to_string(), "web".to_string()]));
            }
            _ => panic!("Expected Node Join command"),
        }
    }

    #[test]
    fn test_cli_node_list_command() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "node", "list"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::List { output }) => {
                assert_eq!(output, "table");
            }
            _ => panic!("Expected Node List command"),
        }
    }

    #[test]
    fn test_cli_node_list_command_json() {
        let cli =
            Cli::try_parse_from(["zlayer-runtime", "node", "list", "--output", "json"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::List { output }) => {
                assert_eq!(output, "json");
            }
            _ => panic!("Expected Node List command"),
        }
    }

    #[test]
    fn test_cli_node_status_command() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "node", "status"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Status { node_id }) => {
                assert!(node_id.is_none());
            }
            _ => panic!("Expected Node Status command"),
        }
    }

    #[test]
    fn test_cli_node_status_command_with_id() {
        let cli =
            Cli::try_parse_from(["zlayer-runtime", "node", "status", "node-abc-123"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Status { node_id }) => {
                assert_eq!(node_id, Some("node-abc-123".to_string()));
            }
            _ => panic!("Expected Node Status command"),
        }
    }

    #[test]
    fn test_cli_node_remove_command() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "node", "remove", "node-123"]).unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Remove { node_id, force }) => {
                assert_eq!(node_id, "node-123");
                assert!(!force);
            }
            _ => panic!("Expected Node Remove command"),
        }
    }

    #[test]
    fn test_cli_node_remove_command_force() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "node", "remove", "--force", "node-123"])
            .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Remove { node_id, force }) => {
                assert_eq!(node_id, "node-123");
                assert!(force);
            }
            _ => panic!("Expected Node Remove command"),
        }
    }

    #[test]
    fn test_cli_node_set_mode_command() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "set-mode",
            "node-123",
            "--mode",
            "dedicated",
            "--services",
            "api",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::SetMode {
                node_id,
                mode,
                services,
            }) => {
                assert_eq!(node_id, "node-123");
                assert_eq!(mode, "dedicated");
                assert_eq!(services, Some(vec!["api".to_string()]));
            }
            _ => panic!("Expected Node SetMode command"),
        }
    }

    #[test]
    fn test_cli_node_label_command() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "label",
            "node-123",
            "environment=production",
        ])
        .unwrap();

        match cli.command {
            Commands::Node(NodeCommands::Label { node_id, label }) => {
                assert_eq!(node_id, "node-123");
                assert_eq!(label, "environment=production");
            }
            _ => panic!("Expected Node Label command"),
        }
    }

    // NOTE: Node-specific tests (generate_secure_token, generate_node_id,
    // generate_join_token_data, parse_cluster_join_token) are in commands/node.rs

    #[test]
    fn test_cli_up_command_no_spec() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "up"]).unwrap();

        match cli.command {
            Commands::Up { spec_path } => {
                assert!(spec_path.is_none());
            }
            _ => panic!("Expected Up command"),
        }
        assert!(!cli.background);
    }

    #[test]
    fn test_cli_up_command_with_spec() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "up", "my-spec.yml"]).unwrap();

        match cli.command {
            Commands::Up { spec_path } => {
                assert_eq!(spec_path, Some(PathBuf::from("my-spec.yml")));
            }
            _ => panic!("Expected Up command"),
        }
    }

    #[test]
    fn test_cli_up_command_background() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "up", "-b"]).unwrap();

        assert!(cli.background);
        assert!(matches!(cli.command, Commands::Up { .. }));
    }

    #[test]
    fn test_cli_up_command_background_global() {
        // Background flag before subcommand
        let cli = Cli::try_parse_from(["zlayer-runtime", "-b", "up"]).unwrap();

        assert!(cli.background);
        assert!(matches!(cli.command, Commands::Up { .. }));
    }

    #[test]
    fn test_cli_down_command_no_deployment() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "down"]).unwrap();

        match cli.command {
            Commands::Down { deployment } => {
                assert!(deployment.is_none());
            }
            _ => panic!("Expected Down command"),
        }
    }

    #[test]
    fn test_cli_down_command_with_deployment() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "down", "my-app"]).unwrap();

        match cli.command {
            Commands::Down { deployment } => {
                assert_eq!(deployment, Some("my-app".to_string()));
            }
            _ => panic!("Expected Down command"),
        }
    }

    #[test]
    fn test_cli_background_flag_default() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "status"]).unwrap();
        assert!(!cli.background);
    }

    #[test]
    fn test_cli_detach_flag_default() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "status"]).unwrap();
        assert!(!cli.detach);
    }

    #[test]
    fn test_cli_detach_flag_long() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "--detach", "up"]).unwrap();
        assert!(cli.detach);
        assert!(!cli.background);
    }

    #[test]
    fn test_cli_detach_flag_after_subcommand() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "up", "--detach"]).unwrap();
        assert!(cli.detach);
    }

    #[test]
    fn test_cli_detach_with_deploy() {
        let cli =
            Cli::try_parse_from(["zlayer-runtime", "--detach", "deploy", "spec.yml"]).unwrap();
        assert!(cli.detach);
        assert!(matches!(cli.command, Commands::Deploy { .. }));
    }

    #[test]
    fn test_deploy_mode_variants() {
        assert_ne!(DeployMode::Foreground, DeployMode::Background);
        assert_ne!(DeployMode::Foreground, DeployMode::Detach);
        assert_ne!(DeployMode::Background, DeployMode::Detach);
    }

    #[test]
    fn test_cli_detach_short_flag() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "-d", "deploy", "spec.yml"]).unwrap();
        assert!(cli.detach);
        assert!(matches!(cli.command, Commands::Deploy { .. }));
    }

    #[test]
    fn test_cli_detach_short_flag_after_subcommand() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "deploy", "-d", "spec.yml"]).unwrap();
        assert!(cli.detach);
        match cli.command {
            Commands::Deploy { spec_path, .. } => {
                assert_eq!(spec_path, Some(PathBuf::from("spec.yml")));
            }
            _ => panic!("Expected Deploy command"),
        }
    }

    #[test]
    fn test_cli_detach_short_flag_with_up() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "up", "-d"]).unwrap();
        assert!(cli.detach);
        assert!(matches!(cli.command, Commands::Up { .. }));
    }

    #[test]
    fn test_cli_ps_command_defaults() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "ps"]).unwrap();

        match cli.command {
            Commands::Ps {
                deployment,
                containers,
                format,
            } => {
                assert!(deployment.is_none());
                assert!(!containers);
                assert_eq!(format, "table");
            }
            _ => panic!("Expected Ps command"),
        }
    }

    #[test]
    fn test_cli_ps_command_with_deployment() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "ps", "--deployment", "my-app"]).unwrap();

        match cli.command {
            Commands::Ps { deployment, .. } => {
                assert_eq!(deployment, Some("my-app".to_string()));
            }
            _ => panic!("Expected Ps command"),
        }
    }

    #[test]
    fn test_cli_ps_command_with_containers() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "ps", "-c"]).unwrap();

        match cli.command {
            Commands::Ps { containers, .. } => {
                assert!(containers);
            }
            _ => panic!("Expected Ps command"),
        }
    }

    #[test]
    fn test_cli_ps_command_json_format() {
        let cli = Cli::try_parse_from(["zlayer-runtime", "ps", "--format", "json"]).unwrap();

        match cli.command {
            Commands::Ps { format, .. } => {
                assert_eq!(format, "json");
            }
            _ => panic!("Expected Ps command"),
        }
    }

    #[test]
    fn test_cli_ps_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "ps",
            "--deployment",
            "prod",
            "--containers",
            "--format",
            "yaml",
        ])
        .unwrap();

        match cli.command {
            Commands::Ps {
                deployment,
                containers,
                format,
            } => {
                assert_eq!(deployment, Some("prod".to_string()));
                assert!(containers);
                assert_eq!(format, "yaml");
            }
            _ => panic!("Expected Ps command"),
        }
    }
}
