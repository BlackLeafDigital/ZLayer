#!/usr/bin/env bash
set -euo pipefail

# ZLayer Dev Script — detects platform, builds, tests, runs.
# Usage: ./run_dev.sh [command]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# --- Platform detection ---
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Darwin) PLATFORM="macos" ;;
    Linux)  PLATFORM="linux" ;;
    *)      echo "Unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
    x86_64|amd64)  ARCH_LABEL="x86_64" ;;
    aarch64|arm64) ARCH_LABEL="arm64" ;;
    *)             ARCH_LABEL="$ARCH" ;;
esac

DATA_DIR="${HOME}/.local/share/zlayer"
ROOTFS="${DATA_DIR}/images/zlayer-manager_native/rootfs"
SOCKET="${DATA_DIR}/run/zlayer.sock"
DEPLOY_SPEC="/tmp/zlayer-deploy-test/manager-native.zlayer.yml"

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
NC='\033[0m'

ok()   { echo -e "  ${GREEN}✓${NC} $1"; }
warn() { echo -e "  ${YELLOW}!${NC} $1"; }
fail() { echo -e "  ${RED}✗${NC} $1"; }
info() { echo -e "  ${BLUE}→${NC} $1"; }
header() { echo -e "\n${BOLD}$1${NC}"; }

# --- Toolchain PATH fix for macOS ---
# Homebrew Rust lacks wasm32 target. If both exist, force rustup.
setup_rust_path() {
    if [ "$PLATFORM" = "macos" ] && [ -d "/opt/homebrew/opt/rust" ] && [ -d "$HOME/.rustup" ]; then
        # Find the active rustup toolchain
        local toolchain
        toolchain="$(rustup show active-toolchain 2>/dev/null | awk '{print $1}')" || true
        if [ -n "$toolchain" ] && [ -d "$HOME/.rustup/toolchains/${toolchain}/bin" ]; then
            export PATH="$HOME/.rustup/toolchains/${toolchain}/bin:$HOME/.cargo/bin:$PATH"
        fi
    fi
}

# ============================================================
# check — verify prerequisites
# ============================================================
cmd_check() {
    header "Checking prerequisites ($PLATFORM / $ARCH_LABEL)"
    local errors=0

    # Rust via rustup
    if command -v rustup &>/dev/null; then
        local rust_ver
        rust_ver="$(rustc --version 2>/dev/null | awk '{print $2}')" || rust_ver="unknown"
        ok "rustup installed (rustc $rust_ver)"
    else
        fail "rustup not found — install from https://rustup.rs"
        errors=$((errors + 1))
    fi

    # wasm32 target
    if rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown; then
        ok "wasm32-unknown-unknown target"
    else
        fail "wasm32 target missing — run: rustup target add wasm32-unknown-unknown"
        errors=$((errors + 1))
    fi

    # cargo-leptos (optional)
    if command -v cargo-leptos &>/dev/null; then
        ok "cargo-leptos installed"
    else
        warn "cargo-leptos not found (needed for full WASM rebuild only)"
        info "Install: cargo install cargo-leptos"
    fi

    # bun (optional)
    if command -v bun &>/dev/null; then
        ok "bun installed"
    else
        warn "bun not found (needed for daisyui CSS in manager)"
        info "Install: curl -fsSL https://bun.sh/install | bash"
    fi

    # macOS-specific checks
    if [ "$PLATFORM" = "macos" ]; then
        if [ -d "/opt/homebrew/opt/rust" ] && [ -d "$HOME/.rustup" ]; then
            warn "Both Homebrew Rust and rustup detected"
            info "This script auto-fixes PATH, but consider: brew uninstall rust"
        fi
        if system_profiler SPDisplaysDataType 2>/dev/null | grep -q Metal; then
            ok "Metal GPU available (MPS tests will run)"
        fi
    fi

    # Rootfs check
    if [ -d "$ROOTFS" ]; then
        ok "Manager rootfs exists at $ROOTFS"
    else
        warn "No manager rootfs — 'deploy' command will skip rootfs copy"
    fi

    echo ""
    if [ $errors -gt 0 ]; then
        fail "$errors required prerequisite(s) missing"
        return 1
    else
        ok "All required prerequisites met"
    fi
}

# ============================================================
# build — build runtime + manager
# ============================================================
cmd_build() {
    header "Building ZLayer ($PLATFORM / $ARCH_LABEL)"
    setup_rust_path

    info "Building runtime (release)..."
    cargo build --release --package zlayer

    info "Building manager SSR (release)..."
    cargo build --release --package zlayer-manager --features ssr

    # Copy to rootfs if it exists
    if [ -d "$ROOTFS/usr/local/bin" ]; then
        info "Copying manager binary to rootfs..."
        cp target/release/zlayer-manager "$ROOTFS/usr/local/bin/"
        ok "Manager binary updated in rootfs"
    fi

    ok "Build complete"
    info "Binaries: target/release/zlayer, target/release/zlayer-manager"

}

# ============================================================
# test — run platform-appropriate tests
# ============================================================
cmd_test() {
    header "Running tests ($PLATFORM / $ARCH_LABEL)"
    setup_rust_path

    local passed=0
    local failed=0

    # Workspace tests
    info "Running workspace tests..."
    if cargo test --workspace; then
        ok "Workspace tests passed"
        passed=$((passed + 1))
    else
        fail "Workspace tests failed"
        failed=$((failed + 1))
    fi

    # Platform-specific tests
    if [ "$PLATFORM" = "macos" ]; then
        info "Running macOS sandbox E2E tests (28 tests)..."
        rm -rf /tmp/zlayer-macos-sandbox-e2e-test 2>/dev/null || true
        mkdir -p /tmp/zlayer-macos-sandbox-e2e-test/{data,logs}
        mkdir -p /tmp/zlayer-macos-sandbox-e2e-test/data/{containers,images}
        if cargo test --package zlayer-agent --test macos_sandbox_e2e -- --nocapture; then
            ok "Sandbox E2E passed"
            passed=$((passed + 1))
        else
            fail "Sandbox E2E failed"
            failed=$((failed + 1))
        fi

        info "Running MPS GPU smoke tests (4 tests)..."
        if cargo test --package zlayer-agent --test macos_mps_smoke -- --nocapture; then
            ok "MPS smoke passed"
            passed=$((passed + 1))
        else
            fail "MPS smoke failed"
            failed=$((failed + 1))
        fi
    fi

    if [ "$PLATFORM" = "linux" ]; then
        info "Running youki E2E tests (requires sudo)..."
        sudo rm -rf /tmp/zlayer-youki-e2e-test/state/* 2>/dev/null || true
        if sudo -E env "PATH=$HOME/.cargo/bin:$PATH" \
            cargo test --package zlayer-agent --test youki_e2e -- --nocapture --test-threads=1; then
            ok "Youki E2E passed"
            passed=$((passed + 1))
        else
            fail "Youki E2E failed"
            failed=$((failed + 1))
        fi
    fi

    # Summary
    echo ""
    header "Test Summary"
    ok "$passed suite(s) passed"
    if [ $failed -gt 0 ]; then
        fail "$failed suite(s) failed"
        return 1
    fi
}

# ============================================================
# dev — run manager locally (no sandbox)
# ============================================================
cmd_dev() {
    header "Starting manager in dev mode"
    setup_rust_path

    # Build if needed
    if [ ! -f target/release/zlayer-manager ]; then
        info "Manager binary not found, building..."
        cargo build --release --package zlayer-manager --features ssr
    fi

    local site_root="$ROOTFS/app/target/site"
    if [ ! -d "$site_root" ]; then
        warn "WASM site root not found at $site_root"
        info "SSR will work but client-side hydration won't load"
        info "To fix: cd crates/zlayer-manager && cargo leptos build --release"
        site_root="crates/zlayer-manager/target/site"
    fi

    local port="${PORT:-4321}"

    echo ""
    ok "Manager starting on http://localhost:${port}"
    info "Press Ctrl+C to stop"
    echo ""

    PORT="$port" LEPTOS_SITE_ROOT="$site_root" \
        exec ./target/release/zlayer-manager
}

# ============================================================
# deploy — full sandbox deploy (macOS)
# ============================================================
cmd_deploy() {
    if [ "$PLATFORM" != "macos" ]; then
        fail "Sandbox deploy is macOS-only. On Linux, use Docker or youki runtime."
        return 1
    fi

    header "Deploying manager via mac-sandbox"

    # Build if needed
    if [ ! -f target/release/zlayer ] || [ ! -f target/release/zlayer-manager ]; then
        info "Missing binaries, running build first..."
        cmd_build
    fi

    # Clean up existing
    info "Stopping existing daemons..."
    pkill -f "zlayer" 2>/dev/null || true
    sleep 1
    pkill -9 -f "zlayer" 2>/dev/null || true
    rm -f "$SOCKET"
    rm -rf "${DATA_DIR}/containers/manager-"* 2>/dev/null || true

    # Start daemon
    info "Starting daemon on port 3669..."
    ./target/release/zlayer serve \
        --bind 0.0.0.0:3669 \
        --socket "$SOCKET" &
    local daemon_pid=$!
    sleep 2

    if ! kill -0 "$daemon_pid" 2>/dev/null; then
        fail "Daemon failed to start"
        return 1
    fi
    ok "Daemon running (pid $daemon_pid)"

    # Deploy spec
    if [ ! -f "$DEPLOY_SPEC" ]; then
        info "Creating deploy spec..."
        mkdir -p "$(dirname "$DEPLOY_SPEC")"
        cat > "$DEPLOY_SPEC" << 'YAML'
version: v1
deployment: zlayer-manager
services:
  manager:
    rtype: service
    image:
      name: zlayer-manager:native
      pull_policy: never
    command:
      entrypoint: ["/usr/local/bin/zlayer-manager"]
    env:
      LEPTOS_SITE_ROOT: "app/target/site"
      RUST_LOG: "info"
    endpoints:
      - name: http
        protocol: http
        port: 8080
        expose: public
    scale:
      mode: fixed
      replicas: 1
YAML
    fi

    info "Deploying..."
    ./target/release/zlayer \
        --runtime mac-sandbox --host-network --no-tui \
        up -d "$DEPLOY_SPEC"

    echo ""
    ok "Manager deployed via sandbox"
    info "Proxy: http://localhost:8080"
    info "Daemon API: http://localhost:3669"
    info "Stop with: ./run_dev.sh clean"
}

# ============================================================
# clean — kill daemons, remove state
# ============================================================
cmd_clean() {
    header "Cleaning up"

    if pgrep -f "zlayer" &>/dev/null; then
        info "Stopping zlayer processes..."
        pkill -f "zlayer" 2>/dev/null || true
        sleep 1
        pkill -9 -f "zlayer" 2>/dev/null || true
        ok "Processes stopped"
    else
        ok "No zlayer processes running"
    fi

    if [ -S "$SOCKET" ]; then
        rm -f "$SOCKET"
        ok "Removed socket"
    fi

    if ls "${DATA_DIR}/containers/"* &>/dev/null 2>&1; then
        rm -rf "${DATA_DIR}/containers/"*
        ok "Removed container state"
    fi

    ok "Clean"
}

# ============================================================
# help
# ============================================================
cmd_help() {
    cat << EOF
${BOLD}ZLayer Dev Script${NC} ($PLATFORM / $ARCH_LABEL)

${BOLD}Usage:${NC} ./run_dev.sh [command]

${BOLD}Commands:${NC}
  check     Verify prerequisites (rustup, wasm32, cargo-leptos, bun)
  build     Build runtime + manager (release, platform-aware)
  test      Run workspace + platform-specific tests
  dev       Run manager locally on port 4321 (no sandbox)
  deploy    Full sandbox deploy (macOS only)
  clean     Kill daemons, remove sockets/containers
  help      Show this help

${BOLD}Default:${NC} check + build + test

${BOLD}Examples:${NC}
  ./run_dev.sh              # full check → build → test
  ./run_dev.sh dev          # quick: run manager at localhost:4321
  ./run_dev.sh test         # just run tests
  PORT=8888 ./run_dev.sh dev  # custom port
EOF
}

# ============================================================
# main
# ============================================================
CMD="${1:-default}"

case "$CMD" in
    check)   cmd_check ;;
    build)   cmd_build ;;
    test)    cmd_test ;;
    dev)     cmd_dev ;;
    deploy)  cmd_deploy ;;
    clean)   cmd_clean ;;
    help|-h|--help) cmd_help ;;
    default)
        cmd_check
        cmd_build
        cmd_test
        ;;
    *)
        fail "Unknown command: $CMD"
        echo ""
        cmd_help
        exit 1
        ;;
esac
