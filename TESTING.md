# ZLayer Testing Guide

## Quick Start

```bash
./scripts/run_dev.sh check   # verify prerequisites
./scripts/run_dev.sh build   # build runtime + manager
./scripts/run_dev.sh test    # run all tests (platform-aware)
./scripts/run_dev.sh dev     # run manager at localhost:4321
./scripts/run_dev.sh         # all of the above (check → build → test)
```

The script detects macOS vs Linux automatically and runs the right tests for your platform. Windows hosts have their own incantations — see the Windows section below.

---

## CI Matrix

The standard CI matrix that contributors are expected to keep green:

| Target          | OS / Arch          | Notes                                                                  |
|-----------------|--------------------|------------------------------------------------------------------------|
| Linux amd64     | x86_64-unknown-linux-gnu | Default native; full workspace + youki/libcontainer E2E         |
| Linux arm64     | aarch64-unknown-linux-gnu | Self-hosted native runner (no cross-builds — see CONTRIBUTING) |
| macOS arm64     | aarch64-apple-darwin | Apple Silicon; Seatbelt sandbox + MPS GPU smoke tests                |
| macOS amd64     | x86_64-apple-darwin | Intel Mac; Seatbelt sandbox (no MPS smoke)                            |
| Windows amd64   | x86_64-pc-windows-msvc | HCS runtime + WSL2 backend; built `--no-default-features --features hcs-runtime,wsl` |

All five targets must build and pass their platform-specific test suites before a release tag is cut.

---

## Prerequisites

### macOS (Apple Silicon)

```bash
# Rust via rustup (NOT Homebrew — Homebrew Rust lacks wasm32 target)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add wasm32-unknown-unknown

# For manager UI builds
cargo install cargo-leptos
cd crates/zlayer-manager && bun install  # daisyui dependency

# Optional: cargo-watch for dev mode
cargo install cargo-watch
```

### cargo-leptos Toolchain Note

If you have both Homebrew Rust and rustup installed, cargo-leptos may pick up the wrong toolchain. Force rustup:

```bash
PATH="$HOME/.rustup/toolchains/1.90-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH" \
  cargo leptos build --release
```

---

## Running Tests

### Full Workspace

```bash
make test
# or
cargo test --workspace
```

### macOS Sandbox E2E (28 tests)

Tests Seatbelt sandbox container lifecycle — create, start, stop, exec, logs, stats, concurrent containers, rootfs cloning, port allocation, GPU profiles, network profiles.

```bash
make test-macos-sandbox
# or
cargo test --package zlayer-agent --test macos_sandbox_e2e -- --nocapture
```

### MPS GPU Smoke Tests (4 tests)

Tests Metal/MPS GPU access inside sandboxed processes on Apple Silicon. Requires Metal-capable GPU.

```bash
make test-mps-smoke
# or
cargo test --package zlayer-agent --test macos_mps_smoke -- --nocapture
```

Verifies:
- `MTLCreateSystemDefaultDevice()` works inside sandbox
- MPSGraph vector addition (1024 floats)
- Runtime Metal shader compilation + GPU dispatch
- GPU access denied when runtime disables it

### Scheduler Tests (86 tests)

```bash
cargo test --package zlayer-scheduler
```

**Note:** The Makefile target `make test-scheduler` uses the stale package name `scheduler`. Use the command above instead.

### Infrastructure Wiring

```bash
cargo test --package zlayer-agent --test infrastructure_wiring -- --nocapture
```

### Individual Crates

The Makefile has stale package names. Use full names:

```bash
cargo test --package zlayer-agent
cargo test --package zlayer-api
cargo test --package zlayer-scheduler
cargo test --package zlayer-spec
cargo test --package zlayer-proxy
cargo test --package zlayer-overlay
cargo test --package zlayer-registry
cargo test --package zlayer-manager --features ssr
```

### Manager UI Tests

```bash
cargo test --package zlayer-manager --features ssr
```

### Linux E2E (youki/libcontainer — requires root)

```bash
make test-e2e
# or single test:
make test-e2e-single TEST=test_cleanup_state_directory
```

### Windows (HCS runtime + WSL2 backend)

Windows uses the HCS (Host Compute Service) runtime for native containers and a WSL2 backend for Linux workloads. Default features are Linux-oriented, so Windows builds and tests **must** disable defaults and opt into `hcs-runtime,wsl`.

#### Build incantation

```powershell
cargo build --release -p zlayer --no-default-features --features hcs-runtime,wsl
```

This is the same invocation used by `.forgejo/workflows/build.yml::build-windows-amd64`.

#### Running tests locally on a Windows host

Run the suite with the same feature gates as the build:

```powershell
# Agent — composite dispatch + Windows overlay/cluster E2E
cargo test --no-default-features --features hcs-runtime,wsl -p zlayer-agent

# Builder — Windows Dockerfile templates + HCS build E2E
cargo test --no-default-features --features hcs-runtime,wsl -p zlayer-builder

# HCS wrapper integration tests
cargo test --no-default-features --features hcs-runtime,wsl -p zlayer-hcs
```

Some tests touch HCS, the Windows firewall, or `netsh` and require an **elevated (Administrator) PowerShell**. Launch the shell with "Run as administrator" before invoking `cargo test` for the agent or HCS suites.

#### Remote-check workflow (MiniWindows CI host)

Most contributors do not have a Windows host handy. The repo ships a remote-check script that rsyncs the workspace to the MiniWindows CI host over SSH and runs the suite there:

```bash
bash scripts/windows-remote-check.sh
```

For the operational runbook (host setup, toolchain notes, troubleshooting LocalSystem vs user-session issues, rsync `/cygdrive/c/...` quirks), see [`docs/windows-ci-runner.md`](docs/windows-ci-runner.md).

#### Windows-specific tests

| Test file                                                                | Coverage                                                                          |
|--------------------------------------------------------------------------|-----------------------------------------------------------------------------------|
| `crates/zlayer-agent/tests/composite_dispatch_e2e.rs`                    | Composite runtime dispatch (HCS + WSL routing on a single host)                   |
| `crates/zlayer-agent/tests/windows_overlay_e2e.rs`                       | Windows overlay networking E2E — userspace WireGuard on Windows                   |
| `crates/zlayer-agent/tests/windows_cluster_join_e2e.rs`                  | Windows cluster join E2E — node bootstrap, scheduler handshake                    |
| `crates/zlayer-builder/tests/windows_templates.rs`                       | Cross-platform parsing of Windows Dockerfile templates (runs on any host)         |
| `crates/zlayer-builder/tests/windows_build_e2e.rs`                       | HCS build E2E — gated `#[ignore]`, requires a real Windows host with HCS enabled  |
| `crates/zlayer-hcs/tests/integration.rs`                                 | HCS wrapper integration tests against the live HCS API                            |

HCS-gated tests are either compiled out on non-Windows hosts via `#[cfg(target_os = "windows")]` or marked `#[ignore]` so CI can schedule them only on the Windows runner. Running `cargo test` on Linux/macOS will **not** execute them — that is by design.

#### Windows clippy / fmt

The pre-commit hook runs the Linux feature set. For Windows-only validation (catches `cfg(windows)` paths that the default lint pass skips), run:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --no-default-features --features hcs-runtime,wsl -- -D warnings
```

Always lint the entire workspace with `--workspace`. Never use `-p <crate>` for clippy — it skips cross-crate warnings.

---

## Running the Manager Locally (macOS)

### Quick dev mode (no sandbox)

```bash
# Build SSR binary
cargo build --release --package zlayer-manager --features ssr

# Run directly
PORT=4321 \
  LEPTOS_SITE_ROOT=~/.local/share/zlayer/images/zlayer-manager_native/rootfs/app/target/site \
  ./target/release/zlayer-manager

# Open http://localhost:4321
```

### Full WASM rebuild (SSR + hydration assets)

```bash
cd crates/zlayer-manager
PATH="$HOME/.rustup/toolchains/1.90-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH" \
  cargo leptos build --release
```

Output: `target/release/zlayer-manager` (arm64 binary) + `target/site/pkg/` (WASM/JS/CSS)

### Deploy via mac-sandbox runtime

```bash
# 1. Build runtime + manager
cargo build --release --package zlayer
cargo build --release --package zlayer-manager --features ssr

# 2. Set up native rootfs (first time only)
ROOTFS=~/.local/share/zlayer/images/zlayer-manager_native/rootfs
mkdir -p $ROOTFS/usr/local/bin $ROOTFS/app/target/site/pkg
cp target/release/zlayer-manager $ROOTFS/usr/local/bin/
cp -r crates/zlayer-manager/target/site/pkg/* $ROOTFS/app/target/site/pkg/

# 3. Create unhashed symlinks for Leptos asset paths
cd $ROOTFS/app/target/site/pkg
ln -sf zlayer-manager.*.js zlayer-manager.js
ln -sf zlayer-manager.*.wasm zlayer-manager_bg.wasm
ln -sf zlayer-manager.*.css zlayer-manager.css
cd -

# 4. Start daemon + deploy
./target/release/zlayer serve \
  --bind 0.0.0.0:3669 \
  --socket ~/.local/share/zlayer/run/zlayer.sock &
sleep 2
./target/release/zlayer \
  --runtime mac-sandbox --host-network --no-tui \
  up -d <your-spec>.zlayer.yml
```

---

## Test Results (v0.9.972 on macOS, Apple M5)

| Suite | Tests | Result |
|-------|-------|--------|
| Full workspace | 2,551 | All pass |
| macOS Sandbox E2E | 28 | All pass |
| MPS GPU Smoke | 4 | All pass |
| Scheduler | 86 | All pass |
| Infrastructure Wiring | 11 | All pass |
| Manager CLI | 6 | All pass |

---

## Known Issues

- **Makefile stale package names**: `test-scheduler`, `test-api`, etc. use old names without the `zlayer-` prefix. Use `cargo test --package zlayer-<name>` directly.
- **Sandbox + Leptos SSR**: Manager returns empty replies inside Seatbelt sandbox due to `0.0.0.0` bind vs `localhost`-only sandbox rule. Run outside sandbox for dev.
- **WASM asset hashes**: Fixed — `LEPTOS_OUTPUT_NAME`, `LEPTOS_SITE_PKG_DIR`, and `LEPTOS_HASH_FILES` runtime env vars are now set in container images so Leptos SSR generates hashed paths matching the built artifacts. Symlink workaround no longer needed.
- **Overlay on macOS**: `boringtun` WireGuard device fails (no `CAP_NET_ADMIN`). Overlay network inactive on mac-sandbox runtime.
