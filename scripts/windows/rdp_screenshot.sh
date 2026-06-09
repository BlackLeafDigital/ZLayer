#!/usr/bin/env bash
# rdp_screenshot.sh — Open an xfreerdp session to MiniWindows, wait a configurable
# settle, screenshot the X window, and print the PNG path. Used by Claude to
# "look at" the desktop / Hyper-V Manager / vmconnect via the multimodal Read
# tool. Standalone usage: ./rdp_screenshot.sh [settle-seconds] [output.png]
#
# Credentials live in env vars so they don't end up in the repo:
#   RDP_HOST   default 192.168.68.92
#   RDP_USER   default MiniWindows
#   RDP_PASS   REQUIRED (set in your shell or .env)
#
# The session is terminated after the screenshot so we don't hold a sustained
# RDP connection that would block the user's Mac. Pass --hold to keep the
# session open (for interactive driving via the spawned xfreerdp window).

set -euo pipefail

SETTLE="${1:-25}"
OUT="${2:-/tmp/zlayer-debug/rdp-$(date -u +%Y%m%dT%H%M%SZ).png}"
HOLD=""
case "${3:-}" in --hold) HOLD=1 ;; esac

RDP_HOST="${RDP_HOST:-192.168.68.92}"
RDP_USER="${RDP_USER:-MiniWindows}"

if [[ -z "${RDP_PASS:-}" ]]; then
  echo "rdp_screenshot.sh: RDP_PASS not set in env" >&2
  exit 2
fi

mkdir -p "$(dirname "$OUT")"

# Start xfreerdp in the background. /size: gives a fixed window we can find by
# title with xdotool/import. /cert:ignore skips host-cert prompts. /log-level:
# OFF keeps stderr quiet.
xfreerdp3 \
  /v:"$RDP_HOST" \
  /u:"$RDP_USER" \
  /p:"$RDP_PASS" \
  /size:1600x1000 \
  /cert:ignore \
  /dynamic-resolution \
  /log-level:WARN \
  /t:"zlayer-rdp" \
  >/tmp/zlayer-debug/rdp-last.log 2>&1 &
RDP_PID=$!

# Wait for the window to appear and settle. xdotool would be ideal; falling
# back to a plain sleep.
sleep "$SETTLE"

# Screenshot the first xfreerdp top-level window via ImageMagick's `import`.
# `-window root` snapshots the entire X root; we crop later if needed. To
# screenshot the freerdp window specifically, use xdotool:
if command -v xdotool >/dev/null 2>&1; then
  WID="$(xdotool search --name 'zlayer-rdp' 2>/dev/null | head -n1 || true)"
  if [[ -n "$WID" ]]; then
    import -window "$WID" "$OUT"
  else
    import -window root "$OUT"
  fi
else
  import -window root "$OUT"
fi

echo "SCREENSHOT=$OUT"

if [[ -z "$HOLD" ]]; then
  kill "$RDP_PID" 2>/dev/null || true
  wait "$RDP_PID" 2>/dev/null || true
fi
