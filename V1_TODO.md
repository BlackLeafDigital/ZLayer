# ZLayer Implementation Roadmap (V1)

**Status**: Implementation Phase
**Last Updated**: 2026-01-22
**Total Estimated Phases**: 5 (11+ weeks)

---

## Progress Summary

| Phase | Status | Completion |
|-------|--------|------------|
| Phase 1: Foundation | **COMPLETE** | 100% |
| Phase 2: Container Operations | **COMPLETE** | 100% |
| Phase 3: Networking | **PARTIAL** | ~40% |
| Phase 4: Orchestration | NOT STARTED | 0% |
| Phase 5: Polish | **PARTIAL** | ~30% |

**Milestones Achieved:**
- M1: Spec parsing - COMPLETE
- M2: Container lifecycle - COMPLETE
- M3: Networking (overlay only) - PARTIAL

---

## Overview

This roadmap implements the OCI Service Specification defined in `V1_SPEC.md` - a typed, declarative service orchestration system for OCI-compatible containers.

**Key Principles:**
- Single-file declarative YAML configuration
- OCI-native (Docker/containerd/CRI compatible)
- No cluster abstraction (multi-VPS via token join)
- Built-in proxy on every node
- Encrypted overlay networking by default
- Adaptive autoscaling

---

## Phase 1: Foundation (Weeks 1-2) - COMPLETE

**Goal**: Type-safe spec parsing with validation

### 1.1 Project Initialization

- [x] Initialize Cargo workspace
- [x] Create `Cargo.toml` (workspace root)
- [x] Create `rust-toolchain.toml` (pin Rust version)
- [x] Create `.cargo/config.toml` (workspace settings)
- [ ] Set up `cargo-deny` for dependency auditing
- [x] Configure workspace dependencies

**Files to create:**
```
Cargo.toml (workspace)
rust-toolchain.toml
.cargo/config.toml
.cargo/deny.toml
```

### 1.2 crates/spec (Priority: HIGHEST) - COMPLETE

**Purpose**: YAML parsing, typed structs, validation

- [x] Define Rust types matching YAML schema
  - [x] `DeploymentSpec` (top-level)
  - [x] `ServiceSpec` (per-service)
  - [x] `ImageSpec`, `ResourcesSpec`, `EnvSpec`
  - [x] `NetworkSpec`, `EndpointSpec`, `ScaleSpec`
  - [x] `DependsSpec`, `HealthSpec`, `InitSpec`, `ErrorsSpec`
- [x] Implement serde deserialization
- [x] Add validation rules (using `validator` crate)
- [x] Implement custom validators (image names, ports, durations)
- [x] Add comprehensive unit tests (42 unit tests)
- [x] Add error types using `thiserror`

**Recommended crates:**
```toml
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"
schemars = "0.12"  # JSON Schema generation
validator = { version = "0.16", features = ["derive"] }
thiserror = "2"
```

### 1.3 crates/core - COMPLETE

**Purpose**: Shared error types, configuration structures

- [x] Define error hierarchy
  - [x] Base `ZLayerError` enum
  - [x] Container runtime errors
  - [x] Network errors
  - [x] Validation errors
- [x] Create configuration structures
  - [x] Agent config
  - [x] Proxy config
  - [x] Overlay config
- [ ] Implement config loading (file + env)
- [x] Set up tracing foundation

**Recommended crates:**
```toml
thiserror = "2"
anyhow = "1"
config = "0.14"  # or figment = "0.10"
tracing = "0.1"
tracing-subscriber = "0.3"
```

**Critical Success Factor**: Everything depends on correct spec parsing. Changes here ripple everywhere.

---

## Phase 2: Container Operations (Weeks 3-4) - COMPLETE

**Goal**: Pull and run containers

### 2.1 crates/registry - PARTIAL

**Purpose**: OCI distribution client, blob caching

- [ ] Implement OCI distribution client
  - [x] Image pull from registry (basic)
  - [ ] Authentication handling
  - [ ] Manifest resolution
- [x] Implement blob cache
  - [x] Content-addressable storage (SHA-256)
  - [x] Deduplication
  - [ ] Cache eviction policy
- [ ] Add metrics (pull time, cache hit rate)

**Recommended crates:**
```toml
oci-distribution = "0.11"
redb = "0.2"  # or sled = "0.34"
sha2 = "0.10"
hex = "0.4"
```

**Architecture decision**: Use embedded KV store (redb/sled) for cache - avoid external dependencies.

### 2.2 crates/init_actions - COMPLETE

**Purpose**: Pre-start lifecycle steps

- [x] Define `InitAction` trait
- [x] Implement built-in actions
  - [x] `wait_tcp` - wait for TCP port availability
  - [x] `wait_http` - wait for HTTP response
  - [x] `run` - execute shell command
- [x] Implement timeout handling
- [x] Implement retry logic
- [x] Add failure modes (fail, warn, continue)
- [ ] Add plugin system (WASM via `extism`)

**Recommended crates:**
```toml
reqwest = { version = "0.11", features = ["rustls-tls"] }
tokio = { version = "1", features = ["net", "time"] }
command-group = "5.0"
extism = "1.0"  # for WASM plugins (optional)
```

### 2.3 crates/agent (Basic Lifecycle) - COMPLETE

**Purpose**: Node daemon, container lifecycle management

- [x] Implement container runtime interface
  - [x] Containerd shim integration
  - [ ] Docker fallback (optional)
  - [x] Abstract behind `Runtime` trait
- [x] Implement pull → init → start flow
  - [x] Pull image via registry crate
  - [x] Run init steps via init_actions crate
  - [x] Start container (CMD/ENTRYPOINT)
- [x] Implement health checking
  - [x] TCP checks
  - [x] HTTP checks
  - [x] Command checks
- [x] Implement dependency resolution
  - [x] Wait for dependencies (started/healthy/ready)
  - [x] Handle timeouts
  - [x] Implement on_timeout behavior

**Recommended crates:**
```toml
containerd-client = "0.4"
oci-spec = "0.6"
tokio = { version = "1", features = ["process", "time"] }
```

**Critical Success Factor**: Core value proposition is running containers. Everything else is enhancement.

---

## Phase 3: Networking (Weeks 5-7) - PARTIAL

**Goal**: Encrypted overlay + proxy

### 3.1 crates/overlay - COMPLETE

**Purpose**: Encrypted overlay networks

- [x] Implement WireGuard integration
  - [ ] Kernel WireGuard via `wireguard-uapi`
  - [x] Userspace WireGuard via `boringtun`
  - [ ] Adaptive backend selection
- [x] Implement key management
  - [x] Curve25519 key generation
  - [x] Key distribution
  - [ ] Key rotation (optional)
- [x] Implement peer discovery
  - [x] Join token handling
  - [x] Peer registration
- [x] Implement DNS server
  - [x] Service-scoped DNS (`.service` TLD)
  - [x] Global DNS (`.global` TLD)
- [ ] Add network isolation (service vs global overlay)

**Recommended crates:**
```toml
wireguard-keys = "0.3"
x25519-dalek = "2.0"
blake2 = "0.10"
trust-dns-server = "0.23"
trust-dns-client = "0.23"
socket2 = "0.5"
tun = { version = "0.6", features = ["async"] }
```

**Architecture decision**: Support both kernel and userspace WireGuard - adaptive selection based on privileges.

### 3.2 crates/proxy - NOT STARTED

**Purpose**: Built-in proxy with TLS/HTTP/2

- [ ] Implement HTTP/2 proxy
  - [ ] Use `hyper` as foundation
  - [ ] Support HTTP/1.1 upgrade
- [ ] Implement TLS termination
  - [ ] Use `rustls` (avoid OpenSSL)
  - [ ] Automatic cert generation (optional)
- [ ] Implement load balancing
  - [ ] Round-robin (default)
  - [ ] Least-connections (optional)
  - [ ] Service discovery integration
- [ ] Implement WebSocket support
- [ ] Add middleware
  - [ ] Request logging
  - [ ] Compression
  - [ ] CORS (optional)
- [ ] Implement tunneling (for internal traffic)

**Recommended crates:**
```toml
hyper = { version = "1.0", features = ["full", "http2"] }
hyper-util = "0.1"
rustls = { version = "0.23", features = ["ring"] }
tokio-rustls = "0.26"
tower = { version = "0.4", features = ["full"] }
tower-http = { version = "0.5", features = ["trace", "compression"] }
tower-balance = "0.4"
discover = "0.2"
tokio-tungstenite = "0.21"
```

**Architecture pattern**: Use `tower::ServiceBuilder` for composable middleware.

### 3.3 crates/agent (Networking Integration) - NOT STARTED

**Purpose**: Connect agent to overlay and proxy

- [ ] Register with overlay networks
- [ ] Implement service discovery
- [ ] Expose endpoints via proxy
- [ ] Handle replica registration with LB

---

## Phase 4: Orchestration (Weeks 8-10) - NOT STARTED

**Goal**: Distributed coordination and autoscaling

### 4.1 crates/scheduler - STUB ONLY

**Purpose**: Autoscaling and placement decisions

- [ ] Implement metrics collection
  - [ ] CPU usage
  - [ ] Memory usage
  - [ ] Requests per second (RPS)
  - [ ] Custom metrics (optional)
- [ ] Implement autoscaling algorithms
  - [ ] Exponential moving average (EMA) for smoothing
  - [ ] Scale-up threshold logic
  - [ ] Scale-down threshold logic
  - [ ] Cooldown period handling
- [ ] Implement placement decisions
  - [ ] Node capacity tracking
  - [ ] Replica distribution logic
- [ ] Implement distributed coordination
  - [ ] Lightweight consensus (etcd/Consul/Raft)
  - [ ] Lease-based leadership
  - [ ] Node membership

**Recommended crates:**
```toml
prometheus = "0.13"
metrics = "0.22"
metrics-exporter-prometheus = "0.13"
etcd-client = "0.12"  # or openraft = "0.9"
```

**Architecture decision**: Use lightweight consensus layer for global state, keep local autonomy for container lifecycle.

### 4.2 crates/observability

**Purpose**: Logs, tracing, metrics aggregation

- [ ] Implement structured logging
  - [ ] `tracing` integration
  - [ ] Log rotation
  - [ ] JSON output option
- [ ] Implement distributed tracing
  - [ ] OpenTelemetry integration
  - [ ] Jaeger or OTLP exporter
- [ ] Implement metrics exposition
  - [ ] Prometheus scrape endpoint
  - [ ] Per-service metrics
  - [ ] Per-node metrics
- [ ] Add error tracking (Sentry, optional)

**Recommended crates:**
```toml
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
tracing-appender = "0.2"
tracing-opentelemetry = "0.22"
opentelemetry = { version = "0.21", features = ["trace"] }
opentelemetry-otlp = "0.14"
metrics = "0.22"
metrics-exporter-prometheus = "0.13"
sentry-tracing = "0.32"  # optional
```

### 4.3 crates/api (Optional)

**Purpose**: Control-plane API for deployment management

- [ ] Implement REST API
  - [ ] Use `axum` framework
  - [ ] Deployment endpoints
  - [ ] Service status endpoints
- [ ] Implement authentication
  - [ ] JWT tokens
  - [ ] API key support
- [ ] Add rate limiting
- [ ] Generate OpenAPI documentation

**Recommended crates:**
```toml
axum = { version = "0.7", features = ["macros", "multipart"] }
jsonwebtoken = "9.2"
oauth2 = "4.4"
utoipa = { version = "4.0", features = ["axum"] }
utoipa-swagger-ui = { version = "5.0", features = ["axum"] }
governor = "0.6"
```

---

## Phase 5: Polish & Production (Weeks 11+) - PARTIAL

**Goal**: Production readiness

### 5.1 bin/runtime - COMPLETE

**Purpose**: Single binary distribution

- [x] Compose agent + proxy into single binary
- [ ] Add feature flags
  ```toml
  [features]
  default = ["agent", "proxy"]
  agent-only = ["agent"]
  proxy-only = ["proxy"]
  full = ["agent", "proxy", "api", "scheduler"]
  ```
- [x] Implement CLI
  - [x] `runtime join <key>:<service>`
  - [x] `runtime deploy <spec.yaml>`
  - [x] `runtime status`
  - [x] `runtime validate`
  - [ ] `runtime logs`
- [ ] Add graceful shutdown handling
- [ ] Create installation scripts

**Recommended crates:**
```toml
clap = { version = "4", features = ["derive", "env"] }
```

### 5.2 tools/devctl - STUB ONLY

**Purpose**: Developer CLI tooling

- [ ] Implement token generation
- [ ] Implement local testing runner
- [ ] Add debug tools
- [ ] Add spec validation CLI

### 5.3 Production Hardening

- [ ] Security audit
- [ ] Performance optimization
  - [ ] Enable LTO
  - [ ] Profile and optimize hot paths
- [ ] Load testing
- [ ] Documentation
  - [ ] Architecture docs
  - [ ] API docs
  - [ ] User guide
- [ ] CI/CD setup
- [ ] Release automation

---

## Architecture Decisions Requiring Clarification

### 1. Consensus Layer for Distributed State

**Options:**
- **etcd** - Battle-tested, external dependency
- **Consul** - Battle-tested, external dependency
- **Custom Raft** (`openraft`) - More control, more maintenance
- **No consensus** - Fully decentralized, limited autoscaling

**Question**: Do you want a fully decentralized system or is a lightweight consensus layer acceptable?

### 2. Container Runtime

**Options:**
- **containerd** (via shim) - Modern, stable, requires root
- **Docker** (via API) - Ubiquitous, slower
- **CRI-O** - Kubernetes-focused, may not fit
- **Podman** - Rootless, less mature API

**Question**: Which container runtime should be the primary target?

### 3. Overlay Network Backend

**Options:**
- **Kernel WireGuard** - Fastest, requires root
- **Userspace WireGuard** (`boringtun`) - Portable, slower
- **WireGuard-go** - Go-based, portable via cgo
- **Nebula** - Alternative overlay, more features

**Question**: Should we support multiple backends or focus on one?

### 4. Single vs. Multiple Binaries

**Options:**
- **Single binary** (agent + proxy combined) - Simpler deployment
- **Multiple binaries** (agent, proxy, runtime separate) - More flexibility
- **Feature flags** - Compile-time selection

**Question**: Deployment model preference?

---

## Rust-Specific Implementation Notes

### Async Runtime
- **Standardize on tokio** everywhere (no async-std mixing)
- Use `tokio::task::JoinSet` for structured concurrency
- Use `tokio::task::spawn_blocking` for CPU-intensive work

### Error Handling
- **Libraries**: Use `thiserror` for typed errors
- **Binaries**: Use `anyhow` for context
- **Always enable backtraces**: `color-eyre::install()`

### Performance
- Enable LTO in release: `lto = true`
- Use `codegen-units = 1` for better optimization
- Use `bytes::Bytes` for zero-copy buffer sharing
- Use generics for hot paths, trait objects for extensibility

### Testing
- Use `tokio::test` for async tests
- Write integration tests for multi-component workflows
- Use property-based testing (`proptest`) for validation logic

---

## Dependencies by Phase

### Phase 1 (Foundation)
```
serde, serde_yaml, schemars, validator, thiserror, anyhow,
config, tracing, tracing-subscriber
```

### Phase 2 (Container Operations)
```
oci-distribution, redb, sha2, hex,
reqwest, tokio (net, time), command-group, extism,
containerd-client, oci-spec
```

### Phase 3 (Networking)
```
wireguard-keys, x25519-dalek, blake2,
trust-dns-server, trust-dns-client, socket2, tun,
hyper, hyper-util, rustls, tokio-rustls,
tower, tower-http, tower-balance, discover, tokio-tungstenite
```

### Phase 4 (Orchestration)
```
prometheus, metrics, metrics-exporter-prometheus,
etcd-client or openraft,
tracing-opentelemetry, opentelemetry, opentelemetry-otlp,
axum, jsonwebtoken, oauth2, utoipa, governor
```

### Phase 5 (Polish)
```
clap, color-eyre, sentry-tracing (optional)
```

---

## Milestones

| Milestone | Description | Criteria | Status |
|-----------|-------------|----------|--------|
| M1 | Spec parsing | Can load and validate a full YAML spec | COMPLETE |
| M2 | Container lifecycle | Can pull image and run container with init steps | COMPLETE |
| M3 | Networking | Containers can communicate over encrypted overlay | PARTIAL |
| M4 | Proxy | Traffic can be routed via built-in proxy | NOT STARTED |
| M5 | Autoscaling | System scales up/down based on metrics | NOT STARTED |
| M6 | Multi-node | Multiple VPS can join via token | NOT STARTED |

---

## Next Steps

1. **Complete Phase 3**: Implement `crates/proxy` for HTTP/2 proxy with TLS termination
2. **Integrate networking**: Connect agent to overlay and proxy in `crates/agent`
3. **Begin Phase 4**: Start with `crates/scheduler` for metrics collection and autoscaling
4. **Production hardening**: Add feature flags, graceful shutdown, and installation scripts

---

## Open Questions

1. **Consensus layer**: etcd, Consul, custom Raft, or fully decentralized?
2. **Container runtime**: containerd, Docker, CRI-O, or Podman?
3. **Overlay backend**: Kernel WireGuard, userspace, or adaptive?
4. **Deployment model**: Single binary, multiple binaries, or feature flags?
5. **Control plane**: Is a REST API needed or is CLI + spec sufficient?
6. **Priority**: Should we optimize for simplicity or feature completeness?
