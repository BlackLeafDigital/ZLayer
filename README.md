# ZLayer

A lightweight, Rust-based container orchestration platform with built-in networking, scaling, and observability.

## Overview

ZLayer provides declarative container orchestration without Kubernetes complexity. It uses [libcontainer](https://github.com/youki-dev/youki) (from the youki project) for direct container management - no daemon required.

### Key Features

- **Daemonless Runtime** - Uses libcontainer directly, no containerd/Docker daemon needed
- **Encrypted Overlay Networks** - WireGuard-based mesh networking between nodes
- **Built-in Proxy** - TLS termination, HTTP/2, load balancing on every node
- **Adaptive Autoscaling** - Scale based on CPU, memory, or requests per second
- **Init Actions** - Pre-start lifecycle hooks (wait for TCP, HTTP, run commands)
- **Health Checks** - TCP, HTTP, and command-based health monitoring
- **OCI Compatible** - Pull images from any OCI-compliant registry

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         ZLayer Node                          │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────────────┐ │
│  │  Proxy  │  │ Agent   │  │Scheduler│  │   Observability │ │
│  │  (TLS)  │  │(Runtime)│  │ (Raft)  │  │ (Metrics/Logs)  │ │
│  └────┬────┘  └────┬────┘  └────┬────┘  └────────┬────────┘ │
│       │            │            │                 │          │
│       └────────────┴────────────┴─────────────────┘          │
│                           │                                   │
│              ┌────────────┴────────────┐                     │
│              │     libcontainer        │                     │
│              │   (OCI Runtime Layer)   │                     │
│              └────────────┬────────────┘                     │
│                           │                                   │
│       ┌───────────────────┼───────────────────┐              │
│       │                   │                   │              │
│  ┌────┴────┐        ┌────┴────┐        ┌────┴────┐         │
│  │Container│        │Container│        │Container│         │
│  └─────────┘        └─────────┘        └─────────┘         │
└─────────────────────────────────────────────────────────────┘
```

## Project Structure

```
crates/
├── agent/          # Container runtime (libcontainer integration)
├── api/            # REST API server
├── init_actions/   # Pre-start lifecycle actions
├── observability/  # Metrics, logging, tracing
├── overlay/        # WireGuard overlay networking
├── proxy/          # L4/L7 proxy with TLS
├── registry/       # OCI image pulling and caching
├── scheduler/      # Raft-based distributed scheduler
├── spec/           # Deployment specification types
└── zlayer-core/    # Shared types and configuration

bin/
└── runtime/        # Main zlayer binary

tools/
└── devctl/         # Development CLI tool
```

## Requirements

- Linux (kernel 5.4+)
- Rust 1.78+
- libseccomp-dev

```bash
# Ubuntu/Debian
sudo apt-get install libseccomp-dev

# Fedora/RHEL
sudo dnf install libseccomp-devel

# Arch
sudo pacman -S libseccomp
```

## Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test --workspace
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

## Development

```bash
# Format code
cargo fmt

# Run clippy
cargo clippy --workspace -- -D warnings

# Run specific crate tests
cargo test -p agent
cargo test -p registry

# Build with specific features
cargo build --package runtime --features full
```

## License

MIT OR Apache-2.0
