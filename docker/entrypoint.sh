#!/bin/bash
set -e

# ZLayer Node Entrypoint
# Starts containerd, waits for it to be ready, then runs zlayer

echo "ZLayer Node starting..."

# Start containerd in background
echo "Starting containerd..."
containerd &
CONTAINERD_PID=$!

# Wait for containerd socket with timeout
SOCKET="${CONTAINERD_SOCKET:-/run/containerd/containerd.sock}"
timeout=30
echo "Waiting for containerd socket at ${SOCKET}..."
while [ ! -S "$SOCKET" ] && [ $timeout -gt 0 ]; do
    sleep 1
    timeout=$((timeout - 1))
done

if [ ! -S "$SOCKET" ]; then
    echo "ERROR: containerd failed to start within timeout"
    exit 1
fi

echo "containerd is ready"

# Create default namespace
NAMESPACE="${ZLAYER_NAMESPACE:-zlayer}"
ctr namespace create "$NAMESPACE" 2>/dev/null || true
echo "Namespace '$NAMESPACE' ready"

# Trap signals for graceful shutdown
cleanup() {
    echo "Shutting down ZLayer..."

    # Kill zlayer if running
    if [ -n "$ZLAYER_PID" ]; then
        kill "$ZLAYER_PID" 2>/dev/null || true
        wait "$ZLAYER_PID" 2>/dev/null || true
    fi

    # Give containers time to stop gracefully
    sleep 2

    # Stop containerd
    echo "Stopping containerd..."
    kill "$CONTAINERD_PID" 2>/dev/null || true
    wait "$CONTAINERD_PID" 2>/dev/null || true

    echo "Shutdown complete"
}
trap cleanup EXIT TERM INT

# Run zlayer with all passed arguments
echo "Starting ZLayer..."
/usr/local/bin/zlayer "$@" &
ZLAYER_PID=$!

# Wait for zlayer to exit
wait "$ZLAYER_PID"
