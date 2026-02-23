# ZLayer Runtime Guide

## What ZLayer Is

A Rust-based container orchestration platform — 19 crates, 5 container runtimes, Raft consensus, overlay networking, L4/L7 proxy, WebAssembly support. The goal: `docker compose` simplicity with Kubernetes-level capabilities, zero daemon overhead on Linux (libcontainer direct), native macOS support via Seatbelt sandbox + APFS.

---

## Architecture

```
User
 │
 ├── zlayer (unified binary)  bin/zlayer         TUI, CLI, daemon, build — all in one
 └── zlayer-manager (web UI)  crates/zlayer-manager   Leptos SSR + WASM, port 9120
      │
      ▼
 zlayer-api (REST)            crates/zlayer-api       Axum, JWT, port 3669
      │
      ├── zlayer-agent        crates/zlayer-agent     Container lifecycle
      │    ├── youki           Linux libcontainer (no daemon)
      │    ├── macos_sandbox   Seatbelt + APFS clonefile
      │    ├── macos_vm        libkrun microkernel VMs
      │    ├── docker          Bollard SDK (cross-platform fallback)
      │    └── wasm            Wasmtime WASIp1/p2
      │
      ├── zlayer-scheduler    crates/zlayer-scheduler  Raft (openraft + SQLite)
      ├── zlayer-proxy        crates/zlayer-proxy      Pingora L4/L7, TLS, ACME
      ├── zlayer-overlay      crates/zlayer-overlay    WireGuard mesh, DNS discovery
      ├── zlayer-builder      crates/zlayer-builder    Dockerfile/ZImagefile builds
      ├── zlayer-registry     crates/zlayer-registry   OCI pull, blob cache (redb)
      ├── zlayer-secrets      crates/zlayer-secrets    ChaCha20-Poly1305, Vault
      └── zlayer-observability crates/zlayer-observability  OTLP + Prometheus
```

---

## Crate Inventory

| Crate | Purpose | Status |
|-------|---------|--------|
| `zlayer-spec` | Deployment YAML (v1 format) parsing | Working |
| `zlayer-core` | Shared types, OCI helpers | Working |
| `zlayer-agent` | Runtime trait + 5 implementations + service manager | Working |
| `zlayer-api` | REST API (Axum, JWT, rate limiting) | Working |
| `zlayer-scheduler` | Raft consensus (openraft, SQLite) | Working (86 tests) |
| `zlayer-proxy` | Pingora reverse proxy (L4/L7, TLS, ACME) | Working |
| `zlayer-overlay` | WireGuard mesh networking, DNS | Working on Linux, broken on macOS (no CAP_NET_ADMIN) |
| `zlayer-tunnel` | Secure tunneling (WebSocket + token auth) | Working |
| `zlayer-builder` | Dockerfile + ZImagefile + runtime templates | Working |
| `zlayer-registry` | OCI image pulling, persistent blob cache | Working |
| `zlayer-storage` | S3-backed layer persistence | Working |
| `zlayer-init-actions` | Pre-start hooks (TCP wait, HTTP probe, S3) | Working |
| `zlayer-secrets` | Secret encryption + Vault backend | Working |
| `zlayer-observability` | OpenTelemetry + Prometheus | Working |
| `zlayer-manager` | Web UI (Leptos SSR + DaisyUI) | Partial (read-only, some mock data) |
| `zlayer-web` | Alternative web UI | Exists |
| `zlayer-tui` | TUI widgets (ratatui) | Exists |
| `zlayer-py` | Python bindings (PyO3) | Exists |

---

## Runtime Trait (`crates/zlayer-agent/src/runtime.rs`)

Every runtime implements this async trait:

```rust
#[async_trait]
pub trait Runtime: Send + Sync {
    async fn pull_image(&self, image: &str) -> Result<()>;
    async fn pull_image_with_policy(&self, image: &str, policy: PullPolicy) -> Result<()>;
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()>;
    async fn start_container(&self, id: &ContainerId) -> Result<()>;
    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()>;
    async fn remove_container(&self, id: &ContainerId) -> Result<()>;
    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState>;
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<String>;
    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)>;
    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats>;
    async fn wait_container(&self, id: &ContainerId) -> Result<i32>;
    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>>;
    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>>;
    async fn get_container_port_override(&self, id: &ContainerId) -> Result<Option<u16>>;
}
```

Container states: `Pending → Initializing → Running → Stopping → Exited { code } | Failed { reason }`

---

## Container Lifecycle (what happens when you deploy)

### 1. Image Pull (`pull_image_with_policy`)
- `zlayer-registry::ImagePuller` resolves the image reference
- Auth via `AuthResolver` (Docker config, env vars)
- Layers downloaded to persistent blob cache (`redb`)
- Image config (entrypoint, cmd, env, workdir) cached separately
- Policy: `Always`, `IfNotPresent`, `Never`

### 2. Container Create (`create_container`)
Each runtime handles this differently:

**Youki (Linux):**
- Extracts cached layers → per-container rootfs at `bundles/{id}/rootfs/`
- Generates OCI `config.json` via `BundleBuilder` (capabilities, cgroups, namespaces, mounts)
- Creates container via libcontainer (no start yet)

**macOS Sandbox:**
- APFS clonefile from image rootfs → `containers/{id}/rootfs/` (copy-on-write, instant)
- Generates Seatbelt SBPL profile (filesystem, network, GPU rules)
- Reserves unique TCP port via `TcpListener::bind("127.0.0.1:0")`
- Writes profile + config to disk

**Docker:**
- Creates container via Docker daemon API (bollard)
- Daemon handles image/rootfs management

### 3. Init Actions (`InitOrchestrator`)
Before starting the main process, runs init steps from the spec:
- `tcp_wait` — wait for a port to be reachable
- `http_probe` — wait for an HTTP endpoint to return 200
- `s3_pull` — download data from S3
- `command` — run arbitrary commands
- Retry policies: `Fail`, `Restart`, `Backoff` (exponential up to 60s)

### 4. Container Start (`start_container`)
- Forks/execs the process (youki: libcontainer, sandbox: fork+sandbox_init+exec, docker: daemon API)
- PID recorded, state → Running
- Memory watchdog spawned if limit configured (sandbox)

### 5. Post-Start Wiring (done by `ServiceInstance::scale_to`)
- Attach to overlay network (get PID → veth pair → IP assignment)
- Register DNS: `{service}.service.local` + `{replica}.{service}.service.local`
- Start health monitor (TCP/HTTP checks)
- Register with proxy as backend

### 6. Container Stop/Remove
- SIGTERM → grace period → SIGKILL → waitpid → cleanup directory

---

## Image & Rootfs Management

### How it's supposed to work (production path via `ServiceInstance`):
```
pull_image("nginx:latest")          ← registry client downloads + caches layers
create_container(id, spec)          ← runtime extracts layers into per-container rootfs
start_container(id)                 ← process starts in isolated rootfs
```
The `ServiceInstance::scale_to()` method handles this entire flow automatically — pull once, create N replicas, each with its own rootfs clone.

### How the E2E tests do it (manually, and incorrectly):
```
prepare_native_rootfs(runtime, image_name)  ← test helper manually copies /bin/sleep etc.
runtime.create_container(id, spec)          ← APFS clones from manually-prepared rootfs
runtime.start_container(id)                 ← starts process
```
The tests bypass `pull_image` entirely and manually populate the image rootfs directory. This works but:
- Races when multiple tests prepare the same image directory in parallel
- Doesn't test the actual image pull → extract → create flow
- Manual setup is fragile and not representative of real usage

### Directory layout (macOS sandbox):
```
{data_dir}/
  images/
    {sanitized_image_name}/
      rootfs/                 ← shared image template (like Docker image layer)
        bin/echo, bin/sleep, ...
  containers/
    {service}-{replica}/
      rootfs/                 ← APFS clone of image rootfs (per-container)
      sandbox.sb              ← Seatbelt profile
      config.json             ← spec
      pid                     ← process ID
      stdout.log, stderr.log
```

---

## Service Manager (`crates/zlayer-agent/src/service.rs`)

`ServiceManager` orchestrates multiple services. `ServiceInstance` manages one service's containers.

### `ServiceInstance::scale_to(replicas)` — the main entry point
1. **Read current state** (short lock)
2. **Scale up** — for each new replica:
   - `pull_image` (once, shared)
   - `create_container` (rootfs extracted/cloned)
   - `init_orchestrator.run()` (init actions)
   - `start_container`
   - Attach overlay network
   - Register DNS
   - Start health monitor
   - Register proxy backend
3. **Scale down** — for excess replicas:
   - Deregister from proxy
   - Stop container (graceful → force)
   - Detach overlay
   - Remove container + cleanup

### Configuration (Builder Pattern):

Since v0.2.0, `ServiceManager` uses a builder pattern. The old `new()` / `with_full_config()` / `set_*()` constructors are deprecated.

```rust
let manager = ServiceManager::builder(runtime)
    .overlay_manager(overlay)        // Container networking
    .proxy_manager(proxy)            // Health-aware load balancing
    .service_registry(registry)      // Pingora proxy HTTP backends
    .stream_registry(streams)        // Pingora proxy L4 backends
    .dns_server(dns)                 // Service discovery
    .deployment_name("prod")         // Hostname generation
    .job_executor(jobs)              // Run-to-completion workloads
    .cron_scheduler(cron)            // Time-based triggers
    .container_supervisor(supervisor) // Crash/panic policy
    .build();
```

All fields except `runtime` are optional. The builder logs warnings for missing recommended subsystems (proxy, service_registry, stream_registry, container_supervisor, deployment_name).

---

## API Endpoints (`crates/zlayer-api`)

| Method | Endpoint | Purpose |
|--------|----------|---------|
| GET | `/health/live`, `/health/ready` | Health checks |
| POST | `/api/v1/deploy` | Deploy from spec |
| GET | `/api/v1/deployments` | List deployments |
| GET | `/api/v1/deployments/{name}` | Get deployment |
| GET | `/api/v1/deployments/{dep}/services` | List services |
| GET | `/api/v1/deployments/{dep}/services/{svc}/logs` | Service logs |
| POST | `/api/v1/build/json` | Trigger build |
| GET | `/api/v1/build/{id}/stream` | Stream build logs |
| CRUD | `/api/v1/secrets` | Secret management |
| POST | `/api/v1/jobs/{name}/trigger` | Trigger job |
| CRUD | `/api/v1/cron` | Cron job management |
| GET | `/api/v1/overlay/status` | Overlay network status |

---

## CLI Binary

### `zlayer` — `bin/zlayer`
Single unified binary for all operations. The former `zlayer-runtime` and `zlayer-build` binaries have been merged into `zlayer`.

**No subcommand / `zlayer tui`**: Launches the interactive TUI dashboard with screens for Dashboard, Deploy, Build Image, Validate, Runtimes, and Quit.

**Daemon mode (`zlayer serve`)**: Starts the full orchestration daemon (API, scheduler, proxy, overlay). Use `--daemon` to run in the background. The daemon auto-starts itself via `std::env::current_exe()` when CLI commands need it — no PATH lookup required.

**Key subcommands**:
| Command | Purpose |
|---------|---------|
| `zlayer up [spec]` | Deploy and start services (like `docker compose up`). Foreground by default, `-b` for background, `-d` for detach-after-stable. |
| `zlayer down [name]` | Stop and remove a deployment (like `docker compose down`). |
| `zlayer ps` | List running deployments, services, and containers. `--containers` for replica detail. |
| `zlayer logs --deployment NAME SERVICE` | Stream service logs. `-f` to follow. |
| `zlayer exec SERVICE -- CMD` | Execute a command in a running container. |
| `zlayer build [context]` | Build a container image from Dockerfile, ZImagefile, or runtime template. |
| `zlayer deploy [spec]` | Deploy from a spec file (also available via TUI). |
| `zlayer stop DEPLOYMENT` | Stop a deployment or individual service. |
| `zlayer status` | Show structured daemon info (runtime, uptime, deployments). |
| `zlayer validate [spec]` | Validate a spec file without deploying. |
| `zlayer pull IMAGE` | Pull an image from a registry. |
| `zlayer export / import` | Export/import OCI image tar archives. |
| `zlayer serve` | Start the API server (with `--daemon` for background). |
| `zlayer pipeline` | Build multiple images from a pipeline manifest. |
| `zlayer runtimes` | List available runtime templates. |
| `zlayer node init/join/list` | Cluster node management. |
| `zlayer token create/decode` | JWT token management. |
| `zlayer tunnel` | Secure tunnel management. |
| `zlayer wasm build/push/export` | WASM build and management. |
| `zlayer manager init/status/stop` | Web UI management. |

### `zlayer-manager` (web UI) — `crates/zlayer-manager`
Leptos SSR web dashboard on port 9120. Reads from API at port 3669.

---

## What's Missing for Docker-Like UX

### CLI Commands Status
| Command | Purpose | Status |
|---------|---------|--------|
| `zlayer up [spec]` | Deploy and start (like `docker compose up`) | **Working** (foreground, background, detach modes) |
| `zlayer down [name]` | Stop and remove a deployment | **Working** |
| `zlayer ps` | List active deployments/services/containers | **Working** (table, JSON, YAML output) |
| `zlayer logs` | Stream service logs | **Working** (`-f` follow, `--instance` filter) |
| `zlayer exec` | Exec into running container | **Working** (auto-detects deployment) |
| `zlayer build` | Build image (Dockerfile, ZImagefile, runtime templates) | **Working** (unified in `zlayer` binary) |
| `zlayer pull [image]` | Pull image from registry | **Working** |
| `zlayer scale [svc] N` | Scale a service | Not yet exposed as CLI command (API + ServiceInstance support it) |

### Auto-Management Gaps
- **Cache**: Image blob cache exists (`redb`) but no automatic cleanup/eviction
- **Overlay**: Should auto-start when deploying multi-node, currently manual
- **Proxy**: Should auto-configure when services have endpoints
- **Rootfs cleanup**: Container removal cleans up, but stale images accumulate

### Manager UI Gaps
- Deploy creation/editing (button exists, no backend)
- Node management (page exists, returns empty)
- SSH tunnels (page exists, mock data)
- Git integration (page exists, mock data)
- Settings save (form exists, no backend)

---

## Deployment Spec Format (`zlayer-spec`)

```yaml
version: v1
deployment: myapp

services:
  web:
    rtype: service                    # service | job | cron
    image:
      name: myapp:latest
    command:
      entrypoint: ["/app/server"]
      args: ["--port", "8080"]
    endpoints:
      - name: http
        protocol: http
        port: 8080
        expose: true                  # expose via proxy
    env:
      DATABASE_URL: "postgres://..."
    resources:
      memory: "512Mi"
      cpu: "1.0"
      gpu:
        vendor: apple
        mode: mps
    scale:
      mode: adaptive
      min: 1
      max: 10
    health:
      check:
        type: http
        path: /health
        port: 8080
        interval: 10s
    init:
      - action: tcp_wait
        host: db
        port: 5432
        timeout: 30s
    storage:
      - type: named
        name: data
        target: /app/data
```

---

## Platform Runtime Selection

```rust
// Automatic selection in create_runtime():
// Linux:  youki (libcontainer) → Docker fallback
// macOS:  SandboxRuntime → VmRuntime → Docker fallback
// Windows: Docker only

// Manual selection via RuntimeConfig enum:
RuntimeConfig::Youki(YoukiConfig { ... })
RuntimeConfig::Docker
RuntimeConfig::Sandbox(MacSandboxConfig { ... })
RuntimeConfig::Vm(VmConfig { ... })
RuntimeConfig::Wasm(WasmConfig { ... })
```

WASM is auto-detected from OCI manifest media types — if the image contains WASM artifacts, the WASM runtime is used regardless of platform.

---

## Test Infrastructure

### Test Suites
| Suite | File | Tests | Status |
|-------|------|-------|--------|
| macOS Sandbox E2E | `tests/macos_sandbox_e2e.rs` | 28 | 5 hanging (see MAC_FAILING.md) |
| macOS MPS GPU | `tests/macos_mps_smoke.rs` | 4 | All pass |
| Scheduler | `crates/zlayer-scheduler/` | 86 | All pass |
| Full workspace | `cargo test` | 2,551 | All pass |

### Running Tests
```bash
# All workspace tests
cargo test

# macOS sandbox E2E
cargo test --package zlayer-agent --test macos_sandbox_e2e -- --nocapture

# Single test
cargo test --package zlayer-agent --test macos_sandbox_e2e test_container_stats -- --nocapture

# Via Makefile
make test-macos-sandbox
```

---

## Key File Paths

| What | Where |
|------|-------|
| Unified binary entry point | `bin/zlayer/src/main.rs` |
| CLI definition (clap) | `bin/zlayer/src/cli.rs` |
| CLI commands | `bin/zlayer/src/commands/` |
| TUI app state machine | `bin/zlayer/src/app.rs` |
| Daemon server | `bin/zlayer/src/daemon.rs` |
| Daemon client (auto-start) | `bin/zlayer/src/daemon_client.rs` |
| Runtime trait | `crates/zlayer-agent/src/runtime.rs` |
| Runtime implementations | `crates/zlayer-agent/src/runtimes/` |
| macOS sandbox (2335 lines) | `crates/zlayer-agent/src/runtimes/macos_sandbox.rs` |
| Service manager (102KB) | `crates/zlayer-agent/src/service.rs` |
| OCI bundle builder (85KB) | `crates/zlayer-agent/src/bundle.rs` |
| Deployment spec types | `crates/zlayer-spec/` |
| API server | `crates/zlayer-api/` |
| Registry client | `crates/zlayer-registry/` |
| Proxy | `crates/zlayer-proxy/` |
| Overlay network | `crates/zlayer-overlay/` |
| Web UI | `crates/zlayer-manager/` |
| Dev script | `scripts/run_dev.sh` |
| Sandbox E2E tests | `crates/zlayer-agent/tests/macos_sandbox_e2e.rs` |
