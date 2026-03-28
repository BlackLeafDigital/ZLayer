#!/usr/bin/env bash
#
# build-macos-images.sh -- Build macOS-native base images for the sandbox builder
#
# Usage:
#   ./scripts/build-macos-images.sh <subcommand> [version ...]
#
# Subcommands:
#   base     Build the base rootfs with macOS system binaries
#   golang   Build Go toolchain image (requires base)
#   rust     Build Rust toolchain image (requires base)
#   node     Build Node.js toolchain image (requires base)
#   python   Build Python + uv toolchain image (requires base)
#   deno     Build Deno toolchain image (requires base)
#   bun      Build Bun toolchain image (requires base)
#   all      Build base, then all toolchain images (latest) in parallel
#
# Multi-version builds:
#   ./scripts/build-macos-images.sh golang 1.22 1.23
#   ./scripts/build-macos-images.sh node 18 20 22 24
#   ./scripts/build-macos-images.sh python 3.10 3.11 3.12 3.13 3.14
#
# Environment variable overrides (takes precedence over API resolution):
#   GO_VERSION=1.22.0 ./scripts/build-macos-images.sh golang
#   NODE_VERSION=22.14.0 ./scripts/build-macos-images.sh node
#   UV_VERSION=0.11.2 PYTHON_VERSION=3.12 ./scripts/build-macos-images.sh python
#   DENO_VERSION=v2.2.4 ./scripts/build-macos-images.sh deno
#   BUN_VERSION=v1.2.5 ./scripts/build-macos-images.sh bun
#   RUST_VERSION=stable ./scripts/build-macos-images.sh rust
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
# Version resolution -- fetch latest versions from upstream APIs
# ---------------------------------------------------------------------------

resolve_latest() {
    local name="$1"
    local url="$2"
    local jq_expr="$3"
    local fallback="$4"
    local result
    result=$(curl -fsSL --connect-timeout 5 --max-time 10 "$url" 2>/dev/null | jq -r "$jq_expr" 2>/dev/null)
    if [[ -z "$result" || "$result" == "null" ]]; then
        echo "    Warning: Could not resolve latest $name, using fallback $fallback" >&2
        echo "$fallback"
    else
        echo "$result"
    fi
}

# Resolve the latest patch version for a partial Go version (e.g. "1.22" -> "1.22.10").
# If already a full version (e.g. "1.22.5"), returns it unchanged.
resolve_go_patch() {
    local partial="$1"
    # If it already has 3 components, assume it is fully specified
    if [[ "$partial" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "$partial"
        return
    fi
    local result
    result=$(curl -fsSL --connect-timeout 5 --max-time 10 "https://go.dev/dl/?mode=json&include=all" 2>/dev/null \
        | jq -r --arg prefix "go${partial}." \
            '[.[] | select(.version | startswith($prefix)) | .version | sub("^go";"")] | sort_by(split(".") | map(tonumber)) | last' \
            2>/dev/null)
    if [[ -z "$result" || "$result" == "null" ]]; then
        echo "    Warning: Could not resolve Go ${partial}.x, using ${partial}.0" >&2
        echo "${partial}.0"
    else
        echo "$result"
    fi
}

# Resolve the latest patch version for a partial Node major (e.g. "20" -> "20.18.2").
resolve_node_patch() {
    local partial="$1"
    # If it already has 3 components, return as-is
    if [[ "$partial" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "$partial"
        return
    fi
    local result
    result=$(curl -fsSL --connect-timeout 5 --max-time 10 "https://nodejs.org/dist/index.json" 2>/dev/null \
        | jq -r --arg major "v${partial}." \
            '[.[] | select(.version | startswith($major))][0].version | sub("^v";"")' \
            2>/dev/null)
    if [[ -z "$result" || "$result" == "null" ]]; then
        echo "    Warning: Could not resolve Node ${partial}.x.x, using ${partial}.0.0" >&2
        echo "${partial}.0.0"
    else
        echo "$result"
    fi
}

# Resolve latest Deno patch for a partial version (e.g. "2.2" -> "v2.2.4").
resolve_deno_patch() {
    local partial="$1"
    # If it starts with "v" and has 3 components, return as-is
    if [[ "$partial" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        [[ "$partial" =~ ^v ]] || partial="v${partial}"
        echo "$partial"
        return
    fi
    # Strip leading v for matching
    local bare="${partial#v}"
    local result
    result=$(curl -fsSL --connect-timeout 5 --max-time 10 "https://api.github.com/repos/denoland/deno/releases" 2>/dev/null \
        | jq -r --arg prefix "v${bare}." \
            '[.[] | select(.tag_name | startswith($prefix)) | .tag_name][0]' \
            2>/dev/null)
    if [[ -z "$result" || "$result" == "null" ]]; then
        echo "    Warning: Could not resolve Deno ${partial}.x, using v${bare}.0" >&2
        echo "v${bare}.0"
    else
        echo "$result"
    fi
}

# Resolve latest Bun patch for a partial version (e.g. "1.2" -> "v1.2.5").
resolve_bun_patch() {
    local partial="$1"
    # If it starts with "v" and has 3 components, return as-is
    if [[ "$partial" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        [[ "$partial" =~ ^v ]] || partial="v${partial}"
        echo "$partial"
        return
    fi
    # Strip leading v for matching
    local bare="${partial#v}"
    local result
    result=$(curl -fsSL --connect-timeout 5 --max-time 10 "https://api.github.com/repos/oven-sh/bun/releases" 2>/dev/null \
        | jq -r --arg prefix "bun-v${bare}." \
            '[.[] | select(.tag_name | startswith($prefix)) | .tag_name | sub("^bun-";"")][0]' \
            2>/dev/null)
    if [[ -z "$result" || "$result" == "null" ]]; then
        echo "    Warning: Could not resolve Bun ${partial}.x, using v${bare}.0" >&2
        echo "v${bare}.0"
    else
        echo "$result"
    fi
}

# Default latest versions — env vars override API resolution
GO_VERSION="${GO_VERSION:-$(resolve_latest "Go" "https://go.dev/dl/?mode=json" '.[0].version | sub("^go";"")' "1.23.6")}"
NODE_VERSION="${NODE_VERSION:-$(resolve_latest "Node" "https://nodejs.org/dist/index.json" '[.[] | select(.lts != false)][0].version | sub("^v";"")' "22.14.0")}"
UV_VERSION="${UV_VERSION:-$(resolve_latest "uv" "https://api.github.com/repos/astral-sh/uv/releases/latest" '.tag_name' "0.11.2")}"
PYTHON_VERSION="${PYTHON_VERSION:-3.13}"
DENO_VERSION="${DENO_VERSION:-$(resolve_latest "Deno" "https://api.github.com/repos/denoland/deno/releases/latest" '.tag_name' "v2.2.4")}"
BUN_VERSION="${BUN_VERSION:-$(resolve_latest "Bun" "https://api.github.com/repos/oven-sh/bun/releases/latest" '.tag_name | sub("^bun-";"")' "v1.2.5")}"
RUST_VERSION="${RUST_VERSION:-stable}"

echo "==> Resolved versions: Go=$GO_VERSION Node=$NODE_VERSION Python=$PYTHON_VERSION uv=$UV_VERSION Deno=$DENO_VERSION Bun=$BUN_VERSION Rust=$RUST_VERSION"

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
    local version="${4:-}"

    local version_label=""
    if [[ -n "$version" ]]; then
        version_label="$(printf ',\n      "io.zlayer.version": "%s"' "$version")"
    fi

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
      "io.zlayer.arch": "$OCI_ARCH"${version_label}
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
    local version="${1:-$GO_VERSION}"
    # Resolve partial version (e.g. "1.22" -> "1.22.10")
    version="$(resolve_go_patch "$version")"

    ensure_base
    echo "==> Building Go ${version} toolchain image..."
    local image_name="golang-${version}"
    local rootfs
    rootfs="$(clone_base "$image_name")"
    local image_dir="$BUILD_DIR/$image_name"

    echo "    Downloading go${version}..."
    local tarball="go${version}.darwin-${ARCH}.tar.gz"
    local url="https://go.dev/dl/${tarball}"
    local dl_path="$BUILD_DIR/.downloads/${tarball}"
    mkdir -p "$BUILD_DIR/.downloads"
    if [[ ! -f "$dl_path" ]]; then
        curl -fSL -o "$dl_path" "$url"
    fi
    mkdir -p "$rootfs/usr/local"
    tar -xzf "$dl_path" -C "$rootfs/usr/local/"

    # Symlinks
    ln -sf ../go/bin/go    "$rootfs/usr/local/bin/go"
    ln -sf ../go/bin/gofmt "$rootfs/usr/local/bin/gofmt"

    write_oci_config "$image_dir" "zlayer-golang" \
        '["PATH=/usr/local/go/bin:/usr/local/bin:/usr/bin:/bin","GOROOT=/usr/local/go","GOPATH=/go"]' \
        "$version"

    create_layer_tar "$image_dir" "golang-${version}"
    echo "==> Go ${version} image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# rust
# ---------------------------------------------------------------------------

build_rust() {
    local version="${1:-$RUST_VERSION}"

    ensure_base
    echo "==> Building Rust ${version} toolchain image..."
    local image_name="rust-${version}"
    local rootfs
    rootfs="$(clone_base "$image_name")"
    local image_dir="$BUILD_DIR/$image_name"

    echo "    Installing Rust toolchain '${version}' via rustup-init..."
    local rustup_init="$BUILD_DIR/.downloads/rustup-init"
    mkdir -p "$BUILD_DIR/.downloads"
    if [[ ! -f "$rustup_init" ]]; then
        local rustup_url="https://static.rust-lang.org/rustup/dist/${ARCH}-apple-darwin/rustup-init"
        curl -fSL -o "$rustup_init" "$rustup_url"
        chmod +x "$rustup_init"
    fi
    local tmp_rust="$BUILD_DIR/.rust-tmp"
    rm -rf "$tmp_rust"
    RUSTUP_HOME="$tmp_rust/rustup" CARGO_HOME="$tmp_rust/cargo" \
        "$rustup_init" -y --no-modify-path --default-toolchain "$version" 2>/dev/null
    local sysroot
    sysroot="$(RUSTUP_HOME="$tmp_rust/rustup" "$tmp_rust/cargo/bin/rustc" --print sysroot)"
    # Capture the actual version string for labeling
    local actual_version
    actual_version="$(RUSTUP_HOME="$tmp_rust/rustup" CARGO_HOME="$tmp_rust/cargo" "$tmp_rust/cargo/bin/rustc" --version | awk '{print $2}')"
    mkdir -p "$rootfs/usr/local/rust"
    cp -a "$sysroot/." "$rootfs/usr/local/rust/"
    # Also copy cargo and friends from cargo/bin
    cp "$tmp_rust/cargo/bin/cargo"   "$rootfs/usr/local/rust/bin/"
    cp "$tmp_rust/cargo/bin/rustfmt" "$rootfs/usr/local/rust/bin/" 2>/dev/null || true
    cp "$tmp_rust/cargo/bin/clippy-driver" "$rootfs/usr/local/rust/bin/" 2>/dev/null || true
    rm -rf "$tmp_rust"

    # Symlinks
    mkdir -p "$rootfs/usr/local/bin"
    for tool in rustc cargo rustfmt clippy-driver; do
        if [[ -f "$rootfs/usr/local/rust/bin/$tool" ]]; then
            ln -sf ../rust/bin/$tool "$rootfs/usr/local/bin/$tool"
        fi
    done

    write_oci_config "$image_dir" "zlayer-rust" \
        '["PATH=/usr/local/rust/bin:/usr/local/bin:/usr/bin:/bin","RUSTUP_HOME=/usr/local/rust","CARGO_HOME=/usr/local/rust"]' \
        "${actual_version:-$version}"

    create_layer_tar "$image_dir" "rust-${version}"
    echo "==> Rust ${version} image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# node
# ---------------------------------------------------------------------------

build_node() {
    local version="${1:-$NODE_VERSION}"
    # Resolve partial version (e.g. "20" -> "20.18.2")
    version="$(resolve_node_patch "$version")"

    ensure_base
    echo "==> Building Node.js ${version} toolchain image..."
    local image_name="node-${version}"
    local rootfs
    rootfs="$(clone_base "$image_name")"
    local image_dir="$BUILD_DIR/$image_name"

    echo "    Downloading node v${version}..."
    local tarball="node-v${version}-darwin-${ARCH}.tar.gz"
    local url="https://nodejs.org/dist/v${version}/${tarball}"
    local dl_path="$BUILD_DIR/.downloads/${tarball}"
    mkdir -p "$BUILD_DIR/.downloads"
    if [[ ! -f "$dl_path" ]]; then
        curl -fSL -o "$dl_path" "$url"
    fi
    mkdir -p "$rootfs/usr/local/node"
    tar -xzf "$dl_path" --strip-components=1 -C "$rootfs/usr/local/node/"

    # Symlinks
    ln -sf ../node/bin/node "$rootfs/usr/local/bin/node"
    ln -sf ../node/bin/npm  "$rootfs/usr/local/bin/npm"
    ln -sf ../node/bin/npx  "$rootfs/usr/local/bin/npx"

    write_oci_config "$image_dir" "zlayer-node" \
        '["PATH=/usr/local/node/bin:/usr/local/bin:/usr/bin:/bin"]' \
        "$version"

    create_layer_tar "$image_dir" "node-${version}"
    echo "==> Node.js ${version} image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# python
# ---------------------------------------------------------------------------

build_python() {
    local version="${1:-$PYTHON_VERSION}"

    ensure_base
    echo "==> Building Python ${version} + uv toolchain image..."
    local image_name="python-${version}"
    local rootfs
    rootfs="$(clone_base "$image_name")"
    local image_dir="$BUILD_DIR/$image_name"

    local uv_version="$UV_VERSION"

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

    # -- python via uv --
    echo "    Installing Python ${version} via uv..."
    # Use uv to install the requested Python version into the rootfs
    # uv python install downloads and manages Python versions
    UV_PYTHON_INSTALL_DIR="$rootfs/usr/local/python" \
        "$rootfs/usr/local/bin/uv" python install "$version" 2>/dev/null || true

    # Find the installed python binary
    local py_bin
    py_bin="$(find "$rootfs/usr/local/python" -name "python${version}" -o -name "python3" 2>/dev/null | head -1)"
    if [[ -n "$py_bin" ]]; then
        # Create symlinks to the installed Python
        local py_rel
        py_rel="$(realpath --relative-to="$rootfs/usr/local/bin" "$py_bin" 2>/dev/null || echo "$py_bin")"
        ln -sf "$py_rel" "$rootfs/usr/local/bin/python3" 2>/dev/null || true
        ln -sf python3 "$rootfs/usr/local/bin/python"
    else
        echo "    Python ${version} install via uv did not produce a binary, creating shim..."
        # Create a shim that uses uv to run python
        cat > "$rootfs/usr/local/bin/python3" <<PYSHIM
#!/bin/sh
exec /usr/local/bin/uv run --python ${version} python3 "\$@"
PYSHIM
        chmod +x "$rootfs/usr/local/bin/python3"
        ln -sf python3 "$rootfs/usr/local/bin/python"
    fi

    write_oci_config "$image_dir" "zlayer-python" \
        '["PATH=/usr/local/python/bin:/usr/local/bin:/usr/bin:/bin","UV_SYSTEM_PYTHON=1"]' \
        "$version"

    create_layer_tar "$image_dir" "python-${version}"
    echo "==> Python ${version} image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# deno
# ---------------------------------------------------------------------------

build_deno() {
    local version="${1:-$DENO_VERSION}"
    # Resolve partial version (e.g. "2.2" -> "v2.2.4")
    version="$(resolve_deno_patch "$version")"
    # Ensure the version starts with "v"
    [[ "$version" =~ ^v ]] || version="v${version}"

    ensure_base
    echo "==> Building Deno ${version} toolchain image..."
    local image_name="deno-${version}"
    local rootfs
    rootfs="$(clone_base "$image_name")"
    local image_dir="$BUILD_DIR/$image_name"

    echo "    Installing Deno ${version}..."
    local deno_arch
    if [[ "$ARCH" == "arm64" ]]; then
        deno_arch="aarch64"
    else
        deno_arch="x86_64"
    fi
    local deno_zip="deno-${deno_arch}-apple-darwin.zip"
    local deno_url="https://github.com/denoland/deno/releases/download/${version}/${deno_zip}"
    local deno_dl="$BUILD_DIR/.downloads/deno-${version}-${deno_arch}.zip"
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
        '["PATH=/usr/local/bin:/usr/bin:/bin","DENO_DIR=/var/tmp/deno"]' \
        "$version"

    create_layer_tar "$image_dir" "deno-${version}"
    echo "==> Deno ${version} image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# bun
# ---------------------------------------------------------------------------

build_bun() {
    local version="${1:-$BUN_VERSION}"
    # Resolve partial version (e.g. "1.2" -> "v1.2.5")
    version="$(resolve_bun_patch "$version")"
    # Ensure the version starts with "v"
    [[ "$version" =~ ^v ]] || version="v${version}"

    ensure_base
    echo "==> Building Bun ${version} toolchain image..."
    local image_name="bun-${version}"
    local rootfs
    rootfs="$(clone_base "$image_name")"
    local image_dir="$BUILD_DIR/$image_name"

    echo "    Installing Bun ${version}..."
    local bun_arch
    if [[ "$ARCH" == "arm64" ]]; then
        bun_arch="aarch64"
    else
        bun_arch="x64"
    fi
    local bun_zip="bun-darwin-${bun_arch}.zip"
    local bun_url="https://github.com/oven-sh/bun/releases/download/bun-${version}/${bun_zip}"
    local bun_dl="$BUILD_DIR/.downloads/bun-${version}-${bun_arch}.zip"
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
        '["PATH=/usr/local/bin:/usr/bin:/bin"]' \
        "$version"

    create_layer_tar "$image_dir" "bun-${version}"
    echo "==> Bun ${version} image ready: $rootfs"
}

# ---------------------------------------------------------------------------
# all -- builds latest of everything in parallel
# ---------------------------------------------------------------------------

build_all() {
    build_base

    echo "==> Building all toolchain images (latest) in parallel..."

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
    echo "Usage: $0 <subcommand> [version ...]"
    echo ""
    echo "Subcommands:"
    echo "  base     Build the base rootfs with macOS system binaries"
    echo "  golang   Build Go toolchain image"
    echo "  rust     Build Rust toolchain image"
    echo "  node     Build Node.js toolchain image"
    echo "  python   Build Python + uv toolchain image"
    echo "  deno     Build Deno toolchain image"
    echo "  bun      Build Bun toolchain image"
    echo "  all      Build base + all toolchain images (latest)"
    echo ""
    echo "Multi-version builds:"
    echo "  $0 golang 1.22 1.23"
    echo "  $0 node 18 20 22 24"
    echo "  $0 python 3.10 3.11 3.12 3.13 3.14"
    echo ""
    echo "Environment overrides:"
    echo "  GO_VERSION=1.22.0 $0 golang"
    echo "  NODE_VERSION=22.14.0 $0 node"
    echo "  PYTHON_VERSION=3.12 $0 python"
    echo ""
    echo "Output directory: $BUILD_DIR/"
}

if [[ $# -lt 1 ]]; then
    usage
    exit 1
fi

# Run versions: if extra args are given, iterate over them; otherwise use default.
run_versions() {
    local builder="$1"
    shift
    if [[ $# -gt 0 ]]; then
        for v in "$@"; do
            "$builder" "$v"
        done
    else
        "$builder"
    fi
}

case "$1" in
    base)
        build_base
        ;;
    golang)
        shift
        run_versions build_golang "$@"
        ;;
    rust)
        shift
        run_versions build_rust "$@"
        ;;
    node)
        shift
        run_versions build_node "$@"
        ;;
    python)
        shift
        run_versions build_python "$@"
        ;;
    deno)
        shift
        run_versions build_deno "$@"
        ;;
    bun)
        shift
        run_versions build_bun "$@"
        ;;
    all)
        build_all
        ;;
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
