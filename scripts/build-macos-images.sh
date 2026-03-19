#!/usr/bin/env bash
#
# build-macos-images.sh -- Build macOS-native base images for the sandbox builder
#
# Usage:
#   ./scripts/build-macos-images.sh <subcommand>
#
# Subcommands:
#   base     Build the base rootfs with macOS system binaries
#   golang   Build Go toolchain image (requires base)
#   rust     Build Rust toolchain image (requires base)
#   node     Build Node.js toolchain image (requires base)
#   python   Build Python + uv toolchain image (requires base)
#   deno     Build Deno toolchain image (requires base)
#   bun      Build Bun toolchain image (requires base)
#   all      Build base, then all toolchain images in parallel
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="${TMPDIR:-/tmp}/zlayer-macos-images"
REGISTRY="ghcr.io/blackleafdigital/zlayer"

# ---------------------------------------------------------------------------
# Architecture detection
# ---------------------------------------------------------------------------

ARCH="$(uname -m)"
if [[ "$ARCH" == "arm64" ]]; then
    OCI_ARCH="arm64"
elif [[ "$ARCH" == "x86_64" ]]; then
    OCI_ARCH="amd64"
else
    echo "Unsupported architecture: $ARCH" >&2
    exit 1
fi

echo "==> Detected architecture: $ARCH (OCI: $OCI_ARCH)"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# Copy a binary and print what we did
copy_bin() {
    local src="$1"
    local dst_dir="$2"
    if [[ -f "$src" ]]; then
        mkdir -p "$dst_dir"
        cp "$src" "$dst_dir/"
    else
        echo "    SKIP (not found): $src"
    fi
}

# Ensure the base image exists; build it if it does not.
ensure_base() {
    local base_rootfs="$BUILD_DIR/base/rootfs"
    if [[ ! -d "$base_rootfs/bin" ]]; then
        echo "==> Base image not found, building it first..."
        build_base
    fi
}

# Clone the base rootfs into a toolchain rootfs via cp -a (APFS clone where
# supported). Returns the new rootfs path on stdout.
clone_base() {
    local name="$1"
    local dst="$BUILD_DIR/$name/rootfs"
    rm -rf "$dst"
    mkdir -p "$dst"
    cp -a "$BUILD_DIR/base/rootfs/." "$dst/"
    echo "$dst"
}

# Write an OCI-like config.json next to a rootfs directory.
write_oci_config() {
    local image_dir="$1"  # parent of rootfs/
    local image_name="$2"
    local extra_env="$3"  # JSON array string, e.g. '["GOROOT=/usr/local/go"]'

    cat > "$image_dir/config.json" <<JSONEOF
{
  "os": "darwin",
  "architecture": "$OCI_ARCH",
  "config": {
    "Env": $extra_env,
    "WorkingDir": "/",
    "Entrypoint": ["/bin/sh"],
    "Cmd": null,
    "Labels": {
      "io.zlayer.image": "$image_name",
      "io.zlayer.arch": "$OCI_ARCH"
    }
  }
}
JSONEOF
}

# Tar the rootfs into an OCI layer blob and print the path.
create_layer_tar() {
    local image_dir="$1"
    local name="$2"
    local tar_path="$image_dir/${name}.layer.tar.gz"
    echo "    Creating layer tarball..."
    tar -czf "$tar_path" -C "$image_dir/rootfs" .
    echo "    Layer: $tar_path ($(du -h "$tar_path" | cut -f1))"
}

# ---------------------------------------------------------------------------
# base
# ---------------------------------------------------------------------------

build_base() {
    echo "==> Building base image..."
    local rootfs="$BUILD_DIR/base/rootfs"
    rm -rf "$rootfs"
    mkdir -p "$rootfs"/{bin,usr/bin,usr/local/bin,etc,etc/ssl,opt,tmp,var/tmp,var/log}

    # -- /bin binaries --
    echo "    Copying /bin binaries..."
    local bin_list=(
        /bin/sh /bin/bash /bin/cat /bin/cp /bin/echo /bin/ls /bin/mkdir
        /bin/mv /bin/rm /bin/sleep /bin/chmod /bin/ln /bin/test /bin/expr
        /bin/date /bin/dd /bin/ps /bin/kill /bin/hostname
    )
    for b in "${bin_list[@]}"; do
        copy_bin "$b" "$rootfs/bin"
    done

    # -- /usr/bin binaries --
    echo "    Copying /usr/bin binaries..."
    local usrbin_list=(
        /usr/bin/env /usr/bin/which /usr/bin/xargs /usr/bin/tar /usr/bin/gzip
        /usr/bin/curl /usr/bin/git /usr/bin/make /usr/bin/true /usr/bin/false
        /usr/bin/head /usr/bin/tail /usr/bin/grep /usr/bin/sed /usr/bin/awk
        /usr/bin/sort /usr/bin/uniq /usr/bin/wc /usr/bin/find /usr/bin/tee
        /usr/bin/touch /usr/bin/cut /usr/bin/tr /usr/bin/dirname
        /usr/bin/basename /usr/bin/realpath /usr/bin/install /usr/bin/id
        /usr/bin/whoami /usr/bin/zip /usr/bin/unzip /usr/bin/openssl
    )
    for b in "${usrbin_list[@]}"; do
        copy_bin "$b" "$rootfs/usr/bin"
    done

    # -- CA certificates --
    echo "    Copying CA certificates..."
    if [[ -d /etc/ssl ]]; then
        cp -a /etc/ssl/. "$rootfs/etc/ssl/"
    fi

    # -- Shell configs --
    if [[ -f /etc/zshrc ]]; then
        cp /etc/zshrc "$rootfs/etc/zshrc"
    fi
    if [[ -f /etc/bashrc ]]; then
        cp /etc/bashrc "$rootfs/etc/bashrc"
    fi

    # -- Ensure tmp dirs have correct permissions --
    chmod 1777 "$rootfs/tmp" "$rootfs/var/tmp"

    # -- OCI config --
    write_oci_config "$BUILD_DIR/base" "zlayer-base" \
        '["PATH=/usr/local/bin:/usr/bin:/bin"]'

    # -- Layer tarball --
    create_layer_tar "$BUILD_DIR/base" "base"

    echo "==> Base image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# golang
# ---------------------------------------------------------------------------

build_golang() {
    ensure_base
    echo "==> Building Go toolchain image..."
    local rootfs
    rootfs="$(clone_base golang)"
    local image_dir="$BUILD_DIR/golang"

    local go_version="1.23.6"

    if command -v go &>/dev/null; then
        echo "    Found local Go installation, copying..."
        local goroot
        goroot="$(go env GOROOT)"
        mkdir -p "$rootfs/usr/local/go"
        cp -a "$goroot/." "$rootfs/usr/local/go/"
    else
        echo "    Go not found locally, downloading go${go_version}..."
        local tarball="go${go_version}.darwin-${ARCH}.tar.gz"
        local url="https://go.dev/dl/${tarball}"
        local dl_path="$BUILD_DIR/.downloads/${tarball}"
        mkdir -p "$BUILD_DIR/.downloads"
        if [[ ! -f "$dl_path" ]]; then
            curl -fSL -o "$dl_path" "$url"
        fi
        mkdir -p "$rootfs/usr/local"
        tar -xzf "$dl_path" -C "$rootfs/usr/local/"
    fi

    # Symlinks
    ln -sf ../go/bin/go    "$rootfs/usr/local/bin/go"
    ln -sf ../go/bin/gofmt "$rootfs/usr/local/bin/gofmt"

    write_oci_config "$image_dir" "zlayer-golang" \
        '["PATH=/usr/local/go/bin:/usr/local/bin:/usr/bin:/bin","GOROOT=/usr/local/go","GOPATH=/go"]'

    create_layer_tar "$image_dir" "golang"
    echo "==> Go image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# rust
# ---------------------------------------------------------------------------

build_rust() {
    ensure_base
    echo "==> Building Rust toolchain image..."
    local rootfs
    rootfs="$(clone_base rust)"
    local image_dir="$BUILD_DIR/rust"

    if command -v rustc &>/dev/null; then
        echo "    Found local Rust installation, copying..."
        local sysroot
        sysroot="$(rustc --print sysroot)"
        mkdir -p "$rootfs/usr/local/rust"
        cp -a "$sysroot/." "$rootfs/usr/local/rust/"
    else
        echo "    Rust not found locally, installing via rustup-init..."
        local rustup_init="$BUILD_DIR/.downloads/rustup-init"
        mkdir -p "$BUILD_DIR/.downloads"
        if [[ ! -f "$rustup_init" ]]; then
            curl -fSL -o "$rustup_init" "https://sh.rustup.rs" --proto '=https' --tlsv1.2
            # Actually download the binary installer
            local rustup_url="https://static.rust-lang.org/rustup/dist/${ARCH}-apple-darwin/rustup-init"
            curl -fSL -o "$rustup_init" "$rustup_url"
            chmod +x "$rustup_init"
        fi
        local tmp_rust="$BUILD_DIR/.rust-tmp"
        rm -rf "$tmp_rust"
        RUSTUP_HOME="$tmp_rust/rustup" CARGO_HOME="$tmp_rust/cargo" \
            "$rustup_init" -y --no-modify-path --default-toolchain stable 2>/dev/null
        local sysroot
        sysroot="$(RUSTUP_HOME="$tmp_rust/rustup" "$tmp_rust/cargo/bin/rustc" --print sysroot)"
        mkdir -p "$rootfs/usr/local/rust"
        cp -a "$sysroot/." "$rootfs/usr/local/rust/"
        # Also copy cargo and friends from cargo/bin
        cp "$tmp_rust/cargo/bin/cargo"   "$rootfs/usr/local/rust/bin/"
        cp "$tmp_rust/cargo/bin/rustfmt" "$rootfs/usr/local/rust/bin/" 2>/dev/null || true
        cp "$tmp_rust/cargo/bin/clippy-driver" "$rootfs/usr/local/rust/bin/" 2>/dev/null || true
        rm -rf "$tmp_rust"
    fi

    # Symlinks
    mkdir -p "$rootfs/usr/local/bin"
    for tool in rustc cargo rustfmt clippy-driver; do
        if [[ -f "$rootfs/usr/local/rust/bin/$tool" ]]; then
            ln -sf ../rust/bin/$tool "$rootfs/usr/local/bin/$tool"
        fi
    done

    write_oci_config "$image_dir" "zlayer-rust" \
        '["PATH=/usr/local/rust/bin:/usr/local/bin:/usr/bin:/bin","RUSTUP_HOME=/usr/local/rust","CARGO_HOME=/usr/local/rust"]'

    create_layer_tar "$image_dir" "rust"
    echo "==> Rust image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# node
# ---------------------------------------------------------------------------

build_node() {
    ensure_base
    echo "==> Building Node.js toolchain image..."
    local rootfs
    rootfs="$(clone_base node)"
    local image_dir="$BUILD_DIR/node"

    local node_version="22.14.0"

    if command -v node &>/dev/null; then
        echo "    Found local Node.js installation, copying..."
        local node_prefix
        node_prefix="$(dirname "$(dirname "$(command -v node)")")"
        mkdir -p "$rootfs/usr/local/node"
        # Copy bin, lib, include, share
        for d in bin lib include share; do
            if [[ -d "$node_prefix/$d" ]]; then
                cp -a "$node_prefix/$d" "$rootfs/usr/local/node/"
            fi
        done
    else
        echo "    Node.js not found locally, downloading v${node_version}..."
        local tarball="node-v${node_version}-darwin-${ARCH}.tar.gz"
        local url="https://nodejs.org/dist/v${node_version}/${tarball}"
        local dl_path="$BUILD_DIR/.downloads/${tarball}"
        mkdir -p "$BUILD_DIR/.downloads"
        if [[ ! -f "$dl_path" ]]; then
            curl -fSL -o "$dl_path" "$url"
        fi
        mkdir -p "$rootfs/usr/local/node"
        tar -xzf "$dl_path" --strip-components=1 -C "$rootfs/usr/local/node/"
    fi

    # Symlinks
    ln -sf ../node/bin/node "$rootfs/usr/local/bin/node"
    ln -sf ../node/bin/npm  "$rootfs/usr/local/bin/npm"
    ln -sf ../node/bin/npx  "$rootfs/usr/local/bin/npx"

    write_oci_config "$image_dir" "zlayer-node" \
        '["PATH=/usr/local/node/bin:/usr/local/bin:/usr/bin:/bin"]'

    create_layer_tar "$image_dir" "node"
    echo "==> Node.js image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# python
# ---------------------------------------------------------------------------

build_python() {
    ensure_base
    echo "==> Building Python + uv toolchain image..."
    local rootfs
    rootfs="$(clone_base python)"
    local image_dir="$BUILD_DIR/python"

    local uv_version="0.6.9"

    # -- uv --
    echo "    Installing uv ${uv_version}..."
    local uv_tarball="uv-${ARCH}-apple-darwin.tar.gz"
    local uv_url="https://github.com/astral-sh/uv/releases/download/${uv_version}/${uv_tarball}"
    local uv_dl="$BUILD_DIR/.downloads/${uv_tarball}"
    mkdir -p "$BUILD_DIR/.downloads"
    if [[ ! -f "$uv_dl" ]]; then
        curl -fSL -o "$uv_dl" "$uv_url"
    fi
    # uv tarballs extract to a directory named uv-$ARCH-apple-darwin/
    local uv_tmp="$BUILD_DIR/.uv-extract"
    rm -rf "$uv_tmp"
    mkdir -p "$uv_tmp"
    tar -xzf "$uv_dl" -C "$uv_tmp"
    # Find the uv binary inside the extracted directory
    local uv_bin
    uv_bin="$(find "$uv_tmp" -name uv -type f | head -1)"
    if [[ -z "$uv_bin" ]]; then
        echo "ERROR: Could not find uv binary in extracted archive" >&2
        exit 1
    fi
    cp "$uv_bin" "$rootfs/usr/local/bin/uv"
    chmod +x "$rootfs/usr/local/bin/uv"
    # uvx is typically a symlink to uv
    local uvx_bin
    uvx_bin="$(find "$uv_tmp" -name uvx -type f | head -1)"
    if [[ -n "$uvx_bin" ]]; then
        cp "$uvx_bin" "$rootfs/usr/local/bin/uvx"
        chmod +x "$rootfs/usr/local/bin/uvx"
    else
        ln -sf uv "$rootfs/usr/local/bin/uvx"
    fi
    rm -rf "$uv_tmp"

    # -- python3 --
    if command -v python3 &>/dev/null; then
        echo "    Found local Python3, copying..."
        local py_real
        py_real="$(python3 -c "import sys; print(sys.prefix)")"
        # Copy the framework/prefix into the rootfs
        mkdir -p "$rootfs/usr/local/python"
        # Copy bin, lib, include
        for d in bin lib include; do
            if [[ -d "$py_real/$d" ]]; then
                cp -a "$py_real/$d" "$rootfs/usr/local/python/"
            fi
        done
        # Symlinks
        if [[ -f "$rootfs/usr/local/python/bin/python3" ]]; then
            ln -sf ../python/bin/python3 "$rootfs/usr/local/bin/python3"
            ln -sf ../python/bin/python3 "$rootfs/usr/local/bin/python"
        fi
    else
        echo "    Python3 not found locally. uv can install Python on first use."
        # Create a shim that uses uv to run python
        cat > "$rootfs/usr/local/bin/python3" <<'PYSHIM'
#!/bin/sh
exec /usr/local/bin/uv run python3 "$@"
PYSHIM
        chmod +x "$rootfs/usr/local/bin/python3"
        ln -sf python3 "$rootfs/usr/local/bin/python"
    fi

    write_oci_config "$image_dir" "zlayer-python" \
        '["PATH=/usr/local/python/bin:/usr/local/bin:/usr/bin:/bin","UV_SYSTEM_PYTHON=1"]'

    create_layer_tar "$image_dir" "python"
    echo "==> Python image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# deno
# ---------------------------------------------------------------------------

build_deno() {
    ensure_base
    echo "==> Building Deno toolchain image..."
    local rootfs
    rootfs="$(clone_base deno)"
    local image_dir="$BUILD_DIR/deno"

    local deno_version="v2.2.4"

    echo "    Installing Deno ${deno_version}..."
    local deno_arch
    if [[ "$ARCH" == "arm64" ]]; then
        deno_arch="aarch64"
    else
        deno_arch="x86_64"
    fi
    local deno_zip="deno-${deno_arch}-apple-darwin.zip"
    local deno_url="https://github.com/denoland/deno/releases/download/${deno_version}/${deno_zip}"
    local deno_dl="$BUILD_DIR/.downloads/${deno_zip}"
    mkdir -p "$BUILD_DIR/.downloads"
    if [[ ! -f "$deno_dl" ]]; then
        curl -fSL -o "$deno_dl" "$deno_url"
    fi
    local deno_tmp="$BUILD_DIR/.deno-extract"
    rm -rf "$deno_tmp"
    mkdir -p "$deno_tmp"
    unzip -q -o "$deno_dl" -d "$deno_tmp"
    cp "$deno_tmp/deno" "$rootfs/usr/local/bin/deno"
    chmod +x "$rootfs/usr/local/bin/deno"
    rm -rf "$deno_tmp"

    write_oci_config "$image_dir" "zlayer-deno" \
        '["PATH=/usr/local/bin:/usr/bin:/bin","DENO_DIR=/var/tmp/deno"]'

    create_layer_tar "$image_dir" "deno"
    echo "==> Deno image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# bun
# ---------------------------------------------------------------------------

build_bun() {
    ensure_base
    echo "==> Building Bun toolchain image..."
    local rootfs
    rootfs="$(clone_base bun)"
    local image_dir="$BUILD_DIR/bun"

    local bun_version="v1.2.5"

    echo "    Installing Bun ${bun_version}..."
    local bun_arch
    if [[ "$ARCH" == "arm64" ]]; then
        bun_arch="aarch64"
    else
        bun_arch="x64"
    fi
    local bun_zip="bun-darwin-${bun_arch}.zip"
    local bun_url="https://github.com/oven-sh/bun/releases/download/bun-${bun_version}/${bun_zip}"
    local bun_dl="$BUILD_DIR/.downloads/${bun_zip}"
    mkdir -p "$BUILD_DIR/.downloads"
    if [[ ! -f "$bun_dl" ]]; then
        curl -fSL -o "$bun_dl" "$bun_url"
    fi
    local bun_tmp="$BUILD_DIR/.bun-extract"
    rm -rf "$bun_tmp"
    mkdir -p "$bun_tmp"
    unzip -q -o "$bun_dl" -d "$bun_tmp"
    # Bun zips contain a directory like bun-darwin-aarch64/bun
    local bun_bin
    bun_bin="$(find "$bun_tmp" -name bun -type f | head -1)"
    if [[ -z "$bun_bin" ]]; then
        echo "ERROR: Could not find bun binary in extracted archive" >&2
        exit 1
    fi
    cp "$bun_bin" "$rootfs/usr/local/bin/bun"
    chmod +x "$rootfs/usr/local/bin/bun"
    # bunx is typically included alongside bun
    local bunx_bin
    bunx_bin="$(find "$bun_tmp" -name bunx -type f | head -1)"
    if [[ -n "$bunx_bin" ]]; then
        cp "$bunx_bin" "$rootfs/usr/local/bin/bunx"
        chmod +x "$rootfs/usr/local/bin/bunx"
    else
        ln -sf bun "$rootfs/usr/local/bin/bunx"
    fi
    rm -rf "$bun_tmp"

    write_oci_config "$image_dir" "zlayer-bun" \
        '["PATH=/usr/local/bin:/usr/bin:/bin"]'

    create_layer_tar "$image_dir" "bun"
    echo "==> Bun image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# all
# ---------------------------------------------------------------------------

build_all() {
    build_base

    echo "==> Building all toolchain images in parallel..."

    local pids=()

    build_golang &
    pids+=($!)

    build_rust &
    pids+=($!)

    build_node &
    pids+=($!)

    build_python &
    pids+=($!)

    build_deno &
    pids+=($!)

    build_bun &
    pids+=($!)

    local failed=0
    for pid in "${pids[@]}"; do
        if ! wait "$pid"; then
            failed=1
        fi
    done

    if [[ "$failed" -ne 0 ]]; then
        echo "ERROR: One or more toolchain builds failed" >&2
        exit 1
    fi

    echo ""
    echo "==> All images built successfully!"
    echo "    Location: $BUILD_DIR/"
    echo ""
    ls -1d "$BUILD_DIR"/*/rootfs 2>/dev/null | while read -r d; do
        local name
        name="$(basename "$(dirname "$d")")"
        local size
        size="$(du -sh "$d" | cut -f1)"
        echo "    $name: $size"
    done
}

# ---------------------------------------------------------------------------
# Main dispatch
# ---------------------------------------------------------------------------

usage() {
    echo "Usage: $0 <subcommand>"
    echo ""
    echo "Subcommands:"
    echo "  base     Build the base rootfs with macOS system binaries"
    echo "  golang   Build Go toolchain image"
    echo "  rust     Build Rust toolchain image"
    echo "  node     Build Node.js toolchain image"
    echo "  python   Build Python + uv toolchain image"
    echo "  deno     Build Deno toolchain image"
    echo "  bun      Build Bun toolchain image"
    echo "  all      Build base + all toolchain images"
    echo ""
    echo "Output directory: $BUILD_DIR/"
}

if [[ $# -lt 1 ]]; then
    usage
    exit 1
fi

case "$1" in
    base)    build_base ;;
    golang)  build_golang ;;
    rust)    build_rust ;;
    node)    build_node ;;
    python)  build_python ;;
    deno)    build_deno ;;
    bun)     build_bun ;;
    all)     build_all ;;
    -h|--help|help)
        usage
        exit 0
        ;;
    *)
        echo "Unknown subcommand: $1" >&2
        usage
        exit 1
        ;;
esac
