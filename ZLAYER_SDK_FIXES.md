# ZLayer SDK + Daemon Fixes for ZArcRunner

*Author: ZArcRunner team. Target audience: ZLayer maintainers. Status: proposal.*

## 1. Context

[ZArcRunner](../ZArcRunner) is a Forgejo Actions runner that uses ZLayer as its container orchestrator in place of Docker. Today ZArcRunner spawns exactly one container per task (the "task executor" sidecar) via the Go client at [`pkg/zlayer/client.go`](../ZArcRunner/pkg/zlayer/client.go) and runs the whole workflow inside that one container (see `DispatchTask` at [`pkg/listener/dispatcher.go:62`](../ZArcRunner/pkg/listener/dispatcher.go) and the YAML parser in [`pkg/taskexecutor/executor.go:75-115`](../ZArcRunner/pkg/taskexecutor/executor.go) which already understands `jobs.<id>.container`, `jobs.<id>.services`, and `steps[].container`). To finish Forgejo Actions parity, ZArcRunner needs to create additional sibling containers and wire them together the way Actions expects. The four roadmap phases that depend on this document are:

- **Phase 3: Job-level `container:`** — user workflow's job runs inside a user-chosen image. Requires the runner to start a second container and stream per-step exec output from inside it.
- **Phase 4: Step-level `container:`** — a single step overrides the job image. Same exec requirements as Phase 3.
- **Phase 5: Services (`services:` map)** — sidecar containers (postgres, redis, etc.) reachable by hostname from the job container on a shared network, with optional `ports`, `volumes`, `credentials:`, and `options: --health-cmd ...`.
- **Phase 6: Registry auth for private images** — `jobs.<id>.container.credentials: { username, password }` and registry credentials stored centrally.

Caching (`actions/cache@v4`) is a separate design (S3-like object store, not a container concern) and is explicitly out of scope here.

The audit found that ZLayer already does significantly more than the ZArcRunner-local Go client exposes, but also that the CLI advertises capabilities (`--network`, `--restart`, `--hostname`, `docker network create`, `docker volume create`) that are silent no-ops at the daemon level today. This document enumerates both sets of gaps: (a) *daemon gaps* where a field/endpoint is missing end-to-end and (b) *SDK gaps* where the daemon supports it but the Go client does not surface it.

## 2. Current state audit

Evidence paths are given relative to the repo root that owns them: `ZL/` = `/var/home/zach/github/ZLayer`, `ZR/` = `/var/home/zach/github/ZArcRunner`.

| Capability | CLI flag / command | Daemon HTTP API | ZArcRunner Go SDK (`pkg/zlayer/`) | Evidence |
|---|---|---|---|---|
| Port publishing (`-p host:container`) | **Yes**, `zlayer docker run -p` | **Partial** — only via `EndpointSpec` on `DeploymentSpec`; **no** `ports` field on `POST /api/v1/containers` | **No** field on `CreateContainerRequest` | ZL `crates/zlayer-docker/src/cli/run.rs:33-35,192,360-405`; ZL `crates/zlayer-api/src/handlers/containers.rs:99-127` (no `ports`); ZL `crates/zlayer-agent/src/runtimes/docker.rs:189-198` (Docker runtime does honour `endpoints[].port→target_port`); ZR `pkg/zlayer/types.go:9-36` |
| User-defined networks (Docker-bridge semantics) | **Stubbed** — `zlayer docker network create` prints `TODO:` | **No** — `/api/v1/networks` is **access-control policies** (CIDRs, members, rules), not bridge networks | **No** network CRUD | ZL `crates/zlayer-docker/src/cli/network.rs:139-179` ("TODO" arms); ZL `crates/zlayer-api/src/handlers/networks.rs:1-6,97-104` (`NetworkPolicySpec`); ZL `crates/zlayer-spec/src/types.rs:1605-1625` (`NetworkPolicySpec` is CIDR ACL) |
| Network aliases (`--network-alias`, service DNS) | **No flag** | **Partial** — overlay DNS is automatic per service name (see `crates/zlayer-agent/src/overlay_manager.rs`) but no per-container alias field | **No** | ZL `crates/zlayer-docker/src/cli/run.rs:57-60` (only `--network` and it's ignored L145-147) |
| Join a container to a named network | `zlayer docker network connect` **stubbed** | **No** — daemon never takes `networks: []` on `CreateContainerRequest` | **No** | ZL `crates/zlayer-docker/src/cli/network.rs:161-168` |
| Host-path bind mount (`-v /host:/ctr[:ro]`) | **Yes** | **Yes** — `volumes[]` on request, materialized as `StorageSpec::Bind` | **Yes** — `VolumeMount` | ZL `crates/zlayer-api/src/handlers/containers.rs:87-96,305-315`; ZR `pkg/zlayer/types.go:117-126` |
| Named volumes (`docker volume create foo; -v foo:/data`) | **Stubbed create/inspect/prune**, working `ls`/`rm` | **Partial** — `GET`/`DELETE /api/v1/volumes`, no `POST` (no create); daemon auto-creates on first mount via `StorageSpec::Named` | **No** volume CRUD; `VolumeMount.Source` is forwarded untyped and daemon heuristically detects bind-vs-named | ZL `crates/zlayer-docker/src/cli/volume.rs:99-122` (TODOs); ZL `crates/zlayer-api/src/handlers/volumes.rs:80-130` (list+delete only); ZL `crates/zlayer-spec/src/types.rs:840-870` (`StorageSpec::Named` exists); ZR `pkg/zlayer/types.go:117-126` |
| Exec one-shot (buffered stdout/stderr + exit code) | **Yes** (`zlayer exec`) | **Yes** — `POST /api/v1/containers/{id}/exec` | **Yes** — `Exec()` | ZL `crates/zlayer-api/src/handlers/containers.rs:1179-1215`; ZR `pkg/zlayer/client.go:330-348` |
| Exec streaming (line-by-line stdout/stderr as they happen) | **No** | **No** — handler collects full buffers then returns (no SSE, no websocket) | **No** | ZL `crates/zlayer-agent/src/runtimes/docker.rs:858-930` buffers the whole thing; no stream variant on the handler at `crates/zlayer-api/src/handlers/containers.rs:1179-1215` |
| Exec with TTY / stdin / interactive | **No** (`zlayer docker run -i/-t` emits warning at `run.rs:139-141`) | **No** | **No** | — |
| Container health checks (Docker-style `--health-cmd`) | **No flag** | **Partial** — `HealthSpec` on `ServiceSpec` (TCP / HTTP / Command), but **not accepted** on `CreateContainerRequest`; handler hard-codes `HealthCheck::Tcp { port: 0 }` | **No** | ZL `crates/zlayer-spec/src/types.rs:1419-1468`; ZL `crates/zlayer-api/src/handlers/containers.rs:335-342` (hard-coded placeholder health) |
| Restart policies | **Flag exists, warns "not yet forwarded"** | **Partial** — `PanicPolicy { action: Restart|Shutdown|Isolate }` on `ErrorsSpec` inside `ServiceSpec`, not Docker-semantics (`always`/`on-failure`/`unless-stopped`) | **No** | ZL `crates/zlayer-docker/src/cli/run.rs:250-255` (warn only); ZL `crates/zlayer-spec/src/types.rs:1570-1597` |
| Resource limits (cpu, memory) | **Yes** | **Yes** — `ContainerResourceLimits` | **Yes** — `ResourceSpec` | ZL `crates/zlayer-api/src/handlers/containers.rs:76-84`; ZR `pkg/zlayer/types.go:108-114` |
| GPU passthrough | **No top-level flag on `docker run`** | **Partial** — `ResourcesSpec.gpu` used by Docker runtime (`runtimes/docker.rs:236-308`) but **not exposed** on the `POST /api/v1/containers` DTO | **No** | — |
| Registry credentials CRUD | **Yes** (`zlayer credential registry …`) | **Yes** — `/api/v1/credentials/registry` | **No** | ZL `crates/zlayer-api/src/handlers/credentials.rs:172-271`; ZR `pkg/zlayer/client.go` has no credentials calls |
| Associate a registry credential with a pull | **Implicit** — daemon uses any matching credential from the store | **Implicit** — no `registry_credential_id` field on `CreateContainerRequest` or `PullImageRequest` | **No** explicit field | ZL `crates/zlayer-api/src/handlers/images.rs:80-90` (`PullImageRequest` has no credential field) |
| Explicit image pull (blocking, no progress) | **Yes** (`zlayer pull`) | **Yes** — `POST /api/v1/images/pull` | **No** | ZL `crates/zlayer-api/src/handlers/images.rs:232-286`; ZR `pkg/zlayer/client.go` |
| Image pull with progress stream | **No** | **No** | **No** | — |
| Image list / remove / tag / prune | **Yes** | **Yes** — `GET /api/v1/images`, `DELETE /api/v1/images/{image}`, `POST /api/v1/images/tag`, `POST /api/v1/system/prune` | **No** | ZL `crates/zlayer-api/src/handlers/images.rs:349-357`; ZR none |
| Container stats (one-shot) | **Yes** | **Yes** — `GET /api/v1/containers/{id}/stats` | **Yes** — `GetStats()` | ZR `pkg/zlayer/client.go:411-427` |
| Container stats streaming | **No** | **No** | **No** | — |
| Container events / watch (start/stop/die/oom) | **No** | **No** — deployments have SSE events (`/api/v1/deployments/{name}/events`) but raw containers do not | **No** | ZL `crates/zlayer-api/src/router.rs:161-164` (deployment events only) |
| Container wait with exit reason | **No** | **Partial** — `GET /api/v1/containers/{id}/wait` returns only `{ id, exit_code }`; no signal / oom_killed flag | **Partial** — `WaitContainer()` returns `int` only | ZL `crates/zlayer-api/src/handlers/containers.rs:1239-1264`; ZR `pkg/zlayer/client.go:363-400` |
| Hostname / DNS / extra hosts | `--hostname` flag exists, **warns "not yet supported"** | **No** — not on `ServiceSpec` nor `CreateContainerRequest` | **No** | ZL `crates/zlayer-docker/src/cli/run.rs:154-156` |
| Container start / stop / restart / kill (lifecycle without create) | **Yes** (via `zlayer container rm`, daemon API) | **Yes** — `/stop`, `/start`, `/restart`, `/kill` | **No** — SDK only has `CreateContainer` + `DeleteContainer` | ZL `crates/zlayer-api/src/router.rs:1275-1283`; ZR `pkg/zlayer/client.go` (no stop/start/restart/kill methods) |
| Log follow (SSE) | **Yes** | **Yes** — `GET /api/v1/containers/{id}/logs?follow=true` (SSE) | **Yes** — `GetLogs(…, follow=true)` returns `io.ReadCloser` | ZL `crates/zlayer-api/src/handlers/containers.rs:995-1031`; ZR `pkg/zlayer/client.go:283-318` |

## 3. Required additions

Every section below describes a gap, the daemon change (if any), the Go SDK change, and an acceptance test. JSON field names are `snake_case` to match ZLayer's existing convention (confirmed at `crates/zlayer-api/src/handlers/containers.rs:99-127`). Go field names use Pascal-case with matching `json:"snake_case,omitempty"` tags; optional primitives use pointers so the `omitempty` zero-value semantics are unambiguous — zero-valued structs are preserved via `omitempty` on the parent pointer.

### 3.1 Port publishing on `CreateContainerRequest`

- **Use case.** ZArcRunner Phase 5 services (`services.redis.ports: ["6379:6379"]`) and Phase 3/4 containers that expose ports back to the host for debugging.
- **Daemon API.** Extend `POST /api/v1/containers`:
  ```json
  {
    "image": "redis:7",
    "ports": [
      { "host_port": 6379, "container_port": 6379, "protocol": "tcp", "host_ip": "0.0.0.0" }
    ]
  }
  ```
  - `host_port` — `u16`, optional. When omitted or `0`, the daemon allocates an ephemeral port and returns it on `ContainerInfo` (see §3.15 below).
  - `container_port` — `u16`, required.
  - `protocol` — `"tcp"` \| `"udp"`, default `"tcp"`.
  - `host_ip` — string, default `"0.0.0.0"`. Accept `"127.0.0.1"` for loopback binds.
- **SDK change.** In [`ZR/pkg/zlayer/types.go`](../ZArcRunner/pkg/zlayer/types.go):
  ```go
  type PortMapping struct {
      HostPort      uint16 `json:"host_port,omitempty"`
      ContainerPort uint16 `json:"container_port"`
      Protocol      string `json:"protocol,omitempty"`   // "tcp" (default), "udp"
      HostIP        string `json:"host_ip,omitempty"`    // "0.0.0.0" default
  }

  // Add to CreateContainerRequest:
  Ports []PortMapping `json:"ports,omitempty"`
  ```
  Also add `Ports []PortMapping` to `ContainerInfo` so callers can recover daemon-assigned ephemeral host ports.
- **Backward compat.** Pure addition. Unset `ports` on existing callers means today's behaviour (no publishing — the only current way containers get published is if they're created via the deployment path, not via `POST /api/v1/containers`).
- **Implementation hint.** The Docker runtime already turns `endpoints[].port → target_port` into `PortBinding`s at `crates/zlayer-agent/src/runtimes/docker.rs:189-198`. Either (a) reuse this by translating `ports[]` into synthetic `EndpointSpec` entries inside `build_service_spec` at `crates/zlayer-api/src/handlers/containers.rs:269-355`, or (b) thread a new `ports` field through `ServiceSpec` (simpler long-term). Option (a) requires a fake `Protocol` + `name` per port — recommend option (b) with a new `ServiceSpec.port_mappings: Vec<PortMapping>` that the Docker runtime iterates in `build_host_config`.
- **Acceptance test.**
  ```bash
  curl -XPOST $ZLAYER/api/v1/containers \
    -d '{"image":"nginx:alpine","ports":[{"host_port":18080,"container_port":80}]}'
  curl -sI http://127.0.0.1:18080 | head -1   # => HTTP/1.1 200 OK
  ```

### 3.2 User-defined networks + network aliases on `CreateContainerRequest`

- **Use case.** Phase 5 services: runner creates a per-job bridge network `zarc-job-<id>`, attaches the job container and each service container to it with service-name aliases (`postgres`, `redis`), so the job can resolve `postgres:5432` by hostname.
- **Daemon change — two tracks.** Today `/api/v1/networks` stores CIDR ACL policies (`NetworkPolicySpec` at `crates/zlayer-spec/src/types.rs:1605-1625`). We need a second, orthogonal noun for *bridge networks / overlay scopes that containers attach to*. Do **not** reuse the existing `/api/v1/networks` path — rename the existing one to `/api/v1/network-policies` (or keep both; they're genuinely different things).
- **Daemon API — new routes.**
  - `POST /api/v1/container-networks`
    ```json
    {
      "name": "zarc-job-42",
      "driver": "bridge",          // or "overlay"; "bridge" is the only required value for ZArcRunner
      "subnet": "10.240.0.0/24",   // optional, daemon allocates if omitted
      "labels": {"zarcrunner": "true", "task-id": "42"},
      "internal": false            // if true, no egress to the outside
    }
    ```
    **Response 201:**
    ```json
    { "id": "bridge-zarc-job-42", "name": "zarc-job-42", "driver": "bridge", "subnet": "10.240.0.0/24", "created_at": "2026-04-20T…" }
    ```
  - `GET /api/v1/container-networks` — list, filterable by `?label=k=v`.
  - `GET /api/v1/container-networks/{id_or_name}` — inspect; includes `attached_containers: [{id, name, aliases, ipv4}]`.
  - `DELETE /api/v1/container-networks/{id_or_name}` — rejects if containers attached unless `?force=true`.
- **`CreateContainerRequest` additions.**
  ```json
  "networks": [
    { "network": "zarc-job-42", "aliases": ["postgres", "db"], "ipv4_address": null }
  ]
  ```
  Each entry joins the container to the named network on start, registering the given aliases in the overlay DNS. At least the `network` field is required; `aliases` defaults to `[name]` (the container's name).
- **SDK change.**
  ```go
  type NetworkAttachment struct {
      Network     string   `json:"network"`
      Aliases     []string `json:"aliases,omitempty"`
      IPv4Address string   `json:"ipv4_address,omitempty"`
  }
  // CreateContainerRequest: add Networks []NetworkAttachment `json:"networks,omitempty"`

  type CreateNetworkRequest struct {
      Name     string            `json:"name"`
      Driver   string            `json:"driver,omitempty"`
      Subnet   string            `json:"subnet,omitempty"`
      Labels   map[string]string `json:"labels,omitempty"`
      Internal bool              `json:"internal,omitempty"`
  }
  type NetworkInfo struct {
      ID        string            `json:"id"`
      Name      string            `json:"name"`
      Driver    string            `json:"driver"`
      Subnet    string            `json:"subnet,omitempty"`
      Labels    map[string]string `json:"labels,omitempty"`
      CreatedAt string            `json:"created_at"`
      Attached  []NetworkAttachedContainer `json:"attached_containers,omitempty"`
  }
  type NetworkAttachedContainer struct {
      ID      string   `json:"id"`
      Name    string   `json:"name"`
      Aliases []string `json:"aliases,omitempty"`
      IPv4    string   `json:"ipv4,omitempty"`
  }

  // New methods on *Client:
  func (c *Client) CreateNetwork(ctx context.Context, req CreateNetworkRequest) (*NetworkInfo, error)
  func (c *Client) ListNetworks(ctx context.Context, labels map[string]string) ([]NetworkInfo, error)
  func (c *Client) GetNetwork(ctx context.Context, idOrName string) (*NetworkInfo, error)
  func (c *Client) DeleteNetwork(ctx context.Context, idOrName string, force bool) error
  // Optional dynamic attach/detach:
  func (c *Client) ConnectContainerToNetwork(ctx context.Context, containerID string, attach NetworkAttachment) error
  func (c *Client) DisconnectContainerFromNetwork(ctx context.Context, containerID, network string, force bool) error
  ```
- **Backward compat.** New routes, new field. Containers that do not supply `networks` keep today's default overlay behaviour.
- **Implementation hint.** ZLayer's per-service overlay at `crates/zlayer-agent/src/overlay_manager.rs` already DNS-registers each service inside a deployment. The new `container-networks` abstraction should (a) for `driver: bridge` and Docker backend, call `bollard::Docker::create_network` then attach each container on start via `connect_network`; (b) for `driver: overlay` and Youki backend, spin a new WireGuard overlay scope. The CLI stubs at `crates/zlayer-docker/src/cli/network.rs:139-179` are the right place to wire up once the API exists.
- **Acceptance test.**
  ```
  POST /api/v1/container-networks {"name":"t","driver":"bridge"}
  POST /api/v1/containers {"image":"postgres:16","name":"pg","networks":[{"network":"t","aliases":["db"]}]}
  POST /api/v1/containers {"image":"alpine:3.19","name":"c","networks":[{"network":"t"}]}
  POST /api/v1/containers/c/exec {"command":["getent","hosts","db"]}   // must resolve to pg's ip
  ```

### 3.3 Health check on `CreateContainerRequest`

- **Use case.** Phase 5 services with `options: --health-cmd "pg_isready"` and the runner's "wait for service ready" gate before starting the job container.
- **Daemon API.** Accept a `health_check` field that mirrors the existing `HealthCheck` enum at `crates/zlayer-spec/src/types.rs:1444-1468`:
  ```json
  "health_check": {
    "type": "command",           // "tcp" | "http" | "command"
    "command": ["pg_isready", "-U", "postgres"],
    "interval":  "10s",          // humantime, default 30s
    "timeout":   "5s",
    "retries":   3,
    "start_period": "30s"        // maps to HealthSpec.start_grace
  }
  ```
  For `type: "tcp"` use `{ "type":"tcp", "port": 5432 }`; for `type: "http"` use `{ "type":"http", "url":"http://localhost:8080/health", "expect_status": 200 }`.
- **SDK change.**
  ```go
  type HealthCheckSpec struct {
      Type        string   `json:"type"`                    // "tcp" | "http" | "command"
      Command     []string `json:"command,omitempty"`
      URL         string   `json:"url,omitempty"`
      Port        uint16   `json:"port,omitempty"`
      ExpectStatus uint16  `json:"expect_status,omitempty"`
      Interval    string   `json:"interval,omitempty"`      // humantime e.g. "10s"
      Timeout     string   `json:"timeout,omitempty"`
      Retries     uint32   `json:"retries,omitempty"`
      StartPeriod string   `json:"start_period,omitempty"`
  }
  // CreateContainerRequest: add HealthCheck *HealthCheckSpec `json:"health_check,omitempty"`
  ```
  Add a `Health` field to `ContainerInfo`:
  ```go
  type ContainerHealth struct {
      Status       string `json:"status"`         // "starting" | "healthy" | "unhealthy"
      FailingSince string `json:"failing_since,omitempty"`
      LastOutput   string `json:"last_output,omitempty"`
  }
  // ContainerInfo: add Health *ContainerHealth `json:"health,omitempty"`
  ```
- **Backward compat.** Pure addition. Without `health_check`, daemon keeps current placeholder `HealthCheck::Tcp { port: 0 }` (no-op) from `crates/zlayer-api/src/handlers/containers.rs:335-342`.
- **Implementation hint.** Replace the hard-coded health placeholder in `build_service_spec` with a translation of the request's `health_check`. The health monitor code at `crates/zlayer-agent/src/health.rs` already handles all three check types end-to-end.
- **Acceptance test.** Create a postgres container with `health_check.type=command, command=["pg_isready"]`. Poll `GET /api/v1/containers/{id}` until `health.status == "healthy"` within 30s.

### 3.4 Restart policy on `CreateContainerRequest`

- **Use case.** Phase 5 services routinely have `options: --restart unless-stopped` so a flaky `postgres` doesn't fail the job on transient crashes.
- **Daemon API.**
  ```json
  "restart_policy": {
    "kind": "on_failure",      // "no" (default) | "always" | "unless_stopped" | "on_failure"
    "max_attempts": 5,         // optional, only meaningful for "on_failure"
    "delay":        "2s"       // optional, humantime
  }
  ```
- **SDK change.**
  ```go
  type RestartPolicy struct {
      Kind        string `json:"kind"`                    // "no" | "always" | "unless_stopped" | "on_failure"
      MaxAttempts uint32 `json:"max_attempts,omitempty"`
      Delay       string `json:"delay,omitempty"`
  }
  // CreateContainerRequest: add RestartPolicy *RestartPolicy `json:"restart_policy,omitempty"`
  ```
- **Backward compat.** Default is `"no"` (today's behaviour — the CLI currently warns then drops the flag at `run.rs:250-255`).
- **Implementation hint.** `PanicPolicy` at `crates/zlayer-spec/src/types.rs:1570-1597` is the right extension point but its enum is `Restart|Shutdown|Isolate`, which doesn't distinguish Docker's four kinds. Add a sibling enum `RestartPolicyKind` used only when the container was created via the `/api/v1/containers` path (not full deployments). The Docker runtime's `HostConfig` at `crates/zlayer-agent/src/runtimes/docker.rs:326-339` needs the bollard `restart_policy: Some(RestartPolicy { name: Some("on-failure"), maximum_retry_count: Some(5) })` added.
- **Acceptance test.** Create container with `restart_policy.kind=on_failure, max_attempts=2` running `sh -c 'exit 1'`. Verify container is restarted twice then stops. Inspect should show `state: "exited(1)"` after the third failure.

### 3.5 Hostname / DNS / extra hosts on `CreateContainerRequest`

- **Use case.** The Forgejo Actions `container:` block allows `hostname`; `services:` maps need inter-service DNS resolution and sometimes `extra_hosts: ["host.docker.internal:host-gateway"]` for common recipes (Forgejo's own workflows depend on this pattern).
- **Daemon API.**
  ```json
  "hostname":    "runner",
  "dns":         ["1.1.1.1", "8.8.8.8"],
  "extra_hosts": ["host.docker.internal:host-gateway", "db:10.240.0.2"]
  ```
- **SDK change.**
  ```go
  // CreateContainerRequest: add
  Hostname   string   `json:"hostname,omitempty"`
  DNS        []string `json:"dns,omitempty"`
  ExtraHosts []string `json:"extra_hosts,omitempty"`
  ```
- **Backward compat.** Pure addition; empty/omitted = today.
- **Implementation hint.** `ServiceSpec` does not model these; add them to `CreateContainerRequest` directly and translate into bollard's `HostConfig.extra_hosts: Some(vec![...])`, `HostConfig.dns`, `Config.hostname` at `crates/zlayer-agent/src/runtimes/docker.rs:326-339` and `626` where `create_container` is called. The `docker run` CLI warning at `run.rs:154-156` should then be deleted.
- **Acceptance test.** `POST /containers {image:alpine, hostname:hx, extra_hosts:["foo:1.2.3.4"]}` then `exec hostname` → `hx`, `exec getent hosts foo` → `1.2.3.4 foo`.

### 3.6 Named volume CRUD

- **Use case.** Phase 5 services that need stable state (`postgres` with a named volume for `/var/lib/postgresql/data`) without the job owning a host path. Also needed so ZArcRunner can pre-seed a cache volume before the job container starts.
- **Daemon API — new route.**
  - `POST /api/v1/volumes`
    ```json
    { "name": "my-vol", "labels": {"owner":"zarc"}, "size": "2Gi", "tier": "local" }
    ```
    - `name` — required; lowercase alnum+`-_` only.
    - `size` — optional size limit string matching humansize (e.g. `"512Mi"`, `"10Gi"`).
    - `tier` — optional; default `"local"`. Values match `zlayer_spec::StorageTier`.
    **Response 201:**
    ```json
    { "name":"my-vol", "path":"/var/lib/zlayer/volumes/my-vol", "size_bytes":0, "labels":{"owner":"zarc"}, "created_at":"..." }
    ```
  - `GET /api/v1/volumes/{name}` — inspect; enrich existing list response shape (adds `labels`, `created_at`, `in_use_by`).
- **`VolumeMount` must distinguish bind from named.** Today the daemon guesses at `crates/zlayer-api/src/handlers/containers.rs:307-315` (everything becomes `StorageSpec::Bind`). Add an explicit `type` discriminator:
  ```json
  "volumes": [
    { "type": "bind",  "source": "/host/path", "target": "/ctr/path", "readonly": false },
    { "type": "volume","source": "my-vol",     "target": "/data" },
    { "type": "tmpfs", "target": "/tmp" }
  ]
  ```
  Backward-compatible interpretation: when `type` is absent, keep today's "always bind" behaviour.
- **SDK change.**
  ```go
  type VolumeMount struct {
      Type     string `json:"type,omitempty"`     // "bind" | "volume" | "tmpfs"; default "bind" (legacy)
      Source   string `json:"source,omitempty"`
      Target   string `json:"target"`
      ReadOnly bool   `json:"readonly,omitempty"`
  }
  type CreateVolumeRequest struct {
      Name   string            `json:"name"`
      Size   string            `json:"size,omitempty"`
      Tier   string            `json:"tier,omitempty"`
      Labels map[string]string `json:"labels,omitempty"`
  }
  type VolumeInfo struct {
      Name      string            `json:"name"`
      Path      string            `json:"path"`
      SizeBytes *uint64           `json:"size_bytes,omitempty"`
      Labels    map[string]string `json:"labels,omitempty"`
      CreatedAt string            `json:"created_at,omitempty"`
      InUseBy   []string          `json:"in_use_by,omitempty"`
  }

  // New methods:
  func (c *Client) CreateVolume(ctx context.Context, req CreateVolumeRequest) (*VolumeInfo, error)
  func (c *Client) ListVolumes(ctx context.Context) ([]VolumeInfo, error)
  func (c *Client) GetVolume(ctx context.Context, name string) (*VolumeInfo, error)
  func (c *Client) DeleteVolume(ctx context.Context, name string, force bool) error
  ```
- **Backward compat.** Existing `VolumeMount{source, target, readonly}` without `type` keeps "bind" semantics.
- **Implementation hint.** `crates/zlayer-api/src/handlers/volumes.rs:88-130` already reads directories off disk; add a `create_volume` handler that `mkdir`s under `state.volume_dir.join(&name)` plus a JSON sidecar for metadata. In `containers.rs:build_service_spec`, branch on `volumes[].type` and emit `StorageSpec::Named` for `"volume"` or `StorageSpec::Bind` for `"bind"`.
- **Acceptance test.** `POST /volumes {name:v1}`; `POST /containers {image:alpine, volumes:[{type:volume,source:v1,target:/d}], command:["sh","-c","echo hi > /d/f"]}`; delete container; create new container with same `v1`; `exec cat /d/f` → `hi`.

### 3.7 Streaming exec

- **Use case.** Phase 4 step-level `container:` — ZArcRunner needs to stream stdout/stderr from each step's `exec` back to Forgejo's log feed in real time instead of buffering a whole `run: |` block before reporting.
- **Daemon API.**
  - `POST /api/v1/containers/{id}/exec?stream=true` (or `POST …/exec/stream`). Request body identical to the current exec (`{command: [...]}`), plus optional `{tty: bool, stdin: string|null, demux: bool, env: {...}}`.
  - Response: `Content-Type: text/event-stream`. Event types matching ZLayer's existing log-stream convention at `crates/zlayer-api/src/handlers/containers.rs:1046-1154`:
    - `event: stdout\ndata: <line>\n\n`
    - `event: stderr\ndata: <line>\n\n`
    - `event: exit\ndata: {"exit_code":0}\n\n` — terminal.
  - When `stream=false` (default) the handler keeps today's buffered behaviour for backward compat.
- **SDK change.**
  ```go
  type ExecOptions struct {
      Command []string          `json:"command"`
      Env     map[string]string `json:"env,omitempty"`
      TTY     bool              `json:"tty,omitempty"`
      Stream  bool              `json:"stream,omitempty"`
  }

  type ExecEvent struct {
      Kind     string `json:"-"`         // "stdout" | "stderr" | "exit"
      Data     string `json:"data,omitempty"`
      ExitCode *int   `json:"exit_code,omitempty"` // populated when Kind=="exit"
  }

  // New method returning an event channel. On context cancel the stream is torn down.
  func (c *Client) ExecStream(ctx context.Context, id string, opts ExecOptions) (<-chan ExecEvent, error)
  ```
  Existing `Exec` stays as the buffered convenience wrapper.
- **Backward compat.** Additive: new `stream` query param and new SDK method. Existing `Exec()` is unchanged.
- **Implementation hint.** The Docker runtime exec loop at `crates/zlayer-agent/src/runtimes/docker.rs:886-909` already reads from a `futures::Stream` via bollard — today it concatenates into two `String`s, but the natural thing is to thread a `tokio::sync::mpsc::Sender<ExecEvent>` through so the handler can pipe it into SSE. Add a new `Runtime::exec_stream(id, cmd) -> Pin<Box<dyn Stream<Item=ExecEvent>>>` method (with a default impl that wraps the existing buffered `exec()` and emits one big stdout event then exit).
- **Acceptance test.**
  ```
  SSE stream of POST /containers/{id}/exec?stream=true
  body: {"command":["sh","-c","echo a; sleep 0.2; echo b 1>&2; exit 3"]}
  # expected events in order (time-separated):
  #   event:stdout data:a
  #   event:stderr data:b
  #   event:exit   data:{"exit_code":3}
  ```

### 3.8 Explicit lifecycle methods in the Go SDK (stop / start / restart / kill)

- **Use case.** ZArcRunner needs to kill the job container when the Forgejo task is cancelled, not wait for `DeleteContainer` to stop-and-remove. Start/restart let the runner recover a stopped service without losing its filesystem.
- **Daemon API.** *No change.* Endpoints exist already at `/api/v1/containers/{id}/{stop,start,restart,kill}` (`crates/zlayer-api/src/router.rs:1275-1283`, handlers at `crates/zlayer-api/src/handlers/containers.rs:715-970`).
- **SDK change.**
  ```go
  type StopOptions struct {
      TimeoutSeconds *uint64 `json:"timeout,omitempty"`
  }

  func (c *Client) StopContainer(ctx context.Context, id string, opts *StopOptions) error
  func (c *Client) StartContainer(ctx context.Context, id string) error
  func (c *Client) RestartContainer(ctx context.Context, id string, opts *StopOptions) error
  func (c *Client) KillContainer(ctx context.Context, id string, signal string) error
  ```
- **Backward compat.** Purely additive.
- **Acceptance test.** Unit test per method against a mock daemon asserting the path, method, and body.

### 3.9 Registry credentials CRUD in the Go SDK

- **Use case.** Phase 6 — runner admin uploads a credential for `ghcr.io` once, then subsequent `container:` blocks for private images just work without ZArcRunner shipping credentials inline.
- **Daemon API.** *No change.* Exists at `/api/v1/credentials/{registry,git}` (`crates/zlayer-api/src/handlers/credentials.rs`).
- **SDK change.**
  ```go
  type RegistryCredential struct {
      ID       string `json:"id"`
      Registry string `json:"registry"`
      Username string `json:"username"`
      AuthType string `json:"auth_type"`   // "basic" | "token"
  }
  type CreateRegistryCredentialRequest struct {
      Registry string `json:"registry"`
      Username string `json:"username"`
      Password string `json:"password"`
      AuthType string `json:"auth_type"`
  }
  func (c *Client) ListRegistryCredentials(ctx context.Context) ([]RegistryCredential, error)
  func (c *Client) CreateRegistryCredential(ctx context.Context, req CreateRegistryCredentialRequest) (*RegistryCredential, error)
  func (c *Client) DeleteRegistryCredential(ctx context.Context, id string) error
  ```
  (Git credentials can land later; ZArcRunner does not use them yet.)
- **Backward compat.** Purely additive.
- **Acceptance test.** Create a basic-auth credential for `ghcr.io`; list to confirm metadata (no password leaked); delete.

### 3.10 Per-create registry credential selection

- **Use case.** Phase 6 — a workflow step wants to pull `ghcr.io/acme/private` with a specific, workflow-scoped credential even when a cluster-default exists for `ghcr.io`.
- **Daemon API.** Add two parallel knobs on both `POST /api/v1/containers` and `POST /api/v1/images/pull`:
  - `registry_credential_id: "<cred-id-from-store>"` — use the named stored credential.
  - `registry_auth: { username, password, auth_type }` — inline one-shot credential; takes precedence over `registry_credential_id`; never persisted.
- **SDK change.**
  ```go
  type RegistryAuth struct {
      Username string `json:"username"`
      Password string `json:"password"`
      AuthType string `json:"auth_type,omitempty"`   // "basic" (default) | "token"
  }
  // CreateContainerRequest: add
  RegistryCredentialID string        `json:"registry_credential_id,omitempty"`
  RegistryAuth         *RegistryAuth `json:"registry_auth,omitempty"`
  // Mirror on PullImageRequest.
  ```
- **Backward compat.** Additive. Today the daemon matches credentials in the store by registry hostname (see `crates/zlayer-api/src/handlers/credentials.rs`); that heuristic stays the default when neither new field is set.
- **Implementation hint.** `pull_image_with_policy` on the `Runtime` trait (`crates/zlayer-agent/src/runtime.rs:91`) currently takes only `(image, policy)`. Extend with an optional `RegistryAuth` struct, defaulting to the store lookup. Change bollard's `CreateImageOptions` usage in `runtimes/docker.rs` to thread the explicit auth through when provided.
- **Acceptance test.** Without a cluster credential, `POST /images/pull` for a gated image fails; same call with inline `registry_auth` succeeds.

### 3.11 Explicit image pull in the Go SDK

- **Use case.** Warm-pull before creating a container so the first `POST /containers` returns quickly; also useful for ZArcRunner to pre-pull the "runner executor" image on startup.
- **Daemon API.** *No change.* Exists at `POST /api/v1/images/pull` (`crates/zlayer-api/src/handlers/images.rs:232-286`). Progress-stream variant is out of scope for now (see §5).
- **SDK change.**
  ```go
  type PullImageRequest struct {
      Reference            string        `json:"reference"`
      PullPolicy           string        `json:"pull_policy,omitempty"`   // "always" default | "if_not_present" | "never"
      RegistryCredentialID string        `json:"registry_credential_id,omitempty"`
      RegistryAuth         *RegistryAuth `json:"registry_auth,omitempty"`
  }
  type PullImageResponse struct {
      Reference string  `json:"reference"`
      Digest    string  `json:"digest,omitempty"`
      SizeBytes *uint64 `json:"size_bytes,omitempty"`
  }
  func (c *Client) PullImage(ctx context.Context, req PullImageRequest) (*PullImageResponse, error)
  ```
- **Backward compat.** Purely additive.
- **Acceptance test.** `PullImage(reference: "alpine:3.19")` returns within 30s and response includes `digest`.

### 3.12 Wait with exit reason

- **Use case.** The runner wants to distinguish "job step failed with exit 1" from "OOM-killed" from "cancelled via SIGTERM" to map them onto the right Forgejo conclusion (`failure` vs `cancelled`).
- **Daemon API.** Extend the existing `GET /api/v1/containers/{id}/wait` response:
  ```json
  {
    "id": "…",
    "exit_code": 137,
    "reason": "signal",          // "exited" | "signal" | "oom_killed" | "runtime_error"
    "signal": "SIGKILL",         // present when reason=="signal"
    "finished_at": "2026-04-20T10:00:00Z"
  }
  ```
- **SDK change.**
  ```go
  type WaitResult struct {
      ID         string `json:"id"`
      ExitCode   int    `json:"exit_code"`
      Reason     string `json:"reason,omitempty"`
      Signal     string `json:"signal,omitempty"`
      FinishedAt string `json:"finished_at,omitempty"`
  }
  // Add a richer variant alongside the existing WaitContainer(int):
  func (c *Client) WaitContainerDetailed(ctx context.Context, id string) (*WaitResult, error)
  ```
- **Backward compat.** Current `ContainerWaitResponse` at `crates/zlayer-api/src/handlers/containers.rs:222-228` only has `id` + `exit_code`; JSON additions are backward-compatible as long as the existing fields stay. The existing Go `WaitContainer(…) (int, error)` still works.
- **Implementation hint.** The `Runtime::wait_container` trait at `crates/zlayer-agent/src/runtime.rs:124` returns just `i32`. Replace with `struct WaitOutcome { exit_code, reason, signal }`; bollard already surfaces `OOMKilled` on `ContainerInspectResponse.state`.
- **Acceptance test.** Start a container with `--memory=10m` running `tail /dev/zero`; wait; expect `reason=="oom_killed"`.

### 3.13 Container event stream

- **Use case.** ZArcRunner wants one watch loop to observe all `zarcrunner=true`-labelled containers for `die`/`oom`/`start`, instead of polling `GetContainer` in a loop per task.
- **Daemon API — new route.**
  - `GET /api/v1/events?follow=true[&label=k=v]` — SSE stream.
    ```
    event: container.start   data: {"id":"…","labels":{…},"at":"…"}
    event: container.die     data: {"id":"…","exit_code":1,"reason":"exited","at":"…"}
    event: container.oom     data: {"id":"…","at":"…"}
    event: container.health  data: {"id":"…","status":"unhealthy","at":"…"}
    ```
- **SDK change.**
  ```go
  type ContainerEvent struct {
      Kind     string            `json:"-"`             // "container.start" | "container.die" | …
      ID       string            `json:"id"`
      Labels   map[string]string `json:"labels,omitempty"`
      ExitCode *int              `json:"exit_code,omitempty"`
      Reason   string            `json:"reason,omitempty"`
      Status   string            `json:"status,omitempty"`
      At       string            `json:"at"`
  }
  type EventOptions struct {
      Labels map[string]string
  }
  func (c *Client) StreamEvents(ctx context.Context, opts EventOptions) (<-chan ContainerEvent, error)
  ```
- **Backward compat.** New route.
- **Implementation hint.** The deployment event-stream at `crates/zlayer-api/src/handlers/deployments.rs` (`stream_deployment_events`) is the blueprint; generalise to a daemon-wide event bus.
- **Acceptance test.** Start an event-stream subscriber filtered by `label=zarcrunner=true`; create a container with that label that exits 7; assert a `container.die` event arrives with `exit_code:7`.

### 3.14 Container stats streaming

- **Use case.** Live CPU/memory dashboards for the job container in the runner UI. Nice-to-have, not on the Phase 3–6 critical path.
- **Daemon API.** Add `?stream=true` to the existing `GET /api/v1/containers/{id}/stats` — SSE, one event per sample, default 2s cadence. Keep the non-streaming one-shot as default.
- **SDK change.**
  ```go
  func (c *Client) StreamStats(ctx context.Context, id string, intervalSeconds int) (<-chan StatsResponse, error)
  ```
- **Backward compat.** Additive.
- **Acceptance test.** Subscribe for 5s; receive ≥2 `StatsResponse` events whose `cpu_usage_usec` is monotonically non-decreasing.

### 3.15 `ContainerInfo` enrichment

Several of the additions above (ports, health, networks) need the existing `GET /api/v1/containers/{id}` response to carry richer state back to the client. Today `ContainerInfo` at `crates/zlayer-api/src/handlers/containers.rs:130-148` has only `id, name, image, state, labels, created_at, pid`.

Required additions:
```go
// ContainerInfo additions:
Ports    []PortMapping     `json:"ports,omitempty"`
Networks []NetworkAttached `json:"networks,omitempty"`
Health   *ContainerHealth  `json:"health,omitempty"`
IPv4     string            `json:"ipv4,omitempty"`
ExitCode *int              `json:"exit_code,omitempty"`    // populated when State starts with "exited("
```
(`NetworkAttached` can reuse `NetworkAttachment` since the shape is identical.)

## 4. Migration / versioning

- **API versioning.** All additions are strictly additive (new endpoints, new optional fields). Stay on `/api/v1`. Two non-additive changes to watch:
  - **`VolumeMount.type` discriminator** — today the daemon silently treats every entry as `bind`. Introducing `type` means existing clients that happen to pass a bare volume name (e.g. `my-vol`) as `source` will now have that interpreted as a bind mount of a host path called `my-vol` (same as today), not a named volume. Document the default as `"bind"` and require callers to opt into `type: "volume"`.
  - **`ContainerWaitResponse` additions** — fine for JSON, but any consumer that used a strict schema validator with `additionalProperties: false` would break. Relax if present.
  If we decide to rename `/api/v1/networks` (CIDR ACL) to `/api/v1/network-policies` and claim `/api/v1/container-networks` for bridge networks, keep `/api/v1/networks` as an alias for at least one minor version and log a deprecation warning. That keeps today's `crates/zlayer-api/src/handlers/networks.rs` callers working.

- **Go SDK compat.** Existing callers of `ZR/pkg/zlayer` use the struct literal form (`CreateContainerRequest{Image: …, Env: …}`). Adding new fields with `omitempty` and pointer-typed optional primitives is source-compatible; no caller needs changes. The only risk is that `VolumeMount.Type` is a new zero-value-valid field — callers who today build `VolumeMount{Source, Target, ReadOnly}` in a struct literal stay on the "bind" default, which matches the daemon's backward-compat interpretation.

- **Optional semantics in Go.** For optional primitives use pointers so the Go zero value (`0`, `""`) doesn't accidentally serialise as a "set" value: e.g. `HostPort uint16` would force every SDK user to explicitly set `0` to request ephemeral allocation, which is exactly what we want — so keep it non-pointer. For *optional structs* (`Resources *ResourceSpec`, `HealthCheck *HealthCheckSpec`, `RestartPolicy *RestartPolicy`) use pointers + `omitempty`. Follow the shape already used by `CreateContainerRequest.Resources` at `ZR/pkg/zlayer/types.go:32`.

## 5. Out of scope

Explicitly **not** requested in this document:

- GPU passthrough DTO (`resources.gpu`) — ZArcRunner's roadmap doesn't cover GPU runners yet; the daemon already honours it for deployment-style specs via `crates/zlayer-agent/src/runtimes/docker.rs:236-308`.
- Windows containers.
- macOS-host container support (`macos_sandbox` / `macos_vm` runtimes).
- WASM services (`ServiceType::Wasm*`).
- Interactive exec (TTY attach, stdin bidirectional websocket). The streaming exec in §3.7 is one-way output + exit code. True TTY is needed for `zlayer docker run -it` parity but not by ZArcRunner.
- Image pull *with progress stream*. ZArcRunner is happy with blocking pull plus the existing digest/size response; progress bars are a future DX nicety.
- Cluster-wide autoscaling hooks, cron, deployment CRUD, projects, workflows — ZArcRunner uses raw `/api/v1/containers` directly and does not touch the deployment layer.
- `actions/cache` — belongs in a separate S3/object-store proposal.

## 6. Test plan

ZLayer should ship an integration test per bullet below. Recommended harness: spin a daemon with `MockRuntime` and the Docker runtime (two test matrices); exercise from the generated Go client at `/var/home/zach/github/ZLayer/clients/go/` (and mirror into `ZR/pkg/zlayer/` once released). For each section, the pass bar is:

| Spec § | Test |
|---|---|
| 3.1 Ports | Create with `ports:[{host_port:0,container_port:80}]`; assert `ContainerInfo.ports[0].host_port > 0` and TCP connect to host:port succeeds. |
| 3.2 Networks | Two containers on the same user network resolve each other by alias; deleting the network with containers attached 409s unless `?force=true`. |
| 3.3 Health | Command health check transitions `starting → healthy` within `interval*retries + start_period`; a failing command transitions to `unhealthy`. |
| 3.4 Restart | `on_failure` with `max_attempts=2` retries exactly twice before giving up. |
| 3.5 Hostname/DNS | Inside the container, `hostname` returns the requested value and `extra_hosts` resolve. |
| 3.6 Volumes | Named volume survives container delete+recreate; `force=false` delete on in-use volume 409s. |
| 3.7 Streaming exec | Stdout line emitted before the command has completed (`sh -c 'echo a; sleep 2; echo b; exit 3'` sends first event before second). |
| 3.8 Lifecycle | Stop, then Start, then Restart, then Kill (SIGTERM) all return 204 in the success case and 404 for unknown IDs. |
| 3.9 / 3.10 Credentials | Private-image pull fails with no cred, succeeds with cred referenced by `registry_credential_id`, and succeeds with inline `registry_auth`. |
| 3.11 Pull | `PullImage` with `pull_policy:"always"` returns a `digest`. |
| 3.12 Wait reason | `--memory 10m; tail /dev/zero` yields `reason:"oom_killed"`. |
| 3.13 Events | Subscriber gets `container.die` within 1s of the container exiting. |
| 3.14 Stats stream | ≥2 samples in 5s with strictly increasing `cpu_usage_usec`. |
| 3.15 ContainerInfo | `GET /containers/{id}` on a container built with ports+networks+health reflects all three. |

---

**Investigation grounding (full paths for convenience):**
`ZArcRunner` Go SDK in use today: `/var/home/zach/github/ZArcRunner/pkg/zlayer/client.go`, `/var/home/zach/github/ZArcRunner/pkg/zlayer/types.go`. Consumers: `/var/home/zach/github/ZArcRunner/pkg/listener/dispatcher.go`, `/var/home/zach/github/ZArcRunner/pkg/taskexecutor/executor.go`. ZLayer daemon HTTP surface: `/var/home/zach/github/ZLayer/crates/zlayer-api/src/router.rs`, `…/handlers/containers.rs`, `…/handlers/networks.rs`, `…/handlers/volumes.rs`, `…/handlers/credentials.rs`, `…/handlers/images.rs`. Docker CLI shim (reveals what the daemon silently drops): `/var/home/zach/github/ZLayer/crates/zlayer-docker/src/cli/run.rs` (lines 33-120 flags vs lines 139-173 warnings), `…/cli/network.rs`, `…/cli/volume.rs`. Runtime trait: `/var/home/zach/github/ZLayer/crates/zlayer-agent/src/runtime.rs`. Core spec types: `/var/home/zach/github/ZLayer/crates/zlayer-spec/src/types.rs` (ServiceSpec L630, HealthSpec L1419, PanicPolicy L1570, NetworkPolicySpec L1605, StorageSpec L840). Generated upstream Go SDK (reference shape): `/var/home/zach/github/ZLayer/clients/go/`.
