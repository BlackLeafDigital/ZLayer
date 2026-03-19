base: zlayer/base:latest

steps:
  - run: |
      XCPATH="$(xcode-select -p 2>/dev/null || echo '')"
      if [ -n "$XCPATH" ] && [ -d "$XCPATH" ]; then
        mkdir -p usr/local/swift/bin usr/local/bin
        # Copy Swift toolchain binaries
        for bin in swift swiftc swift-build swift-package swift-test swift-run sourcekit-lsp; do
          FOUND="$(xcrun --find "$bin" 2>/dev/null || echo '')"
          [ -n "$FOUND" ] && [ -f "$FOUND" ] && cp "$FOUND" usr/local/swift/bin/
        done
        # Copy essential build tools
        for bin in clang clang++ ld ar libtool dsymutil strip; do
          FOUND="$(xcrun --find "$bin" 2>/dev/null || echo '')"
          [ -n "$FOUND" ] && [ -f "$FOUND" ] && cp "$FOUND" usr/local/swift/bin/
        done
        # Symlink into PATH
        for bin in usr/local/swift/bin/*; do
          [ -f "$bin" ] && ln -sf "../swift/bin/$(basename "$bin")" "usr/local/bin/$(basename "$bin")"
        done
      else
        echo "ERROR: Xcode Command Line Tools not found. Install with: xcode-select --install" >&2
        exit 1
      fi

  - run: |
      # Copy Swift runtime libraries needed for linking
      XCPATH="$(xcode-select -p 2>/dev/null)"
      TOOLCHAIN="$XCPATH/Toolchains/XcodeDefault.xctoolchain"
      if [ -d "$TOOLCHAIN/usr/lib/swift/macosx" ]; then
        mkdir -p usr/local/swift/lib/swift
        cp -R "$TOOLCHAIN/usr/lib/swift/macosx" usr/local/swift/lib/swift/
      fi

labels:
  org.opencontainers.image.title: "ZLayer macOS Swift"
  org.opencontainers.image.vendor: "Black Leaf Digital"
