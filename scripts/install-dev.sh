#!/bin/bash
set -eu

# ZLayer Side-by-Side Dev Installer
#
# Builds the local repo and installs a separate dev daemon (e.g. `zlayer-dev`)
# alongside the production `zlayer` install. Uses a distinct binary path,
# data directory, and service name (systemd unit on Linux, launchd label on
# macOS) so it does not collide with the release daemon.
#
# Env knobs:
#   ZLAYER_DEV_NAME       Binary/unit/data-dir basename (default: zlayer-dev)
#   ZLAYER_DEV_DATA_DIR   Data directory
#                           Linux default: /var/lib/${ZLAYER_DEV_NAME}
#                           macOS default: ${HOME}/.${ZLAYER_DEV_NAME}
#   ZLAYER_DEV_BIN_DIR    Binary directory (default: /usr/local/bin)
#   ZLAYER_DEV_SKIP_BUILD If non-empty, skip `cargo build --release`
#   ZLAYER_DEV_REGISTER   If non-empty, also register & start the service

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

# --- OS detection ---
case "$(uname -s)" in
    Linux)  OS="linux" ;;
    Darwin) OS="darwin" ;;
    *)
        echo "Error: install-dev.sh only supports Linux and macOS." >&2
        echo "For Windows, use scripts/install-dev.ps1." >&2
        exit 1
        ;;
esac

# --- Config ---
ZLAYER_DEV_NAME="${ZLAYER_DEV_NAME:-zlayer-dev}"
if [ -z "${ZLAYER_DEV_DATA_DIR:-}" ]; then
    case "$OS" in
        linux)  ZLAYER_DEV_DATA_DIR="/var/lib/${ZLAYER_DEV_NAME}" ;;
        darwin) ZLAYER_DEV_DATA_DIR="${HOME}/.${ZLAYER_DEV_NAME}" ;;
    esac
fi
ZLAYER_DEV_BIN_DIR="${ZLAYER_DEV_BIN_DIR:-/usr/local/bin}"
ZLAYER_DEV_SKIP_BUILD="${ZLAYER_DEV_SKIP_BUILD:-}"

# Use `sudo` only when the target path is outside $HOME. On macOS the default
# data dir lives under $HOME and must be user-owned for launchd-as-user.
maybe_sudo() {
    # $1 = path being written/created, remaining args = command to run
    local target="$1"
    shift
    case "$target" in
        "$HOME"|"$HOME"/*) "$@" ;;
        *)                 sudo "$@" ;;
    esac
}

echo "ZLayer dev install configuration:"
echo "  OS: ${OS}"
echo "  Repo root: ${REPO_ROOT}"
echo "  Daemon name: ${ZLAYER_DEV_NAME}"
echo "  Binary dir: ${ZLAYER_DEV_BIN_DIR}"
echo "  Data dir: ${ZLAYER_DEV_DATA_DIR}"
echo "  Skip build: ${ZLAYER_DEV_SKIP_BUILD:-no}"
echo ""

# --- Build ---
if [ -z "$ZLAYER_DEV_SKIP_BUILD" ]; then
    echo "Building zlayer (release)..."
    cargo build --release -p zlayer
fi

TARGET_BIN="${REPO_ROOT}/target/release/zlayer"
if [ ! -f "$TARGET_BIN" ]; then
    echo "Error: ${TARGET_BIN} not found." >&2
    echo "Re-run without ZLAYER_DEV_SKIP_BUILD, or build manually:" >&2
    echo "  cargo build --release -p zlayer" >&2
    exit 1
fi

# --- Stop existing dev daemon (idempotent) ---
echo ""
echo "Stopping any existing ${ZLAYER_DEV_NAME} instance..."
if [ "$OS" = "linux" ]; then
    sudo systemctl stop "${ZLAYER_DEV_NAME}.service" 2>/dev/null || true
fi
if [ -x "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" ]; then
    case "$OS" in
        linux)
            sudo "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" daemon stop --daemon-name "${ZLAYER_DEV_NAME}" 2>/dev/null || true
            ;;
        darwin)
            # No sudo — a user-level launchd install lives under gui/$uid; bootout
            # must run as that user to target the right launchd domain.
            "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" daemon stop --daemon-name "${ZLAYER_DEV_NAME}" 2>/dev/null || true
            ;;
    esac
fi

# --- Clean stale WireGuard interfaces scoped to this daemon (Linux only) ---
# macOS has no overlay/WireGuard interfaces — the sandbox runtime doesn't use
# overlay networking (see daemon.rs around the with_overlay handling).
if [ "$OS" = "linux" ]; then
    for iface in $(ip -br link 2>/dev/null | awk -v p="zl-${ZLAYER_DEV_NAME}-" '$1 ~ "^"p {print $1}'); do
        echo "  Removing stale interface: $iface"
        sudo ip link delete "$iface" 2>/dev/null || true
    done
fi

# --- Install binary ---
echo ""
echo "Installing binary to ${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}..."
sudo install -m 0755 "$TARGET_BIN" "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}"

# --- SELinux relabel (Linux only) ---
if [ "$OS" = "linux" ] && command -v getenforce >/dev/null 2>&1; then
    case "$(getenforce 2>/dev/null)" in
        Enforcing|Permissive)
            if command -v semanage >/dev/null 2>&1; then
                sudo semanage fcontext -a -t bin_t "${ZLAYER_DEV_BIN_DIR}(/.*)?" 2>/dev/null \
                    || sudo semanage fcontext -m -t bin_t "${ZLAYER_DEV_BIN_DIR}(/.*)?" 2>/dev/null \
                    || true
                sudo restorecon -RFv "${ZLAYER_DEV_BIN_DIR}" >/dev/null 2>&1 || true
            fi
            sudo chcon -t bin_t "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" 2>/dev/null || true
            echo "SELinux: labeled ${ZLAYER_DEV_NAME} as bin_t"
            ;;
    esac
fi

# --- Container runtime dependencies (Linux only) ---
# libseccomp + cgroups v2 are libcontainer prerequisites. macOS uses a
# different sandbox runtime that needs neither.
if [ "$OS" = "linux" ]; then
    echo ""
    echo "Checking container runtime dependencies..."
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

    if [ ! -f /sys/fs/cgroup/cgroup.controllers ]; then
        echo "Warning: cgroups v2 not detected at /sys/fs/cgroup/"
        echo "The container runtime requires cgroups v2. Check your kernel configuration."
    else
        echo "cgroups v2 found."
    fi
fi

# --- Create data directories ---
echo ""
echo "Creating data directories under ${ZLAYER_DEV_DATA_DIR}..."
maybe_sudo "${ZLAYER_DEV_DATA_DIR}" mkdir -p \
    "${ZLAYER_DEV_DATA_DIR}/containers" \
    "${ZLAYER_DEV_DATA_DIR}/rootfs" \
    "${ZLAYER_DEV_DATA_DIR}/bundles" \
    "${ZLAYER_DEV_DATA_DIR}/cache" \
    "${ZLAYER_DEV_DATA_DIR}/volumes"
maybe_sudo "${ZLAYER_DEV_DATA_DIR}" install -d -m 0750 "${ZLAYER_DEV_DATA_DIR}/secrets"

# --- Install service unit + daemon (opt-in) ---
if [ -n "${ZLAYER_DEV_REGISTER:-}" ]; then
    echo ""
    echo "Installing ${ZLAYER_DEV_NAME} daemon..."
    case "$OS" in
        linux)
            sudo "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" \
                --data-dir "${ZLAYER_DEV_DATA_DIR}" \
                daemon --daemon-name "${ZLAYER_DEV_NAME}" \
                install --no-admin-prompt
            ;;
        darwin)
            # No sudo on macOS — the daemon installs into ~/Library/LaunchAgents
            # for non-root users (see launchd_context() in daemon.rs).
            "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" \
                --data-dir "${ZLAYER_DEV_DATA_DIR}" \
                daemon --daemon-name "${ZLAYER_DEV_NAME}" \
                install --no-admin-prompt
            ;;
    esac
else
    echo ""
    echo "ZLAYER_DEV_REGISTER not set — skipping service registration."
    echo "The binary is installed at ${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}."
    echo ""
    echo "To register and start as a system service later:"
    echo "    ZLAYER_DEV_REGISTER=1 ./scripts/install-dev.sh"
    echo ""
    echo "To run the binary directly (no service manager):"
    case "$OS" in
        linux)
            echo "    sudo ${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME} \\"
            echo "        --data-dir ${ZLAYER_DEV_DATA_DIR} \\"
            echo "        --daemon-name ${ZLAYER_DEV_NAME} \\"
            echo "        serve --bind 0.0.0.0:3669"
            ;;
        darwin)
            echo "    ${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME} \\"
            echo "        --data-dir ${ZLAYER_DEV_DATA_DIR} \\"
            echo "        --daemon-name ${ZLAYER_DEV_NAME} \\"
            echo "        serve --bind 0.0.0.0:3669"
            ;;
    esac
fi

# --- Shell completions ---
echo ""
echo "Installing shell completions..."
install_completion() {
    # $1 = shell name, $2 = destination path
    local script
    if ! script="$("${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" completions "$1" 2>/dev/null)"; then
        echo "  Warning: completions $1 failed; skipping"
        return 0
    fi
    local dir
    dir="$(dirname "$2")"
    case "$dir" in
        "$HOME"/*)
            mkdir -p "$dir"
            printf '%s\n' "$script" > "$2"
            ;;
        *)
            sudo mkdir -p "$dir"
            printf '%s\n' "$script" | sudo tee "$2" >/dev/null
            ;;
    esac
    echo "  Installed ${1} completion to $2"
}
install_completion bash "/etc/bash_completion.d/${ZLAYER_DEV_NAME}"
install_completion zsh "${HOME}/.zsh/completions/_${ZLAYER_DEV_NAME}"
install_completion fish "${HOME}/.config/fish/completions/${ZLAYER_DEV_NAME}.fish"

# --- Final message ---
echo ""
echo "${ZLAYER_DEV_NAME} installed."
echo ""
if [ -n "${ZLAYER_DEV_REGISTER:-}" ]; then
    echo "Inspect:"
    case "$OS" in
        linux)
            echo "  sudo systemctl status ${ZLAYER_DEV_NAME}"
            echo "  sudo journalctl -fu ${ZLAYER_DEV_NAME}"
            echo "  ${ZLAYER_DEV_NAME} status"
            echo ""
            echo "Stop / remove:"
            echo "  sudo systemctl stop ${ZLAYER_DEV_NAME}"
            echo "  sudo ${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME} daemon --daemon-name ${ZLAYER_DEV_NAME} uninstall"
            ;;
        darwin)
            # The plist label strips the "zlayer-" prefix; "zlayer-dev" -> "com.zlayer.daemon-dev".
            label_suffix="${ZLAYER_DEV_NAME#zlayer-}"
            if [ "$label_suffix" = "$ZLAYER_DEV_NAME" ] || [ "$ZLAYER_DEV_NAME" = "zlayer" ]; then
                plist_label="com.zlayer.daemon"
            else
                plist_label="com.zlayer.daemon-${label_suffix}"
            fi
            echo "  launchctl list | grep ${plist_label}"
            echo "  tail -f ${ZLAYER_DEV_DATA_DIR}/logs/daemon.log"
            echo "  ${ZLAYER_DEV_NAME} status"
            echo ""
            echo "Stop / remove:"
            echo "  ${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME} daemon --daemon-name ${ZLAYER_DEV_NAME} stop"
            echo "  ${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME} daemon --daemon-name ${ZLAYER_DEV_NAME} uninstall"
            ;;
    esac
else
    echo "Binary-only install (no service unit registered)."
    echo ""
    echo "Run manually:"
    case "$OS" in
        linux)
            echo "  sudo ${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME} \\"
            echo "      --data-dir ${ZLAYER_DEV_DATA_DIR} \\"
            echo "      --daemon-name ${ZLAYER_DEV_NAME} \\"
            echo "      serve --bind 0.0.0.0:3669"
            ;;
        darwin)
            echo "  ${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME} \\"
            echo "      --data-dir ${ZLAYER_DEV_DATA_DIR} \\"
            echo "      --daemon-name ${ZLAYER_DEV_NAME} \\"
            echo "      serve --bind 0.0.0.0:3669"
            ;;
    esac
    echo ""
    echo "Register as a service later:"
    echo "  ZLAYER_DEV_REGISTER=1 ./scripts/install-dev.sh"
fi
