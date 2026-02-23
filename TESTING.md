# ZLayer Testing Guide

## Quick Start

```bash
./scripts/run_dev.sh check   # verify prerequisites
./scripts/run_dev.sh build   # build runtime + manager
./scripts/run_dev.sh test    # run all tests (platform-aware)
./scripts/run_dev.sh dev     # run manager at localhost:4321
./scripts/run_dev.sh         # all of the above (check → build → test)
```

The script detects macOS vs Linux automatically and runs the right tests for your platform.

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
- **WASM asset hashes**: Leptos SSR generates unhashed `/pkg/zlayer-manager.js` paths but cargo-leptos builds with content hashes. Workaround: symlinks in rootfs.
- **Overlay on macOS**: `boringtun` WireGuard device fails (no `CAP_NET_ADMIN`). Overlay network inactive on mac-sandbox runtime.
