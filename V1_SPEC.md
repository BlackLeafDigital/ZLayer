````markdown
# OCI Service Specification (Draft v1)

A typed, declarative service definition language for OCI-compatible containers.

This spec provides **lifecycle orchestration, networking, scaling, and placement** for containers, without Kubernetes complexity and without embedding application logic into YAML.

---

## 1. Goals

1. Single-file declarative configuration
2. OCI-native (Docker / containerd / CRI)
3. No cluster abstraction
4. Built-in proxy on every node (TLS, HTTP/2, certs, tunneling)
5. Encrypted overlay networking by default
6. Adaptive autoscaling by default
7. Typed, statically-validatable schema
8. **Init-style lifecycle actions (not long-running)**
9. No templating / interpolation
10. Minimal primitives, composable behavior

---

## 2. Mental Model

- A **deployment** defines a set of workloads.
- Every node runs:
  - runtime agent
  - built-in proxy (TLS, HTTP/2, certs, tunneling)
- Every deployment creates:
  - a **global encrypted overlay network**
- Every service creates:
  - a **service-scoped encrypted overlay**
- Nodes can:
  - join globally
  - or join a specific service via `key:service`
- **Containers run normally**
  - the container’s actual process comes from the image’s CMD/ENTRYPOINT
  - this spec controls *when* and *where* the container starts, plus routing/scaling/networking

---

## 3. Top-Level Structure

\```yaml
version: v1
deployment: string

services:
  <name>:
    rtype: service | job | cron
    image: {}
    resources: {}
    env: {}
    network: {}
    endpoints: []
    scale: {}
    depends: []
    health: {}
    init: {}
    errors: {}
\```

---

## 4. Resource Types (`rtype`)

### `service` (default)
- Long-running container
- Receives traffic
- Load-balanced
- Scalable
- Process lifetime = container lifetime

### `job`
- Run-to-completion container
- Triggered by endpoint, CLI, or internal system
- Process exits when done

### `cron`
- Scheduled run-to-completion container
- Same semantics as `job`, time-triggered

---

## 5. Image

\```yaml
image:
  name: string
  pull_policy: always | if_not_present | never
\```

---

## 5.5. Command Override (Optional)

By default, containers use the image's `ENTRYPOINT` and `CMD` as defined in the Dockerfile/OCI image. The `command` field allows optional overrides following Docker/OCI conventions.

\```yaml
command:
  entrypoint: [string]?  # Override image ENTRYPOINT
  args: [string]?        # Override image CMD
  workdir: string?       # Override working directory
\```

### Behavior

| Field | Effect |
|---|---|
| `entrypoint` | Replaces the image's `ENTRYPOINT` |
| `args` | Replaces the image's `CMD` |
| `workdir` | Replaces the image's `WORKDIR` |

### Semantics

- If **neither** `entrypoint` nor `args` is specified, the image's `ENTRYPOINT` and `CMD` are used unchanged.
- If **only `args`** is specified, it replaces the image's `CMD` but the image's `ENTRYPOINT` is preserved. The args are passed to the entrypoint.
- If **only `entrypoint`** is specified, it replaces the image's `ENTRYPOINT` and the image's `CMD` is used as arguments.
- If **both** are specified, both the image's `ENTRYPOINT` and `CMD` are replaced.
- `workdir` can be specified independently and does not affect entrypoint/args behavior.

### Example

\```yaml
# Run a custom script instead of the default entrypoint
command:
  entrypoint: ["/bin/sh", "-c"]
  args: ["python /app/worker.py --mode=background"]
  workdir: /app
\```

\```yaml
# Override just the arguments (entrypoint preserved)
command:
  args: ["--config", "/etc/myapp/production.yaml"]
\```

---

## 6. Resources (Limits)

Upper bounds, not reservations.

\```yaml
resources:
  cpu: number
  memory: string
\```

---

## 7. Environment Variables

\```yaml
env:
  NODE_ENV: production
  DATABASE_URL: $E:DATABASE_URL
\```

Values prefixed with `$E:` are resolved from the runtime environment at start time.  
No interpolation, no templating.

---

## 8. Networking

### Overlay Networks

\```yaml
network:
  overlays:
    service:
      enabled: true
      encrypted: true
      isolated: true
    global:
      enabled: true
      encrypted: true
\```

- Service overlay → service replicas only
- Global overlay → all services in deployment
- No exposure unless defined in `endpoints`

---

## 9. Endpoints (Proxy Bindings)

Endpoints define **where traffic is bound**, not how requests are shaped.

\```yaml
endpoints:
  - name: string
    protocol: http | https | tcp | udp | websocket
    port: number
    path: string?
    expose: public | internal
\```

### Endpoint semantics by `rtype`

| `rtype` | Behavior |
|---|---|
| `service` | Traffic routed to running containers (load-balanced across replicas) |
| `job` | Connection triggers container execution (run-to-completion), then returns output (implementation-defined) |
| `cron` | Optional manual trigger / visibility; schedule is the primary trigger |

Notes:
- The proxy is built into every node and handles TLS, HTTP/2, certs, and tunneling.
- This spec does **not** define request schemas, auth rules, or headers. Endpoints are bindings + routing only.

---

## 10. Scaling

### Modes

\```yaml
scale:
  mode: adaptive | fixed | manual
\```

### Adaptive (Default)

\```yaml
scale:
  mode: adaptive
  min: number
  max: number
  cooldown: duration?
  targets:
    cpu: percent?
    memory: percent?
    rps: number?
\```

- Runtime scales up/down based on live metrics.
- Load balancing automatically spans replicas.
- Replicas can span multiple VPS nodes.

### Fixed

\```yaml
scale:
  mode: fixed
  replicas: number
\```

### Manual

\```yaml
scale:
  mode: manual
\```

---

## 11. Dependencies (`depends`)

Defines **startup ordering and readiness gating**, including explicit timeouts.

\```yaml
depends:
  - service: string
    condition: started | healthy | ready
    timeout: duration
    on_timeout: fail | warn | continue
\```

Semantics:
- `started`: the dependent container process exists
- `healthy`: dependent service health check passes
- `ready`: dependent service is available for routing (proxy considers it ready)

---

## 12. Health Checks

Used for readiness, dependency resolution, and scaling signals.

\```yaml
health:
  start_grace: duration?
  interval: duration?
  timeout: duration?
  retries: number
  check:
    type: tcp | http | command
\```

---

## 13. Init Actions (`init`)

### Purpose

Init actions are:
- **pre-start lifecycle steps**
- executed **before the container process starts**
- used for:
  - waiting on dependencies
  - validating environment
  - performing one-off setup
  - avoiding duplicated shell glue scripts

They **do not keep the container alive** and **do not replace the image CMD/ENTRYPOINT**.

### Init Definition

\```yaml
init:
  steps:
    - id: string
      uses: string
      with: object?
      retry: number?
      timeout: duration?
      on_failure: fail | warn | continue
\```

### Execution Model

1. Image is pulled
2. Init steps run in order
3. If required steps succeed:
   - container process starts normally (image CMD/ENTRYPOINT)
4. On failure:
   - behavior determined by `on_failure` and global error policy

---

## 14. Built-in Init Actions (Examples)

\```yaml
init_actions:
  wait_tcp:
    inputs:
      host: string
      port: number

  wait_http:
    inputs:
      url: string
      expect_status: number?

  run:
    inputs:
      command: string
\```

---

## 15. Example: Service with Init Checks

\```yaml
services:
  api:
    rtype: service
    image:
      name: ghcr.io/org/api:latest
      pull_policy: if_not_present

    env:
      DATABASE_URL: $E:DATABASE_URL

    init:
      steps:
        - id: wait-db
          uses: init.wait_tcp
          with:
            host: db.global
            port: 5432
          timeout: 600s
          on_failure: fail

        - id: check-pgadmin
          uses: init.run
          with:
            command: test -n "$PGADMIN_URL"
          on_failure: continue

    endpoints:
      - name: api
        protocol: https
        port: 3000
        path: /api
        expose: public

    scale:
      mode: adaptive
      min: 2
      max: 50
      cooldown: 15s
      targets:
        cpu: 70%
        rps: 800
\```

---

## 15.5. Example: Service with Command Override

This example shows how `command` overrides work alongside `init` steps. Remember:
- **Init steps** handle pre-start workflows (database polling, migrations, environment validation) and exit when complete
- **Command override** controls the main container process that runs after init completes

\```yaml
services:
  worker:
    rtype: service
    image:
      name: python:3.12-slim
      pull_policy: if_not_present

    # Override the default python REPL with a custom worker script
    command:
      entrypoint: ["python"]
      args: ["/app/worker.py", "--queue=high-priority", "--concurrency=4"]
      workdir: /app

    env:
      REDIS_URL: $E:REDIS_URL
      DATABASE_URL: $E:DATABASE_URL

    # Init steps run BEFORE the command, then exit
    # The container stays healthy because the main process (command) is running
    init:
      steps:
        - id: wait-redis
          uses: init.wait_tcp
          with:
            host: redis.global
            port: 6379
          timeout: 60s
          on_failure: fail

        - id: wait-db
          uses: init.wait_tcp
          with:
            host: db.global
            port: 5432
          timeout: 120s
          on_failure: fail

        - id: run-migrations
          uses: init.run
          with:
            command: python /app/migrate.py --apply
          timeout: 300s
          on_failure: fail

    network:
      overlays:
        service:
          enabled: true
          encrypted: true

    scale:
      mode: adaptive
      min: 1
      max: 20
      targets:
        cpu: 80%

  # Example: only override args, keep image entrypoint
  nginx-custom:
    rtype: service
    image:
      name: nginx:alpine
      pull_policy: if_not_present

    # nginx image has ENTRYPOINT ["nginx"], just override CMD
    command:
      args: ["-g", "daemon off;", "-c", "/etc/nginx/custom.conf"]

    endpoints:
      - name: http
        protocol: http
        port: 80
        expose: public

    scale:
      mode: fixed
      replicas: 3
\```

---

## 16. Error Handling

\```yaml
errors:
  on_init_failure:
    action: fail | restart | backoff
  on_panic:
    action: restart | shutdown | isolate
\```

---

## 17. Multi-VPS Join

### Join Token Format

\```text
<key>:<service>
\```

Examples:
- `abc123:python`
- `abc123:nginx`

### Join Semantics

When a node runs:

\```bash
runtime join abc123:python
\```

The node MUST:
1. Authenticate key
2. Resolve service name from the active deployment manifest
3. Join overlays required for that service
4. Pull image
5. Run init steps
6. Start container (normal CMD/ENTRYPOINT)
7. Register replica with service discovery + built-in load balancer
8. Begin serving traffic once ready/healthy gates pass (as configured)

---

## 18. Join Policy (Per Service)

Controls who can join a service and how.

\```yaml
network:
  join:
    mode: open | token | closed
    scope: service | global
\```

- `open`: any trusted node in deployment can self-enroll
- `token`: requires a join key (recommended default)
- `closed`: only control-plane/scheduler can place replicas

---

## 19. Job Semantics (Clarified)

For `rtype: job`:
- Trigger starts a container execution (endpoint trigger, CLI, internal system)
- Init steps run
- Container runs normal CMD/ENTRYPOINT
- Container exits
- Proxy may return logs / exit code / opaque result (implementation-defined)

No request schema is defined here; the built-in proxy owns payload handling.

---

## 20. Cron Semantics

For `rtype: cron`:
- Scheduler triggers a container execution periodically
- Init steps run
- Container runs normal CMD/ENTRYPOINT
- Container exits

---

## 21. Scaling via Join

\```bash
runtime join <key>:python
\```

The VPS will:
- pull/install the service image
- join encrypted overlays
- run init steps
- start the container
- register into the service LB
- begin serving traffic automatically
````

````markdown
# Suggested Rust Monorepo Structure (2024/2025+ Edition, Cargo Workspace)

Below is a pragmatic, scalable monorepo layout for a Rust “latest edition” project
(edition set per-crate, workspace-managed dependencies, shared tooling, optional multi-binary setup).

This assumes you have:
- a core runtime agent
- a built-in proxy component
- an orchestrator/control-plane component (optional)
- shared libraries (spec types, validation, networking, metrics, etc.)

---

## Repository Layout

\```text
repo/
  Cargo.toml
  Cargo.lock
  rust-toolchain.toml
  .cargo/
    config.toml

  crates/
    spec/
      Cargo.toml
      src/
        lib.rs
        types.rs
        validate.rs
        parse.rs

    core/
      Cargo.toml
      src/
        lib.rs
        error.rs
        config.rs

    agent/
      Cargo.toml
      src/
        main.rs
        join.rs
        supervisor.rs
        deploy.rs
        state.rs

    proxy/
      Cargo.toml
      src/
        lib.rs
        main.rs
        tls.rs
        routes.rs
        tunnel.rs
        lb.rs

    overlay/
      Cargo.toml
      src/
        lib.rs
        wireguard.rs
        keys.rs
        peers.rs
        dns.rs

    scheduler/
      Cargo.toml
      src/
        lib.rs
        placement.rs
        autoscale.rs
        metrics.rs

    init_actions/
      Cargo.toml
      src/
        lib.rs
        wait_tcp.rs
        wait_http.rs
        run.rs

    registry/
      Cargo.toml
      src/
        lib.rs
        oci_pull.rs
        cache.rs

    observability/
      Cargo.toml
      src/
        lib.rs
        logs.rs
        tracing.rs
        metrics.rs

    api/
      Cargo.toml
      src/
        lib.rs
        main.rs
        handlers.rs
        auth.rs

  bin/
    runtime/
      Cargo.toml
      src/
        main.rs

  docs/
    spec.md
    architecture.md

  examples/
    simple.yaml
    nginx-python.yaml
    cron-job.yaml

  scripts/
    install.sh
    ci.sh
\```

---

## Workspace `Cargo.toml` (Root)

\```toml
[workspace]
resolver = "2"
members = [
  "crates/spec",
  "crates/core",
  "crates/agent",
  "crates/proxy",
  "crates/overlay",
  "crates/scheduler",
  "crates/init_actions",
  "crates/registry",
  "crates/observability",
  "crates/api",
  "bin/runtime",
]

[workspace.package]
edition = "2024"
license = "MIT OR Apache-2.0"
repository = "https://example.com/your/repo"
rust-version = "1.78" # example; pin appropriately

[workspace.dependencies]
anyhow = "1"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"
serde_json = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "signal"] }
tracing = "0.1"
tracing-subscriber = "0.3"
clap = { version = "4", features = ["derive"] }
\```

---

## Crate Responsibilities (Quick Notes)

- `crates/spec`: YAML parsing, typed structs, validation, defaults
- `crates/agent`: node daemon; join flow; pulls images; starts containers; runs init steps; registers with LB
- `crates/proxy`: embedded proxy; TLS/certs; routing; LB hooks; tunnel integration
- `crates/overlay`: encrypted overlay networks; peers; key management; DNS
- `crates/scheduler`: autoscaling logic; placement decisions; metrics ingestion
- `crates/init_actions`: pre-start steps library used by agent
- `crates/registry`: OCI pulls/build cache, image resolution
- `crates/observability`: tracing/metrics/log plumbing shared everywhere
- `crates/api`: control-plane API (optional if fully decentralized)
- `bin/runtime`: "single binary" distribution option that composes agent+proxy (includes all CLI tooling: format, tokens, local run)

---

## Optional: “Single Binary” Composition

If you want `runtime` to run both agent + proxy:
- keep `agent` and `proxy` as libraries (`lib.rs`)
- have `bin/runtime` spawn both subsystems under one process

That keeps deployment dead simple while preserving modularity.
````
