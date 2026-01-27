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
- **Init Actions** - Pre-start lifecycle hooks (wait for TCP, HTTP, run commands)
- **Health Checks** - TCP, HTTP, and command-based health monitoring
- **OCI Compatible** - Pull images from any OCI-compliant registry
- **REST API** - Deploy, manage, and build images via HTTP API with streaming progress

## Architecture

```mermaid
graph TB
    subgraph ZLayer Node
        API[REST API]
        Proxy[Proxy<br/>TLS/HTTP2/LB]
        Agent[Agent<br/>Runtime]
        Scheduler[Scheduler<br/>Raft]
        Obs[Observability<br/>Metrics/Logs]

        API --> Agent
        Proxy --> Agent
        Scheduler --> Agent
        Obs --> Agent

        subgraph Runtime Layer
            LC[libcontainer<br/>OCI Runtime]
            LC --> C1[Container]
            LC --> C2[Container]
            LC --> C3[Container]
        end

        Agent --> LC
    end

    subgraph Builder Subsystem
        DF[Dockerfile Parser]
        BA[Buildah Executor]
        RT[Runtime Templates]
        DF --> BA
        RT --> BA
    end

    subgraph Overlay Networking
        IP[IP Allocator]
        Boot[Bootstrap<br/>Join/Init]
        WG[WireGuard Mesh]
        IP --> Boot --> WG
    end

    Agent --> Builder Subsystem
    Agent --> Overlay Networking
```

## Project Structure

```
crates/
├── agent/          # Container runtime (libcontainer integration)
├── api/            # REST API server with build endpoints and streaming
├── builder/        # Dockerfile parser, buildah integration, runtime templates
├── init_actions/   # Pre-start lifecycle actions
├── observability/  # Metrics, logging, tracing
├── overlay/        # WireGuard overlay networking, IP allocation, health checks
├── proxy/          # L4/L7 proxy with TLS
├── registry/       # OCI image pulling and caching
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
