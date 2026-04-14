# Changelog

All notable changes to this project will be documented in this file.

## [0.10.76]

### Added
- **`zlayer` system group provisioned during `daemon install` (Linux).**
  `zlayer daemon install` now creates a `zlayer` system group (via `groupadd
  --system`), adds `$SUDO_USER` to it (via `usermod -aG`), and chgrps +
  chmods the shared build-facing data directories — `registry`, `cache`, and
  `bundles` — so unprivileged users can run `zlayer build` against the
  system-wide data dir without sudo. Secrets, raft, and live container
  runtime state (containers/rootfs/volumes) stay root-only. Applied before
  the `systemctl daemon-reload` call so group setup still takes effect on
  WSL distros with systemd disabled. Membership in `zlayer` is
  root-equivalent on the host (same trust model as the `docker` group). No
  change on macOS (single-user data dir) or Windows.

### Fixed
- **`zlayer build` silently skipped local-registry import on permission
  errors.** When the builder's local-registry import step hit EACCES —
  typically because an unprivileged user ran `zlayer build` against the
  root-owned `/var/lib/zlayer/registry` — it logged a warning and returned
  success. The daemon then couldn't find the just-built image locally and
  fell through to Docker Hub, producing confusing 401s for images that were
  never supposed to leave the box. The import step now returns a hard error
  that names the registry path and the underlying cause.
- **`zlayer-manager` image build failed copying `hash.txt`.** The 0.10.75
  fix copied from `target/hash.txt`, but cargo-leptos 0.3.x writes `hash.txt`
  next to the compiled binary (`target/release/hash.txt`).
  `ZImagefile.zlayer-manager` now copies from the correct path, verified
  locally by running `cargo leptos build --release` and inspecting the
  output.
- **`cargo install cargo-leptos` in zlayer-manager/zlayer-web images broke
  when `core2 0.4.0` was yanked.** Fresh semver resolution picked the
  yanked version transitively via `libflate`. Both Dockerfiles and
  ZImagefiles now use `cargo install cargo-leptos --locked`, which reuses
  the lockfile shipped with cargo-leptos and avoids the yank entirely.

## [0.10.75]

### Added
- **Interactive deploy TUI.** `zlayer deploy` and `zlayer up` now render the
  existing ratatui-based deploy TUI by default on a TTY. The TUI animates the
  deployment plan, infra status panel, per-service lifecycle (Registering →
  Scaling → Running or Failed), and a scrollable log pane. SSE events from
  the daemon are translated into structured `DeployEvent`s so the panels
  populate in real time. Ctrl+C inside the TUI detaches cleanly (the daemon
  keeps running). `--no-tui` (already a global flag) falls back to the plain
  logger; dry-run and non-TTY stdout (CI/piped) also stay plain.

## [0.10.73]

### Fixed
- **zlayer-manager white screen in production.** Container images were missing
  `LEPTOS_OUTPUT_NAME`, `LEPTOS_SITE_PKG_DIR`, and `LEPTOS_HASH_FILES` runtime
  env vars, so Leptos SSR generated unhashed script paths that didn't match the
  hashed files built by cargo-leptos. Also added `assets-dir` to `Leptos.toml`
  so the logo and other static assets are copied into the site directory.
- **zlayer-web same missing env vars.** Applied the same runtime env var fix to
  the zlayer-web container images.

### Added
- **ZLayer Desktop (Tauri v2).** Native desktop app wrapping the zlayer-manager
  web UI in a Tauri webview with an embedded SSR server. All Leptos server
  functions work unchanged — the Tauri app spawns the Axum server on a random
  localhost port and navigates the webview to it. New crate: `bin/zlayer-desktop`.
- **`zlayer_manager::server` module.** Extracted server startup logic from
  `main.rs` into a reusable library function so both the CLI binary and the
  Tauri desktop app can start the embedded server.
- **Instance management.** The manager can now connect to multiple remote
  ZLayer instances. Added an instance list UI in Settings with add/edit/delete/
  switch and a "Test Connection" button. Active connection shown in sidebar.
  Instances persist to `~/.config/zlayer/instances.json`.

### Changed
- **zlayer-manager default port 9120 → 6677.** Port 9120 conflicted with
  Komodo. Updated CLI default, Leptos config, container images, deployment
  spec generator, and documentation.

## [0.10.72]

### Fixed
- **`zlayer status` reports "not running" when daemon runs as root via
  systemd.** The Unix socket at `/var/run/zlayer.sock` was created with
  mode `0o660` (owner+group only). When the daemon runs as root, the
  socket is owned by `root:root`, so non-root users cannot stat it and
  `zlayer status` thinks nothing is there. Changed to `0o666`; access
  control is already handled by the local auth token.
- **`zlayer daemon status` reports "not installed" on SELinux/Fedora
  Atomic systems.** The `detect_service_level()` function tried to stat
  `/etc/systemd/system/zlayer.service` to decide system vs user service,
  but on SELinux-confined systems non-root users cannot stat files in
  that directory. The fallback returned `User`, causing all `systemctl`
  calls to use `--user` (wrong namespace). Removed the dead
  `ServiceLevel::User` code path entirely — on Linux, zlayer is always
  installed as a system service.

## [0.10.71]

### Fixed
- **`zlayer deploy` keeps serving stale `:latest` images after a new
  release is pushed.** The end-to-end pull path had three independent
  cache bugs that all had to be fixed together. First, the
  `zlayer-registry` manifest cache was keyed by tag (`manifest:<image>`)
  in a persistent ZQL-backed store with no TTL and no revalidation, so
  any cached mutable-tag manifest was served forever across daemon
  restarts. `pull_manifest` now revalidates mutable tags (`:latest`,
  `:dev`, `:edge`, `:main`, `:master`, empty/missing tag) via
  `HEAD /v2/{repo}/manifests/{tag}` and compares the upstream digest
  against a new `manifest-digest:<image>` sidecar entry; on mismatch the
  stale cache is invalidated before the refetch. Registry-unreachable
  errors fall back to the cached copy with a warning so offline deploys
  still work. Pinned tags and digest refs keep their fast-path behavior.
  Second, `YoukiRuntime::pull_image` hard-coded `PullPolicy::IfNotPresent`,
  dropping any `pull_policy: always` the user set in their spec before
  it reached the puller; `pull_image_with_policy` and `pull_image_layers`
  now thread the spec's policy all the way through to the registry
  client via the new `ImagePuller::pull_image_with_policy(...,
  force_refresh)` entry point, which invalidates the manifest cache
  when `force_refresh` is true. Third,
  `DockerRuntime::pull_image_with_policy` trusted `docker inspect_image`
  alone to short-circuit `IfNotPresent` pulls, never consulting the
  registry; it now compares the local image's `repo_digests` entry
  against the upstream digest returned by bollard's
  `inspect_registry_image` (the Docker distribution API) and re-pulls
  on mismatch.
- **`zlayer build` / `zlayer-build` reuse stale base images.**
  `buildah from` was invoked with no `--pull` flag, so any upstream
  republish of the base image was invisible to subsequent local builds
  until `buildah rmi` was run manually. Builds now default to
  `--pull=newer`, matching modern build-tool conventions: fast when
  nothing has changed, correct when the registry has a newer copy.
  Controlled via the new `--pull=<newer|always|never>` and `--no-pull`
  flags on `zlayer build`.

### Added
- **`zlayer image ls`, `zlayer image rm <image>`, `zlayer system prune`
  subcommands.** First-class image management from the CLI via new
  `/api/v1/images`, `/api/v1/images/{image}`, and `/api/v1/system/prune`
  REST endpoints on the daemon, backed by new
  `Runtime::list_images`, `Runtime::remove_image`, and
  `Runtime::prune_images` trait methods. Implemented for the Docker
  runtime (via bollard's image-management APIs) and the Youki runtime
  (walks the persistent `zlayer-registry` cache to remove manifest
  entries and their referenced layer blobs, and prunes orphaned
  content-addressed blobs). Other runtimes inherit a default
  "unsupported" error. Users can now force a fresh pull of a stale
  `:latest` image with `zlayer image rm <image>` + redeploy instead of
  manually wiping the persistent cache directory.

## [0.10.70]

### Fixed
- **Deploy "Stabilization timed out" no longer masks the real container
  failure.** When a container's init process crashed during startup (bad
  image, missing libs, failed mount), the overlay attach code would run
  against the now-dead PID and emit misleading `RTNETLINK answers: No such
  process` / `File exists` / `Invalid "netns" value` errors. The user then
  saw a generic `Stabilization timed out: N/N replicas, healthy=false`
  with no pointer to the root cause. The agent now (a) checks the
  container's state between `start_container` and overlay attach, (b)
  returns the container's log tail when the init already exited, and
  (c) includes each failing service's recent log lines in the
  stabilization timeout error itself.
- **Veth leak from failed overlay attaches.** Each failed
  `attach_to_interface` used to leave a `veth-<pid>` pair (one end named
  `eth0`) orphaned in the host network namespace. The next deploy's
  `ip link add ... peer name eth0` then failed with RTNETLINK "File
  exists", cascading across redeploys. The overlay manager now (a) creates
  the container-side veth with a unique name (`vc-<pid>`) and renames it
  to `eth0` only after moving into the container's netns, (b) deletes
  the pair on any attach-path failure, and (c) sweeps orphan veth pairs
  whose owning PID is no longer alive before each new attach.
- **`zlayer-manager` and `zlayer-web` Dockerfiles fail at runtime with
  `version 'GLIBC_2.38' not found`.** The cargo-chef builder
  (`lukemathwalker/cargo-chef:latest-rust-1.90`) runs on Debian trixie
  (glibc 2.41), but the Dockerfile runtime stage was pinned to
  `debian:bookworm-slim` (glibc 2.36). Binaries referencing GLIBC_2.38+
  symbols would not start. The Dockerfile runtime stages now use
  `debian:trixie-slim` to match the builder glibc. The ZImagefile variants
  are unaffected because they build on `rust:1.90-bookworm` (glibc 2.36)
  and already target `bookworm-slim`.

## [0.10.66]

### Fixed
- **Container creation "failed to prepare rootfs" on Linux.** libcontainer 0.5.7
  used `is_file()` to decide bind mount destination type, which returned false
  for Unix sockets — causing the ZLayer API socket mount to create a directory
  instead of a file, failing with EINVAL. Upgraded to libcontainer git rev
  `a68a38c4` (youki-dev/youki#3484) which uses `!is_dir()` instead.
- **Improved container creation error reporting.** libcontainer errors now use
  Debug formatting to preserve the full error chain, so mount failures show
  the actual syscall error (e.g., EINVAL, EPERM) instead of just
  "failed to prepare rootfs".

### Changed
- **Socket bind mount uses `typ("bind")`** instead of `typ("none")` in the OCI
  spec, ensuring libcontainer's bind mount code path handles the source
  correctly.
- **Set `rootfsPropagation` to `"private"`** in OCI spec (matches Docker default).

### Added
- **`scripts/install-dev.sh`** — builds from source and installs like `install.sh`
  but reports `0.0.0-dev`. Run `install.sh` to go back to a release build.

## [0.10.65]

### Fixed
- **SELinux 203/EXEC on Fedora/RHEL.** Binaries installed under
  `/var/lib/zlayer/bin` inherited `var_lib_t`, which systemd's `init_t` cannot
  exec as a service entrypoint, so `systemctl start zlayer` failed with
  `status=203/EXEC`. `install.sh` now relabels the binary to `bin_t` via
  `semanage fcontext` + `restorecon` (persistent) with `chcon` as a fallback
  for systems without `policycoreutils-python-utils` (e.g., Fedora Silverblue).
- **Daemon startup failure now shows current error, not stale logs.** Daemon log
  files are truncated before each start so failure diagnostics only contain
  output from the current attempt. Switched from `RotationStrategy::Never` to
  `Daily` rotation with 7-day retention. Added `--since=-2min` to the
  journalctl fallback query on Linux. Previously, `zlayer daemon install/start`
  could print week-old tracing entries from previous attempts.
- **Systemd unit upgraded to `Type=notify`** with `sd_notify(READY=1)` via the
  pure-Rust `sd-notify` crate, matching Docker/containerd/CRI-O. `systemctl
  start zlayer` now blocks until the daemon is truly ready (all init phases
  complete, API socket bound) instead of returning immediately.
- **Fixed daemon.json / socket bind race condition.** `daemon.json` was written
  ~300 lines before the API socket bound, so the CLI could think the daemon was
  ready while the socket wasn't listening yet.

### Changed
- **Install script now auto-configures PATH.** When the install directory is not
  already on PATH, the user is prompted (Y/n) and the script writes
  `/etc/profile.d/zlayer.sh` (Linux) or `/etc/paths.d/zlayer` (macOS).
- **Install script reports installed vs target version.** Shows both versions
  before downloading and prompts before reinstalling the same version.

### Added
- **Networks management page** in ZLayer Manager UI at `/networks`: full CRUD
  interface for network access-control policies. Displays stats row (total
  networks, members, rules, active policies), networks table with View/Delete
  actions, detail modal showing CIDRs as badges, members table with kind badges,
  and access rules table with allow/deny badges. Create modal accepts name,
  description, and comma/newline-separated CIDRs. Delete confirmation modal
  included. API client methods (`list_networks`, `get_network`,
  `create_network`, `delete_network`) and Leptos server functions added. Sidebar
  updated with "Networks" link in the Network section.
- **Network policy enforcement in reverse proxy**: the proxy now evaluates
  `NetworkPolicySpec` access rules against incoming requests. When a source IP
  belongs to a network with defined policies, access is allowed or denied based
  on service/deployment/port rules (deny takes priority, default deny when
  governed). The `NetworkPolicyChecker` is shared between the API layer and
  the proxy so that policy changes via the Networks API take effect immediately.
- **Networks API** (`/api/v1/networks`): CRUD endpoints for network
  access-control groups. Supports creating, listing, getting, updating, and
  deleting `NetworkPolicySpec` objects that define membership (users, groups,
  nodes, CIDRs) and service access rules. Includes `ip_matches_network()`
  helper for CIDR-based access matching.
- **`ServiceNetworkSpec` rename**: the per-service overlay/join-policy config
  formerly named `NetworkSpec` is now `ServiceNetworkSpec` to avoid collision
  with the new standalone `NetworkPolicySpec` type.
- **Reverse Proxy management page** in ZLayer Manager UI at `/proxy`: displays
  proxy stats (total routes, healthy/total backends, TLS certificates, active
  streams), routes table with expandable backend details and health badges, TLS
  certificates table with expiry warnings for certificates expiring within 7
  days, and stream proxies table with TCP/UDP protocol badges. Sidebar updated
  with "Reverse Proxy" and "SSH Tunnels" links in the Network section.
- **Proxy management API client and server functions** in ZLayer Manager:
  adds `ProxyRoute`, `ProxyBackendGroup`, `TlsCertificate`, and `StreamProxy`
  types to the API client with methods `list_proxy_routes()`,
  `list_proxy_backends()`, `list_tls_certificates()`, and
  `list_stream_proxies()`. Corresponding Leptos server functions
  (`get_proxy_routes`, `get_proxy_backends`, `get_tls_certificates`,
  `get_stream_proxies`) provide data access for the proxy management UI page.
- **Nodes management page** in ZLayer Manager: replaces the "Coming Soon" stub
  with a live cluster node table sourced from the Raft cluster state. Shows
  stats row (total nodes, healthy count, leader, cluster health), a table with
  node ID, address, role (leader/voter/learner badge), overlay IP, status
  (ready/draining/dead badge), CPU, and memory. Includes a "Generate Join
  Token" card when a leader is present.
- `ClusterNodeSummary` in `/api/v1/cluster/nodes` now returns enriched data:
  `advertise_addr`, `overlay_ip`, `cpu_total`, `cpu_used`, `memory_total`,
  `memory_used`, `registered_at`, `last_heartbeat`, `role`, and `mode`.
- `list_cluster_nodes()` method on `ZLayerClient` API client for calling
  `GET /api/v1/cluster/nodes`.
- Dashboard `get_system_stats()` now reports real node counts and aggregate
  CPU/memory usage from cluster state instead of hardcoded zeros.
- **Service endpoint display** in deployment detail modal: each service row now
  shows its configured endpoints (protocol, port, URL) in an expandable sub-table.
  Protocol badges use DaisyUI color coding (HTTP=info, HTTPS=success,
  WebSocket=secondary, gRPC=accent, others=ghost). Services with no endpoints
  show "No endpoints configured".
- `ServiceEndpoint` type in Manager server functions for passing endpoint data
  from the API to the UI.
- `get_services()` now fetches per-service details to include endpoint
  information alongside the service summary data.

## [2026-04-03]

### Added
- Theme toggle button in Manager navbar: toggles between "dark" and "zlayer"
  (light) DaisyUI themes with localStorage persistence across page refreshes.
  Sun icon in dark mode, moon icon in light mode.
- Structured logging: `LogEntry`, `LogStream`, `LogSource`, `LogQuery` types,
  `FileLogWriter`/`MemoryLogWriter`, `FileLogReader`/`apply_query()`,
  `LogOutputConfig`/`LogDestination`, `LogsConfig` on `ServiceSpec`.
- `max_files` on `FileLoggingConfig` for rotated log cleanup.
- Daemon log rotation via `tracing-appender` with daily rotation.

### Changed
- `DEFAULT_WG_PORT` moved to `zlayer-core` for cross-platform availability.
- `Runtime` trait: `container_logs()`/`get_logs()` return `Vec<LogEntry>`.
  All runtimes (youki, docker, macOS sandbox, WASM) updated.
- `ServiceManager::get_service_logs()` returns `Vec<LogEntry>`.
- Daemon systemd unit uses `StandardOutput=journal`; daemon manages files via
  `tracing-appender`.
- Fresh daemon log on every start; `wait_for_daemon_ready` reads newest file.
- Log rotation handles `.jsonl` + `executions/` cleanup.
- Raft bootstrap skips `initialize()` on already-initialized nodes.
- `install.sh` cleanup: stale interfaces, UAPI sockets, port-holding processes.

## [2026-04-02]

### Changed
- Default WireGuard overlay port changed from 51820 to 51420 to avoid
  conflicts with system WireGuard VPNs.
- Replaced all hardcoded `51820` port literals with `DEFAULT_WG_PORT` constant
  and `node_config.overlay_port` configuration throughout the codebase.
- Daemon startup WireGuard port cleanup: replaced passive polling with active
  process identification and termination. Reads overlay port from
  `node_config.json` instead of hardcoding, identifies the port holder via
  `ss`/`lsof`, and sends SIGTERM/SIGKILL to stale zlayer/boringtun processes.
- `OverlayManager` now accepts a configurable overlay port via
  `with_overlay_port()` builder method instead of hardcoding.
- Raft bootstrap on restart now checks metrics before calling `initialize()`,
  suppressing the noisy openraft ERROR log on already-initialized nodes.
- `install.sh`: expanded cleanup on upgrade — removes stale `zl-*` network
  interfaces, WireGuard UAPI sockets, and kills stale port-holding processes.

## [2026-04-01]

### Added
- New `zlayer-docker` crate: Docker CLI, Compose, and API socket compatibility layer.
  - `compose` module: Parse `docker-compose.yaml` files and convert to ZLayer `DeploymentSpec`.
    Handles ports (short/long syntax), volumes (bind/named/tmpfs), environment (map/list),
    depends_on (with conditions), healthchecks, deploy resources/replicas, and more.
  - `cli` module: Docker-compatible CLI subcommands (`zlayer docker run/ps/stop/build/compose/etc.`)
    with full argument parsing mirroring Docker CLI flags.
  - `socket` module: Docker Engine API v1.43 socket emulation over Unix domain socket.
    Stub endpoints for containers, images, volumes, networks, and system info.
  - 104 unit tests + 5 integration tests covering compose parsing, conversion, and round-trips.
  - Example `docker-compose.yaml` in `examples/` (nginx + API + postgres + redis stack).
  - Wired into `bin/zlayer` as `zlayer docker` subcommand (feature-gated: `docker-compat`).
- `zlayer daemon install --docker-socket`: opt-in Docker API socket at `/var/run/docker.sock`
  and Docker CLI shim at `/usr/local/bin/docker` → `zlayer docker`.
- `zlayer serve --docker-socket`: spawns Docker API socket server alongside the daemon.

## [2026-03-31]

### Fixed
- `install.sh` falling back to `~/.local/bin` which breaks `daemon install` on
  immutable distros (Fedora Atomic/Silverblue/uBlue). Now write-probes
  `/usr/local/bin` (k3s pattern), falls back to `/opt/bin`. Binary always lands
  in a system path accessible to systemd.
- `pick_system_binary_path()` last-resort fallback returning `/opt/zlayer/bin/zlayer`
  without creating the directory, causing ENOENT on `std::fs::copy`.
- Sandbox `copy_dir_recursive` failing on symlinks (common in macOS Homebrew images),
  breaking secondary tag creation (`:latest`). Symlinks are now preserved instead of
  falling through to `tokio::fs::copy`.
- Crate publishing failing for all crates: `zlayer-wsl` had a hardcoded `version = "0.0.0-dev"`
  instead of `version.workspace = true`, causing workspace resolution to fail after the
  release sed version bump.
- Sandbox backend push failure for multi-tag images ("rootfs not found"): secondary
  tags now get on-disk directories via `tag_image()` before the push phase runs.
- Sandbox backend tar archive failure on macOS ("No such file or directory"): tar
  builder now preserves symlinks instead of following them, fixing broken Homebrew
  symlinks and producing correct OCI layers.
- Build workflow `upload-temp-packages` output: `package_base_url` is now set
  unconditionally so re-runs don't pass an empty artifact URL to the release workflow.

## [2026-03-30]

### Added
- `host` field on `EndpointSpec` for host-based/subdomain proxy routing
  (e.g. `host: "api.example.com"` or `host: "*.example.com"`), wired into
  `RouteEntry::from_endpoint()` so specs can declare host patterns directly.
- GPU model affinity: `model` field on `GpuSpec` pins workloads to nodes with
  a matching GPU model (substring match, e.g. `model: "A100"`).
- GPU device index tracking: scheduler now allocates specific GPU indices per
  container, preventing device overlap when multiple containers share a node.
  `PlacementDecision` carries `gpu_indices` for downstream device injection.
- GPU environment variable injection: containers automatically receive
  `NVIDIA_VISIBLE_DEVICES`/`CUDA_VISIBLE_DEVICES` (NVIDIA),
  `ROCR_VISIBLE_DEVICES`/`HIP_VISIBLE_DEVICES` (AMD), or
  `ZE_AFFINITY_MASK` (Intel) based on vendor and allocated indices.
- Gang scheduling (`scheduling: gang` on `GpuSpec`): all-or-nothing placement
  for distributed GPU jobs — if any replica cannot be placed, all are rolled back.
- Distributed job coordination (`distributed` on `GpuSpec`): injects
  `MASTER_ADDR`, `MASTER_PORT`, `WORLD_SIZE`, `RANK`, `LOCAL_RANK`, and
  backend-specific env vars (`NCCL_SOCKET_IFNAME`/`GLOO_SOCKET_IFNAME`).
- GPU spread scheduling (`scheduling: spread`): distributes GPU workloads across
  nodes instead of bin-packing them onto the fewest nodes.
- GPU sharing modes (`sharing` on `GpuSpec`): `mps` for NVIDIA Multi-Process
  Service (up to 8 containers per GPU) and `time-slice` for round-robin sharing
  (up to 4 containers per GPU). Fractional GPU tracking in scheduler.
- `MpsDaemonManager` in `zlayer-agent` for reference-counted NVIDIA MPS daemon
  lifecycle management (auto-start on first MPS container, auto-stop on last).
- GPU utilization in heartbeat: `GpuUtilizationReport` struct with per-GPU
  utilization %, memory, temperature, and power reported via Raft heartbeat.
- GPU metrics collection module (`gpu_metrics`): portable metrics via
  `nvidia-smi` (NVIDIA), sysfs (AMD/Intel) — no hard driver dependencies.
- GPU health monitoring: detects thermal throttling, ECC errors, and
  unresponsive GPUs via `check_gpu_health()`.
- GPU metrics in `zlayer-observability`: utilization (percent), memory used/total
  (bytes), temperature (celsius), and power draw (watts). Each metric is a
  Prometheus `GaugeVec` with `gpu_index` and `node` labels.
- CDI (Container Device Interface) support: discovers specs from `/etc/cdi/`
  and `/var/run/cdi/`, resolves fully-qualified device names, merges container
  edits, and can generate NVIDIA specs via `nvidia-ctk`.
- macOS sandbox backend now supports `push_image`, `tag_image`, `manifest_create`,
  `manifest_add`, and `manifest_push` operations. Previously, pipeline builds on
  macOS succeeded but push/manifest operations always failed with "Operation 'push'
  is not supported by this backend". The sandbox backend now tars the rootfs, builds
  OCI manifests, and pushes via the registry client (requires the `cache` feature,
  which the `zlayer` CLI already enables). Multi-platform manifest lists are stored
  on disk and pushed as OCI image indexes.
- `ImagePuller::push_image_index_to_registry` method for pushing OCI image indexes
  (manifest lists) to remote registries.

## [2026-03-29]

### Fixed
- `zlayer-paths` crate now publishable (removed `publish = false`) and added to
  CI release publish order so crates depending on it can resolve it from the registry.
- `daemon install` no longer fails with "Text file busy" (ETXTBSY) when the old
  binary is still running via systemd. The install function now stops the service
  and unlinks the destination before copying.
- `admin_password` file permissions are now enforced to 0o644 on every daemon
  startup, not just on initial creation. Previously, files created with the old
  0o600 permissions stayed unreadable to non-root tools (E2E tests, CLI) forever.
- `DockerConfigAuth` now respects the `DOCKER_CONFIG` environment variable
  (standard Docker convention) for locating `config.json`.

## [2026-03-28]

### Fixed
- Daemon startup failures now produce diagnostic output instead of
  "no log output found at /var/log/zlayer/daemon.log". The systemd unit
  template now redirects stdout/stderr to the daemon log file via
  `StandardOutput=append:` and `StandardError=append:` directives. The log
  directory is pre-created before starting the service. On timeout,
  `wait_for_daemon_ready()` falls back to querying journalctl and provides
  a diagnostic hint.

## [2026-03-27]

### Fixed
- Install scripts (`install.sh`, `install.py`) now install `libseccomp` runtime
  library, verify cgroups v2, and create `/var/lib/zlayer/` state directories.
  Previously, fresh installs would fail to start containers because the bundled
  libcontainer runtime requires libseccomp at runtime but nothing installed it.
- Container, job, cron, and build API routes were missing the `AuthState`
  extension because it was applied inside `build_router_with_deployment_state()`
  before routes were nested in `serve.rs`. The `Extension(auth_state)` layer is
  now re-applied after all `.nest()` calls so every route receives auth context.
- `install.sh`: daemon install output was silently discarded (`>/dev/null 2>&1`).
  Now shows output so users can see what happened. Linux installs use `sudo`,
  macOS installs do not. Removed redundant post-install status checks that
  duplicated the daemon's own readiness reporting.
- Linux `daemon install`: `systemctl daemon-reload` and `systemctl enable` exit
  codes were silently discarded (`let _ = ...`). They now propagate errors and
  bail with the stderr message on failure.
- Non-deterministic sandbox builder cache hash. `compute_dockerfile_hash()` used
  `format!("{:?}", dockerfile)` which produced different hashes across runs due to
  `HashMap` randomized iteration order. Replaced with deterministic per-instruction
  `cache_key()` hashing. Also fixed `SandboxBackend::build_image()` not forwarding
  `options.source_hash` to `SandboxImageBuilder`, so pipeline-provided hashes were
  being silently ignored.

### Added
- Content-based cache invalidation for the sandbox builder. `SandboxImageConfig`
  now carries a `source_hash` field (SHA-256 of the Dockerfile/ZImagefile). When
  a cached image exists with a matching hash, the rebuild is skipped entirely.
  Pipeline builds (`build_single_image`) also compute a file hash up front and
  short-circuit before even creating an `ImageBuilder` when the output image is
  unchanged. `BuildOptions` and `ImageBuilder` gain a `source_hash` setter.
  `build_toolchain_as_image()` and `build_base_image()` in `macos_image_resolver`
  now embed source hashes in their cached configs.
- Multi-version builds in `scripts/build-macos-images.sh` — e.g.
  `./scripts/build-macos-images.sh node 18 20 22 24` builds Node 18, 20, 22, 24.
  Partial versions auto-resolve to latest patch via upstream APIs. Environment
  variable overrides (`GO_VERSION`, `NODE_VERSION`, `PYTHON_VERSION`, etc.) take
  precedence over API resolution. Output directories are now versioned
  (`$BUILD_DIR/golang-1.23.6/rootfs/`). OCI config.json includes version labels.
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

### Refactored
- Refactored package installer in `macos_image_resolver` to use a `ResolvedPackage`
  enum (`HomebrewBottle`, `DirectRelease`, `Tap`, `UvPython`) instead of
  shoehorning everything into `BrewFormulaInfo`. Renamed `fetch_formula_info()`
  to `resolve_package()`, `install_single_bottle()` to `install_package()`, and
  `fetch_and_extract_bottle()` to `install_with_deps()`. Added
  `install_direct_release()` for downloading and extracting forge release assets,
  `install_uv_python()` for Python provisioning via uv, and `DiscoveryResponse`
  for structured RepoSourceSyncer discovery. Dependency BFS walk now only recurses
  for `HomebrewBottle` variants. Added `"golang"` -> `("go", false)` to
  `map_single_package_hardcoded()`.
- Extracted the buildah build orchestration loop (stage walking, container
  creation, instruction execution, COPY --from resolution, cache tracking,
  commit, cleanup) from `ImageBuilder::build()` into
  `BuildahBackend::build_image()`. `ImageBuilder::build()` is now a thin
  coordinator: parse Dockerfile/ZImagefile/template, handle WASM early return,
  delegate to the backend. `LayerCacheTracker` moved to `backend/buildah.rs`.
  Removed inline `build_with_sandbox()`, `resolve_stages()`,
  `resolve_base_image()`, `create_container()`, `commit_container()`,
  `tag_image()`, `push_image()`, and `generate_build_id()` from `builder.rs`.
- Pipeline executor (`pipeline/executor.rs`) now always uses
  `ImageBuilder::with_backend()` instead of falling back to
  `ImageBuilder::with_executor()`, wrapping a bare executor in a
  `BuildahBackend` when no explicit backend is provided.

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
