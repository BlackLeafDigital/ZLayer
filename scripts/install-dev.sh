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
#                    overwrites /usr/local/bin/zlayer, fully uninstalls the
#                    running prod daemon, then re-registers + starts it
#                    using the production defaults (no --data-dir or
#                    --daemon-name overrides), with --docker-socket and
#                    --with-overlay (Linux), and symlinks
#                    /var/run/docker.sock to the zlayer Docker socket.
#                    `--replace --data-dir <PATH>` still takes the place
#                    of the running zlayer daemon, but installs the new
#                    one with that explicit data dir.
#   --data-dir PATH  Override the data directory for the new install.
#                    Equivalent to ZLAYER_DEV_DATA_DIR=PATH.
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
# Track whether the user explicitly specified a data dir (via env var preset
# OR --data-dir flag). In --replace mode this controls whether we pass
# --data-dir to the daemon at install time: without it we let the daemon
# pick the production default, exactly as the release install.sh does.
REPLACE=""
DATA_DIR_EXPLICIT=""
if [ -n "${ZLAYER_DEV_DATA_DIR:-}" ]; then
    DATA_DIR_EXPLICIT=1
fi
while [ $# -gt 0 ]; do
    case "$1" in
        --replace)    REPLACE=1; shift ;;
        --register)   ZLAYER_DEV_REGISTER=1; shift ;;
        --skip-build) ZLAYER_DEV_SKIP_BUILD=1; shift ;;
        --data-dir)
            if [ $# -lt 2 ]; then
                echo "Error: --data-dir requires a path argument" >&2
                exit 2
            fi
            ZLAYER_DEV_DATA_DIR="$2"
            DATA_DIR_EXPLICIT=1
            shift 2
            ;;
        --data-dir=*)
            ZLAYER_DEV_DATA_DIR="${1#--data-dir=}"
            DATA_DIR_EXPLICIT=1
            shift
            ;;
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
# `zlayer` install: same binary name, daemon registered, docker socket
# exposed, /var/run/docker.sock symlinked, overlay caps on Linux.
# Honors any explicit env / CLI overrides the user set.
#
# REPLACE_PROD_DATA_DIR is the data dir of the *running* prod daemon being
# replaced — always the platform default, separate from ZLAYER_DEV_DATA_DIR
# (which is where the *new* install will land, possibly elsewhere). Used by
# the cleanup pass to wipe leftover state from the old daemon.
if [ -n "$REPLACE" ]; then
    ZLAYER_DEV_NAME="${ZLAYER_DEV_NAME:-zlayer}"
    ZLAYER_DEV_REGISTER="${ZLAYER_DEV_REGISTER:-1}"
    case "$(uname -s)" in
        Linux)  REPLACE_PROD_DATA_DIR="/var/lib/zlayer" ;;
        Darwin) REPLACE_PROD_DATA_DIR="${HOME}/.zlayer" ;;
    esac
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

# On macOS, sign the binary with the virtualization entitlement so the VZ
# runtime can boot guest VMs. Ad-hoc by default (fine for local use); set
# VZ_SIGN_IDENTITY to a Developer ID for a distributable build. No-op elsewhere.
if [ "$OS" = "darwin" ] && [ -x "${REPO_ROOT}/scripts/sign-vz.sh" ]; then
    echo "Signing ${TARGET_BIN} for the macOS VZ runtime..."
    "${REPO_ROOT}/scripts/sign-vz.sh" "$TARGET_BIN" "${VZ_SIGN_IDENTITY:--}" || \
        echo "Warning: VZ signing failed; the VZ runtime won't boot VMs until signed." >&2
fi

# --- Stop the existing daemon ---
# In --replace mode we fully uninstall the running production zlayer
# (removes the systemd unit / launchd plist and stops it). In dev mode
# we just stop the side-by-side dev daemon by its dev name.
echo ""
if [ -n "$REPLACE" ]; then
    echo "Uninstalling running zlayer daemon (if any)..."
    if [ "$OS" = "linux" ]; then
        sudo systemctl stop zlayer.service 2>/dev/null || true
    fi
    if [ -x "${ZLAYER_DEV_BIN_DIR}/zlayer" ]; then
        case "$OS" in
            linux)
                sudo "${ZLAYER_DEV_BIN_DIR}/zlayer" daemon uninstall 2>/dev/null || true
                ;;
            darwin)
                # No sudo — user-level launchd lives under gui/$uid; bootout must
                # run as that user to target the right launchd domain.
                "${ZLAYER_DEV_BIN_DIR}/zlayer" daemon uninstall 2>/dev/null || true
                ;;
        esac
    fi
else
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
                "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" daemon stop --daemon-name "${ZLAYER_DEV_NAME}" 2>/dev/null || true
                ;;
        esac
    fi
fi

# --- Clean stale WireGuard interfaces scoped to this daemon (Linux only) ---
# macOS has no overlay/WireGuard interfaces — the sandbox runtime doesn't use
# overlay networking (see daemon.rs around the with_overlay handling).
# Deployment-name prefix is "zlayer" in --replace mode, ZLAYER_DEV_NAME otherwise.
if [ "$OS" = "linux" ]; then
    if [ -n "$REPLACE" ]; then
        iface_prefix="zl-zlayer-"
    else
        iface_prefix="zl-${ZLAYER_DEV_NAME}-"
    fi
    for iface in $(ip -br link 2>/dev/null | awk -v p="$iface_prefix" '$1 ~ "^"p {print $1}'); do
        echo "  Removing stale interface: $iface"
        sudo ip link delete "$iface" 2>/dev/null || true
    done
fi

# --- Extra cleanup in --replace mode (mirrors install.sh) ---
# Stomping on the production install: also clean WireGuard UAPI sockets,
# kill stale boringtun/zlayer holders of the WG port, and remove stale
# state files of the OLD daemon (always at platform defaults — not at
# ZLAYER_DEV_DATA_DIR, which is where the NEW install will land) so the
# freshly-installed daemon comes up clean.
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
    sudo rm -f "${REPLACE_PROD_DATA_DIR}/daemon.json" 2>/dev/null || true
    sudo rm -f /var/run/zlayer.sock 2>/dev/null || true
    sudo rm -f "${REPLACE_PROD_DATA_DIR}/run/zlayer.pid" 2>/dev/null || true
    sleep 3
fi
if [ -n "$REPLACE" ] && [ "$OS" = "darwin" ]; then
    rm -f "${REPLACE_PROD_DATA_DIR}/daemon.json" 2>/dev/null || true
    rm -f "${REPLACE_PROD_DATA_DIR}/run/zlayer.sock" 2>/dev/null || true
    rm -f "${REPLACE_PROD_DATA_DIR}/run/zlayer.pid" 2>/dev/null || true
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
    "${ZLAYER_DEV_DATA_DIR}/tmp" \
    "${ZLAYER_DEV_DATA_DIR}/volumes"
# A prior root/system daemon install may have left the data dir (or subdirs)
# root-owned; `install -d -m 0750` and the daemon's own writes then fail with
# "Operation not permitted" for this non-root install because maybe_sudo does
# NOT sudo for $HOME paths. When the data dir lives under $HOME (the rootless
# per-user layout) reclaim ownership of the WHOLE tree once, so every subsequent
# step — and the rootless daemon itself — can write it. For a system data dir
# (e.g. /var/lib/zlayer) only reclaim the secrets subdir, leaving the system
# tree owned by the system service.
case "${ZLAYER_DEV_DATA_DIR}" in
    "${HOME}"/*)
        if [ -d "${ZLAYER_DEV_DATA_DIR}" ] && [ ! -O "${ZLAYER_DEV_DATA_DIR}" ]; then
            echo "Reclaiming ownership of ${ZLAYER_DEV_DATA_DIR} (was root-owned)..."
            sudo chown -R "$(id -un)" "${ZLAYER_DEV_DATA_DIR}" || true
        fi
        ;;
    *)
        if [ -d "${ZLAYER_DEV_DATA_DIR}/secrets" ] && [ ! -O "${ZLAYER_DEV_DATA_DIR}/secrets" ]; then
            echo "Reclaiming ownership of ${ZLAYER_DEV_DATA_DIR}/secrets (was root-owned)..."
            sudo chown -R "$(id -un)" "${ZLAYER_DEV_DATA_DIR}/secrets" || true
        fi
        ;;
esac
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

    # In --replace mode we call the install the same way install.sh does:
    # no --data-dir / --daemon-name overrides — let the daemon use its
    # built-in production defaults. EXCEPT when the user explicitly passed
    # --data-dir / ZLAYER_DEV_DATA_DIR; then forward that, but still no
    # --daemon-name (it's always "zlayer" in --replace mode).
    GLOBAL_ARGS=()
    DAEMON_NAME_ARGS=()
    if [ -n "$REPLACE" ]; then
        if [ -n "$DATA_DIR_EXPLICIT" ]; then
            GLOBAL_ARGS=(--data-dir "${ZLAYER_DEV_DATA_DIR}")
        fi
    else
        GLOBAL_ARGS=(--data-dir "${ZLAYER_DEV_DATA_DIR}")
        DAEMON_NAME_ARGS=(--daemon-name "${ZLAYER_DEV_NAME}")
    fi

    case "$OS" in
        linux)
            # Intentional word-splitting of DAEMON_INSTALL_FLAGS below.
            # shellcheck disable=SC2086
            sudo "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" \
                ${GLOBAL_ARGS[@]+"${GLOBAL_ARGS[@]}"} \
                daemon ${DAEMON_NAME_ARGS[@]+"${DAEMON_NAME_ARGS[@]}"} \
                install ${DAEMON_INSTALL_FLAGS}
            ;;
        darwin)
            # No sudo on macOS: `daemon install` is rootless — it registers a
            # per-user launchd Agent under ~/Library/LaunchAgents writing only to
            # ~/.zlayer (see launchd_context()/install() in daemon.rs). It self-
            # elevates with a single surgical sudo ONLY if the overlay system
            # service actually changed; a daemon-only reinstall prompts for
            # nothing.
            # Intentional word-splitting of DAEMON_INSTALL_FLAGS below.
            # shellcheck disable=SC2086
            "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" \
                ${GLOBAL_ARGS[@]+"${GLOBAL_ARGS[@]}"} \
                daemon ${DAEMON_NAME_ARGS[@]+"${DAEMON_NAME_ARGS[@]}"} \
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
        # The docker socket path is fixed by the daemon at install time
        # via `default_docker_socket_path()` — which uses the *platform-
        # default* data dir, not whatever --data-dir override the user
        # passed. So even with a custom --data-dir, the actual socket
        # lives at the platform default.
        case "$OS" in
            linux)
                # Linux daemon runs as root via systemd → hardcoded.
                DOCKER_SOCKET_TARGET="/var/run/zlayer/docker.sock"
                ;;
            darwin)
                # macOS daemon runs as user via launchd → under HOME.
                DOCKER_SOCKET_TARGET="${REPLACE_PROD_DATA_DIR}/run/docker.sock"
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
