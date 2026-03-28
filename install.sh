#!/bin/sh
set -eu

# ZLayer Installer - Downloads from GitHub Releases
# Usage: curl -sSL https://raw.githubusercontent.com/BlackLeafDigital/ZLayer/main/install.sh | sh
#
# Options:
#   ZLAYER_VERSION=0.9.6     - Install specific version
#   ZLAYER_INSTALL_DIR=/path - Custom install directory

REPO="BlackLeafDigital/ZLayer"
BINARY="zlayer"

# --- Detect OS ---
OS="$(uname -s)"
case "$OS" in
    Linux)  OS="linux" ;;
    Darwin) OS="darwin" ;;
    MINGW*|MSYS*|CYGWIN*)
        echo "Windows detected. Use PowerShell instead:" >&2
        echo "  irm https://raw.githubusercontent.com/BlackLeafDigital/ZLayer/main/install.ps1 | iex" >&2
        exit 1
        ;;
    *)
        echo "Error: Unsupported OS: $OS" >&2
        exit 1
        ;;
esac

# --- Detect arch ---
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64|amd64)  ARCH="amd64" ;;
    aarch64|arm64) ARCH="arm64" ;;
    *)
        echo "Error: Unsupported architecture: $ARCH" >&2
        exit 1
        ;;
esac

# --- Resolve version from GitHub ---
VERSION="${ZLAYER_VERSION:-}"
if [ -z "$VERSION" ]; then
    echo "Fetching latest version from GitHub..."
    VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
    if [ -z "$VERSION" ]; then
        echo "Error: Could not determine latest version. Set ZLAYER_VERSION and retry." >&2
        exit 1
    fi
fi

# Normalize: TAG has v prefix, VERSION_NUM does not
case "$VERSION" in
    v*) TAG="$VERSION"; VERSION_NUM="${VERSION#v}" ;;
    *)  TAG="v${VERSION}"; VERSION_NUM="$VERSION" ;;
esac

# --- Resolve install dir ---
INSTALL_DIR="${ZLAYER_INSTALL_DIR:-}"
if [ -z "$INSTALL_DIR" ]; then
    if [ -w /usr/local/bin ]; then
        INSTALL_DIR="/usr/local/bin"
    else
        INSTALL_DIR="${HOME}/.local/bin"
    fi
fi
mkdir -p "$INSTALL_DIR"

# --- Download from GitHub Releases ---
ARTIFACT="${BINARY}-${VERSION_NUM}-${OS}-${ARCH}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ARTIFACT}"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "Downloading ${BINARY} ${TAG} (${OS}/${ARCH})..."
if ! curl -fsSL --connect-timeout 30 --max-time 120 "$URL" -o "${TMPDIR}/archive.tar.gz"; then
    echo "Error: Download failed from ${URL}" >&2
    exit 1
fi

echo "Installing to ${INSTALL_DIR}..."
tar -xzf "${TMPDIR}/archive.tar.gz" -C "$TMPDIR"

BIN_PATH="$(find "$TMPDIR" -name "$BINARY" -type f | head -1)"
if [ -z "$BIN_PATH" ]; then
    echo "Error: ${BINARY} not found in archive" >&2
    exit 1
fi

# --- Stop running zlayer before overwriting binary ---
if [ -f "${INSTALL_DIR}/${BINARY}" ]; then
    echo "Stopping zlayer..."
    "${INSTALL_DIR}/${BINARY}" daemon uninstall >/dev/null 2>&1 || true
    # Clean up stale state
    rm -f "${HOME}/.local/share/zlayer/daemon.json" 2>/dev/null || true
    rm -f "${HOME}/.local/share/zlayer/run/zlayer.sock" 2>/dev/null || true
    rm -f /var/run/zlayer.sock 2>/dev/null || true
    sleep 2
fi

if [ -w "$INSTALL_DIR" ]; then
    cp "$BIN_PATH" "${INSTALL_DIR}/${BINARY}"
    chmod +x "${INSTALL_DIR}/${BINARY}"
elif command -v sudo >/dev/null 2>&1; then
    sudo cp "$BIN_PATH" "${INSTALL_DIR}/${BINARY}"
    sudo chmod +x "${INSTALL_DIR}/${BINARY}"
else
    echo "Error: Cannot write to ${INSTALL_DIR}" >&2
    exit 1
fi

echo ""
echo "${BINARY} ${TAG} installed to ${INSTALL_DIR}/${BINARY}"

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo ""
        echo "Add to PATH: export PATH=\"${INSTALL_DIR}:\$PATH\""
        ;;
esac

# --- Linux: install container runtime dependencies ---
if [ "$OS" = "linux" ]; then
    echo ""
    echo "Checking container runtime dependencies..."

    # libseccomp is required by the bundled container runtime (libcontainer/youki)
    if ! ldconfig -p 2>/dev/null | grep -q libseccomp; then
        echo "Installing libseccomp (required for container runtime)..."
        if command -v apt-get >/dev/null 2>&1; then
            sudo apt-get update -qq && sudo apt-get install -y -qq libseccomp2
        elif command -v dnf >/dev/null 2>&1; then
            sudo dnf install -y libseccomp
        elif command -v yum >/dev/null 2>&1; then
            sudo yum install -y libseccomp
        elif command -v pacman >/dev/null 2>&1; then
            sudo pacman -S --noconfirm libseccomp
        elif command -v apk >/dev/null 2>&1; then
            sudo apk add libseccomp
        elif command -v zypper >/dev/null 2>&1; then
            sudo zypper install -y libseccomp2
        else
            echo "Warning: Could not install libseccomp automatically."
            echo "Please install it manually for your distribution."
            echo "The container runtime will not work without it."
        fi
    else
        echo "libseccomp found."
    fi

    # Verify cgroups v2 (required by libcontainer)
    if [ ! -f /sys/fs/cgroup/cgroup.controllers ]; then
        echo "Warning: cgroups v2 not detected at /sys/fs/cgroup/"
        echo "The container runtime requires cgroups v2. Check your kernel configuration."
    else
        echo "cgroups v2 found."
    fi

    # Create container runtime directories
    echo "Setting up container runtime directories..."
    sudo mkdir -p /var/lib/zlayer/containers /var/lib/zlayer/rootfs \
        /var/lib/zlayer/bundles /var/lib/zlayer/cache /var/lib/zlayer/volumes
fi

# --- Install and start service ---
SKIP_SERVICE="${ZLAYER_NO_SERVICE:-}"
if [ -z "$SKIP_SERVICE" ]; then
    echo ""
    echo "Installing zlayer daemon..."
    case "$OS" in
        linux)
            sudo "${INSTALL_DIR}/${BINARY}" daemon install
            ;;
        darwin)
            "${INSTALL_DIR}/${BINARY}" daemon install
            ;;
    esac
fi

echo ""
echo "Run 'zlayer --help' to get started."
