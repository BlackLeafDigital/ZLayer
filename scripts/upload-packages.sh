#!/bin/bash
set -euo pipefail

# Upload build artifacts to Forgejo generic packages (2 at a time)
# Usage: ./scripts/upload-packages.sh <base_url> <auth> <artifacts_dir>

BASE_URL="$1"
AUTH="$2"
ARTIFACTS_DIR="$3"
MAX_PARALLEL=2

upload_file() {
  local file="$1"
  local url="$2"
  local filename
  filename=$(basename "$file")
  local local_size
  local_size=$(stat -c%s "$file")
  local max_retries=5
  local retry=0
  local backoff=5

  # Skip if already uploaded with matching size
  local remote_size
  remote_size=$(curl -sI --user "$AUTH" "$url" 2>/dev/null \
    | grep -i content-length | tail -1 | tr -dc '0-9') || true
  if [ -n "$remote_size" ] && [ "$remote_size" = "$local_size" ]; then
    echo "= Skipping $filename (already uploaded, ${local_size} bytes)"
    return 0
  elif [ -n "$remote_size" ]; then
    echo "~ Size mismatch for $filename (local=${local_size}, remote=${remote_size}), re-uploading..."
    curl -s -X DELETE --user "$AUTH" "$url" || true
    sleep 1
  fi

  while [ $retry -lt $max_retries ]; do
    echo "Uploading $filename (attempt $((retry + 1))/$max_retries, ${local_size} bytes)..."
    local response
    response=$(mktemp)
    local http_code
    http_code=$(curl -s -w "%{http_code}" \
      --max-time 600 \
      --connect-timeout 30 \
      --retry 3 \
      --retry-delay 5 \
      --user "$AUTH" \
      --upload-file "$file" \
      -o "$response" \
      "$url" 2>&1) || http_code="000"

    case "$http_code" in
      200|201)
        echo "OK $filename uploaded"
        rm -f "$response"
        return 0
        ;;
      409)
        echo "409 conflict for $filename, deleting and retrying..."
        curl -s -X DELETE --user "$AUTH" "$url" || true
        sleep 2
        ;;
      *)
        echo "Upload failed for $filename with HTTP $http_code"
        echo "Response: $(cat "$response" 2>/dev/null || echo 'empty')"
        ;;
    esac
    rm -f "$response"

    retry=$((retry + 1))
    if [ $retry -lt $max_retries ]; then
      echo "Retrying $filename in ${backoff}s..."
      sleep $backoff
      backoff=$((backoff * 2))
    fi
  done

  echo "FAIL: $filename after $max_retries attempts"
  return 1
}

# Collect all files (supports nested dirs from download-artifact or flat from curl)
files=()
# Nested: artifacts/zlayer-runtime-linux-amd64/zlayer-runtime-linux-amd64.tar.gz
for artifact in "${ARTIFACTS_DIR}"/zlayer-*/*.tar.gz; do
  [ -f "$artifact" ] && files+=("$artifact")
done
# Flat: artifacts/binaries/zlayer-runtime-linux-amd64.tar.gz or artifacts/*.tar.gz
for artifact in "${ARTIFACTS_DIR}"/*.tar.gz "${ARTIFACTS_DIR}"/*/*.tar.gz; do
  [ -f "$artifact" ] && files+=("$artifact")
done
# Container images
for archive in "${ARTIFACTS_DIR}"/container-images/*.tar; do
  [ -f "$archive" ] && files+=("$archive")
done
# Deduplicate
declare -A seen
unique_files=()
for f in "${files[@]}"; do
  fname=$(basename "$f")
  if [ -z "${seen[$fname]+x}" ]; then
    seen[$fname]=1
    unique_files+=("$f")
  fi
done
files=("${unique_files[@]}")

echo "Found ${#files[@]} files to upload"

# Upload in batches of $MAX_PARALLEL
results_dir=$(mktemp -d)
trap 'rm -rf "$results_dir"' EXIT

for ((i = 0; i < ${#files[@]}; i += MAX_PARALLEL)); do
  pids=()
  batch=("${files[@]:i:MAX_PARALLEL}")

  for file in "${batch[@]}"; do
    filename=$(basename "$file")
    (
      if upload_file "$file" "${BASE_URL}/${filename}"; then
        touch "${results_dir}/${filename}.ok"
      else
        touch "${results_dir}/${filename}.fail"
      fi
    ) &
    pids+=($!)
  done

  for pid in "${pids[@]}"; do
    wait "$pid" || true
  done
done

# Check results
failed=0
for file in "${files[@]}"; do
  filename=$(basename "$file")
  if [ -f "${results_dir}/${filename}.fail" ]; then
    echo "::error::Failed to upload ${filename}"
    failed=1
  fi
done

if [ $failed -ne 0 ]; then
  echo "::error::Some uploads failed"
  exit 1
fi

echo "All uploads completed successfully"
