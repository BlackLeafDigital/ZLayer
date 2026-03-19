base: zlayer/base:latest

steps:
  - run: |
      ARCH="$(uname -m)"
      [ "$ARCH" = "arm64" ] && ZIG_ARCH="aarch64" || ZIG_ARCH="x86_64"
      # Check if zig is installed locally first
      ZIG_PATH="$(which zig 2>/dev/null || echo '')"
      if [ -n "$ZIG_PATH" ] && [ -f "$ZIG_PATH" ]; then
        ZIG_DIR="$(dirname "$ZIG_PATH")"
        mkdir -p usr/local/zig
        cp -R "$ZIG_DIR"/../lib/zig usr/local/zig/lib 2>/dev/null || true
        cp "$ZIG_PATH" usr/local/zig/zig
      else
        # Download latest stable from ziglang.org
        ZIG_INDEX=$(curl -fsSL 'https://ziglang.org/download/index.json')
        ZIG_URL=$(echo "$ZIG_INDEX" | grep -o "https://[^\"]*${ZIG_ARCH}-macos[^\"]*\.tar\.xz" | head -1)
        if [ -z "$ZIG_URL" ]; then
          echo "ERROR: Could not find Zig download for ${ZIG_ARCH}-macos" >&2
          exit 1
        fi
        mkdir -p usr/local/zig
        curl -fsSL "$ZIG_URL" | tar -xJ -C usr/local/zig --strip-components=1
      fi
      mkdir -p usr/local/bin
      ln -sf ../zig/zig usr/local/bin/zig

labels:
  org.opencontainers.image.title: "ZLayer macOS Zig"
  org.opencontainers.image.vendor: "Black Leaf Digital"
