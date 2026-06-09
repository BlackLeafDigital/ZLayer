#!/usr/bin/env bash
# local-runner-up.sh — start a local Forgejo Actions runner (ZArcRunner preferred,
# falls back to act-based forgejo-runner) registered against the user's Forgejo
# instance.
#
# Usage:
#   scripts/local-runner-up.sh [--foreground] [--token TOKEN] [--url URL]
#                              [--name NAME] [--labels LABELS] [...]
#
# Idempotent: if a runner is already up, prints status and exits 0.

set -euo pipefail

ZARCRUNNER_PID_FILE="/tmp/zarcrunner.pid"
LOCAL_PID_FILE="/tmp/zlayer-local-runner.pid"
KIND_FILE="/tmp/zlayer-local-runner.kind"

# ---------- helpers ----------

err() {
    printf 'error: %s\n' "$*" >&2
}

pid_is_alive() {
    local pid="$1"
    [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null
}

# Reads a PID from a file, echoes it if the process is alive, otherwise nothing.
live_pid_from_file() {
    local file="$1"
    [[ -f "$file" ]] || return 0
    local pid
    pid="$(cat "$file" 2>/dev/null || true)"
    if pid_is_alive "$pid"; then
        printf '%s' "$pid"
    fi
}

# ---------- idempotency ----------

check_already_running() {
    local zpid lpid kind="unknown"
    zpid="$(live_pid_from_file "$ZARCRUNNER_PID_FILE")"
    lpid="$(live_pid_from_file "$LOCAL_PID_FILE")"
    if [[ -f "$KIND_FILE" ]]; then
        kind="$(cat "$KIND_FILE" 2>/dev/null || echo unknown)"
    fi
    if [[ -n "$zpid" ]]; then
        printf 'runner already running (PID=%s, kind=zarcrunner) — use scripts/local-runner-down.sh to stop\n' "$zpid"
        exit 0
    fi
    if [[ -n "$lpid" ]]; then
        printf 'runner already running (PID=%s, kind=%s) — use scripts/local-runner-down.sh to stop\n' "$lpid" "$kind"
        exit 0
    fi
    # Clean up stale PID files (process died without cleanup).
    [[ -f "$ZARCRUNNER_PID_FILE" ]] && rm -f "$ZARCRUNNER_PID_FILE"
    [[ -f "$LOCAL_PID_FILE" ]] && rm -f "$LOCAL_PID_FILE"
    [[ -f "$KIND_FILE" ]] && rm -f "$KIND_FILE"
}

# ---------- detection ----------

detect_runner() {
    if command -v zarcrunner >/dev/null 2>&1; then
        printf 'zarcrunner'
        return 0
    fi
    if command -v forgejo-runner >/dev/null 2>&1; then
        printf 'forgejo-runner'
        return 0
    fi
    return 1
}

print_install_hints_and_exit() {
    cat >&2 <<'EOF'
No runner found on PATH. Install one of:
  - ZArcRunner: cd /home/zach/github/ZArcRunner && make build && sudo install zarcrunner /usr/local/bin/
  - ForgejoRunner: curl -fsSL https://forge.blackleafdigital.com/Public/ForgejoRunner/raw/branch/main/scripts/install-linux.sh | bash
EOF
    exit 1
}

# ---------- ZArcRunner path ----------

run_zarcrunner() {
    local repo="${ZARCRUNNER_REPO:-/home/zach/github/ZArcRunner}"
    local script="${repo}/scripts/register-blackleaf.sh"
    if [[ ! -f "$script" ]]; then
        err "ZARCRUNNER_REPO=${repo} does not contain scripts/register-blackleaf.sh — set ZARCRUNNER_REPO env to your ZArcRunner checkout"
        exit 1
    fi
    if [[ ! -x "$script" ]]; then
        err "ZArcRunner register script is not executable: ${script}"
        exit 1
    fi
    # Forward every arg verbatim. exec replaces this process so the user gets
    # ZArcRunner's signal handling, foreground behavior, etc. natively.
    exec bash "$script" "$@"
}

# ---------- ForgejoRunner (act fork) path ----------

# Parse our own arg set for the forgejo-runner path. Unknown args are dropped
# silently (the act fork takes no extra runtime args beyond `daemon`).
parse_forgejo_args() {
    FG_FOREGROUND=0
    FG_TOKEN=""
    FG_URL=""
    FG_NAME=""
    FG_LABELS=""
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --foreground)
                FG_FOREGROUND=1
                shift
                ;;
            --token)
                FG_TOKEN="${2:-}"
                shift 2
                ;;
            --url)
                FG_URL="${2:-}"
                shift 2
                ;;
            --name)
                FG_NAME="${2:-}"
                shift 2
                ;;
            --labels)
                FG_LABELS="${2:-}"
                shift 2
                ;;
            *)
                shift
                ;;
        esac
    done
}

default_labels() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"
    if [[ "$os" == "Darwin" ]]; then
        if [[ "$arch" == "arm64" ]]; then
            printf 'local-dev,darwin,arm64,actfork'
        else
            printf 'local-dev,darwin,%s,actfork' "$arch"
        fi
    else
        printf 'local-dev,linux,%s,actfork' "$arch"
    fi
}

# Detect whether `forgejo-runner register` supports --token or --secret.
# Older act-fork builds used --secret; modern builds use --token. If both
# appear in --help, prefer --token.
detect_token_flag() {
    local help
    if ! help="$(forgejo-runner register --help 2>&1)"; then
        # Couldn't read help — default to --token (modern default).
        printf -- '--token'
        return 0
    fi
    if grep -qE -- '(^|[[:space:]])--token([[:space:]=]|$)' <<<"$help"; then
        printf -- '--token'
        return 0
    fi
    if grep -qE -- '(^|[[:space:]])--secret([[:space:]=]|$)' <<<"$help"; then
        printf -- '--secret'
        return 0
    fi
    # Unknown — try --token anyway; the binary will reject if invalid.
    printf -- '--token'
}

run_forgejo_runner() {
    parse_forgejo_args "$@"

    # Token resolution: env > --token > file
    local token="$FG_TOKEN"
    if [[ -z "$token" && -n "${FORGEJO_RUNNER_TOKEN:-}" ]]; then
        token="$FORGEJO_RUNNER_TOKEN"
    fi
    local token_file="${HOME}/.config/forgejo-runner/token"
    if [[ -z "$token" && -f "$token_file" ]]; then
        token="$(tr -d '[:space:]' <"$token_file")"
    fi
    if [[ -z "$token" ]]; then
        err "no Forgejo runner registration token found. Set FORGEJO_RUNNER_TOKEN, pass --token TOKEN, or write the token to ${token_file}"
        exit 1
    fi

    # URL resolution: env > --url > default
    local url="${FORGEJO_RUNNER_INSTANCE:-$FG_URL}"
    if [[ -z "$url" ]]; then
        url="https://forge.blackleafdigital.com"
    fi

    # Name resolution: env > --name > default
    local name="${FORGEJO_RUNNER_NAME:-$FG_NAME}"
    if [[ -z "$name" ]]; then
        name="$(hostname)-fjorunner"
    fi

    # Labels resolution: env > --labels > os/arch default
    local labels="${FORGEJO_RUNNER_LABELS:-$FG_LABELS}"
    if [[ -z "$labels" ]]; then
        labels="$(default_labels)"
    fi

    local workdir="${XDG_STATE_HOME:-$HOME/.local/state}/forgejo-runner-local"
    mkdir -p "$workdir"
    cd "$workdir"

    local token_flag
    token_flag="$(detect_token_flag)"

    printf 'registering forgejo-runner (act fork)\n'
    printf '  instance: %s\n' "$url"
    printf '  name:     %s\n' "$name"
    printf '  labels:   %s\n' "$labels"
    printf '  workdir:  %s\n' "$workdir"
    printf '  flag:     %s\n' "$token_flag"

    # Register. The act fork writes a `.runner` file in cwd.
    if ! forgejo-runner register \
        --no-interactive \
        --instance "$url" \
        "$token_flag" "$token" \
        --name "$name" \
        --labels "$labels"; then
        err "forgejo-runner register failed"
        exit 1
    fi

    if [[ ! -f "${workdir}/.runner" ]]; then
        err "registration appeared to succeed but ${workdir}/.runner was not created"
        exit 1
    fi

    if [[ "$FG_FOREGROUND" -eq 1 ]]; then
        printf 'starting forgejo-runner in foreground\n'
        exec forgejo-runner daemon
    fi

    local log="${workdir}/runner.log"
    # shellcheck disable=SC2024  # nohup redirection is intentional
    nohup forgejo-runner daemon >"$log" 2>&1 &
    local pid=$!
    printf '%s\n' "$pid" >"$LOCAL_PID_FILE"
    printf '%s\n' "$pid" >"${workdir}/runner.pid"
    printf 'forgejo-runner\n' >"$KIND_FILE"

    # Poll for 3 seconds to catch immediate startup failures.
    local i
    for i in 1 2 3; do
        sleep 1
        if ! pid_is_alive "$pid"; then
            err "forgejo-runner daemon died within ${i}s of startup. Last 20 lines of ${log}:"
            tail -n 20 "$log" >&2 || true
            rm -f "$LOCAL_PID_FILE" "${workdir}/runner.pid" "$KIND_FILE"
            exit 1
        fi
    done

    printf '\nforgejo-runner started\n'
    printf '  PID:     %s\n' "$pid"
    printf '  workdir: %s\n' "$workdir"
    printf '  log:     %s\n' "$log"
    printf '  stop:    scripts/local-runner-down.sh\n'
}

# ---------- main ----------

main() {
    check_already_running

    local kind
    if ! kind="$(detect_runner)"; then
        print_install_hints_and_exit
    fi

    case "$kind" in
        zarcrunner)
            run_zarcrunner "$@"
            ;;
        forgejo-runner)
            run_forgejo_runner "$@"
            ;;
        *)
            err "internal: unknown runner kind: ${kind}"
            exit 1
            ;;
    esac
}

main "$@"
