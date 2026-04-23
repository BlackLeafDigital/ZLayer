#!/usr/bin/env bash
# Sync this working tree to a Windows host over SSH and run cargo check + clippy
# for both the default (HCS-only) and the hcs-runtime,wsl composite builds.
#
# The SSH session runs as a real user (not LocalSystem), so WSL is usable and
# the WSL_E_LOCAL_SYSTEM_NOT_SUPPORTED error the Forgejo runner hits does not
# apply here.
#
# Assumes the remote shell is PowerShell (the OpenSSH default on Windows).
# Cargo and protoc must already be on the remote user's PATH.
#
# Env:
#   ZLAYER_WIN_HOST   SSH target (user@host). Required. Example: user@192.168.1.10
#   ZLAYER_WIN_PATH   Remote path to sync into. Default: /cygdrive/c/src/ZLayer
#                     (Chocolatey rsync.exe is cygwin-backed; maps to C:\src\ZLayer for cargo.)
#
# Usage:
#   ZLAYER_WIN_HOST=user@host ./scripts/windows-remote-check.sh

set -euo pipefail

if [[ -z "${ZLAYER_WIN_HOST:-}" ]]; then
  echo "ZLAYER_WIN_HOST is required (e.g. user@192.168.1.10)" >&2
  exit 2
fi

REMOTE_UNIX_PATH="${ZLAYER_WIN_PATH:-/cygdrive/c/src/ZLayer}"
# Translate /cygdrive/c/src/ZLayer -> C:\src\ZLayer for PowerShell `cd`.
REMOTE_WIN_PATH="$(printf '%s' "$REMOTE_UNIX_PATH" | sed -E 's|^/cygdrive/([a-zA-Z])/|\1:\\|; s|^/([a-zA-Z])/|\1:\\|; s|/|\\|g')"

LOCAL_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

run_remote_ps() {
  local label="$1"; shift
  local cmd="$1"; shift
  echo ">> $label"
  # `uv run --python 3.12 --` provisions a Python 3.12 interpreter so pyo3-build-config
  # (zlayer-py dep, pulled transitively into --workspace) can find a Python at build time.
  # MiniWindows has uv installed via chocolatey but no system Python on the SSH session PATH.
  ssh "$ZLAYER_WIN_HOST" "\$env:PROTOC = 'C:\\ProgramData\\chocolatey\\bin\\protoc.exe'; cd '$REMOTE_WIN_PATH'; uv run --python 3.12 -- $cmd"
}

echo ">> syncing $LOCAL_ROOT -> $ZLAYER_WIN_HOST:$REMOTE_UNIX_PATH"
rsync -az --delete \
  --exclude target/ \
  --exclude .git/ \
  --exclude '.iter/' \
  "$LOCAL_ROOT"/ "$ZLAYER_WIN_HOST:$REMOTE_UNIX_PATH/"

run_remote_ps "cargo check (default features)" \
  "cargo check --workspace --all-targets"

run_remote_ps "cargo check (hcs-runtime,wsl composite)" \
  "cargo check --package zlayer --features hcs-runtime,wsl --all-targets"

run_remote_ps "cargo clippy (default features)" \
  "cargo clippy --workspace --all-targets -- -D warnings"

run_remote_ps "cargo clippy (hcs-runtime,wsl composite)" \
  "cargo clippy --package zlayer --features hcs-runtime,wsl --all-targets -- -D warnings"

echo ">> all remote checks passed"
