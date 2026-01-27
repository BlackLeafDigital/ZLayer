# ZLayer

A lightweight, Rust-based container orchestration platform with built-in networking, scaling, and observability.

## Overview

ZLayer provides declarative container orchestration without Kubernetes complexity. It uses [libcontainer](https://github.com/youki-dev/youki) (from the youki project) for direct container management - no daemon required.

### Key Features

- **Daemonless Runtime** - Uses libcontainer directly, no containerd/Docker daemon needed
- **Built-in Image Builder** - Dockerfile parser with buildah integration and runtime templates
- **Encrypted Overlay Networks** - WireGuard-based mesh networking with IP allocation and health checking
- **Smart Scheduler** - Node placement with Shared/Dedicated/Exclusive allocation modes
- **Built-in Proxy** - TLS termination, HTTP/2, load balancing on every node
- **Adaptive Autoscaling** - Scale based on CPU, memory, or requests per second
- **Init Actions** - Pre-start lifecycle hooks (wait for TCP, HTTP, S3 pull/push, run commands)
- **Health Checks** - TCP, HTTP, and command-based health monitoring
- **OCI Compatible** - Pull images from any OCI-compliant registry
- **REST API** - Deploy, manage, and build images via HTTP API with streaming progress
- **S3 Layer Persistence** - Persist container state to S3 with crash-tolerant uploads
- **OpenTelemetry Tracing** - Distributed tracing with OTLP export and context propagation

## Architecture

```mermaid
graph TB
    subgraph Node[ZLayer Node]
        API[REST API]
        Proxy[Proxy TLS/HTTP2/LB]
        Agent[Agent Runtime]
        Scheduler[Scheduler Raft]
        Obs[Observability]

        API --> Agent
        Proxy --> Agent
        Scheduler --> Agent
        Obs --> Agent

        subgraph Runtime[Runtime Layer]
            LC[libcontainer]
            SM[Storage Manager]
            LC --> C1[Container]
            LC --> C2[Container]
            LC --> C3[Container]
        end

        Agent --> LC
        Agent --> SM
    end

    subgraph Builder[Builder Subsystem]
        DF[Dockerfile Parser]
        BA[Buildah Executor]
        RT[Runtime Templates]
        DF --> BA
        RT --> BA
    end

    subgraph Overlay[Overlay Networking]
        IP[IP Allocator]
        Boot[Bootstrap]
        WG[WireGuard Mesh]
        IP --> Boot --> WG
    end

    subgraph Storage[Storage Backends]
        LV[Local Volumes]
        S3C[S3 Cache]
        LS[Layer Storage]
        LV --> SM
        S3C --> SM
        LS --> SM
    end

    Agent --> Builder
    Agent --> Overlay
```

## Project Structure

```
crates/
├── agent/          # Container runtime (libcontainer integration, storage manager)
├── api/            # REST API server with build endpoints and streaming
├── builder/        # Dockerfile parser, buildah integration, runtime templates
├── init_actions/   # Pre-start lifecycle actions (TCP, HTTP, S3, commands)
├── layer-storage/  # S3-backed layer persistence with crash-tolerant uploads
├── observability/  # Metrics, logging, OpenTelemetry tracing
├── overlay/        # WireGuard overlay networking, IP allocation, health checks
├── proxy/          # L4/L7 proxy with TLS
├── registry/       # OCI image pulling and caching (with optional S3 backend)
├── scheduler/      # Raft-based distributed scheduler with placement logic
├── spec/           # Deployment specification types
└── zlayer-core/    # Shared types and configuration

bin/
└── runtime/        # Main zlayer binary
```

## Requirements

- Linux (kernel 5.4+)
- Rust 1.85+
- libseccomp-dev

```bash
# Ubuntu/Debian
sudo apt-get install libseccomp-dev

# Fedora/RHEL
sudo dnf install libseccomp-devel

# Arch
sudo pacman -S libseccomp
```

## Installation

### Quick Install (Recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/BlackLeafDigital/ZLayer/main/install.sh | bash
```

### From Package Registry

Download the latest release for your architecture:

```bash
# For amd64 (latest)
curl -fsSL https://forge.blackleafdigital.com/api/packages/BlackLeafDigital/generic/zlayer/latest/zlayer-linux-amd64.tar.gz | tar xz
sudo mv zlayer /usr/local/bin/

# For arm64 (latest)
curl -fsSL https://forge.blackleafdigital.com/api/packages/BlackLeafDigital/generic/zlayer/latest/zlayer-linux-arm64.tar.gz | tar xz
sudo mv zlayer /usr/local/bin/
```

Or pin to a specific version:

```bash
# Replace VERSION with desired version (e.g., v0.1.0)
curl -fsSL https://forge.blackleafdigital.com/api/packages/BlackLeafDigital/generic/zlayer/VERSION/zlayer-linux-amd64.tar.gz | tar xz
```

### From Source

```bash
# Clone the repo
git clone https://github.com/BlackLeafDigital/ZLayer.git
cd ZLayer

# Install dependencies (Ubuntu/Debian)
sudo apt-get install -y protobuf-compiler libseccomp-dev libssl-dev pkg-config cmake

# Build release binary
cargo build --release --package runtime

# Install
sudo cp target/release/zlayer /usr/local/bin/
```

## Quick Start

### 1. Define a deployment

```yaml
# deployment.yaml
version: v1
deployment: my-app

services:
  web:
    rtype: service
    image:
      name: nginx:latest
    endpoints:
      - name: http
        protocol: http
        port: 80
        expose: public
    scale:
      mode: adaptive
      min: 1
      max: 5
    health:
      check:
        type: http
        url: /health
        expect_status: 200
```

### 2. Deploy

```bash
zlayer deploy deployment.yaml
```

### 3. Check status

```bash
zlayer status my-app
```

## Deployment Spec

See [V1_SPEC.md](./V1_SPEC.md) for the complete specification.

### Resource Types

| Type | Description |
|------|-------------|
| `service` | Long-running, load-balanced container |
| `job` | Run-to-completion, triggered by endpoint/CLI |
| `cron` | Scheduled run-to-completion |

### Scaling Modes

| Mode | Description |
|------|-------------|
| `adaptive` | Auto-scale based on CPU/memory/RPS targets |
| `fixed` | Fixed number of replicas |
| `manual` | No automatic scaling |

### Node Allocation Modes

| Mode | Description |
|------|-------------|
| `shared` | Containers bin-packed onto nodes with available capacity |
| `dedicated` | Each replica gets its own node (1:1 mapping) |
| `exclusive` | Service has nodes exclusively to itself (no other services) |

### Storage & Persistence

ZLayer provides comprehensive storage options for containers.

#### Volume Mounts

Mount external storage into containers:

| Type | Description | Lifecycle |
|------|-------------|-----------|
| `bind` | Host path mounted into container | Host-managed |
| `named` | Persistent named volume | Survives container restarts |
| `anonymous` | Auto-named volume | Cleaned on container removal |
| `tmpfs` | Memory-backed filesystem | Lost on container stop |
| `s3` | S3-backed FUSE mount (requires s3fs) | Remote-managed |

#### Storage Tiers

Named and anonymous volumes support configurable performance tiers:

| Tier | Description | SQLite Safe |
|------|-------------|-------------|
| `local` | Direct local filesystem (SSD/NVMe) | Yes |
| `cached` | bcache-backed tiered storage | Yes |
| `network` | NFS/network storage | No |

#### Layer Persistence

Persist container filesystem changes (OverlayFS upper layer) to S3:

- **Automatic snapshots** - Tar + zstd compress the upper layer on container stop
- **Crash-tolerant uploads** - Multipart S3 uploads with resume capability
- **Content-addressable** - Layers keyed by SHA256 digest (automatic deduplication)
- **Cross-node restore** - Containers can resume state on different nodes

#### S3 Init Actions

Transfer files to/from S3 as part of container lifecycle:

| Action | Description |
|--------|-------------|
| `init.s3_pull` | Download files from S3 before container starts |
| `init.s3_push` | Upload files to S3 after container stops |

#### Example Storage Configuration

```yaml
services:
  api:
    image:
      name: myapi:latest
    storage:
      # Bind mount (read-only config)
      - type: bind
        source: /etc/myapp/config
        target: /app/config
        readonly: true

      # Named persistent volume
      - type: named
        name: api-data
        target: /app/data
        tier: local  # SQLite-safe

      # Anonymous cache (auto-cleaned)
      - type: anonymous
        target: /app/cache

      # Memory-backed temp
      - type: tmpfs
        target: /app/tmp
        size: 256Mi
        mode: 1777

      # S3 model storage (requires s3fs)
      - type: s3
        bucket: my-models
        prefix: v1/
        target: /app/models
        readonly: true
        endpoint: https://s3.us-west-2.amazonaws.com

    # S3 init actions for artifact transfer
    init:
      - action: init.s3_pull
        bucket: my-artifacts
        key: models/latest.bin
        destination: /app/models/model.bin
```

## Image Building

ZLayer includes a built-in image builder that supports Dockerfiles and runtime templates.

### Runtime Templates

Build images without writing a Dockerfile using pre-configured templates:

| Runtime | Description |
|---------|-------------|
| `node20`, `node22` | Node.js apps on Alpine |
| `python312`, `python313` | Python apps on Debian slim |
| `rust` | Static musl binaries |
| `go` | Static Go binaries |
| `deno` | Deno JavaScript/TypeScript runtime |
| `bun` | Bun JavaScript runtime |

### Building via API

```bash
# Build from a Dockerfile
curl -X POST http://localhost:3000/api/v1/build/json \
  -H "Content-Type: application/json" \
  -d '{
    "context_path": "/path/to/app",
    "tags": ["myapp:latest"]
  }'

# Build using a runtime template
curl -X POST http://localhost:3000/api/v1/build/json \
  -H "Content-Type: application/json" \
  -d '{
    "context_path": "/path/to/node/app",
    "runtime": "node22",
    "tags": ["myapp:latest"]
  }'

# Stream build progress via SSE
curl http://localhost:3000/api/v1/build/{id}/stream
```

### Building via CLI

```bash
# Build from Dockerfile
zlayer build /path/to/app -t myapp:latest

# Build with runtime template (auto-detected)
zlayer build /path/to/app -t myapp:latest --runtime node22

# List available templates
zlayer runtimes
```

## CLI Reference

The `zlayer` binary provides comprehensive command-line management:

### Node Management

```bash
# Initialize a new cluster (this node becomes leader)
zlayer node init --advertise-addr 10.0.0.1

# Join an existing cluster
zlayer node join 10.0.0.1:8080 --token <TOKEN> --advertise-addr 10.0.0.2

# List nodes in cluster
zlayer node list

# Show node status
zlayer node status

# Add labels to nodes (for node selectors)
zlayer node label <node-id> gpu=true

# Generate a join token for workers
zlayer node generate-join-token -d my-deploy -a http://10.0.0.1:8080
```

### Deployment Management

```bash
# Deploy from spec file
zlayer deploy deployment.yaml

# Validate a deployment spec
zlayer validate deployment.yaml

# View deployment status
zlayer status

# Stream logs from a service
zlayer logs -d my-app -s web --follow

# Stop a deployment
zlayer stop my-app --service web
```

### Build Commands

```bash
# Build from Dockerfile
zlayer build . -t myapp:latest

# Build with runtime template
zlayer build . --runtime node22 -t myapp:latest

# Auto-detect runtime from project files
zlayer build . --runtime-auto -t myapp:latest

# Build with arguments
zlayer build . --build-arg VERSION=1.0 -t myapp:1.0.0

# Multi-stage builds with target
zlayer build . --target production -t myapp:prod

# List available runtime templates
zlayer runtimes
```

### API Server

```bash
# Start the REST API server
zlayer serve --bind 0.0.0.0:8080

# With JWT secret
zlayer serve --bind 0.0.0.0:8080 --jwt-secret <secret>
```

### Token Management

```bash
# Create a JWT token for API access
zlayer token create --subject dev --hours 24 --roles admin

# Create token (quiet mode for scripting)
zlayer token create --quiet

# Decode and inspect a token
zlayer token decode <token>

# Show token system info
zlayer token info
```

### Spec Inspection

```bash
# Dump parsed spec as JSON
zlayer spec dump deployment.yaml --format json

# Dump parsed spec as YAML (default)
zlayer spec dump deployment.yaml

# Validate a spec file
zlayer spec validate deployment.yaml

# Inspect a running deployment
zlayer spec inspect my-deployment --format table
zlayer spec inspect my-deployment --api http://localhost:8080 --format json
```

### Local Development

```bash
# Run a deployment locally (development mode)
zlayer run deployment.yaml

# Dry run - validate and show plan
zlayer run deployment.yaml --dry-run

# With port offset for multiple instances
zlayer run deployment.yaml --port-offset 1000

# Production environment
zlayer run deployment.yaml --env prod
```

## Observability

ZLayer includes built-in observability with Prometheus metrics and OpenTelemetry tracing.

### OpenTelemetry Tracing

Distributed tracing with automatic instrumentation of container operations:

- **OTLP Export** - Send traces to any OpenTelemetry-compatible backend (Jaeger, Tempo, etc.)
- **Context Propagation** - W3C Trace Context for distributed trace correlation
- **Container Spans** - Automatic spans for create, start, stop, remove, exec operations
- **Semantic Attributes** - Standard attributes (`container.id`, `service.name`, etc.)

#### Configuration via Environment Variables

```bash
# Enable tracing
export OTEL_TRACES_ENABLED=true
export OTEL_SERVICE_NAME=zlayer-agent
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317

# Sampling (0.0 to 1.0)
export OTEL_TRACES_SAMPLER_ARG=0.1  # Sample 10% of traces

# Environment tag
export DEPLOYMENT_ENVIRONMENT=production
```

### Prometheus Metrics

Expose metrics for scraping:

```bash
zlayer serve --metrics-bind 0.0.0.0:9090
```

Available metrics include container counts, request latencies, and resource utilization.

## Development

```bash
# Format code
cargo fmt

# Run clippy
cargo clippy --workspace -- -D warnings

# Run all tests
cargo test --workspace

# Run specific crate tests
cargo test -p agent
cargo test -p registry
cargo test -p builder
cargo test -p scheduler

# Build with specific features
cargo build --package runtime --features full
```

## License

Apache-2.0
