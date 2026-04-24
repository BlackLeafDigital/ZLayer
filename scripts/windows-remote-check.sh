#!/usr/bin/env bash
# Sync this working tree to a Windows host over SSH and run cargo check,
# clippy, and tests for both the default (HCS-only) and the
# hcs-runtime,wsl composite builds. Mirrors the `test-windows` job in
# `.forgejo/workflows/ci.yaml` so a local pass here means CI should pass.
#
# The SSH session runs as a real user (not LocalSystem), so WSL is usable
# and the WSL_E_LOCAL_SYSTEM_NOT_SUPPORTED error the Forgejo runner hits
# does not apply here.
#
# Assumes the remote shell is PowerShell (the OpenSSH default on Windows).
# Cargo and protoc must already be on the remote user's PATH.
#
# Output is teed to a timestamped log file under `logs/` so multiple
# runs sort lexicographically:
#
#   logs/run_YYYYMMDDTHHMMSSZ_<slug>_log.txt
#
# Env:
#   ZLAYER_WIN_HOST   SSH target (user@host). Optional. If unset, the script
#                     probes the known MiniWindows endpoints in order:
#                       1. MiniWindows@192.168.68.92           (LAN)
#                       2. MiniWindows@100.96.69.45            (Netbird overlay)
#                       3. MiniWindows@miniwindows.net.home    (Netbird DNS)
#                     The first one that responds to `ssh -o ConnectTimeout=3
#                     <host> 'echo ok'` is used.
#   ZLAYER_WIN_PATH   Remote path to sync into. Default: /cygdrive/c/src/ZLayer
#                     (Chocolatey rsync.exe is cygwin-backed; maps to C:\src\ZLayer.)
#   ZLAYER_WIN_SKIP_E2E
#                     If set to "1", skip the `--ignored` composite_dispatch_e2e
#                     integration test (which needs a real Windows host with
#                     Wintun + WSL distro). Default: 0 (run it).
#   ZLAYER_WIN_SKIP_SYNC
#                     If set to "1", skip the rsync step and re-use the tree
#                     already on the remote. Useful for iterating after a
#                     manual edit on the remote.
#
# Usage:
#   ./scripts/windows-remote-check.sh                       # auto-detect host
#   ZLAYER_WIN_HOST=user@host ./scripts/windows-remote-check.sh

set -uo pipefail

# Auto-detect the MiniWindows host if not overridden.
DEFAULT_WIN_HOSTS=(
  "MiniWindows@192.168.68.92"
  "MiniWindows@100.96.69.45"
  "MiniWindows@miniwindows.net.home"
)

if [[ -z "${ZLAYER_WIN_HOST:-}" ]]; then
  echo ">> auto-detecting MiniWindows host"
  for candidate in "${DEFAULT_WIN_HOSTS[@]}"; do
    if ssh -o BatchMode=yes -o ConnectTimeout=3 -o StrictHostKeyChecking=accept-new \
           "$candidate" 'echo ok' >/dev/null 2>&1; then
      ZLAYER_WIN_HOST="$candidate"
      echo "   using: $ZLAYER_WIN_HOST"
      break
    else
      echo "   unreachable: $candidate"
    fi
  done
  if [[ -z "${ZLAYER_WIN_HOST:-}" ]]; then
    echo "ERROR: none of the default MiniWindows hosts were reachable." >&2
    echo "       Tried: ${DEFAULT_WIN_HOSTS[*]}" >&2
    echo "       Set ZLAYER_WIN_HOST=user@host to override." >&2
    exit 2
  fi
fi

REMOTE_UNIX_PATH="${ZLAYER_WIN_PATH:-/cygdrive/c/src/ZLayer}"
# Translate /cygdrive/c/src/ZLayer -> C:\src\ZLayer for PowerShell `cd`.
REMOTE_WIN_PATH="$(printf '%s' "$REMOTE_UNIX_PATH" | sed -E 's|^/cygdrive/([a-zA-Z])/|\1:\\|; s|^/([a-zA-Z])/|\1:\\|; s|/|\\|g')"

SKIP_E2E="${ZLAYER_WIN_SKIP_E2E:-0}"
SKIP_SYNC="${ZLAYER_WIN_SKIP_SYNC:-0}"

LOCAL_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# -----------------------------------------------------------------------------
# Log file setup — lexicographically sortable so `ls logs/` shows newest last.
# -----------------------------------------------------------------------------
LOG_DIR="$LOCAL_ROOT/logs"
mkdir -p "$LOG_DIR"
TS="$(date -u +%Y%m%dT%H%M%SZ)"
SLUG="$(echo "$ZLAYER_WIN_HOST" | tr -c 'A-Za-z0-9' '_' | tr -s '_')"
LOG_FILE="$LOG_DIR/run_${TS}_${SLUG}_log.txt"

# -----------------------------------------------------------------------------
# Remote command helpers.
# -----------------------------------------------------------------------------
FAIL_STEPS=()

run_remote_ps() {
  local label="$1"; shift
  local cmd="$1"; shift
  echo
  echo ">> [$(date -u +%H:%M:%SZ)] $label"
  echo "   cmd: $cmd"
  # `uv run --python 3.12 --` provisions a Python 3.12 interpreter so
  # pyo3-build-config (zlayer-py dep, pulled transitively into --workspace)
  # can find a Python at build time. MiniWindows has uv installed via
  # chocolatey but no system Python on the SSH session PATH.
  if ssh "$ZLAYER_WIN_HOST" "\$env:PROTOC = 'C:\\ProgramData\\chocolatey\\bin\\protoc.exe'; cd '$REMOTE_WIN_PATH'; uv run --python 3.12 -- $cmd"; then
    echo "   >> OK: $label"
  else
    local rc=$?
    echo "   >> FAIL ($rc): $label"
    FAIL_STEPS+=("$label")
  fi
}

# -----------------------------------------------------------------------------
# The body of the check. Wrapped in a function so we can pipe its whole
# output through `tee` at the call site — this is more robust than
# `exec > >(tee ...)` which can EAGAIN-break rsync's non-blocking socket
# writes.
# -----------------------------------------------------------------------------
main_body() {
  echo "======================================================================"
  echo " ZLayer Windows remote check"
  echo " host:       $ZLAYER_WIN_HOST"
  echo " remote:     $REMOTE_UNIX_PATH  (-> $REMOTE_WIN_PATH)"
  echo " local root: $LOCAL_ROOT"
  echo " started:    $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo " skip_e2e:   $SKIP_E2E"
  echo " skip_sync:  $SKIP_SYNC"
  echo "======================================================================"

  # ---------------------------------------------------------------------------
  # Sync the tree.
  # ---------------------------------------------------------------------------
  if [[ "$SKIP_SYNC" == "1" ]]; then
    echo ">> skipping rsync (ZLAYER_WIN_SKIP_SYNC=1)"
  else
    echo ">> syncing $LOCAL_ROOT -> $ZLAYER_WIN_HOST:$REMOTE_UNIX_PATH"
    if ! rsync -az --delete \
        --exclude target/ \
        --exclude .git/ \
        --exclude '.iter/' \
        --exclude 'logs/' \
        "$LOCAL_ROOT"/ "$ZLAYER_WIN_HOST:$REMOTE_UNIX_PATH/"; then
      echo "ERROR: rsync failed" >&2
      FAIL_STEPS+=("rsync")
      return 1
    fi
  fi

  # ---------------------------------------------------------------------------
  # Checks (compile + clippy). Only the `hcs-runtime,wsl` composite is
  # exercised — the "default features" Windows build is a thin CLI without
  # zlayer-agent, which `build.yml` never ships
  # (`--no-default-features --features hcs-runtime,wsl` is the only shipped
  # combo).
  # ---------------------------------------------------------------------------
  run_remote_ps "cargo check (hcs-runtime,wsl composite)" \
    "cargo check --package zlayer --features hcs-runtime,wsl --all-targets"

  run_remote_ps "cargo clippy (hcs-runtime,wsl composite)" \
    "cargo clippy --package zlayer --features hcs-runtime,wsl --all-targets -- -D warnings"

  # ---------------------------------------------------------------------------
  # Tests — mirrors `.forgejo/workflows/ci.yaml::test-windows`.
  # ---------------------------------------------------------------------------
  run_remote_ps "cargo test zlayer-agent (hcs-runtime,wsl --lib)" \
    "cargo test --package zlayer-agent --features hcs-runtime,wsl --lib --no-fail-fast"

  # Mirrors `.forgejo/workflows/build.yml::build-windows-amd64::Test zlayer-hcs
  # on Windows`. Previously this script's `--workspace --lib` only covered lib
  # unit tests, not integration-test binaries under crates/zlayer-hcs/tests/,
  # so the build.yml step could fail while the local check stayed green.
  run_remote_ps "cargo test --package zlayer-hcs (mirrors build.yml)" \
    "cargo test --package zlayer-hcs --no-fail-fast"

  if [[ "$SKIP_E2E" == "1" ]]; then
    echo
    echo ">> skipping composite_dispatch_e2e (ZLAYER_WIN_SKIP_E2E=1)"
  else
    run_remote_ps "cargo test composite_dispatch_e2e (hcs-runtime,wsl --ignored)" \
      "cargo test --package zlayer-agent --features hcs-runtime,wsl --test composite_dispatch_e2e -- --ignored --nocapture"
  fi

  # ---------------------------------------------------------------------------
  # Summary.
  # ---------------------------------------------------------------------------
  echo
  echo "======================================================================"
  echo " ZLayer Windows remote check — summary"
  echo " finished: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "======================================================================"
  if [[ ${#FAIL_STEPS[@]} -eq 0 ]]; then
    echo " all steps OK"
  else
    echo " FAILED steps:"
    for step in "${FAIL_STEPS[@]}"; do
      echo "   - $step"
    done
  fi
  echo " log: $LOG_FILE"
  echo " host: $ZLAYER_WIN_HOST"

  if [[ ${#FAIL_STEPS[@]} -ne 0 ]]; then
    return 1
  fi
  return 0
}

echo "log file: $LOG_FILE"
# Tee at the top level — avoids `exec > >(tee ...)` process-substitution
# edge cases that break rsync's socket writes with EAGAIN.
main_body 2>&1 | tee "$LOG_FILE"
# Propagate the body's exit status (first in the pipeline).
exit "${PIPESTATUS[0]}"
