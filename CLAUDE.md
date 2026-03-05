# CLAUDE.md - ZLayer Repository Guide

This document provides guidance for AI assistants working on the ZLayer codebase.

## Repository Overview

ZLayer is a lightweight, Rust-based container orchestration platform with built-in networking, scaling, and observability. It uses libcontainer (from youki) for direct container management without requiring a daemon.

## Project Structure

```
bin/
  runtime/         # zlayer-runtime binary (full orchestration runtime)
  zlayer-build/    # Lightweight image builder CLI
  zlayer/          # Main zlayer CLI (interactive TUI + runtime passthrough)

crates/
  zlayer-agent/    # Container runtime (libcontainer integration, storage manager)
  zlayer-api/      # REST API server with build endpoints and streaming
  zlayer-builder/  # Dockerfile/ZImagefile parsing, buildah integration, runtime templates
  zlayer-core/     # Shared types and configuration
  zlayer-init-actions/  # Pre-start lifecycle actions (TCP, HTTP, S3, commands)
  zlayer-manager/  # Web-based management UI (Leptos SSR + WASM)
  zlayer-observability/ # Metrics, logging, OpenTelemetry tracing
  zlayer-overlay/  # Encrypted overlay networking (boringtun), IP allocation, DNS discovery
  zlayer-proxy/    # L4/L7 proxy with TLS
  zlayer-py/       # Python bindings
  zlayer-registry/ # OCI image pulling and caching
  zlayer-scheduler/# Raft-based distributed scheduler
  zlayer-secrets/  # Secrets management
  zlayer-spec/     # Deployment specification types
  zlayer-storage/  # Storage backends (local, S3)
  zlayer-tunnel/   # Secure tunneling for node-to-node access
  zlayer-web/      # Web frontend (Leptos SSR + hydration)

docker/            # Container build files (Dockerfiles, ZImagefiles)
examples/          # Example deployments and ZImagefiles
wit/               # WebAssembly interface definitions
```

## Key Crates

### bin/runtime
The `zlayer-runtime` binary. Provides full orchestration including:
- Container lifecycle management via libcontainer
- Encrypted overlay networking (boringtun userspace WireGuard)
- REST API server
- Raft-based scheduling
- WASM runtime via wasmtime

### bin/zlayer-build
Lightweight CLI for image building only (~10MB). Depends only on `zlayer-builder` and `zlayer-spec`. Commands:
- `build` - Build images from Dockerfile, ZImagefile, or runtime template
- `pipeline` - Build multiple images from ZPipeline.yaml
- `runtimes` - List available runtime templates
- `validate` - Parse and validate build files

### bin/zlayer
The main `zlayer` CLI. Provides an interactive TUI for image building (same capabilities as `zlayer-build` but menu-driven) and passes through runtime commands to `zlayer-runtime` when available.

### crates/zlayer-builder
Core image building library. Key modules:
- `builder.rs` - High-level `ImageBuilder` API
- `dockerfile/` - Dockerfile parsing and instruction types
- `zimage/` - ZImagefile YAML parsing and Dockerfile conversion
- `pipeline/` - ZPipeline multi-image build orchestration
- `buildah/` - Buildah command generation and execution
- `templates/` - Runtime templates for common languages (node, python, rust, go)
- `tui/` - Terminal UI for build progress

### crates/zlayer-spec
Deployment specification types. Defines the YAML schema for:
- Services, jobs, and cron resources
- Scaling modes (adaptive, fixed, manual)
- Storage configuration
- Health checks
- Tunneling

## Build System

### Cargo Workspace
The project uses a Cargo workspace with all crates defined in the root `Cargo.toml`. Key profiles:
- `dev` - Fast incremental builds with optimized dependencies
- `release` - Full LTO, single codegen unit, stripped symbols
- `release-fast` - Faster release builds for development

### Building

```bash
# Full runtime binary (produces zlayer-runtime)
cargo build --release --package runtime

# Lightweight builder CLI
cargo build --release -p zlayer-build

# Main CLI (produces zlayer)
cargo build --release -p zlayer

# All tests
cargo test --workspace
```

### ZImagefiles
YAML-based alternative to Dockerfiles. Located in `docker/` directory:
- `ZImagefile.zlayer-node` - Node with embedded containerd
- `ZImagefile.zlayer-web` - Leptos web frontend
- `ZImagefile.zlayer-manager` - Management UI

### ZPipeline
Multi-image build pipeline defined in `ZPipeline.yaml` (also accepts `zlayer-pipeline.yaml`). Supports:
- Variable substitution (`${VERSION}`, `${REGISTRY}`)
- Dependency ordering via `depends_on`
- Wave-based parallel execution
- Coordinated push operations

Build pipeline:
```bash
zlayer-build pipeline --set VERSION=0.1.0           # Auto-discovers ZPipeline.yaml or zlayer-pipeline.yaml
zlayer-build pipeline -f ZPipeline.yaml --set VERSION=0.1.0  # Explicit path
```

## Code Patterns

### Error Handling
- Library crates use `thiserror` for structured errors
- Binary crates use `anyhow` for ergonomic error handling
- All public APIs return `Result` types

### Async Runtime
- Tokio multi-threaded runtime
- `async-trait` for async trait methods
- Structured concurrency with `JoinSet` for parallel operations

### Serialization
- `serde` with `serde_yaml` for configuration files
- `serde_json` for API responses
- Custom deserializers for flexible input parsing

### Testing
- Unit tests in `#[cfg(test)]` modules
- Integration tests in `tests/` directories
- Property-based testing where appropriate

## Development Workflow

### Running Locally

```bash
# Start the API server (via zlayer-runtime)
cargo run --package runtime -- serve --bind 0.0.0.0:3669

# Deploy a spec (via zlayer-runtime)
cargo run --package runtime -- deploy deployment.yaml

# Build an image
cargo run -p zlayer-build -- build . -t myapp:latest
```

### Formatting and Linting

```bash
cargo fmt --all
cargo clippy --workspace -- -D warnings
```

## Changelog

**Always update CHANGELOG.md when making changes.** Keep it updated as you work, not just at release time.

Format:
```markdown
## [Unreleased]

### Added
- New feature X

### Changed
- Modified behavior Y

### Fixed
- Bug fix Z
```

When a release is cut, the [Unreleased] section becomes the new version.

## AFTER ANY CODE CHANGE: TEST IT

Read the diff. Write a test for the exact thing that changed. Run it. If it touches kernel APIs (network interfaces, cgroups, namespaces), test against kernel limits. `cargo test` passing is meaningless if the broken path has no test. Interface names max 15 chars (IFNAMSIZ). See `make_interface_name()` in `overlay_manager.rs`.

## Important Notes

- Linux only for production (uses libcontainer/cgroups)
- Requires `libseccomp-dev` for building
- Buildah required for image building (auto-installed if missing)
- Minimum Rust version: 1.85
