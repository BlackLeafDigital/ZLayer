#!/usr/bin/env bash
# local-runner-down.sh — stop whichever local runner is up (ZArcRunner or the
# act-based forgejo-runner). Idempotent: prints "nothing to do" and exits 0 if
# no runner is running.
#
# Usage:
#   scripts/local-runner-down.sh [--remove-registration]

set -euo pipefail

ZARCRUNNER_PID_FILE="/tmp/zarcrunner.pid"
LOCAL_PID_FILE="/tmp/zlayer-local-runner.pid"
KIND_FILE="/tmp/zlayer-local-runner.kind"

REMOVE_REGISTRATION=0

# ---------- arg parse ----------

while [[ $# -gt 0 ]]; do
    case "$1" in
        --remove-registration)
            REMOVE_REGISTRATION=1
            shift
            ;;
        -h|--help)
            cat <<'EOF'
Usage: local-runner-down.sh [--remove-registration]

Stops the local Forgejo Actions runner (ZArcRunner or forgejo-runner act fork).

Options:
  --remove-registration   Also unregister the runner from Forgejo (best-effort).
EOF
            exit 0
            ;;
        *)
            printf 'warning: unknown argument: %s\n' "$1" >&2
            shift
            ;;
    esac
done

# ---------- helpers ----------

err() {
    printf 'error: %s\n' "$*" >&2
}

pid_is_alive() {
    local pid="$1"
    [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null
}

# Stop a PID with SIGTERM, wait up to 10s, then SIGKILL.
stop_pid() {
    local pid="$1"
    local label="$2"
    if ! pid_is_alive "$pid"; then
        printf '%s PID %s already dead\n' "$label" "$pid"
        return 0
    fi
    printf 'stopping %s (PID %s)... ' "$label" "$pid"
    kill -TERM "$pid" 2>/dev/null || true
    local i
    for i in 1 2 3 4 5 6 7 8 9 10; do
        if ! pid_is_alive "$pid"; then
            printf 'stopped (SIGTERM, %ss)\n' "$i"
            return 0
        fi
        sleep 1
    done
    printf 'still alive after 10s, SIGKILL... '
    kill -KILL "$pid" 2>/dev/null || true
    sleep 1
    if pid_is_alive "$pid"; then
        printf 'FAILED — PID %s still alive\n' "$pid"
        return 1
    fi
    printf 'stopped (SIGKILL)\n'
}

# Attempt to stop one PID file. Returns 0 on success or if file absent;
# echoes "stopped" via stdout to indicate work was done.
stop_pid_file() {
    local file="$1"
    local label="$2"
    [[ -f "$file" ]] || return 1
    local pid
    pid="$(cat "$file" 2>/dev/null || true)"
    if [[ -z "$pid" ]]; then
        printf '%s PID file %s was empty, removing\n' "$label" "$file"
        rm -f "$file"
        return 1
    fi
    stop_pid "$pid" "$label" || true
    rm -f "$file"
    return 0
}

# ---------- main ----------

did_work=0

if [[ -f "$ZARCRUNNER_PID_FILE" ]]; then
    if stop_pid_file "$ZARCRUNNER_PID_FILE" "zarcrunner"; then
        did_work=1
    fi
fi

if [[ -f "$LOCAL_PID_FILE" ]]; then
    kind="unknown"
    if [[ -f "$KIND_FILE" ]]; then
        kind="$(cat "$KIND_FILE" 2>/dev/null || echo unknown)"
    fi
    if stop_pid_file "$LOCAL_PID_FILE" "local-runner($kind)"; then
        did_work=1
    fi
fi

# Always clear the kind marker.
[[ -f "$KIND_FILE" ]] && rm -f "$KIND_FILE"

if [[ "$did_work" -eq 0 ]]; then
    printf 'no runner up\n'
    if [[ "$REMOVE_REGISTRATION" -eq 1 ]]; then
        printf '(--remove-registration ignored: no runner was running)\n'
    fi
    exit 0
fi

# ---------- optional unregister ----------

if [[ "$REMOVE_REGISTRATION" -eq 1 ]]; then
    # Determine which path to use based on what we just stopped. The
    # ZArcRunner PID file existing (now removed) implies ZArcRunner; otherwise
    # ForgejoRunner.
    repo="${ZARCRUNNER_REPO:-/home/zach/github/ZArcRunner}"
    unregister_script="${repo}/scripts/unregister-blackleaf.sh"
    if command -v zarcrunner >/dev/null 2>&1 && [[ -f "$unregister_script" ]]; then
        printf '\nremoving ZArcRunner registration via %s\n' "$unregister_script"
        if ! bash "$unregister_script" --remove-registration; then
            err "ZArcRunner unregister script returned non-zero"
        fi
    else
        workdir="${XDG_STATE_HOME:-$HOME/.local/state}/forgejo-runner-local"
        cat <<EOF

manual step required to remove forgejo-runner registration:
  rm -rf ${workdir}/.runner
  OR delete the runner via the Forgejo UI:
      ${FORGEJO_RUNNER_INSTANCE:-https://forge.blackleafdigital.com}/-/admin/actions/runners

(forgejo-runner has no clean unregister command in some versions.)
EOF
    fi
fi

exit 0
