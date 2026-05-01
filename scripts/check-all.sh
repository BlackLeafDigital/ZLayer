#!/usr/bin/env bash
# Usage: scripts/check-all.sh [--with-tests]
#
# Workspace-wide check that exercises every compilation surface ZLayer
# actually ships, on the local machine using rustup cross-targets:
#
#   1. native workspace               (host triple, default features)
#   2. zlayer-manager  ssr feature    (server-side native bin)
#   3. zlayer-web      ssr feature    (server-side native bin)
#   4. zlayer-manager  hydrate lib    (wasm32-unknown-unknown)
#   5. zlayer-web      hydrate lib    (wasm32-unknown-unknown)
#   6. workspace check (msvc)         (x86_64-pc-windows-msvc, no UI crates)
#   7. workspace check (apple aarch64)(aarch64-apple-darwin, no UI crates)
#   8. clippy on native workspace     (host, -D warnings)
#   9. clippy (msvc)                  (x86_64-pc-windows-msvc, no UI crates)
#  10. clippy (apple aarch64)         (aarch64-apple-darwin, no UI crates)
#
# Notes:
#   - The Leptos UI crates (zlayer-manager, zlayer-web) have `default = []`
#     so a bare `cargo check -p ...` does almost nothing useful. We exercise
#     them under their real feature sets: `--features ssr` for the native
#     server bin, `--features hydrate --target wasm32-unknown-unknown` for
#     the browser lib. They're excluded from every workspace-level check
#     because their default-feature compile is a no-op.
#   - Cross-target check/clippy uses rustup targets only. NO zigbuild, NO
#     cross, NO apt :arm64 — repo policy. Pure-rust deps will type-check
#     fine across targets; crates that link C libs (libseccomp, openssl-sys
#     under non-vendored mode, etc.) may fail without the matching SDK.
#     Run with --with-tests to additionally run `cargo test` on the host.
#
# Optional surfaces:
#   --with-tests   also run `cargo test --workspace` (host only) minus
#                  the UI crates.

set -euo pipefail

WITH_TESTS=0

for arg in "$@"; do
  case "$arg" in
    --with-tests) WITH_TESTS=1 ;;
    -h|--help)
      sed -n '2,33p' "$0"
      exit 0
      ;;
    *)
      echo "ERROR: unknown flag: $arg" >&2
      echo "       valid flags: --with-tests" >&2
      exit 2
      ;;
  esac
done

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# UI crates: default = [] so workspace-level checks would compile them as
# near-empty. We exercise them under their real feature sets explicitly.
UI_EXCLUDES=(--exclude zlayer-manager --exclude zlayer-web)

# Cross-targets we want check/clippy coverage for.
WASM_TARGET="wasm32-unknown-unknown"
WIN_TARGET="x86_64-pc-windows-msvc"
MAC_TARGET="aarch64-apple-darwin"

ensure_target() {
  local triple="$1"
  if ! rustup target list --installed | grep -q "^${triple}$"; then
    echo "    installing rustup target: $triple"
    rustup target add "$triple"
  fi
}

section() {
  echo
  echo "### $1"
  echo "----------------------------------------------------------------------"
}

# Make sure every target we'll ask cargo to use is present before we start
# so a missing-target error doesn't show up halfway through.
ensure_target "$WASM_TARGET"
ensure_target "$WIN_TARGET"
ensure_target "$MAC_TARGET"

section "1/10 native workspace (host, excluding UI crates)"
cargo check --workspace --all-targets "${UI_EXCLUDES[@]}"

section "2/10 zlayer-manager (ssr feature, server bin)"
cargo check -p zlayer-manager --features ssr

section "3/10 zlayer-web (ssr feature, server bin)"
cargo check -p zlayer-web --features ssr

section "4/10 zlayer-manager (hydrate lib, $WASM_TARGET)"
cargo check -p zlayer-manager --lib --features hydrate --target "$WASM_TARGET"

section "5/10 zlayer-web (hydrate lib, $WASM_TARGET)"
cargo check -p zlayer-web --lib --features hydrate --target "$WASM_TARGET"

section "6/10 workspace check ($WIN_TARGET, excluding UI crates)"
cargo check --workspace --all-targets "${UI_EXCLUDES[@]}" --target "$WIN_TARGET"

section "7/10 workspace check ($MAC_TARGET, excluding UI crates)"
cargo check --workspace --all-targets "${UI_EXCLUDES[@]}" --target "$MAC_TARGET"

section "8/10 clippy on native workspace (host, -D warnings)"
cargo clippy --workspace --all-targets "${UI_EXCLUDES[@]}" -- -D warnings

section "9/10 clippy ($WIN_TARGET, excluding UI crates, -D warnings)"
cargo clippy --workspace --all-targets "${UI_EXCLUDES[@]}" --target "$WIN_TARGET" -- -D warnings

section "10/10 clippy ($MAC_TARGET, excluding UI crates, -D warnings)"
cargo clippy --workspace --all-targets "${UI_EXCLUDES[@]}" --target "$MAC_TARGET" -- -D warnings

if [[ "$WITH_TESTS" == "1" ]]; then
  section "optional: cargo test --workspace (host, excluding UI crates)"
  cargo test --workspace "${UI_EXCLUDES[@]}"
fi

echo
echo "======================================================================"
echo " ALL CHECKS PASSED"
echo "======================================================================"
