# CLAUDE.md - ZLayer Repository Guide

This document provides guidance for AI assistants working on the ZLayer codebase.

## Repository Overview

ZLayer is a lightweight, Rust-based container orchestration platform with built-in networking, scaling, and observability. It uses libcontainer (from youki) for direct container management without requiring a daemon.

## Project Structure

```
bin/
  zlayer/          # Main zlayer CLI (orchestration + image building, single binary)
  zlayer-desktop/  # Tauri-based desktop wrapper for the Manager UI (not in workspace)

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

images/            # Container build files (Dockerfiles, ZImagefiles)
examples/          # Example deployments and ZImagefiles
wit/               # WebAssembly interface definitions
```

## Key Crates

### bin/zlayer
The `zlayer` CLI — single binary for orchestration and image building. Provides:
- Container lifecycle management via libcontainer (the agent is built in)
- Encrypted overlay networking (boringtun userspace WireGuard)
- REST API server via `zlayer serve` (add `--daemon` to run in the background)
- Raft-based scheduling
- WASM runtime via wasmtime
- Image building: `build`, `pipeline`, `runtimes`, `validate` (plus an interactive TUI)
- Deployment/service/job/cron subcommands: `deploy`, `logs`, `exec`, `stop`, `ps`, `status`, etc.

CLI subcommands talk to the running daemon over a Unix socket via `DaemonClient`.

### bin/zlayer-desktop
Tauri-based desktop wrapper around the `zlayer-manager` web UI. Not part of the Cargo workspace; built independently via Tauri's tooling.

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
# Main zlayer CLI (single binary: orchestration + image building)
cargo build --release -p zlayer

# All tests
cargo test --workspace
```

### ZImagefiles
YAML-based alternative to Dockerfiles. Located in `images/` directory:
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
zlayer pipeline --set VERSION=0.1.0           # Auto-discovers ZPipeline.yaml or zlayer-pipeline.yaml
zlayer pipeline -f ZPipeline.yaml --set VERSION=0.1.0  # Explicit path
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
# Start the API server (daemon mode)
cargo run -p zlayer -- serve --daemon --bind 0.0.0.0:3669

# Deploy a spec
cargo run -p zlayer -- deploy deployment.yaml

# Build an image
cargo run -p zlayer -- build . -t myapp:latest
```

### Formatting and Linting

**CRITICAL: `cargo fmt --all` MUST be run after EVERY code change. No exceptions. Ever. Do it before clippy, before tests, before commits. If code is not formatted, it is not done.**

**CRITICAL: ALWAYS lint the ENTIRE workspace, not individual crates.** Never use `-p <crate>` for clippy. Always use `--workspace`.

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

After ANY code change, run BOTH commands above on the full workspace before considering the work done. `cargo fmt --all` FIRST, then clippy. No exceptions.

### Pre-commit Hook

A pre-commit hook lives in `.githooks/pre-commit` and runs `cargo fmt --all` + `cargo clippy --workspace --all-targets -- -D warnings` on every commit. Git must be configured to use it:

```bash
git config core.hooksPath .githooks
```

Never skip it with `--no-verify`. If it fails, fix the code.

## Changelog

**Always update CHANGELOG.md when making changes.** Keep it updated as you work, not just at release time. Use the actual upcoming version number as the section header — never `[Unreleased]`.

Format:
```markdown
## 0.2.0 - 2026-04-15

### Added
- New feature X

### Changed
- Modified behavior Y

### Fixed
- Bug fix Z
```

## AFTER ANY CODE CHANGE: TEST IT

Read the diff. Write a test for the exact thing that changed. Run it. If it touches kernel APIs (network interfaces, cgroups, namespaces), test against kernel limits. `cargo test` passing is meaningless if the broken path has no test. Interface names max 15 chars (IFNAMSIZ). See `make_interface_name()` in `overlay_manager.rs`.

## Important Notes

- Linux, macOS, and Windows supported (Windows uses WSL2 backend)
- Requires `libseccomp-dev` for building on Linux
- Buildah required for image building (auto-installed if missing)
- Minimum Rust version: 1.91
