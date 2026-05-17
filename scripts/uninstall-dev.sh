#!/usr/bin/env bash
# ZLayer dev installer teardown.
# Counterpart to install-dev.sh — calls the binary's `daemon uninstall`
# with the cleanup flags so the system is restored to pre-install state.
#
# Env knobs:
#   ZLAYER_DEV_NAME       (default: zlayer-dev)
#   ZLAYER_DEV_DATA_DIR   (default: /var/lib/${ZLAYER_DEV_NAME})
#   ZLAYER_DEV_BIN_DIR    (default: /usr/local/bin)
#   ZLAYER_DEV_WIPE_DATA  (default: empty — if non-empty, also delete data dir)
#
set -eu

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

case "$(uname -s)" in
    Linux) ;;
    *)
        echo "Error: uninstall-dev.sh currently supports Linux only." >&2
        exit 1
        ;;
esac

ZLAYER_DEV_NAME="${ZLAYER_DEV_NAME:-zlayer-dev}"
ZLAYER_DEV_DATA_DIR="${ZLAYER_DEV_DATA_DIR:-/var/lib/${ZLAYER_DEV_NAME}}"
ZLAYER_DEV_BIN_DIR="${ZLAYER_DEV_BIN_DIR:-/usr/local/bin}"

echo ""
echo "Tearing down: ${ZLAYER_DEV_NAME}"
echo "  binary:    ${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}"
echo "  data_dir:  ${ZLAYER_DEV_DATA_DIR}"
echo "  wipe data: ${ZLAYER_DEV_WIPE_DATA:-no}"
echo ""

# Best-effort stop any manually-launched serve processes (the
# non-registered path that `daemon uninstall` can't see).
sudo pkill -f "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME} .* serve" 2>/dev/null || true
sleep 1
sudo pkill -9 -f "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME} .* serve" 2>/dev/null || true

# Remove stale WireGuard interfaces with our daemon's prefix.
# IFNAMSIZ caps interface names at 15 chars, so the prefix is
# capped accordingly.
WG_PREFIX="zl-${ZLAYER_DEV_NAME}"
WG_PREFIX_TRIMMED="${WG_PREFIX:0:14}-"  # 14 + '-' = 15 max
for iface in $(ip -br link 2>/dev/null | awk -v p="$WG_PREFIX_TRIMMED" '$1 ~ "^"p {print $1}'); do
    echo "  Removing stale interface: $iface"
    sudo ip link delete "$iface" 2>/dev/null || true
done

# Delegate to `daemon uninstall` with the cleanup flags.
EXTRA_FLAGS="--remove-binary --remove-completions"
if [ -n "${ZLAYER_DEV_WIPE_DATA:-}" ]; then
    EXTRA_FLAGS="${EXTRA_FLAGS} --purge-data"
fi

if [ -x "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" ]; then
    sudo "${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}" \
        --data-dir "${ZLAYER_DEV_DATA_DIR}" \
        --daemon-name "${ZLAYER_DEV_NAME}" \
        daemon uninstall ${EXTRA_FLAGS} || true
else
    echo "Binary already gone at ${ZLAYER_DEV_BIN_DIR}/${ZLAYER_DEV_NAME}."
    echo "Falling back to best-effort orphan cleanup..."
    sudo systemctl stop "${ZLAYER_DEV_NAME}.service" 2>/dev/null || true
    sudo rm -f "/etc/systemd/system/${ZLAYER_DEV_NAME}.service"
    sudo systemctl daemon-reload 2>/dev/null || true
    sudo rm -f "/etc/bash_completion.d/${ZLAYER_DEV_NAME}"
    rm -f "${HOME}/.zsh/completions/_${ZLAYER_DEV_NAME}"
    rm -f "${HOME}/.config/fish/completions/${ZLAYER_DEV_NAME}.fish"
    if [ -n "${ZLAYER_DEV_WIPE_DATA:-}" ]; then
        sudo rm -rf "${ZLAYER_DEV_DATA_DIR}"
    fi
fi

echo ""
echo "${ZLAYER_DEV_NAME} uninstalled."
if [ -z "${ZLAYER_DEV_WIPE_DATA:-}" ]; then
    echo ""
    echo "Data dir retained at: ${ZLAYER_DEV_DATA_DIR}"
    echo "  (re-run with ZLAYER_DEV_WIPE_DATA=1 to delete it)"
fi
