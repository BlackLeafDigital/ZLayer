#!/bin/bash
set -euo pipefail

# ZLayer Installer
# Usage: curl -fsSL https://zlayer.dev/install.sh | bash

REPO="BlackLeafDigital/ZLayer"
BINARY_NAME="zlayer"
INSTALL_DIR="${ZLAYER_INSTALL_DIR:-/usr/local/bin}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

info() { echo -e "${BLUE}[INFO]${NC} $*"; }
success() { echo -e "${GREEN}[OK]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*" >&2; exit 1; }

# Detect OS
detect_os() {
    case "$(uname -s)" in
        Linux*)  echo "linux" ;;
        Darwin*) error "macOS is not supported. ZLayer requires Linux kernel features (cgroups, namespaces)." ;;
        *)       error "Unsupported operating system: $(uname -s)" ;;
    esac
}

# Detect architecture
detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)   echo "amd64" ;;
        aarch64|arm64)  echo "arm64" ;;
        *)              error "Unsupported architecture: $(uname -m)" ;;
    esac
}

# Get latest release version from GitHub API
get_latest_version() {
    local url="https://api.github.com/repos/${REPO}/releases/latest"

    if command -v curl &> /dev/null; then
        curl -fsSL "$url" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/'
    elif command -v wget &> /dev/null; then
        wget -qO- "$url" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/'
    else
        error "Neither curl nor wget found. Please install one of them."
    fi
}

# Download file
download() {
    local url="$1"
    local output="$2"

    if command -v curl &> /dev/null; then
        curl -fsSL "$url" -o "$output"
    elif command -v wget &> /dev/null; then
        wget -q "$url" -O "$output"
    else
        error "Neither curl nor wget found. Please install one of them."
    fi
}

# Main installation
main() {
    echo ""
    echo "  ______                    "
    echo " |___  / |                  "
    echo "    / /  | |     __ _ _   _  ___ _ __ "
    echo "   / /   | |    / _\` | | | |/ _ \\ '__|"
    echo "  / /__  | |___| (_| | |_| |  __/ |   "
    echo " /_____| |______\\__,_|\\__, |\\___|_|   "
    echo "                       __/ |          "
    echo "                      |___/           "
    echo ""
    echo "  Container Orchestration Runtime"
    echo ""

    # Detect platform
    local os=$(detect_os)
    local arch=$(detect_arch)
    info "Detected platform: ${os}-${arch}"

    # Get version
    local version="${ZLAYER_VERSION:-}"
    if [[ -z "$version" ]]; then
        info "Fetching latest version..."
        version=$(get_latest_version)
        if [[ -z "$version" ]]; then
            error "Failed to determine latest version. Set ZLAYER_VERSION manually."
        fi
    fi
    info "Installing ZLayer ${version}"

    # Construct download URL
    local filename="${BINARY_NAME}-${version}-${os}-${arch}.tar.gz"
    local url="https://github.com/${REPO}/releases/download/${version}/${filename}"

    # Create temp directory
    local tmpdir=$(mktemp -d)
    trap "rm -rf $tmpdir" EXIT

    # Download
    info "Downloading from ${url}..."
    download "$url" "${tmpdir}/${filename}"

    # Extract
    info "Extracting..."
    tar -xzf "${tmpdir}/${filename}" -C "$tmpdir"

    # Find the binary (might be in root or in a subdirectory)
    local binary_path=$(find "$tmpdir" -name "$BINARY_NAME" -type f | head -1)
    if [[ -z "$binary_path" ]]; then
        error "Binary not found in archive"
    fi

    # Install
    info "Installing to ${INSTALL_DIR}..."

    # Check if we need sudo
    if [[ -w "$INSTALL_DIR" ]]; then
        cp "$binary_path" "${INSTALL_DIR}/${BINARY_NAME}"
        chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
    elif command -v sudo &> /dev/null; then
        sudo cp "$binary_path" "${INSTALL_DIR}/${BINARY_NAME}"
        sudo chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
    else
        # Try user-local install
        INSTALL_DIR="${HOME}/.local/bin"
        mkdir -p "$INSTALL_DIR"
        cp "$binary_path" "${INSTALL_DIR}/${BINARY_NAME}"
        chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
        warn "Installed to ${INSTALL_DIR} (add to PATH if needed)"
    fi

    # Verify installation
    if command -v "$BINARY_NAME" &> /dev/null; then
        success "ZLayer ${version} installed successfully!"
        echo ""
        "$BINARY_NAME" --version 2>/dev/null || true
    else
        success "ZLayer ${version} installed to ${INSTALL_DIR}/${BINARY_NAME}"
        if [[ ":$PATH:" != *":${INSTALL_DIR}:"* ]]; then
            warn "Add ${INSTALL_DIR} to your PATH:"
            echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
        fi
    fi

    echo ""
    info "Get started:"
    echo "  zlayer --help"
    echo "  zlayer serve              # Start the API server"
    echo "  zlayer deploy --spec app.yaml  # Deploy services"
    echo ""
}

main "$@"
