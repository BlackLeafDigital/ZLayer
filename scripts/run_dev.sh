#!/usr/bin/env bash
set -euo pipefail

# ZLayer Dev Script — detects platform, builds, tests, runs.
# Usage: ./run_dev.sh [command]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

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

# WSL2 detection (reports as Linux but runs under Windows host)
IS_WSL=false
if [ "$PLATFORM" = "linux" ] && [ -f /proc/version ]; then
    if grep -qi 'microsoft\|WSL' /proc/version 2>/dev/null; then
        IS_WSL=true
    fi
fi

# Platform-aware paths (match Rust defaults in bin/zlayer/src/cli.rs)
if [ "$PLATFORM" = "macos" ]; then
    DATA_DIR="${HOME}/.local/share/zlayer"
    SOCKET="${DATA_DIR}/run/zlayer.sock"
    LOG_DIR="${DATA_DIR}/logs"
    RUN_DIR="${DATA_DIR}/run"
else
    DATA_DIR="/var/lib/zlayer"
    SOCKET="/var/run/zlayer.sock"
    LOG_DIR="/var/log/zlayer"
    RUN_DIR="/var/run/zlayer"
fi
ROOTFS="${DATA_DIR}/images/zlayer-manager_native/rootfs"
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

# Run with sudo on Linux (container ops need root for cgroups/namespaces)
run_priv() {
    if [ "$PLATFORM" = "linux" ]; then
        sudo "$@"
    else
        "$@"
    fi
}

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

    # Linux-specific checks
    if [ "$PLATFORM" = "linux" ]; then
        # pkg-config
        if command -v pkg-config &>/dev/null; then
            ok "pkg-config installed"
        else
            fail "pkg-config not found — run: sudo apt-get install -y pkg-config"
            errors=$((errors + 1))
        fi

        # cmake
        if command -v cmake &>/dev/null; then
            ok "cmake installed"
        else
            fail "cmake not found — run: sudo apt-get install -y cmake"
            errors=$((errors + 1))
        fi

        # protobuf compiler
        if command -v protoc &>/dev/null; then
            ok "protobuf-compiler installed ($(protoc --version 2>/dev/null || echo 'unknown'))"
        else
            fail "protobuf-compiler not found — run: sudo apt-get install -y protobuf-compiler"
            errors=$((errors + 1))
        fi

        # libseccomp-dev (checked via pkg-config, fallback to dpkg)
        if command -v pkg-config &>/dev/null && pkg-config --exists libseccomp 2>/dev/null; then
            ok "libseccomp-dev installed"
        elif dpkg -s libseccomp-dev &>/dev/null 2>&1; then
            ok "libseccomp-dev installed"
        else
            fail "libseccomp-dev not found — run: sudo apt-get install -y libseccomp-dev"
            errors=$((errors + 1))
        fi

        # libssl-dev (checked via pkg-config, fallback to dpkg)
        if command -v pkg-config &>/dev/null && pkg-config --exists openssl 2>/dev/null; then
            ok "libssl-dev installed"
        elif dpkg -s libssl-dev &>/dev/null 2>&1; then
            ok "libssl-dev installed"
        else
            fail "libssl-dev not found — run: sudo apt-get install -y libssl-dev"
            errors=$((errors + 1))
        fi

        # sudo access (needed for deploy/test commands)
        if sudo -n true 2>/dev/null; then
            ok "sudo access available (passwordless)"
        elif groups 2>/dev/null | grep -qw sudo; then
            ok "sudo access available (may prompt for password)"
        else
            warn "sudo may not be available (needed for deploy/test commands)"
        fi

        # Docker (optional fallback runtime)
        if command -v docker &>/dev/null; then
            ok "docker installed (fallback runtime)"
        else
            warn "docker not found (optional fallback runtime)"
        fi

        # WSL2 info
        if [ "$IS_WSL" = true ]; then
            info "Running under WSL2 (full Linux kernel features available)"
        fi

        # One-liner install hint
        if [ $errors -gt 0 ]; then
            echo ""
            info "Install all required deps: sudo apt-get install -y protobuf-compiler libseccomp-dev libssl-dev pkg-config cmake"
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

    # Ensure cargo-leptos is installed (needed for manager WASM build)
    if ! command -v cargo-leptos &>/dev/null; then
        info "Installing cargo-leptos (needed for manager WASM build)..."
        cargo install cargo-leptos
        ok "cargo-leptos installed"
    fi

    info "Building runtime (release)..."
    cargo build --release --package zlayer

    info "Building manager (SSR + WASM via cargo-leptos)..."
    (cd crates/zlayer-manager && cargo leptos build --release)

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
# dev — run manager with live reload (cargo-leptos watch)
# ============================================================
cmd_dev() {
    header "Starting manager in dev mode (live reload)"
    setup_rust_path

    # Ensure cargo-leptos is installed
    if ! command -v cargo-leptos &>/dev/null; then
        info "Installing cargo-leptos (needed for watch mode)..."
        cargo install cargo-leptos
        ok "cargo-leptos installed"
    fi

    local port="${PORT:-4321}"

    echo ""
    ok "Manager starting on http://localhost:${port}"
    info "Live reload: saves to crates/zlayer-manager/ trigger rebuild"
    info "Press Ctrl+C to stop"
    echo ""

    cd crates/zlayer-manager
    exec cargo leptos watch --port "$port"
}

# ============================================================
# deploy — full deploy with overlay networking
# ============================================================
cmd_deploy() {
    local runtime_label
    if [ "$PLATFORM" = "linux" ]; then
        runtime_label="youki (auto)"
    else
        runtime_label="mac-sandbox"
    fi
    header "Deploying manager ($PLATFORM / $runtime_label)"

    # Build if needed
    if [ ! -f target/release/zlayer ] || [ ! -f target/release/zlayer-manager ]; then
        info "Missing binaries, running build first..."
        cmd_build
    fi

    # Clean up existing
    info "Stopping existing daemons..."
    run_priv pkill -f "zlayer" 2>/dev/null || true
    sleep 1
    run_priv pkill -9 -f "zlayer" 2>/dev/null || true
    run_priv rm -f "$SOCKET"
    run_priv rm -rf "${DATA_DIR}/containers/manager-"* 2>/dev/null || true

    # Ensure directories exist (Linux uses system paths that may not exist)
    if [ "$PLATFORM" = "linux" ]; then
        sudo mkdir -p "$DATA_DIR" "$LOG_DIR" "$RUN_DIR" "${DATA_DIR}/containers" "${DATA_DIR}/images"
    fi

    # Start daemon
    info "Starting daemon on port 3669..."
    run_priv ./target/release/zlayer serve \
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

    # Deploy with platform-appropriate runtime (overlay networking enabled)
    info "Deploying with overlay networking..."
    if [ "$PLATFORM" = "linux" ]; then
        sudo ./target/release/zlayer \
            --runtime auto --no-tui \
            up -d "$DEPLOY_SPEC"
    else
        ./target/release/zlayer \
            --runtime mac-sandbox --no-tui \
            up -d "$DEPLOY_SPEC"
    fi

    echo ""
    ok "Manager deployed ($runtime_label)"
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
        run_priv pkill -f "zlayer" 2>/dev/null || true
        sleep 1
        run_priv pkill -9 -f "zlayer" 2>/dev/null || true
        ok "Processes stopped"
    else
        ok "No zlayer processes running"
    fi

    if [ -S "$SOCKET" ]; then
        run_priv rm -f "$SOCKET"
        ok "Removed socket"
    fi

    if ls "${DATA_DIR}/containers/"* &>/dev/null 2>&1; then
        run_priv rm -rf "${DATA_DIR}/containers/"*
        ok "Removed container state"
    fi

    if [ "$PLATFORM" = "linux" ] && [ -d "$LOG_DIR" ]; then
        if ls "${LOG_DIR}/"* &>/dev/null 2>&1; then
            sudo rm -rf "${LOG_DIR}/"*
            ok "Removed log files"
        fi
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
  check     Verify prerequisites (platform-aware)
  build     Build runtime + manager (release)
  test      Run workspace + platform-specific tests
  dev       Run manager with live reload (cargo-leptos watch)
  deploy    Full deploy with overlay networking (macOS: sandbox, Linux: youki)
  clean     Kill daemons, remove sockets/containers/logs
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
