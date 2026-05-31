#!/bin/bash
set -eu

# ZLayer Side-by-Side Dev Installer
#
# Builds the local repo and installs a separate dev daemon (e.g. `zlayer-dev`)
# alongside the production `zlayer` install. Uses a distinct binary path,
# data directory, and service name (systemd unit on Linux, launchd label on
# macOS) so it does not collide with the release daemon.
#
# Flags:
#   --replace        Install AS the production `zlayer` (not `zlayer-dev`):
#                    overwrites /usr/local/bin/zlayer, uses production data
#                    directory, registers + starts the daemon with
#                    --docker-socket (and --with-overlay on Linux), then
#                    fixes /var/run/docker.sock to point at the zlayer
#                    Docker socket (backing up any existing entry).
#   --register       Register and start the service after install.
#                    Equivalent to ZLAYER_DEV_REGISTER=1.
#   --skip-build     Skip `cargo build --release -p zlayer`.
#                    Equivalent to ZLAYER_DEV_SKIP_BUILD=1.
#   -h | --help      Show this help.
#
# Env knobs (override flag-driven defaults):
#   ZLAYER_DEV_NAME       Binary/unit/data-dir basename (default: zlayer-dev,
#                           or `zlayer` when --replace).
#   ZLAYER_DEV_DATA_DIR   Data directory
#                           Linux default: /var/lib/${ZLAYER_DEV_NAME}
#                           macOS default: ${HOME}/.${ZLAYER_DEV_NAME}
#   ZLAYER_DEV_BIN_DIR    Binary directory (default: /usr/local/bin)
#   ZLAYER_DEV_SKIP_BUILD If non-empty, skip `cargo build --release`
#   ZLAYER_DEV_REGISTER   If non-empty, also register & start the service

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

usage() {
    sed -n '4,33p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

# --- CLI args ---
REPLACE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --replace)    REPLACE=1; shift ;;
        --register)   ZLAYER_DEV_REGISTER=1; shift ;;
        --skip-build) ZLAYER_DEV_SKIP_BUILD=1; shift ;;
        -h|--help)    usage; exit 0 ;;
        *)
            echo "Error: unknown argument: $1" >&2
            echo "" >&2
            usage >&2
            exit 2
            ;;
    esac
done

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

# --- --replace overrides ---
# Replace mode hijacks the install target so it stands in for the production
# `zlayer` install: same binary name, same data dir, daemon registered, docker
# socket exposed, /var/run/docker.sock symlinked, overlay caps on Linux.
# Honors any explicit env overrides the user set.
if [ -n "$REPLACE" ]; then
    ZLAYER_DEV_NAME="${ZLAYER_DEV_NAME:-zlayer}"
    ZLAYER_DEV_REGISTER="${ZLAYER_DEV_REGISTER:-1}"
fi

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

# --- Extra cleanup in --replace mode (mirrors install.sh) ---
# When we're stomping on the production install, also clean WireGuard UAPI
# sockets, kill stale boringtun/zlayer holders of the WG port, and remove
# stale state files so the freshly-installed daemon comes up clean.
if [ -n "$REPLACE" ] && [ "$OS" = "linux" ]; then
    sudo rm -f /var/run/wireguard/zl-*.sock 2>/dev/null || true
    if command -v ss >/dev/null 2>&1; then
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
    sudo rm -f "${ZLAYER_DEV_DATA_DIR}/daemon.json" 2>/dev/null || true
    sudo rm -f "/var/run/${ZLAYER_DEV_NAME}.sock" 2>/dev/null || true
    sudo rm -f "${ZLAYER_DEV_DATA_DIR}/run/${ZLAYER_DEV_NAME}.pid" 2>/dev/null || true
    sleep 3
fi
if [ -n "$REPLACE" ] && [ "$OS" = "darwin" ]; then
    rm -f "${ZLAYER_DEV_DATA_DIR}/daemon.json" 2>/dev/null || true
    rm -f "${ZLAYER_DEV_DATA_DIR}/run/${ZLAYER_DEV_NAME}.sock" 2>/dev/null || true
    rm -f "${ZLAYER_DEV_DATA_DIR}/run/${ZLAYER_DEV_NAME}.pid" 2>/dev/null || true
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

    # --replace mode wires the daemon up the same way the production install.sh
    # does when ZLAYER_DOCKER_SOCKET=1 + ZLAYER_WITH_OVERLAY=1: docker socket
    # exposed, and overlay caps on Linux (no-op on macOS sandbox runtime).
    DAEMON_INSTALL_FLAGS="--no-admin-prompt"
    if [ -n "$REPLACE" ]; then
        DAEMON_INSTALL_FLAGS="${DAEMON_INSTALL_FLAGS} --docker-socket"
        if [ "$OS" = "linux" ]; then
            DAEMON_INSTALL_FLAGS="${DAEMON_INSTALL_FLAGS} --with-overlay"
        fi
    fi

    case "$OS" in
        linux)
            # Intentional word-splitting of DAEMON_INSTALL_FLAGS below.
            # shellcheck disable=SC2086
            sudo "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" \
                --data-dir "${ZLAYER_DEV_DATA_DIR}" \
                daemon --daemon-name "${ZLAYER_DEV_NAME}" \
                install ${DAEMON_INSTALL_FLAGS}
            ;;
        darwin)
            # No sudo on macOS — the daemon installs into ~/Library/LaunchAgents
            # for non-root users (see launchd_context() in daemon.rs).
            # Intentional word-splitting of DAEMON_INSTALL_FLAGS below.
            # shellcheck disable=SC2086
            "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" \
                --data-dir "${ZLAYER_DEV_DATA_DIR}" \
                daemon --daemon-name "${ZLAYER_DEV_NAME}" \
                install ${DAEMON_INSTALL_FLAGS}
            ;;
    esac

    # --- Fix /var/run/docker.sock symlink in --replace mode ---
    # `zlayer docker install` is the canonical way to repoint
    # /var/run/docker.sock at the zlayer Docker API socket (it handles
    # missing / ours-already / foreign-symlink / foreign-file cases and
    # writes a backup before replacing). We skip the rest of its work
    # (shims, env vars, daemon restart) since `daemon install` above
    # already restarted the daemon with --docker-socket.
    #
    # We must pass --socket-path explicitly because the default resolves
    # against $HOME, and `sudo` (used to write /var/run/docker.sock)
    # clobbers $HOME to /var/root on macOS — which would point the
    # symlink at a non-existent /var/root/.zlayer/run/docker.sock.
    if [ -n "$REPLACE" ]; then
        case "$OS" in
            linux)
                # The Linux daemon runs as root via systemd, so its
                # docker socket always lives at the system path.
                DOCKER_SOCKET_TARGET="/var/run/zlayer/docker.sock"
                ;;
            darwin)
                # macOS daemon runs as user via launchd; socket lives
                # under the user's data dir.
                DOCKER_SOCKET_TARGET="${ZLAYER_DEV_DATA_DIR}/run/docker.sock"
                ;;
        esac
        echo ""
        echo "Fixing /var/run/docker.sock symlink (target: ${DOCKER_SOCKET_TARGET})..."
        if ! sudo "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" docker install \
            --socket-path "${DOCKER_SOCKET_TARGET}" \
            --no-shim --no-env --no-daemon-restart \
            --replace-docker-sock --skip-symlink-prompt; then
            echo "  Warning: docker symlink fix failed (continuing)."
        fi
    fi
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
if [ -n "$REPLACE" ]; then
    INSTALLED_VERSION="$("${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" --version 2>/dev/null | awk '{print $2}')" || INSTALLED_VERSION="(unknown)"
    echo "${ZLAYER_DEV_NAME} (local ${INSTALLED_VERSION}) installed AS the production zlayer."
    echo "  Binary:        ${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}"
    echo "  Data dir:      ${ZLAYER_DEV_DATA_DIR}"
    echo "  Docker socket: /var/run/docker.sock (symlinked to zlayer)"
    if [ "$OS" = "linux" ]; then
        echo "  Overlay:       enabled (CAP_NET_ADMIN / CAP_SYS_ADMIN)"
    fi
else
    echo "${ZLAYER_DEV_NAME} installed."
fi
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
