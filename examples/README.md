# ZLayer Examples

This directory contains example deployments demonstrating ZLayer's capabilities for both WebAssembly (WASM) modules and traditional container workloads.

## Overview

ZLayer is a unified deployment platform that supports:
- **WebAssembly modules**: Lightweight, sandboxed, near-native performance
- **Container workloads**: Full OCI container support with Docker compatibility

## Directory Structure

```
examples/
├── README.md                    # This file
├── simple.yaml                  # Minimal deployment example
├── simple.zlayer.yml            # Same as above, canonical filename
├── full-service.yaml            # Complete service configuration
├── zimagefile-node/             # ZImagefile + Dockerfile: Node.js multi-stage
├── zimagefile-rust/             # ZImagefile + Dockerfile: Rust with cache mounts
├── zimagefile-runtime/          # ZImagefile: runtime template shorthand
├── wasm/                        # WebAssembly examples
│   ├── rust-hello/              # Rust WASM module
│   ├── go-hello/                # Go WASM module (TinyGo)
│   └── deployment.yaml          # WASM deployment manifest
└── containers/                  # Container examples
    ├── nginx-static/            # Static content server
    ├── postgres-db/             # PostgreSQL database
    ├── redis-cache/             # Redis cache
    └── multi-service/           # Full application stack
```

## Quick Start

### Prerequisites

1. **ZLayer CLI installed**:
   ```bash
   curl -sSL https://get.zlayer.io | sh
   ```

2. **ZLayer cluster access** (or local development mode):
   ```bash
   zlayer cluster init --local
   ```

### Deploy an Example

```bash
# Simple nginx deployment
zlayer deploy -f examples/simple.yaml

# Check status
zlayer status my-simple-app
```

## Examples

### WASM Examples

WebAssembly modules run in a sandboxed environment with:
- Near-native performance
- Fine-grained capability control
- Language-agnostic (Rust, Go, C/C++, AssemblyScript)
- Sub-millisecond cold starts

| Example | Language | Description |
|---------|----------|-------------|
| [rust-hello](wasm/rust-hello/) | Rust | Minimal Rust WASM module |
| [go-hello](wasm/go-hello/) | Go (TinyGo) | Minimal Go WASM module |

**Deploy WASM examples:**
```bash
cd examples/wasm/rust-hello
cargo component build --release
zlayer deploy --wasm target/wasm32-wasip2/release/rust_hello_wasm.wasm
```

### Container Examples

Standard OCI containers for full application support:

| Example | Image | Description |
|---------|-------|-------------|
| [nginx-static](containers/nginx-static/) | nginx:alpine | Static file serving |
| [postgres-db](containers/postgres-db/) | postgres:16 | PostgreSQL with persistence |
| [redis-cache](containers/redis-cache/) | redis:7 | Redis caching layer |
| [multi-service](containers/multi-service/) | Multiple | Complete application stack |

**Deploy container examples:**
```bash
# Simple nginx
zlayer deploy -f examples/containers/nginx-static/deployment.yaml

# PostgreSQL (requires password)
export POSTGRES_PASSWORD="secure-password"
zlayer deploy -f examples/containers/postgres-db/deployment.yaml

# Multi-service stack
export POSTGRES_PASSWORD="secure-password"
export API_SECRET_KEY="your-secret"
zlayer deploy -f examples/containers/multi-service/deployment.yaml
```

### ZImagefile Examples (Image Builds)

ZImagefiles are ZLayer's YAML-based alternative to Dockerfiles. They support
the same features (multi-stage builds, cache mounts, cross-stage copies) in
a structured YAML format.

| Example | Mode | Description |
|---------|------|-------------|
| [zimagefile-node](zimagefile-node/) | Multi-stage | Node.js app with builder + runtime stages |
| [zimagefile-rust](zimagefile-rust/) | Multi-stage | Rust binary with cargo cache mounts |
| [zimagefile-runtime](zimagefile-runtime/) | Runtime template | Zero-config Node.js build via `runtime: node22` |

Each ZImagefile example includes a side-by-side Dockerfile so you can compare
the two formats directly.

**Build ZImagefile examples:**
```bash
# ZLayer auto-detects ZImagefile in the current directory
cd examples/zimagefile-node
zlayer build -t my-node-app:latest .

# Or specify explicitly
zlayer build -f ZImagefile -t my-rust-app:latest examples/zimagefile-rust/
```

### Canonical Deployment Filename (.zlayer.yml)

ZLayer auto-detects files named `*.zlayer.yml` in the current directory, so
you can deploy without specifying `-f`:

```bash
# Deploys using simple.zlayer.yml automatically
cd examples
zlayer deploy
```

See [simple.zlayer.yml](simple.zlayer.yml) for an example.

## Deployment Manifest Reference

### Minimal Example (simple.yaml)

```yaml
version: v1
deployment: my-app

services:
  web:
    rtype: service
    image:
      name: nginx:alpine
    endpoints:
      - name: http
        port: 80
        expose: public
    scale:
      replicas: 2
```

### Full Configuration (full-service.yaml)

See [full-service.yaml](full-service.yaml) for a complete example with:
- Resource limits
- Environment variables
- Network overlays
- Service dependencies
- Health checks
- Initialization steps
- Autoscaling configuration
- Cron jobs

## WASM vs Containers

| Feature | WASM | Containers |
|---------|------|------------|
| Startup time | Sub-millisecond | Seconds |
| Memory overhead | Minimal | ~20-50MB per container |
| Isolation | Capability-based sandbox | Namespace isolation |
| Ecosystem | Growing | Mature |
| Use cases | Functions, plugins, edge | Full applications |

**When to use WASM:**
- Lightweight functions and plugins
- Edge computing
- Plugin systems
- Performance-critical code paths

**When to use Containers:**
- Full application stacks
- Existing Docker images
- Complex dependencies
- Database servers

## ZLayer Auto-Detection

ZLayer automatically detects the workload type:

1. **WASM detection**: File extension (`.wasm`) or binary magic number
2. **Container detection**: OCI image reference format

You can also explicitly specify:

```yaml
runtime:
  type: wasm  # or 'container'
```

## Environment Variables

Use environment variables in deployments:

```yaml
env:
  # Direct value
  MY_VAR: "value"

  # From host environment
  DATABASE_URL: $E:DATABASE_URL

  # From ZLayer secrets
  API_KEY: $S:API_KEY_SECRET
```

## Service Discovery

Services communicate using internal DNS:

```
<service-name>.<deployment-name>.service:<port>
```

Example:
```
database.my-app.service:5432
```

Or simplified within the same deployment:
```
database.service:5432
```

## Health Checks

Supported health check types:

```yaml
health:
  check:
    # HTTP endpoint
    type: http
    url: http://localhost:8080/health
    expect_status: 200

    # TCP port
    type: tcp
    port: 5432

    # Command execution
    type: exec
    command: ["pg_isready", "-U", "user"]
```

## Scaling

### Fixed Scaling

```yaml
scale:
  mode: fixed
  replicas: 3
```

### Adaptive Scaling

```yaml
scale:
  mode: adaptive
  min: 2
  max: 20
  cooldown: 30s
  targets:
    cpu: 70      # Scale at 70% CPU
    rps: 1000    # Scale at 1000 requests/second
    memory: 80   # Scale at 80% memory
```

## More Resources

- [ZLayer Documentation](https://docs.zlayer.io)
- [ZLayer SDK (Rust)](../clients/zlayer-sdk/rust/)
- [ZLayer SDK Examples](../clients/zlayer-sdk/rust/examples/)
- [Deployment Specification](https://docs.zlayer.io/spec/deployment)

## License

MIT OR Apache-2.0
