# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

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

### Added
- `zlayer daemon` subcommand for service lifecycle management:
  - `install` — Register as system service (launchd on macOS, systemd on Linux)
  - `uninstall` — Remove service registration
  - `start` / `stop` / `restart` — Control the daemon
  - `status` — Show daemon and service registration status
  - `reset` — Wipe Raft state and node identity for clean reinitialization

- Windows compilation for the `zlayer` binary: moved Unix-only crate dependencies
  (`zlayer-agent`, `zlayer-overlay`, `zlayer-api`, `zlayer-proxy`, `zlayer-scheduler`,
  `zlayer-secrets`, `zlayer-storage`, `zlayer-init-actions`, `secrecy`) behind
  `[target.'cfg(unix)'.dependencies]`. Gate Unix-only modules (`daemon`, `daemon_client`,
  `config`) and command modules (`deploy`, `exec`, `join`, `node`, `ps`, `serve`, `token`)
  with `#[cfg(unix)]`. Disk detection in `resources.rs` stubbed to 0 on non-Unix platforms.
  Cross-platform commands (build, validate, pipeline, spec, wasm, tunnel, manager, registry)
  remain available on Windows. Runtime commands bail with a helpful error on Windows.
  CI Windows build changed from `--features wsl,docker` to `--features wsl`.
- CI: Windows build packaging replaced `pwsh`/`Compress-Archive` with `bash`/`7z`
  to fix `Cannot find: pwsh in PATH` on Forgejo runners without PowerShell.
- CI: Upload script now collects `.zip` artifacts (Windows builds) in addition to
  `.tar.gz`, fixing 404 errors when the release workflow tried to download them.
- CI: Release workflow sanitizes version numbers to strip leading zeros from
  semver components, preventing `invalid leading zero in patch version` cargo errors.
- CI: `[np]` flag in commit messages skips the auto-tag and publish workflow while
  still running checks, tests, and builds.
- CI: Release workflow cleans buildah storage before loading OCI archives to prevent
  stale overlay layer errors (`no such file or directory` on `buildah pull`).

### Added
- `zlayer daemon` CLI subcommand for managing the zlayer background service:
  `install`, `uninstall`, `start`, `stop`, `restart`, `status`, and `reset`.
  Uses launchd on macOS and systemd on Linux.
- Raw container lifecycle REST API endpoints under `/api/v1/containers` for direct
  container management independent of the deployment/service abstraction. Endpoints:
  `POST /` (create+start), `GET /` (list with label filter), `GET /{id}` (inspect),
  `DELETE /{id}` (stop+remove), `GET /{id}/logs` (tail or SSE stream),
  `POST /{id}/exec` (execute command), `GET /{id}/wait` (block until exit),
  `GET /{id}/stats` (CPU/memory stats). Includes OpenAPI documentation, JWT auth
  (operator role for mutating operations, viewer for read operations), and a new
  `ContainerApiState` + `build_router_with_containers()` router constructor.
- Multi-platform build support for ZPipeline. Optional `platforms` field on pipeline
  defaults and per-image config enables building for multiple architectures (e.g.,
  linux/amd64, linux/arm64). Multi-arch builds create OCI manifest lists via buildah.
  CLI `--platform` flag overrides platforms for all images. Requires `qemu-user-static`
  for cross-architecture emulation.
- Dynamic Raft voter management: the cluster now maintains an optimal odd number
  of voters for fault tolerance. The formula is `min(7, largest_odd <= eligible)`,
  so 1 node = 1 voter, 2 nodes = 1 voter + 1 learner (first-up-wins), 3 nodes =
  3 voters, 5 nodes = 5 voters, 7+ nodes = 7 voters with the rest as learners.
  Nodes with `mode=replicate` are always learners regardless of cluster size.
  When voters die, `rebalance_voters()` automatically promotes eligible learners
  to maintain the target count.
- `MemberRole` enum (`Voter`/`Learner`) returned from `RaftCoordinator::add_member()`.
  The cluster join response now includes a `role` field indicating the assigned role.
  `GET /api/v1/cluster/nodes` now shows "leader", "voter", or "learner" per node.
- `mode` field on `NodeInfo`, `AddMemberParams`, and `RegisterNode` request. Nodes
  joining with `mode=replicate` are always learners; `mode=full` nodes are eligible
  for voter promotion via the dynamic voter management logic.
- `RaftCoordinator::rebalance_voters()` method to adjust the voter set after node
  joins, deaths, or removals. Called automatically during `add_member()`,
  `remove_member()`, and the dead-node detection loop.
- `RaftCoordinator::remove_member()` method to fully remove a node from Raft
  membership and deregister it from the cluster state machine.
- `ConsensusNode` helper methods: `voter_ids()`, `voter_count()`, `learner_ids()`,
  `all_member_ids()` for querying Raft membership from OpenRaft metrics.
- `target_voters()` utility function for computing the optimal voter count.
- Force-leader disaster recovery feature for Raft consensus. When the original
  cluster leader is permanently lost, a surviving node can be promoted via
  `POST /api/v1/cluster/force-leader` or `zlayer node force-leader`. This saves
  the current cluster state to a recovery marker, shuts down Raft, and on daemon
  restart wipes Raft storage and re-bootstraps as a single-node leader replaying
  preserved service state. Safety checks prevent accidental use when the leader
  is still reachable (<30s quorum ack).
- `zlayer node force-leader` CLI subcommand for disaster recovery.
- NAT traversal Phase 4: Custom ZLayer relay protocol with BLAKE2b-256 authentication.
  - `RelayClient` (`nat/turn.rs`): Allocates relay addresses, creates permissions,
    and runs a local UDP proxy bridging WireGuard traffic through the relay server.
  - `RelayServer` (`nat/relay.rs`): Built-in UDP relay server with per-allocation
    relay ports, permission-based forwarding, and session management.
  - `RelayDiscovery` (`nat/discovery.rs`): Static configuration-based relay server
    discovery from `NatConfig::turn_servers`.
  - `NatTraversal` now gathers relay candidates after STUN reflexive candidates and
    refreshes relay allocations during periodic maintenance.
  - `OverlayBootstrap` optionally starts the built-in relay server during NAT
    traversal initialization when `relay_server` config is present.
- macOS overlay networking support: boringtun now creates `utun` devices on macOS
  via kernel control sockets (same mechanism as Tailscale CLI and wireguard-go).
  Interface configuration uses `ifconfig`/`route` instead of Linux `ip` commands.
  The kernel auto-assigns `utunN` names, which are discovered after creation via
  UAPI socket scanning. Requires `sudo` on macOS (equivalent to Linux `CAP_NET_ADMIN`).
- `OverlayTransport::interface_name()` getter to retrieve the resolved interface name
  (useful on macOS where the kernel assigns the actual `utunN` name).
- Platform-appropriate ping timeout args in overlay health checker (macOS `-W` uses
  milliseconds vs Linux seconds).
- Storage replication status API endpoint (`GET /api/v1/storage/status`): returns
  SQLite-to-S3 replication state including enabled flag, active/disabled/error status,
  last sync timestamp, and pending change count. Requires authentication (read-only).
- S3-backed volume sync for container named volumes (`zlayer-agent`, `s3` feature):
  `StorageManager` now accepts an optional `LayerSyncManager` for automatic backup/restore
  of named volumes to S3. Volumes are registered and restored from S3 on first creation,
  and synced on container stop via the new `Runtime::sync_container_volumes()` trait method.
  New methods: `set_layer_sync()`, `ensure_volume_with_sync()`, `sync_volume()`,
  `sync_all_volumes()`.

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

### Changed
- zlayer-consensus: replaced serde_json round-trip in state machine apply with direct
  `EntryPayload` matching (~10x faster apply). Switched to consistent bincode serialization
  for snapshots. Removed unnecessary serde_json dependency.

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

### Added
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
