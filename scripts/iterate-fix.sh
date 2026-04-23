#!/usr/bin/env bash
# Run the given command, tee stdout+stderr to .iter/<timestamp>-<slug>.log,
# print the log path, and exit with the command's status.
#
# Usage:
#   scripts/iterate-fix.sh cargo clippy --workspace --all-targets -- -D warnings

set -u -o pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <command> [args...]" >&2
  exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$root/.iter"

ts="$(date -u +%Y%m%dT%H%M%SZ)"
slug="$(printf '%s' "$*" | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9' '-' | sed -E 's/-+/-/g; s/^-//; s/-$//' | cut -c1-60)"
log="$root/.iter/${ts}-${slug}.log"

echo "+ $*" | tee "$log"
{ "$@" 2>&1; status=$?; exit $status; } | tee -a "$log"
status="${PIPESTATUS[0]}"

echo ">> log: $log"
exit "$status"
