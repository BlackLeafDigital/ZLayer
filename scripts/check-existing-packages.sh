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
)

IMAGES=(
  "zlayer-node-${VERSION}-oci.tar"
  "zlayer-web-${VERSION}-oci.tar"
  "zlayer-manager-${VERSION}-oci.tar"
)

missing=0
found=0

for artifact in "${BINARIES[@]}" "${IMAGES[@]}"; do
  http_code=$(curl -sI -o /dev/null -w "%{http_code}" --user "$AUTH" "${PKG_URL}/${artifact}" 2>/dev/null) || http_code="000"
  if [ "$http_code" = "200" ]; then
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
