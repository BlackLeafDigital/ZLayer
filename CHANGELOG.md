# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
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
- Proxy backend registration: containers are now registered with the load balancer on startup (previously backends were never added, causing "No healthy backends" errors)
- Health check target address: TCP and HTTP health checks now connect to the container's overlay IP instead of 127.0.0.1
- Error reporting now includes service names and error details in deploy output
- Youki runtime reads image CMD, ENTRYPOINT, Env, WorkingDir, and User from OCI image config
- Docker runtime cleans up stale containers before re-deploy
- Integration tests un-ignored in `youki_e2e.rs`
