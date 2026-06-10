#!/bin/bash
set -euo pipefail

# Check if release artifacts already exist for a given version
# Usage: ./scripts/check-existing-packages.sh <version> <auth>
# Exit 0 and prints "false" (need_build=false) if ALL exist
# Exit 0 and prints "true" (need_build=true) if ANY are missing

VERSION="$1"
AUTH="$2"
PKG_URL="https://forge.blackleafdigital.com/api/packages/BlackLeafDigital/generic/zlayer/${VERSION}"

BINARIES=(
  zlayer-linux-amd64.tar.gz
  zlayer-linux-arm64.tar.gz
  zlayer-darwin-amd64.tar.gz
  zlayer-darwin-arm64.tar.gz
  zlayer-windows-amd64.zip
  # Compat alias for one release post-F-7a consolidation; drop after next release.
  zlayer-windows-amd64-hcs.exe
)

IMAGES=(
  "zlayer-node-${VERSION}-oci.tar"
  "zlayer-web-${VERSION}-oci.tar"
  "zlayer-manager-${VERSION}-oci.tar"
)

missing=0
found=0

for artifact in "${BINARIES[@]}" "${IMAGES[@]}"; do
  # 1-byte ranged GET, NOT HEAD. Forgejo's generic-packages router does
  # not register a HEAD handler — chi returns 405 (Method Not Allowed)
  # for every HEAD probe, so the old `curl -sI` flow counted every
  # artifact as "MISSING" regardless of whether it was published. A
  # ranged GET returns 206 for present, 404 for missing, 405-free.
  # Same pattern the verify-artifact action uses.
  http_code=$(curl -s --range 0-0 -o /dev/null -w "%{http_code}" --user "$AUTH" "${PKG_URL}/${artifact}" 2>/dev/null) || http_code="000"
  if [ "$http_code" = "200" ] || [ "$http_code" = "206" ]; then
    echo "  EXISTS ${artifact}"
    found=$((found + 1))
  else
    echo "  MISSING ${artifact} (HTTP $http_code)"
    missing=$((missing + 1))
  fi
done

total=$((found + missing))
echo ""
echo "Found ${found}/${total} artifacts for version ${VERSION}"

if [ $missing -eq 0 ]; then
  echo "All artifacts exist -- no build needed"
  echo "false"
else
  echo "${missing} artifacts missing -- build required"
  echo "true"
fi
