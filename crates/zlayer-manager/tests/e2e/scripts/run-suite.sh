#!/usr/bin/env bash
# Hermetic runner for the zlayer-manager Intellitester suite.
#
#   1. Build zlayer + zlayer-manager (release).
#   2. Spin up a daemon against a throwaway --data-dir.
#   3. Serve the manager via cargo-leptos against that daemon.
#   4. Run intellitester against:
#       - auth workflow (bootstrap -> logout -> login)
#       - nav-smoke (every protected route renders)
#       - stale-session regression (Part A): bootstrap + persist state,
#         sqlite3 wipe the user row, verify cookies cleared on bounce.
#   5. Tear everything down and clean up the temp dir.
#
# Usage (from repo root):
#
#   crates/zlayer-manager/tests/e2e/scripts/run-suite.sh
#
# Pre-reqs:
#   - rust toolchain (cargo-leptos installed: `cargo install cargo-leptos`)
#   - npm/pnpm available on PATH (for `pnpm dlx intellitester`)
#   - playwright browsers installed (one-time: `pnpm dlx playwright install chromium`)
#   - sqlite3 on PATH (for the stale-session user wipe)

set -euo pipefail

ONLY=""
NO_BUILD=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --only) ONLY="$2"; shift 2 ;;
        --no-build) NO_BUILD=1; shift ;;
        -h|--help)
            echo "Usage: $0 [--only <test.yaml>] [--no-build]"
            echo ""
            echo "Options:"
            echo "  --only <test.yaml>  Run a single intellitester test file (relative to"
            echo "                      crates/zlayer-manager/tests/e2e/) instead of the"
            echo "                      full suite. Skips the stale-session sqlite step."
            echo "  --no-build          Skip the cargo build / cargo leptos build phase."
            echo "                      Useful when iterating locally against an already-"
            echo "                      compiled target/release."
            echo "  -h, --help          Show this message."
            echo ""
            echo "Environment overrides:"
            echo "  ZLAYER_E2E_API_PORT     Daemon API bind port      (default 13669)"
            echo "  ZLAYER_E2E_MANAGER_PORT Manager (Leptos) bind port (default 16677)"
            echo "  ZLAYER_E2E_WG_PORT      Overlay WireGuard UDP port (default 51421)"
            echo "  ZLAYER_E2E_DNS_PORT     Overlay DNS server port    (default 15354)"
            exit 0
            ;;
        *) echo "Unknown flag: $1" >&2; exit 2 ;;
    esac
done

REPO_ROOT="$(cd "$(dirname "$0")/../../../../.." && pwd)"
E2E_DIR="$REPO_ROOT/crates/zlayer-manager/tests/e2e"
INTELLITESTER_PIN="intellitester@^0.4.5"

# Non-default ports so we don't collide with any zlayer daemon already
# running on the host (the canonical 3669/6677 pair, plus 51420/15353
# for the overlay UDP+DNS sockets). Override via env if needed.
ZLAYER_E2E_API_PORT="${ZLAYER_E2E_API_PORT:-13669}"
ZLAYER_E2E_MANAGER_PORT="${ZLAYER_E2E_MANAGER_PORT:-16677}"
ZLAYER_E2E_WG_PORT="${ZLAYER_E2E_WG_PORT:-51421}"
ZLAYER_E2E_DNS_PORT="${ZLAYER_E2E_DNS_PORT:-15354}"

export ZLAYER_E2E_DATA_DIR="$(mktemp -d -t zlayer-e2e-XXXXXX)"
export ZLAYER_DATA_DIR="$ZLAYER_E2E_DATA_DIR"
export ZLAYER_JWT_SECRET="${ZLAYER_JWT_SECRET:-e2e-secret-do-not-use-in-prod-do-not-share-this-key-1234567890}"
export ZLAYER_API_URL="http://127.0.0.1:${ZLAYER_E2E_API_PORT}"
export LEPTOS_SITE_ADDR="127.0.0.1:${ZLAYER_E2E_MANAGER_PORT}"
export RUST_LOG="${RUST_LOG:-info,zlayer_manager=debug}"

# Intellitester's collectMissingEnvVars (core/loader.ts) scans every
# ${VAR} reference in the test YAML and prompts interactively when one
# isn't in process.env — even when the var is defined in the test's own
# `variables:` block. Export the test creds here so the prompt never
# fires and the suite runs unattended.
export ADMIN_EMAIL="${ADMIN_EMAIL:-admin@e2e.local}"
export ADMIN_PASSWORD="${ADMIN_PASSWORD:-e2e-test-password-1234}"
export ADMIN_DISPLAY="${ADMIN_DISPLAY:-E2E Admin}"

DAEMON_PID=""
MANAGER_PID=""

cleanup() {
    local rc=$?
    stop_stack
    rm -rf "$ZLAYER_E2E_DATA_DIR"
    rm -f "$E2E_DIR/.stale-session.state.json"
    exit "$rc"
}
trap cleanup EXIT INT TERM

start_stack() {
    echo "==> Starting daemon on 127.0.0.1:${ZLAYER_E2E_API_PORT} (data-dir: $ZLAYER_E2E_DATA_DIR, wg: ${ZLAYER_E2E_WG_PORT}, dns: ${ZLAYER_E2E_DNS_PORT})"
    ( cd "$REPO_ROOT" && \
      ./target/release/zlayer --data-dir "$ZLAYER_E2E_DATA_DIR" \
        serve --bind "127.0.0.1:${ZLAYER_E2E_API_PORT}" \
        --deployment-name zlayer-e2e \
        --wg-port "${ZLAYER_E2E_WG_PORT}" \
        --dns-port "${ZLAYER_E2E_DNS_PORT}" ) &
    DAEMON_PID=$!

    for _ in $(seq 1 60); do
        if curl -sf "http://127.0.0.1:${ZLAYER_E2E_API_PORT}/healthz" >/dev/null 2>&1 \
           || curl -sf "http://127.0.0.1:${ZLAYER_E2E_API_PORT}/" >/dev/null 2>&1; then
            break
        fi
        sleep 1
    done

    echo "==> Starting manager on 127.0.0.1:${ZLAYER_E2E_MANAGER_PORT}"
    ( cd "$REPO_ROOT/crates/zlayer-manager" && \
      ZLAYER_API_URL="$ZLAYER_API_URL" \
      LEPTOS_SITE_ADDR="$LEPTOS_SITE_ADDR" \
      RUST_LOG="$RUST_LOG" \
      ../../target/release/zlayer-manager ) &
    MANAGER_PID=$!

    for _ in $(seq 1 60); do
        if curl -sf "http://127.0.0.1:${ZLAYER_E2E_MANAGER_PORT}/" >/dev/null 2>&1; then
            break
        fi
        sleep 1
    done
}

stop_stack() {
    if [[ -n "$MANAGER_PID" ]]; then
        kill "$MANAGER_PID" 2>/dev/null || true
        wait "$MANAGER_PID" 2>/dev/null || true
        MANAGER_PID=""
    fi
    if [[ -n "$DAEMON_PID" ]]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
        DAEMON_PID=""
    fi
}

echo "==> Fresh data dir: $ZLAYER_E2E_DATA_DIR"

if [[ -z "$NO_BUILD" ]]; then
    echo "==> Building zlayer + zlayer-manager (release)"
    ( cd "$REPO_ROOT" && cargo build --release -p zlayer )
    ( cd "$REPO_ROOT/crates/zlayer-manager" && cargo leptos build --release )
else
    echo "==> Skipping build (--no-build)"
fi

start_stack

cd "$E2E_DIR"

if [[ -n "$ONLY" ]]; then
    echo "==> Running single test: $ONLY"
    pnpm dlx --silent "$INTELLITESTER_PIN" run \
        "$E2E_DIR/$ONLY"
    echo "==> Single test passed: $ONLY"
else
    echo "==> Running auth workflow (bootstrap -> logout -> login)"
    pnpm dlx --silent "$INTELLITESTER_PIN" run \
        "$E2E_DIR/auth.workflow.yaml"

    echo "==> Running nav-smoke test"
    pnpm dlx --silent "$INTELLITESTER_PIN" run \
        "$E2E_DIR/nav-smoke.test.yaml"

    # --- Stale-session regression (Part A) ---
    #
    # The auth workflow left an admin in users.db. For this scenario we
    # need a fresh DB so the bootstrap step runs cleanly, then we kill the
    # stack, wipe the data dir, restart, and walk the bug-triggering
    # sequence: bootstrap + login (saves cookies) -> sqlite3 wipe the row
    # -> verify the next request clears cookies.

    echo "==> Resetting data dir for stale-session scenario"
    stop_stack
    "$E2E_DIR/scripts/reset-data-dir.sh"
    start_stack

    rm -f "$E2E_DIR/.stale-session.state.json"

    echo "==> Stale-session: bootstrap + login + saveStorageState"
    pnpm dlx --silent "$INTELLITESTER_PIN" run \
        "$E2E_DIR/stale-session-setup.test.yaml"

    echo "==> Stale-session: wiping admin@e2e.local from users.db"
    sqlite3 "$ZLAYER_E2E_DATA_DIR/users.db" \
        "DELETE FROM users WHERE email='admin@e2e.local';"

    echo "==> Stale-session: verify cookies cleared on next request"
    pnpm dlx --silent "$INTELLITESTER_PIN" run \
        --storage-state "$E2E_DIR/.stale-session.state.json" \
        "$E2E_DIR/stale-session-verify.test.yaml"

    echo "==> All e2e checks passed."
fi
