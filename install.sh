#!/bin/sh
set -eu

# ZLayer CLI Installer
# Usage: curl -sSL https://zlayer.dev/install.sh | sh
#   or:  ZLAYER_VERSION=v0.9.0 curl -sSL https://zlayer.dev/install.sh | sh

REPO="zachhandley/ZLayer"
BINARY="zlayer-cli"

# --- Detect OS ---
OS="$(uname -s)"
case "$OS" in
    Linux)
        if [ -f /proc/version ] && grep -qi microsoft /proc/version 2>/dev/null; then
            echo "Error: WSL detected. Please install ZLayer natively on your Linux distro or Windows." >&2
            exit 1
        fi
        OS="linux"
        ;;
    Darwin)
        OS="darwin"
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

# --- Resolve version ---
VERSION="${ZLAYER_VERSION:-}"
if [ -z "$VERSION" ]; then
    echo "Fetching latest version..."
    VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')"
    if [ -z "$VERSION" ]; then
        echo "Error: Could not determine latest version. Set ZLAYER_VERSION and retry." >&2
        exit 1
    fi
fi

# Strip leading 'v' for filename, keep for tag
TAG="$VERSION"
case "$VERSION" in
    v*) VERSION_NUM="${VERSION#v}" ;;
    *)  VERSION_NUM="$VERSION"; TAG="v${VERSION}" ;;
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

# --- Download and install ---
URL="https://github.com/${REPO}/releases/download/${TAG}/${BINARY}-${VERSION_NUM}-${OS}-${ARCH}.tar.gz"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "Downloading ${BINARY} ${TAG} (${OS}/${ARCH})..."
curl -fsSL "$URL" -o "${TMPDIR}/archive.tar.gz"

echo "Installing to ${INSTALL_DIR}..."
tar -xzf "${TMPDIR}/archive.tar.gz" -C "$TMPDIR"

# Find the binary in the extracted files
BIN_PATH="$(find "$TMPDIR" -name "$BINARY" -type f | head -1)"
if [ -z "$BIN_PATH" ]; then
    echo "Error: ${BINARY} not found in archive" >&2
    exit 1
fi

if [ -w "$INSTALL_DIR" ]; then
    cp "$BIN_PATH" "${INSTALL_DIR}/${BINARY}"
    chmod +x "${INSTALL_DIR}/${BINARY}"
elif command -v sudo >/dev/null 2>&1; then
    sudo cp "$BIN_PATH" "${INSTALL_DIR}/${BINARY}"
    sudo chmod +x "${INSTALL_DIR}/${BINARY}"
else
    echo "Error: Cannot write to ${INSTALL_DIR} and sudo is not available." >&2
    echo "Set ZLAYER_INSTALL_DIR to a writable directory and retry." >&2
    exit 1
fi

# --- Verify ---
if "${INSTALL_DIR}/${BINARY}" --version >/dev/null 2>&1; then
    echo ""
    echo "zlayer-cli ${TAG} installed successfully!"
else
    echo ""
    echo "zlayer-cli ${TAG} installed to ${INSTALL_DIR}/${BINARY}"
fi

# Check PATH
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo ""
        echo "Add ${INSTALL_DIR} to your PATH:"
        echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
        ;;
esac

echo ""
echo "Run 'zlayer-cli' to get started."
