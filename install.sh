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
# Binary must be in a system path for systemd (not ~/.local/bin).
# Write-probe /usr/local/bin first, fall back to /var/lib/zlayer/bin
# (always writable, matches zlayer-paths ZLayerDirs::bin()).
INSTALL_DIR="${ZLAYER_INSTALL_DIR:-}"
if [ -z "$INSTALL_DIR" ]; then
    PROBE="/usr/local/bin/.zlayer_probe_$$"
    if sudo touch "$PROBE" 2>/dev/null && sudo rm -f "$PROBE" 2>/dev/null; then
        INSTALL_DIR="/usr/local/bin"
    else
        INSTALL_DIR="/var/lib/zlayer/bin"
    fi
fi
sudo mkdir -p "$INSTALL_DIR"

# --- Check installed version ---
CURRENT_VERSION=""
if [ -x "${INSTALL_DIR}/${BINARY}" ]; then
    CURRENT_VERSION="$("${INSTALL_DIR}/${BINARY}" --version 2>/dev/null | sed -n 's/.*[[:space:]]\([0-9][0-9.]*\).*/\1/p')" || true
fi

echo ""
if [ -n "$CURRENT_VERSION" ]; then
    echo "  Installed: v${CURRENT_VERSION}"
fi
echo "  Target:    ${TAG}"

if [ "$CURRENT_VERSION" = "$VERSION_NUM" ]; then
    printf "\nAlready at %s. Reinstall anyway? [y/N] " "$TAG"
    read -r REPLY </dev/tty || REPLY="n"
    case "$REPLY" in
        [yY]*) ;;
        *) echo "Skipping."; exit 0 ;;
    esac
fi

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
    sudo "${INSTALL_DIR}/${BINARY}" daemon uninstall >/dev/null 2>&1 || true

    # Clean up stale WireGuard interfaces (Linux only)
    if [ "$OS" = "linux" ]; then
        for iface in $(ip -br link 2>/dev/null | awk '/^zl-/{print $1}'); do
            echo "  Removing stale interface: $iface"
            sudo ip link delete "$iface" 2>/dev/null || true
        done
    fi

    # Remove stale WireGuard UAPI sockets
    sudo rm -f /var/run/wireguard/zl-*.sock 2>/dev/null || true

    # Kill stale zlayer/boringtun processes holding the WireGuard port
    if [ "$OS" = "linux" ] && command -v ss >/dev/null 2>&1; then
        for pid in $(ss -ulnp 'sport = :51420' 2>/dev/null | grep -oP 'pid=\K[0-9]+'); do
            pname=$(ps -p "$pid" -o comm= 2>/dev/null || true)
            case "$pname" in
                *zlayer*|*boringtun*)
                    echo "  Killing stale process: $pname (PID $pid)"
                    sudo kill "$pid" 2>/dev/null || true
                    ;;
            esac
        done
    fi

    # Clean up stale state files
    rm -f /var/lib/zlayer/daemon.json 2>/dev/null || true
    rm -f /var/run/zlayer.sock 2>/dev/null || true
    rm -f /var/lib/zlayer/run/zlayer.pid 2>/dev/null || true
    sleep 3
fi

sudo cp "$BIN_PATH" "${INSTALL_DIR}/${BINARY}"
sudo chmod +x "${INSTALL_DIR}/${BINARY}"

# --- SELinux: relabel binary so systemd's init_t can exec it ---
# Files under /var/lib inherit var_lib_t, which init_t cannot exec as a
# service entrypoint. Set bin_t via semanage (persistent) + restorecon,
# with chcon as fallback. No-op on non-SELinux distros.
if [ "$OS" = "linux" ] && command -v getenforce >/dev/null 2>&1; then
    case "$(getenforce 2>/dev/null)" in
        Enforcing|Permissive)
            if command -v semanage >/dev/null 2>&1; then
                sudo semanage fcontext -a -t bin_t "${INSTALL_DIR}(/.*)?" 2>/dev/null \
                    || sudo semanage fcontext -m -t bin_t "${INSTALL_DIR}(/.*)?" 2>/dev/null \
                    || true
                sudo restorecon -RFv "${INSTALL_DIR}" >/dev/null 2>&1 || true
            fi
            # chcon works even when policycoreutils-python-utils (semanage)
            # isn't installed — e.g., on the Silverblue base image.
            sudo chcon -t bin_t "${INSTALL_DIR}/${BINARY}" 2>/dev/null || true
            echo "SELinux: labeled ${BINARY} as bin_t"
            ;;
    esac
fi

echo ""
echo "${BINARY} ${TAG} installed to ${INSTALL_DIR}/${BINARY}"

# --- Warn if an older zlayer binary shadows the one we just installed ---
# command -v walks PATH in order, so a stale binary earlier on PATH (typically
# ~/.local/bin/zlayer left over from a previous manual install) will silently
# be used instead, which produces "it builds but the daemon can't find the
# image" failures that are miserable to debug.
RESOLVED="$(command -v "$BINARY" 2>/dev/null || true)"
if [ -n "$RESOLVED" ] && [ "$RESOLVED" != "${INSTALL_DIR}/${BINARY}" ]; then
    echo ""
    echo "Warning: another '${BINARY}' binary appears earlier on your PATH:"
    echo "    $RESOLVED"
    echo "It will shadow the version just installed. Remove with:"
    echo "    rm '$RESOLVED'"
    echo "Then re-open your shell (or run 'hash -r')."
fi

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        printf "\n%s is not in your PATH. Add it now? [Y/n] " "$INSTALL_DIR"
        read -r REPLY </dev/tty || REPLY="y"
        case "$REPLY" in
            [nN]*)
                echo "Skipped. Add manually: export PATH=\"${INSTALL_DIR}:\$PATH\""
                ;;
            *)
                case "$OS" in
                    linux)
                        printf 'export PATH="%s:$PATH"\n' "$INSTALL_DIR" | sudo tee /etc/profile.d/zlayer.sh >/dev/null
                        echo "Added to PATH via /etc/profile.d/zlayer.sh"
                        ;;
                    darwin)
                        echo "$INSTALL_DIR" | sudo tee /etc/paths.d/zlayer >/dev/null
                        echo "Added to PATH via /etc/paths.d/zlayer"
                        ;;
                esac
                echo "Open a new terminal for the change to take effect."
                ;;
        esac
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

# --- Install shell completions (fail-soft) ---
# Drop completion scripts for bash/zsh/fish. Never abort the installer on
# failure — completions are a UX nicety, not a correctness requirement.
install_completion_shell() {
    # $1 = shell name (bash|zsh|fish), $2 = destination path
    _sh="$1"
    _dest="$2"
    _dir="$(dirname "$_dest")"
    _script=""
    if ! _script="$("${INSTALL_DIR}/${BINARY}" completions "$_sh" 2>/dev/null)"; then
        echo "  Warning: '${BINARY} completions ${_sh}' failed; skipping ${_sh} completion."
        return 0
    fi
    if [ -z "$_script" ]; then
        echo "  Warning: empty ${_sh} completion script; skipping."
        return 0
    fi
    # Decide whether we need sudo for this destination.
    case "$_dir" in
        "$HOME"/*|"$HOME")
            if ! mkdir -p "$_dir" 2>/dev/null; then
                echo "  Warning: could not create ${_dir}; skipping ${_sh} completion."
                return 0
            fi
            if ! printf '%s\n' "$_script" > "$_dest" 2>/dev/null; then
                echo "  Warning: could not write ${_dest}; skipping ${_sh} completion."
                return 0
            fi
            ;;
        *)
            if ! sudo mkdir -p "$_dir" 2>/dev/null; then
                echo "  Warning: could not create ${_dir}; skipping ${_sh} completion."
                return 0
            fi
            if ! printf '%s\n' "$_script" | sudo tee "$_dest" >/dev/null 2>&1; then
                echo "  Warning: could not write ${_dest}; skipping ${_sh} completion."
                return 0
            fi
            ;;
    esac
    echo "  Installed zlayer ${_sh} completion to ${_dest}"
    return 0
}

echo ""
echo "Installing shell completions..."

# Bash: prefer Homebrew prefix if detected, else system-wide if root writable,
# else user-local XDG path.
if command -v bash >/dev/null 2>&1; then
    BASH_DEST=""
    if command -v brew >/dev/null 2>&1; then
        BREW_PREFIX="$(brew --prefix 2>/dev/null || true)"
        if [ -n "$BREW_PREFIX" ] && [ -d "${BREW_PREFIX}/etc/bash_completion.d" ]; then
            BASH_DEST="${BREW_PREFIX}/etc/bash_completion.d/zlayer"
        fi
    fi
    if [ -z "$BASH_DEST" ]; then
        if [ "$(id -u)" = "0" ] || sudo -n true 2>/dev/null; then
            if [ -d /etc/bash_completion.d ] || sudo mkdir -p /etc/bash_completion.d 2>/dev/null; then
                BASH_DEST="/etc/bash_completion.d/zlayer"
            fi
        fi
    fi
    if [ -z "$BASH_DEST" ]; then
        BASH_DEST="${HOME}/.local/share/bash-completion/completions/zlayer"
    fi
    install_completion_shell bash "$BASH_DEST" || true
fi

# Zsh: user-local fpath directory. Hint user to add fpath if not present.
if command -v zsh >/dev/null 2>&1; then
    ZSH_DIR="${HOME}/.zsh/completions"
    ZSH_DEST="${ZSH_DIR}/_zlayer"
    install_completion_shell zsh "$ZSH_DEST" || true
    ZSHRC="${HOME}/.zshrc"
    if [ -f "$ZSHRC" ]; then
        if ! grep -q "fpath+=${ZSH_DIR}" "$ZSHRC" 2>/dev/null \
            && ! grep -q "fpath+=~/.zsh/completions" "$ZSHRC" 2>/dev/null \
            && ! grep -q "fpath+=(~/.zsh/completions)" "$ZSHRC" 2>/dev/null; then
            echo "  Hint: add 'fpath+=~/.zsh/completions' to ~/.zshrc before 'compinit' to enable zsh completions."
        fi
    else
        echo "  Hint: add 'fpath+=~/.zsh/completions' to ~/.zshrc before 'compinit' to enable zsh completions."
    fi
fi

# Fish: XDG completions directory.
if command -v fish >/dev/null 2>&1; then
    FISH_DEST="${HOME}/.config/fish/completions/zlayer.fish"
    install_completion_shell fish "$FISH_DEST" || true
fi

echo ""
echo "Run 'zlayer --help' to get started."
