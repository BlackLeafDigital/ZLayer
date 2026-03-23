# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- `BuildBackend` trait in `zlayer-builder::backend` providing a pluggable
  abstraction over container build tooling. Includes `BuildahBackend` (wraps
  buildah CLI) and `SandboxBackend` (macOS Seatbelt, cfg-gated). Added
  `detect_backend()` for runtime auto-detection with `ZLAYER_BACKEND` env
  override support. Added `BuildError::NotSupported` variant for unsupported
  backend operations.
- `ImageBuilder::with_backend()` constructor for creating a builder with an
  explicit `BuildBackend`. `ImageBuilder::new()` now auto-detects the backend
  via `detect_backend()`. `with_executor()` wraps the executor in a
  `BuildahBackend` for trait-based dispatch.
- `PipelineExecutor::with_backend()` constructor for creating a pipeline
  executor with an explicit `BuildBackend`. Push, manifest, and per-image
  build operations delegate to the backend when set.

### Changed
- CLI `pipeline` command now uses `detect_backend()` + `PipelineExecutor::with_backend()`
  instead of directly creating a `BuildahExecutor`, enabling automatic sandbox
  fallback on macOS.
- Register container lifecycle routes (`/api/v1/containers`) in the daemon API
  server, enabling direct container creation, inspection, logs, exec, and stats
  endpoints independent of the deployment/service abstraction.

### Fixed
- macOS sandbox runtime default data directory changed from `~/.local/share/zlayer`
  to `~/.zlayer` to match the builder and other components.
- macOS images CI workflow (`macos-images.yml`): replaced `uses: ./` action
  reference (which tried to download from GitHub and hung) with building zlayer
  from source and running the pipeline directly.

### Changed
- `map_linux_packages()` in `macos_image_resolver` is now async and fetches
  package mappings from RepoSources (`zachhandley.github.io/RepoSources/maps/`)
  with a 7-day local cache at `{data_dir}/cache/package-maps/{distro}.json`.
  Resolution order: cached/fetched RepoSources map, name transformation
  heuristics (strip `-dev`, `lib` prefix, version digits), then hardcoded
  fallback. The original hardcoded mapping is preserved as `map_single_package_hardcoded()`.
- Homebrew bottle install success log now includes the formula version
  (`versions.stable`) from the Homebrew API.

- Refactored `fetch_and_extract_bottle()` in `macos_image_resolver` to resolve
  the full Homebrew dependency tree (BFS) before installing a formula. Split into
  three functions: `fetch_formula_info()`, `install_single_bottle()`, and the
  public `fetch_and_extract_bottle()` entry point. Added `dependencies` and
  `versions` fields to `BrewFormulaInfo`. Formulas already present in the rootfs
  Cellar are skipped, and individual dependency failures are logged as warnings
  without aborting the overall installation.
- Refactored `sandbox_builder::setup_base_image()` to use the 3-tier macOS image
  resolution from `macos_image_resolver`. Old inline toolchain provisioning block
  replaced with Tier 1 (local cache), Tier 2 (GHCR pull), Tier 3 (local build)
  pipeline. Non-rewritable images retain existing cache/pull logic.
- `sandbox_builder::load_base_image_config()` now checks for rewritten macOS image
  `config.json` before falling through to the existing `image_config.json` / registry
  pull path.
- Toolchain env injection in `build()` now guards against duplicating env vars that
  are already present in the config (e.g., when loaded from a cached image config).
- `sandbox_builder::execute_run()` now intercepts Linux package manager install
  commands (`apt-get install`, `apk add`, `yum install`, `dnf install`) and fetches
  Homebrew bottles into the sandbox rootfs before the translated (no-op) command runs.

### Added
- `extract_package_install_packages()` helper: parses package names from apt-get,
  apk, yum, and dnf install commands, handling flags, sudo, and `&&`-compound
  commands.
- `find_after_subcommand()` helper: locates the argument tail after a subcommand
  keyword, skipping leading flags.
- 3-tier macOS image resolution system (`macos_image_resolver`) — rewrites Docker Hub
  image references to macOS-native equivalents. Tier 1: pulls pre-built sandbox images
  from `ghcr.io/blackleafdigital/zlayer`. Tier 2: builds toolchain images locally via
  `macos_toolchain`. Tier 3: creates minimal base rootfs for distro images. Includes
  GHCR auth resolution (GHCR_TOKEN, GITHUB_TOKEN, Docker config), Homebrew bottle
  fetching for installing packages into sandbox rootfs, and Linux-to-brew package
  name mapping.
- GraalVM CE toolchain resolver for macOS sandbox builds — resolves exact versions
  (`21.0.5`), partial/major versions (`21` -> latest 21.x.y), and `latest` by scanning
  GitHub releases from `graalvm/graalvm-ce-builds`. Downloads macOS tarballs and
  extracts with `--strip-components=3` (same JDK structure as Adoptium). Sets both
  `JAVA_HOME` and `GRAALVM_HOME` environment variables. Detects base images containing
  "graalvm" in the name (e.g., `graalvm-ce`, `graalvm/graalvm-ce`).
- Swift toolchain resolver for macOS sandbox builds — provisions Swift from the host
  system's Xcode Command Line Tools rather than downloading. Locates the toolchain via
  `xcrun --find swiftc`, copies it into the cache keyed by the detected version, and
  symlinks into the build rootfs at `/usr/local/swift`. Detects `swift` base images.
- Java (Adoptium/Temurin) toolchain resolver for macOS sandbox builds — resolves `latest`
  (fetches most recent LTS from Adoptium API), exact feature versions (`21`, `17`, `8`),
  and dotted versions (`21.0.5` strips to major `21`). Uses the Adoptium binary API which
  redirects to `.tar.gz` downloads. Extracts with `--strip-components=3` to handle the
  macOS `jdk-X/Contents/Home/` tarball structure. Detects `eclipse-temurin`, `amazoncorretto`,
  and `openjdk` base images.
- Bun toolchain resolver for macOS sandbox builds — resolves exact versions (`1.2.3`),
  partial versions (`1` -> latest 1.x.y), and `latest` by scanning GitHub releases from
  `oven-sh/bun`. Downloads `bun-darwin-{arch}.zip` assets and extracts the binary into
  the `bin/` directory structure for proper PATH integration.
- Python toolchain resolver for macOS sandbox builds — uses standalone builds from
  `astral-sh/python-build-standalone` on GitHub. Resolves exact versions (`3.12.1`),
  partial versions (`3.12` -> latest 3.12.x), and `latest` by scanning GitHub release
  assets. Downloads `install_only_stripped` tarballs for the host architecture.
- Rust toolchain resolver for macOS sandbox builds — resolves exact versions (e.g. `1.82.0`),
  partial versions (`1.82` -> `1.82.0`), and `latest` (fetches stable channel TOML). Downloads
  standalone installer tarballs from `static.rust-lang.org`, extracts, and runs `install.sh`
  with `--prefix` to provision rustc + cargo into the build cache.

### Changed
- Sandbox builder: macOS toolchain provisioning now symlinks cached toolchains into the
  build rootfs instead of copying. The cache at `~/.zlayer/toolchains/{lang}-{version}-{arch}/`
  is the immutable source; each build gets a symlink to it, saving disk space and build time.

### Fixed
- Sandbox builder: macOS toolchain provisioning (`macos_toolchain.rs`) — auto-detects
  language from base image (e.g. `golang:1.23-alpine` → Go 1.23), downloads self-contained
  macOS binaries from official APIs (go.dev, nodejs.org, etc.), and provisions them directly
  into the build rootfs. No brew, no global state, parallel-build safe. Supports Go + Node
  initially, with version resolution for any published version.
- Sandbox builder: `macos_compat.rs` package manager commands (`apk add`, `apt-get install`)
  are now no-ops — toolchains are provisioned in rootfs, so Linux package installs are skipped
- Sandbox builder: PATH augmentation now always includes rootfs-prefixed paths and Homebrew
  paths, even when the base image already defines its own PATH (e.g. golang images)
- Sandbox builder: broadened Seatbelt mach-lookup to allow all services during build-time,
  fixing brew/ruby/git failures caused by restricted XPC access
- Sandbox builder: RunFailed errors now include stderr output for better diagnostics
- action.yml: added curl timeouts (`--connect-timeout 30 --max-time 120`) to prevent
  indefinite hangs when downloading ZLayer binaries from GitHub releases
- install.sh: added curl timeouts to binary download
- Sandbox builder: per-build HOME directory (`{image_dir}/home/`) instead of hardcoded
  `/root` — fixes `Read-only file system @ dir_s_mkdir - /root` errors from brew/git
- Sandbox builder: base image config (ENV, WORKDIR, USER, ENTRYPOINT, CMD, Shell,
  Healthcheck) is now loaded from OCI image metadata and merged into the build config —
  previously all base image config was silently discarded
- Sandbox builder: base image config is cached alongside rootfs as `image_config.json`
- Registry: `ImageConfig` now parses Shell and Healthcheck fields from OCI image config

### Changed
- Moved `docker/` directory to `images/` and updated all references across the codebase
  (ZPipeline.yaml, CLAUDE.md, Dockerfiles, ZImagefiles, pipeline module docs, builder README)
- Buildah "not found" warning suppressed on macOS (now debug-level); non-macOS unchanged
- Sandbox builder: broadened Seatbelt FS rules for build-time (allows RUN commands to
  write to absolute host paths since there is no chroot on macOS)

### Added
- macOS-native base images defined as ZImagefiles (`images/macos/`):
  `zlayer/base`, `zlayer/golang`, `zlayer/rust`, `zlayer/node`, `zlayer/python` (with uv),
  `zlayer/deno`, `zlayer/bun` — containing macOS Mach-O binaries for sandbox builds
- macOS base image ZPipeline (`images/macos/ZPipeline.yaml`) orchestrating all 7 images
  with dependency ordering, tagged for `ghcr.io/blackleafdigital/zlayer/`
- macOS image builder script (`scripts/build-macos-images.sh`) for bootstrapping native
  rootfs images with architecture detection (arm64/x86_64)
- Registry client: two-pass platform resolver on macOS — prefers `darwin/{arch}` for
  native ZLayer sandbox images, falls back to `linux/{arch}` for Docker Hub images
- Sandbox builder: ELF binary detection on macOS — when a RUN command fails with exit
  126/127, checks if the binary is a Linux ELF and produces a clear error message
  suggesting zlayer/ base images instead of Alpine/Debian
- Sandbox builder: PATH includes rootfs bin dirs + Homebrew paths (`/opt/homebrew/bin`)
  so that macOS-native binaries installed in the image are found
- Sandbox builder: end-to-end integration tests (`sandbox_build_e2e.rs`) covering
  manifest pull, layer pull, simple build, and ENV/WORKDIR build verification
- Sandbox builder: registry image pull via `zlayer-registry` (ImagePuller + LayerUnpacker)
  when the `cache` feature is enabled, instead of requiring pre-pulled base images
- Sandbox builder: multi-stage build support -- all stages are built sequentially and
  `COPY --from=stage` resolves files from previously-built stage rootfs directories
- Sandbox builder: ARG/ENV variable substitution (`${VAR}`, `${VAR:-default}`, `$VAR`)
  applied to RUN commands, COPY/ADD sources and destinations, ENV values, WORKDIR,
  LABEL values, and USER instructions
- Sandbox builder: ADD URL sources -- downloads via `reqwest` with automatic archive
  extraction for `.tar`, `.tar.gz`, `.tgz`, `.tar.bz2`, `.tar.xz`, and `.zip` files
- Sandbox builder: ADD local archive auto-extraction for tar/gzip/bzip2/xz/zip formats
- Sandbox builder: SHELL instruction support -- custom shell stored in config and used
  for subsequent RUN shell-form commands
- Sandbox builder: HEALTHCHECK instruction -- stores command, interval, timeout,
  start_period, and retries in `SandboxImageConfig`
- Sandbox builder: USER instruction now sets the USER environment variable for RUN
  commands and resolves usernames from the rootfs `/etc/passwd`
- Sandbox builder: COPY/ADD `--chown` and `--chmod` flags applied after file operations
  using `nix::unistd::chown` and `std::os::unix::fs::PermissionsExt`

### Fixed
- Dockerfile COPY parser: fixed source/destination extraction to use the external
  parser's separated `destination` field instead of incorrectly splitting the
  `sources` vector (which already excluded the destination)
- Sandbox builder: COPY/ADD destination path handling now correctly distinguishes
  file targets from directory targets (`.`, trailing `/`, multiple sources)

## [2026-03-17]

### Added
- macOS-native image builder (`SandboxImageBuilder`) that uses the Seatbelt sandbox
  instead of buildah, enabling `zlayer build` on macOS without requiring buildah or
  a Linux VM. Supports FROM, RUN, COPY, ADD, ENV, WORKDIR, ENTRYPOINT, CMD, EXPOSE,
  ARG, LABEL, USER, VOLUME, and STOPSIGNAL instructions (single-stage builds).
- `ImageBuilder` automatically falls back to the sandbox builder on macOS when buildah
  is not installed, instead of failing with a "buildah not found" error.
- `examples/zlayer-web.zlayer.yml` deployment spec for the Leptos web frontend

### Fixed
- Daemon now regenerates the admin password if the `admin_password` file was
  deleted while the credential store still has the (Argon2id-hashed) entry,
  preventing permanently unrecoverable admin credentials
- `zlayer daemon install` and `zlayer daemon start` now write a `spawner.pid`
  marker file before launching the daemon via launchctl/systemctl, so the new
  daemon's `cleanup_stale_daemon()` skips killing the spawning CLI process
  (previously caused SIGTERM / exit 143)
- `cleanup_stale_daemon()` now reads `{data_dir}/spawner.pid` as a fallback when
  `ZLAYER_SPAWNER_PID` env var is not set (e.g., when launched via launchd/systemd),
  preventing the daemon from killing the `zlayer daemon install/start` CLI process
- GitHub Actions E2E workflow aligned with Forgejo: added `CARGO_BUILD_JOBS: "4"`,
  `--test-threads=1` for Youki tests, and post-sudo permission fix to resolve
  SQLite "database is locked" contention and test timeouts
- `daemon install` placed `--data-dir` after the `serve` subcommand, but it's a
  top-level CLI arg — clap rejected it, causing the daemon to crash-loop on start
  (both macOS launchd and Linux systemd)

### Previously added
- Containers automatically receive `ZLAYER_API_URL`, `ZLAYER_TOKEN`, and
  `ZLAYER_SOCKET` environment variables for authenticated API access
- Unix socket bind-mounted into containers for local auth bypass (Linux/Docker)
- macOS sandbox containers get socket path in writable dirs for local access
- `zlayer token show` command to retrieve admin API credentials
- Admin password persisted to `{data_dir}/admin_password` (mode 0600) on first bootstrap

### Fixed
- Overlay networking now auto-starts reliably on daemon restart by waiting for
  the WireGuard UDP port (51820) to be freed after killing the old daemon
  (fixes `errno=48` / EADDRINUSE on restart).
- Stale network interface cleanup uses `ifconfig` on macOS instead of Linux-only
  `ip` command, properly destroying orphaned utun devices with matching WireGuard
  sockets.
- Raft data directory now uses platform-specific data dir instead of hardcoded
  `/var/lib/zlayer/raft` (fixes "Permission denied" on macOS without root).
- Raft bootstrap "already bootstrapped" message downgraded to `info!` (expected
  on every restart with persistent storage).
- Overlay warning messages no longer reference Linux-only concepts (veth pairs)
  on macOS. macOS hint now directs users to `sudo` or `zlayer daemon install`.

## [0.9.990]

### Added
- Resource-aware node join: `NodeInfo` now tracks CPU cores, memory, disk, GPU resources, and
  node status (ready/draining/dead). Join flow (`POST /api/v1/cluster/join`) includes hardware
  resource information from the joining node.
- Cross-platform system resource detection (`bin/zlayer/src/resources.rs`): detects CPU, memory,
  disk, and GPU resources using Linux `/proc/meminfo`, macOS `sysctl`, and disk via `statvfs`.
- Leader self-registration: the leader node registers itself with real hardware specs on bootstrap
  instead of placeholder values.
- Worker heartbeat endpoint (`POST /api/v1/cluster/heartbeat`): worker nodes report resource
  usage every 5 seconds. Dead-node detection loop marks stale nodes as "dead" after 30 seconds
  of missed heartbeats.
- Distributed scheduling: `Scheduler::build_node_states()` converts Raft cluster state into a
  placement-ready node list. `Scheduler::compute_placement()` feeds it through the existing
  bin-packing/dedicated/exclusive placement algorithms.
- Remote container dispatch: `Scheduler::execute_distributed_scaling()` dispatches container
  creation to remote nodes via internal API calls. `apply_scaling()` now chooses distributed
  multi-node or local-only scheduling based on cluster size.
- Service assignment tracking: service-to-node assignments are replicated through Raft via
  `UpdateServiceAssignment` entries.
- Container rescheduling on node death: when the dead-node detection loop marks a node as "dead",
  the scheduler's `handle_node_death()` method automatically reschedules affected services to
  remaining live nodes using the placement algorithm.
- `Scheduler::with_raft()` constructor: accepts a pre-existing `Arc<RaftCoordinator>` so the
  daemon can share its Raft coordinator with the scheduler.
- `PlacementState::remove_node()` and `PlacementState::remove_service_from_node()`: cleanup
  methods for clearing stale container placements when a node dies.
- Node recovery: when a previously "dead" node resumes sending heartbeats, the heartbeat handler
  automatically proposes `UpdateNodeStatus { status: "ready" }` to recover it.
- Internal authentication token is now generated during daemon init and shared between the
  scheduler and API InternalState, ensuring consistent auth across the system.
- `Runtime::get_container_port_override()` trait method: allows runtimes to report a
  dynamically assigned port for containers that share the host network. Default returns
  `None` (no override). macOS sandbox runtime returns the assigned port.
- `Container.port_override` field: stores the runtime-assigned port for proxy backend
  address construction.
- Seatbelt profiles now include the dynamically assigned port in their network bind rules.
- Stabilization and endpoint reporting (Phase 11):
  - `wait_for_stabilization()` in daemon.rs: polls ServiceManager for replica count convergence and health checks with configurable timeout
  - `ServiceHealthSummary` / `StabilizationResult` types for structured stabilization outcomes
  - `DeploymentDetails` response now includes `service_health` array with per-service replica counts, health status, and endpoint URLs
  - `create_deployment` handler orchestrates when wired: parses spec, stores as Deploying, spawns async task to register services / setup overlay / configure proxy / scale, updates to Running or Failed
  - `delete_deployment` handler tears down services (scale to 0, remove) before deleting from storage
  - `DeploymentState.with_orchestration()` constructor wires ServiceManager, OverlayManager, and ProxyManager into the handler
  - `build_router_with_deployment_state()` router builder accepts pre-built DeploymentState for orchestration
  - Enhanced CLI deploy output: success shows per-service endpoints and replica counts from daemon, failure shows per-service status with health info
  - CLI poll loop uses live `service_health` data from daemon instead of spec-based estimates
  - Serve command wires orchestration handles into DeploymentState so API deployments are fully orchestrated
- Raft distributed scheduler integration (Phase 10):
  - `RaftCoordinator` integration in daemon init with `PersistentRaftStorage` (SQLite-backed)
  - `RaftService` background Raft RPC server for cluster communication
  - `add_member()` for dynamic cluster membership changes
  - `POST /api/v1/cluster/join` and `GET /api/v1/cluster/nodes` API endpoints with `ClusterApiState`
  - Join token validation with HMAC-based authentication
  - CIDR-aware IP allocation via `IpAllocator` for overlay addresses (collision-safe)
- Secrets wiring into container environment (Phase 8):
  - `BundleBuilder.with_secrets_provider()` / `with_deployment_scope()`: thread secrets provider through OCI bundle creation
  - `$S:secret-name` env vars resolved at container start time via `resolve_env_with_secrets`
  - Falls back to `$E:` (host env) resolution when no secrets provider is configured
  - `CredentialStore`: Argon2id-based API key authentication built on `PersistentSecretsStore`
  - `CredentialStore.validate()`, `create_api_key()`, `ensure_admin()` for credential lifecycle
  - Blanket `SecretsProvider` / `SecretsStore` impls for `Arc<T>` enabling shared ownership
  - Auth handler (`/auth/token`) validates against `CredentialStore` instead of hardcoded dev credentials
  - `AuthState.credential_store` / `ApiConfig.credential_store` for injecting credential store into API
  - Admin credential bootstrapped on first daemon start (random password logged once)
  - Unix socket connections auto-injected with admin JWT via `run_dual_with_local_auth()`
  - Local CLI-to-daemon IPC requires no explicit authentication
- TCP/UDP L4 proxy wiring (Phase 6):
  - `TcpStreamService.serve()`: standalone accept loop for TCP proxying without Pingora infrastructure
  - `UdpStreamService.serve()`: accepts externally-bound `UdpSocket` for UDP proxying
  - `ProxyManager` now handles TCP/UDP protocols in `ensure_ports_for_service()`: binds listeners, spawns stream proxy tasks
  - `ProxyManager.set_stream_registry()` / `with_stream_registry()`: wires L4 stream registry
  - `StreamService` health-aware backend selection: `BackendHealth` enum (Healthy/Unhealthy/Unknown), skips unhealthy backends in round-robin
  - `StreamRegistry.spawn_health_checker()`: background TCP connect probe (every 5s, 2s timeout)
  - Daemon wires `StreamRegistry` into both `ProxyManager` and `ServiceManager`
  - Health checker task integrated into daemon lifecycle (started on init, aborted on shutdown)
- Public vs Internal endpoint enforcement (Phase 7):
  - Public endpoints (`expose: public`) bind to `0.0.0.0` (all interfaces)
  - Internal endpoints (`expose: internal`) bind to the overlay IP, or `127.0.0.1` if no overlay is available
  - Defense-in-depth: proxy rejects non-overlay sources (outside `10.200.0.0/16`) for internal routes with HTTP 403
  - `OverlayManager.node_ip()` getter exposes the node's global overlay IP
  - Removed duplicate `ExposeType` enum from `zlayer-tunnel`; now imports from `zlayer-spec`
- Container logging (Phase 5):
  - Structured log paths: container stdout/stderr written to `/var/log/zlayer/{deployment}/{service}/{container_id}.{stdout,stderr}.log`
  - `YoukiConfig.log_base_dir` / `YoukiConfig.deployment_name`: configurable log directory hierarchy
  - Bundle symlinks: `{bundle}/logs/{stdout,stderr}.log` symlink to structured paths for backward compatibility
  - Daemon auto-configures Youki runtime with log directory and deployment name on Linux
  - Structured log rotation: hourly task rotates oversized files in `/var/log/zlayer/` (keeps last 10%)
  - Old log cleanup removes `.log` files older than 7 days from the structured log directory
- GPU inventory detection module (`zlayer-agent::gpu_detector`) that scans sysfs PCI devices for GPUs
  - Auto-detects vendor (NVIDIA, AMD, Intel) via PCI vendor IDs
  - Reads VRAM from PCI BAR regions, AMD `mem_info_vram_total`, or nvidia-smi
  - Reads GPU model names from DRM subsystem or nvidia-smi
  - Resolves device paths (`/dev/nvidiaN`, `/dev/dri/cardN`, `/dev/dri/renderDN`)
- GPU fields on `NodeResources` in scheduler placement: `gpu_total`, `gpu_used`, `gpu_models`, `gpu_memory_mb`, `gpu_vendor`, and `gpu_available()` helper
- `GpuInfoSummary` struct and `gpus` field on `NodeInfo` in Raft cluster state for distributed GPU awareness
- GPU detection runs at node init and node join, with results logged and displayed
- `zlayer exec` command: batch command execution in running containers via the daemon API
- `zlayer ps` command: lists deployments, services, and containers via the daemon API
  - Supports `--deployment` filter, `--containers` flag for replica detail, `--format` (table/json/yaml)
  - Container listing API endpoint: `GET /api/v1/deployments/{name}/services/{service}/containers`
- Deploy TUI with ratatui-based interactive progress display for `zlayer deploy` and `zlayer up`
  - Phase-aware layout: deploying, running dashboard, and shutdown views
  - Infrastructure phase progress indicators (spinners, checkmarks)
  - Per-service deployment status with replica counts and health indicators
  - Scrollable log pane capturing warnings and errors
  - `--no-tui` flag for CI/non-interactive environments with clean PlainDeployLogger fallback
- `restore_deployments()`: automatic deployment recovery on daemon restart from SQLite storage
- `DaemonClient`: HTTP-over-Unix-socket client with auto-start daemon and exponential backoff retry
- `load_or_init_node_config()`: node configuration auto-initialization with WireGuard key generation on first run
- Overlay tunnel setup on node join: configures WireGuard peers, TUN interface, and persists bootstrap state for daemon reload
- Shared stabilization module: `wait_for_stabilization()` moved to `zlayer-agent` crate for reuse across daemon and API
- Deployment delete cleanup: `delete_deployment` handler tears down overlay, proxy routes, and service replicas before removing storage
- DNS handle lifecycle: `DnsHandle` kept alive in `DaemonState` for runtime record management
- Glob-based spec file discovery: `zlayer deploy` now finds `*.zlayer.yml` and `*.zlayer.yaml` files
- Pipeline file auto-discovery: accepts `ZPipeline.yaml` or `zlayer-pipeline.yaml` (no longer requires `-f` flag)
- Per-crate log filtering: default verbosity reduced to WARN for internal crates, use `-v` for old behavior

### Changed
- zlayer-consensus: replaced serde_json round-trip in state machine apply with direct
  `EntryPayload` matching (~10x faster apply). Switched to consistent bincode serialization
  for snapshots. Removed unnecessary serde_json dependency.
- Replaced kernel WireGuard with boringtun (Cloudflare's Rust userspace WireGuard implementation) for overlay networking
  - No longer requires WireGuard kernel module (`modprobe wireguard` / kernel 5.6+)
  - No longer requires `wireguard-tools` package (`wg` binary)
  - Uses TUN device via `/dev/net/tun` (universally available on Linux)
  - Still requires CAP_NET_ADMIN or root for TUN device creation
  - Renamed `WireGuardManager` to `OverlayTransport`
  - All configuration now done via UAPI protocol (no external binary dependencies)
- `zlayer manager init` now creates `manager.zlayer.yml` instead of `.zlayer.yml`
- Deploy output uses event-driven architecture (DeployEvent channel) instead of mixed println/tracing
- API server default port changed from 8080 to 3669
- Overlay network failure is now fatal when services require networking (use `--host-network` to bypass)

### Fixed
- zlayer-consensus: `handle_full_snapshot` was a no-op (snapshot data was received but never
  installed into the state machine). Now correctly deserializes and installs snapshot state.
- zlayer-consensus: `snapshot_logs_since_last` and `enable_prevote` config settings were silently
  ignored during Raft node initialization. Both are now applied to the openraft `Config`.
- Overlay container attachment: added platform guard (`#[cfg(not(target_os = "linux"))]`) to
  `OverlayManager::attach_container()` so non-Linux platforms skip veth/nsenter commands and
  return the node's overlay IP directly, eliminating noisy error logs on macOS.
- macOS sandbox networking: multiple replicas of the same service no longer conflict on ports.
  Each sandbox container is assigned a unique dynamic port (via OS port-0 allocation), passed
  to the process as `PORT` / `ZLAYER_PORT` environment variables. The proxy routes to each
  replica's unique `127.0.0.1:{assigned_port}` backend address instead of all replicas sharing
  the same port. Port guard listeners prevent TOCTOU races between `create_container()` and
  `start_container()`.
- Proxy backend registration: containers are now registered with the load balancer on startup (previously backends were never added, causing "No healthy backends" errors)
- Health check target address: TCP and HTTP health checks now connect to the container's overlay IP instead of 127.0.0.1
- Error reporting now includes service names and error details in deploy output
- Youki runtime reads image CMD, ENTRYPOINT, Env, WorkingDir, and User from OCI image config
- Docker runtime cleans up stale containers before re-deploy
- Integration tests un-ignored in `youki_e2e.rs`
