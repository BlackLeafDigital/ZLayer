#!/bin/sh
set -eu

# ZLayer Dev Installer — builds from source and installs like install.sh
# Reports version as 0.0.0-dev. Run the normal install.sh to go back to release.
#
# Usage: ./scripts/install-dev.sh          # build release + install
#        ./scripts/install-dev.sh --fast   # use release-fast profile
#        ./scripts/install-dev.sh --skip-build  # install already-built binary

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

BINARY="zlayer"
PROFILE="release"
SKIP_BUILD=false

for arg in "$@"; do
    case "$arg" in
        --fast) PROFILE="release-fast" ;;
        --skip-build) SKIP_BUILD=true ;;
        -h|--help)
            echo "Usage: $0 [--fast] [--skip-build]"
            echo ""
            echo "  --fast        Use release-fast profile (faster builds, slightly less optimized)"
            echo "  --skip-build  Skip cargo build, install existing target/release/zlayer"
            exit 0
            ;;
        *) echo "Unknown option: $arg"; exit 1 ;;
    esac
done

# --- Detect OS ---
OS="$(uname -s)"
case "$OS" in
    Linux)  OS="linux" ;;
    Darwin) OS="darwin" ;;
    *)      echo "Error: Unsupported OS: $OS" >&2; exit 1 ;;
esac

# --- Build ---
if [ "$SKIP_BUILD" = false ]; then
    echo "Building zlayer (profile: $PROFILE)..."
    cd "$REPO_ROOT"
    cargo build --profile "$PROFILE" --package zlayer
fi

# Resolve binary path (release-fast outputs to target/release-fast/)
if [ "$PROFILE" = "release" ]; then
    TARGET_BIN="$REPO_ROOT/target/release/$BINARY"
else
    TARGET_BIN="$REPO_ROOT/target/$PROFILE/$BINARY"
fi

if [ ! -f "$TARGET_BIN" ]; then
    echo "Error: Binary not found at $TARGET_BIN" >&2
    echo "Run without --skip-build or build first." >&2
    exit 1
fi

DEV_VERSION="$("$TARGET_BIN" --version 2>/dev/null || echo "0.0.0-dev")"
echo "Binary: $TARGET_BIN"
echo "Version: $DEV_VERSION"

# --- Resolve install dir (same logic as install.sh) ---
INSTALL_DIR=""
PROBE="/usr/local/bin/.zlayer_probe_$$"
if sudo touch "$PROBE" 2>/dev/null && sudo rm -f "$PROBE" 2>/dev/null; then
    INSTALL_DIR="/usr/local/bin"
else
    INSTALL_DIR="/var/lib/zlayer/bin"
fi
sudo mkdir -p "$INSTALL_DIR"

# --- Check current version ---
CURRENT_VERSION=""
if [ -x "${INSTALL_DIR}/${BINARY}" ]; then
    CURRENT_VERSION="$("${INSTALL_DIR}/${BINARY}" --version 2>/dev/null || echo "unknown")"
fi

echo ""
if [ -n "$CURRENT_VERSION" ]; then
    echo "  Installed: $CURRENT_VERSION"
fi
echo "  Installing: $DEV_VERSION"
echo "  Target: ${INSTALL_DIR}/${BINARY}"

# --- Stop running zlayer before overwriting ---
if [ -f "${INSTALL_DIR}/${BINARY}" ]; then
    echo ""
    echo "Stopping zlayer..."
    sudo "${INSTALL_DIR}/${BINARY}" daemon uninstall >/dev/null 2>&1 || true

    if [ "$OS" = "linux" ]; then
        # Clean up stale WireGuard interfaces
        for iface in $(ip -br link 2>/dev/null | awk '/^zl-/{print $1}'); do
            echo "  Removing stale interface: $iface"
            sudo ip link delete "$iface" 2>/dev/null || true
        done
    fi

    sudo rm -f /var/run/wireguard/zl-*.sock 2>/dev/null || true
    sudo rm -f /var/lib/zlayer/daemon.json 2>/dev/null || true
    sudo rm -f /var/run/zlayer.sock 2>/dev/null || true
    sudo rm -f /var/lib/zlayer/run/zlayer.pid 2>/dev/null || true
    sleep 2
fi

# --- Install binary ---
sudo cp "$TARGET_BIN" "${INSTALL_DIR}/${BINARY}"
sudo chmod +x "${INSTALL_DIR}/${BINARY}"

# --- SELinux: relabel binary so systemd can exec it ---
if [ "$OS" = "linux" ] && command -v getenforce >/dev/null 2>&1; then
    case "$(getenforce 2>/dev/null)" in
        Enforcing|Permissive)
            if command -v semanage >/dev/null 2>&1; then
                sudo semanage fcontext -a -t bin_t "${INSTALL_DIR}(/.*)?" 2>/dev/null \
                    || sudo semanage fcontext -m -t bin_t "${INSTALL_DIR}(/.*)?" 2>/dev/null \
                    || true
                sudo restorecon -RFv "${INSTALL_DIR}" >/dev/null 2>&1 || true
            fi
            sudo chcon -t bin_t "${INSTALL_DIR}/${BINARY}" 2>/dev/null || true
            echo "SELinux: labeled ${BINARY} as bin_t"
            ;;
    esac
fi

# --- Create runtime directories (Linux) ---
if [ "$OS" = "linux" ]; then
    sudo mkdir -p /var/lib/zlayer/containers /var/lib/zlayer/rootfs \
        /var/lib/zlayer/bundles /var/lib/zlayer/cache /var/lib/zlayer/volumes
fi

# --- Install and start service ---
echo ""
echo "Installing zlayer daemon service..."
case "$OS" in
    linux)
        sudo "${INSTALL_DIR}/${BINARY}" daemon install
        ;;
    darwin)
        "${INSTALL_DIR}/${BINARY}" daemon install
        ;;
esac

echo ""
echo "Dev install complete: ${INSTALL_DIR}/${BINARY} ($DEV_VERSION)"
echo ""
echo "To go back to a release build:"
echo "  curl -sSL https://raw.githubusercontent.com/BlackLeafDigital/ZLayer/main/install.sh | sh"
