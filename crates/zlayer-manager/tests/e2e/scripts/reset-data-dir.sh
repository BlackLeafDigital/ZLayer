#!/usr/bin/env bash
# Wipe the local daemon's data dir so the bootstrap test gets a clean
# users.db every run. Driven by ZLAYER_E2E_DATA_DIR; set by run-suite.sh.
set -euo pipefail

: "${ZLAYER_E2E_DATA_DIR:?ZLAYER_E2E_DATA_DIR must be set}"
rm -rf "$ZLAYER_E2E_DATA_DIR"
mkdir -p "$ZLAYER_E2E_DATA_DIR"
