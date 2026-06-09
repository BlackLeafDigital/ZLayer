#!/usr/bin/env bash
# Code-sign the `zlayer` binary with the virtualization entitlement so the macOS
# VZ runtime can create/boot guest VMs.
#
# Usage:
#   scripts/sign-vz.sh [path-to-zlayer-binary] [signing-identity]
#
#   path:     defaults to target/release/zlayer, then target/debug/zlayer.
#   identity: defaults to "-" (ad-hoc — sufficient for LOCAL use on this Mac).
#             Pass a Developer ID ("Developer ID Application: …") to produce a
#             build that boots VMs on OTHER machines.
#
# The entitlement (com.apple.security.virtualization) is NOT a restricted
# entitlement, so an ad-hoc signature carrying it grants virtualization locally
# without an Apple Developer account.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ent="${repo_root}/bin/zlayer/zlayer.entitlements"

bin="${1:-}"
if [ -z "${bin}" ]; then
    if [ -x "${repo_root}/target/release/zlayer" ]; then
        bin="${repo_root}/target/release/zlayer"
    else
        bin="${repo_root}/target/debug/zlayer"
    fi
fi
identity="${2:--}"

if [ ! -f "${bin}" ]; then
    echo "error: binary not found: ${bin} (build it first with cargo build)" >&2
    exit 1
fi

echo "Signing ${bin}"
echo "  identity:     ${identity}"
echo "  entitlements: ${ent}"
codesign --force --options runtime --sign "${identity}" --entitlements "${ent}" "${bin}"

echo "Verifying entitlement…"
if codesign -d --entitlements :- "${bin}" 2>/dev/null | grep -q "com.apple.security.virtualization"; then
    echo "OK: com.apple.security.virtualization present."
else
    echo "WARNING: virtualization entitlement not found after signing." >&2
    exit 1
fi
