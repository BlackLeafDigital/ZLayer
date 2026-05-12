#!/usr/bin/env bash
# Local hermetic Docker Engine API compatibility test for the zlayer compat shim.
#
# Spawns an isolated zlayer daemon with --docker-socket on its own data-dir,
# ports, and socket path, then runs a battery of real `docker` CLI commands
# against it.
#
# Process tracking:
#   - State dir at /tmp/zlayer-docker-compat-state holds pidfile + last-test-dir.
#   - Daemon is spawned in its own process group via `setsid`; cleanup kills the
#     whole pgid so child processes (e.g. spawned containers) die with it.
#   - On startup, any leftover daemon listed in the pidfile is reaped.
#   - Test dirs are timestamped under /tmp/zlayer-docker-compat-<ts>-<rand>/ and
#     pruned to the most-recent 5 unless KEEP_TEST_DIR=1.
#
# Modes:
#   bash docker-compat-local.sh             # run the suite
#   bash docker-compat-local.sh --cleanup   # kill any tracked daemon and prune
#                                           # all test dirs, no test run
#   bash docker-compat-local.sh --verbose   # RUST_LOG=info,zlayer=debug
#   KEEP_TEST_DIR=1 bash docker-compat-local.sh  # don't prune the test dir
#
# Env overrides:
#   ZLAYER_BIN  - path to zlayer binary (default: ./target/release/zlayer)

set -uo pipefail

STATE_DIR="/tmp/zlayer-docker-compat-state"
PIDFILE="$STATE_DIR/daemon.pid"
PGIDFILE="$STATE_DIR/daemon.pgid"
LASTDIR_FILE="$STATE_DIR/last-test-dir"
DIR_PREFIX="/tmp/zlayer-docker-compat-"
mkdir -p "$STATE_DIR"

# --- Reap any tracked leftover daemon ---
reap_tracked() {
    if [ -f "$PGIDFILE" ]; then
        local old_pgid
        old_pgid="$(cat "$PGIDFILE" 2>/dev/null || true)"
        if [ -n "$old_pgid" ] && kill -0 "-$old_pgid" 2>/dev/null; then
            echo "Reaping leftover daemon process group $old_pgid"
            kill -TERM "-$old_pgid" 2>/dev/null || true
            for _ in 1 2 3 4 5; do
                kill -0 "-$old_pgid" 2>/dev/null || break
                sleep 1
            done
            kill -KILL "-$old_pgid" 2>/dev/null || true
        fi
        rm -f "$PGIDFILE" "$PIDFILE"
    elif [ -f "$PIDFILE" ]; then
        local old_pid
        old_pid="$(cat "$PIDFILE" 2>/dev/null || true)"
        if [ -n "$old_pid" ] && kill -0 "$old_pid" 2>/dev/null; then
            echo "Reaping leftover daemon PID $old_pid"
            kill -TERM "$old_pid" 2>/dev/null || true
            for _ in 1 2 3 4 5; do
                kill -0 "$old_pid" 2>/dev/null || break
                sleep 1
            done
            kill -KILL "$old_pid" 2>/dev/null || true
        fi
        rm -f "$PIDFILE"
    fi
}

# --- Prune test dirs (keep most-recent N unless N=0 to wipe all) ---
prune_test_dirs() {
    local keep="${1:-5}"
    # shellcheck disable=SC2012
    ls -1dt "$DIR_PREFIX"* 2>/dev/null | tail -n "+$((keep + 1))" | while read -r d; do
        [ -d "$d" ] && rm -rf "$d"
    done
}

# --- --cleanup mode ---
if [ "${1:-}" = "--cleanup" ]; then
    reap_tracked
    echo "Pruning all test dirs under $DIR_PREFIX*"
    rm -rf "$DIR_PREFIX"*
    rm -f "$LASTDIR_FILE"
    echo "Done."
    exit 0
fi

# --- --verbose mode ---
VERBOSE=0
if [ "${1:-}" = "--verbose" ]; then
    VERBOSE=1
    shift
fi

# --- Reap any leftover from a previous failed run ---
reap_tracked
prune_test_dirs 5

# --- Hermetic config ---
TS="$(date -u +%Y%m%dT%H%M%SZ)"
TEST_DIR="$(mktemp -d "${DIR_PREFIX}${TS}-XXXXXX")"
echo "$TEST_DIR" > "$LASTDIR_FILE"
API_PORT="${ZLAYER_DOCKER_COMPAT_API_PORT:-23669}"
WG_PORT="${ZLAYER_DOCKER_COMPAT_WG_PORT:-51422}"
DNS_PORT="${ZLAYER_DOCKER_COMPAT_DNS_PORT:-15355}"
SOCK="$TEST_DIR/docker.sock"
DATA_DIR="$TEST_DIR/data"
LOG="$TEST_DIR/zlayer.log"
mkdir -p "$DATA_DIR"

# --- Resolve zlayer binary ---
ZLAYER_BIN="${ZLAYER_BIN:-./target/release/zlayer}"
if [ ! -x "$ZLAYER_BIN" ]; then
    if command -v zlayer >/dev/null 2>&1; then
        ZLAYER_BIN="$(command -v zlayer)"
    else
        echo "Error: zlayer binary not found. Run: cargo build --release -p zlayer" >&2
        exit 2
    fi
fi
echo "Binary:    $ZLAYER_BIN"
echo "Test dir:  $TEST_DIR"
echo "API port:  $API_PORT"
echo "Sock:      $SOCK"
echo "Log:       $LOG"
echo

DAEMON_PID=""
DAEMON_PGID=""

cleanup() {
    local rc=$?
    if [ -n "$DAEMON_PGID" ] && kill -0 "-$DAEMON_PGID" 2>/dev/null; then
        kill -TERM "-$DAEMON_PGID" 2>/dev/null || true
        for _ in 1 2 3 4 5; do
            kill -0 "-$DAEMON_PGID" 2>/dev/null || break
            sleep 1
        done
        kill -KILL "-$DAEMON_PGID" 2>/dev/null || true
    elif [ -n "$DAEMON_PID" ] && kill -0 "$DAEMON_PID" 2>/dev/null; then
        kill -TERM "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    rm -f "$PIDFILE" "$PGIDFILE"
    if [ "${KEEP_TEST_DIR:-0}" = "1" ]; then
        echo "Kept test dir: $TEST_DIR"
    else
        rm -rf "$TEST_DIR"
    fi
    exit "$rc"
}
trap cleanup EXIT INT TERM HUP

# --- Spawn daemon in its own session/pgid ---
echo "Starting hermetic zlayer daemon (own pgid)..."
RUST_LOG_OVERRIDE=""
if [ "$VERBOSE" = "1" ]; then
    RUST_LOG_OVERRIDE="info,zlayer=debug,zlayer_docker=debug,openraft=warn"
fi
env ${RUST_LOG_OVERRIDE:+RUST_LOG="$RUST_LOG_OVERRIDE"} \
    setsid "$ZLAYER_BIN" --data-dir "$DATA_DIR" \
    serve --bind "127.0.0.1:${API_PORT}" \
          --deployment-name zlayer-docker-compat \
          --wg-port "${WG_PORT}" \
          --dns-port "${DNS_PORT}" \
          --docker-socket \
          --docker-socket-path "$SOCK" \
    > "$LOG" 2>&1 &
DAEMON_PID=$!
# Capture pgid (== PID for setsid leader)
DAEMON_PGID="$DAEMON_PID"
echo "$DAEMON_PID" > "$PIDFILE"
echo "$DAEMON_PGID" > "$PGIDFILE"
echo "Daemon PID/PGID: $DAEMON_PID"

# --- Wait for both /healthz AND the unix socket ---
echo -n "Waiting for ready"
ready=0
for i in $(seq 1 120); do
    if curl -sf "http://127.0.0.1:${API_PORT}/health/live" >/dev/null 2>&1 && [ -S "$SOCK" ]; then
        ready=1
        break
    fi
    if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
        echo
        echo "Daemon exited during startup. Last log lines:"
        tail -60 "$LOG" | sed 's/^/  /'
        exit 3
    fi
    if [ $((i % 10)) -eq 0 ]; then echo -n " ${i}s"; else echo -n .; fi
    sleep 1
done
echo
if [ "$ready" != "1" ]; then
    echo "Daemon did not become ready in 120s."
    echo "  - API port bound? $(ss -ltn 2>/dev/null | awk '{print $4}' | grep -q ":${API_PORT}\$" && echo yes || echo no)"
    echo "  - Docker sock exists? $([ -S "$SOCK" ] && echo yes || echo no)"
    echo "  - /health/live response: $(curl -sS -m 2 "http://127.0.0.1:${API_PORT}/health/live" 2>&1 | head -c 200)"
    echo
    echo "Daemon log tail (60 lines):"
    tail -60 "$LOG" | sed 's/^/  /'
    exit 3
fi
echo "Daemon ready."
echo

# --- Point real docker CLI at the hermetic socket ---
export DOCKER_HOST="unix://$SOCK"
export DOCKER_BUILDKIT=0
unset DOCKER_CONFIG

# --- Test runner ---
PASS=0
FAIL=0
FAILED=()
OUT="$TEST_DIR/cmd-out"

run() {
    local name="$1"
    shift
    if "$@" >"$OUT" 2>&1; then
        echo "  ok    $name"
        PASS=$((PASS + 1))
    else
        local rc=$?
        echo "  FAIL  $name  (exit $rc)"
        sed 's/^/        /' "$OUT"
        FAIL=$((FAIL + 1))
        FAILED+=("$name")
    fi
}

echo "=== Tier 1: read-only Engine API surface ==="
run "docker version"        docker version
run "docker info"           docker info
run "docker system df"      docker system df
run "docker ps -a"          docker ps -a
run "docker images"         docker images
run "docker network ls"     docker network ls
run "docker volume ls"      docker volume ls

echo
echo "=== Tier 2: mutating ops (no container launch) ==="
run "docker network create" docker network create zc-test-net
run "docker network rm"     docker network rm     zc-test-net
run "docker volume create"  docker volume create  zc-test-vol
run "docker volume rm"      docker volume rm      zc-test-vol

echo
echo "=== Tier 3: image + container lifecycle ==="
run "docker pull alpine"            docker pull alpine:3.19
run "docker run --rm alpine echo"   docker run --rm alpine:3.19 echo zlayer-compat-ok
run "docker rmi alpine"             docker rmi alpine:3.19

echo
echo "=== Tier 4: compose ==="
COMPOSE_FIXTURE="$TEST_DIR/compose.yml"
cat > "$COMPOSE_FIXTURE" <<'YML'
services:
  hello:
    image: alpine:3.19
    command: ["echo", "hello from zlayer compose compat"]
YML
run "docker compose config" docker compose -f "$COMPOSE_FIXTURE" config
run "docker compose up"     docker compose -f "$COMPOSE_FIXTURE" up --abort-on-container-exit
run "docker compose down"   docker compose -f "$COMPOSE_FIXTURE" down

# --- Report ---
echo
echo "============================================================"
echo "  Passed: $PASS   Failed: $FAIL"
echo "============================================================"
if [ "$FAIL" -gt 0 ]; then
    echo "Failed commands:"
    printf '  - %s\n' "${FAILED[@]}"
    echo
    echo "Daemon log: $LOG"
    echo "Re-run with --verbose for info+debug logs, or KEEP_TEST_DIR=1 to keep \$TEST_DIR."
    exit 1
fi
echo "All compatibility checks passed."
