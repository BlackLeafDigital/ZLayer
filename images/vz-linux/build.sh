#!/usr/bin/env bash
#
# build.sh — build the Linux kernel `Image` + initramfs that the macOS Apple-
# Virtualization (VZ) Linux runtime boots to run OCI Linux containers on Apple
# Silicon.
#
# Outputs (under "$OUT_DIR"):
#   * Image               — UNCOMPRESSED arm64 kernel image (VZ requires raw,
#                           NOT Image.gz; VZLinuxBootLoader cannot decompress).
#   * initramfs.cpio.gz   — gzip newc cpio whose /init is the zlayer-vzagent
#                           PID1 agent, plus a static busybox for the
#                           mount/udhcpc/switch_root fallback path.
#   * manifest.json       — kernel version + sha256 of Image and initramfs.
#
# This is a Linux-only build (it cross-compiles an arm64 kernel + a static-musl
# aarch64 agent). It is NOT expected to run on the macOS developer host; CI runs
# it on an x86_64 Linux runner via cargo-zigbuild / a GCC arm64 cross toolchain.
#
# Reproducibility / GPLv2: the kernel binary is built from the unmodified
# upstream source tarball pinned by KERNEL_VERSION, configured by the
# kernel-arm64.defconfig in this directory. See KERNEL_SOURCE.md.

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration (all overridable via the environment)
# ---------------------------------------------------------------------------

# Pinned current longterm (LTS) kernel. Verify the newest non-EOL LTS at build
# time with:
#     curl -fsSL https://www.kernel.org/releases.json \
#       | python3 -c 'import json,sys; d=json.load(sys.stdin); \
#         print([r["version"] for r in d["releases"] if r["moniker"]=="longterm" and not r["iseol"]])'
# As of this writing the newest non-EOL longterm line is 6.18.x (latest patch
# 6.18.34). Bump KERNEL_VERSION when a newer LTS patch ships.
KERNEL_VERSION="${KERNEL_VERSION:-6.18.34}"

# Static busybox for the in-initramfs fallback tools (mount, udhcpc,
# switch_root). Verify latest at https://busybox.net/downloads/.
BUSYBOX_VERSION="${BUSYBOX_VERSION:-1.38.0}"

# Cross target for the kernel. Apple Silicon is arm64.
ARCH="arm64"
# GNU cross prefix used by the kernel Makefile. cargo-zigbuild handles the
# agent itself; the kernel build still wants a binutils/gcc cross toolchain.
CROSS_COMPILE="${CROSS_COMPILE:-aarch64-linux-gnu-}"

# Repo root (this script lives in <repo>/images/vz-linux).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# Output + scratch directories. Default under the repo unless ZLAYER_DATA_DIR
# is set, in which case we honor it (matches the rest of the toolchain).
if [[ -n "${ZLAYER_DATA_DIR:-}" ]]; then
  OUT_DIR="${OUT_DIR:-${ZLAYER_DATA_DIR}/vz-linux}"
else
  OUT_DIR="${OUT_DIR:-${REPO_ROOT}/target/vz-linux}"
fi
WORK_DIR="${WORK_DIR:-${OUT_DIR}/.work}"

DEFCONFIG="${SCRIPT_DIR}/kernel-arm64.defconfig"

# Number of parallel jobs.
if command -v nproc >/dev/null 2>&1; then
  JOBS="${JOBS:-$(nproc)}"
else
  JOBS="${JOBS:-4}"
fi

KERNEL_TARBALL="linux-${KERNEL_VERSION}.tar.xz"
KERNEL_MAJOR="${KERNEL_VERSION%%.*}"
KERNEL_URL="https://cdn.kernel.org/pub/linux/kernel/v${KERNEL_MAJOR}.x/${KERNEL_TARBALL}"

BUSYBOX_TARBALL="busybox-${BUSYBOX_VERSION}.tar.bz2"
BUSYBOX_URL="https://busybox.net/downloads/${BUSYBOX_TARBALL}"

log() { printf '\033[1;34m[vz-linux]\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31m[vz-linux] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

# sha256 helper that works with either coreutils or busybox.
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

require() {
  command -v "$1" >/dev/null 2>&1 || die "required tool not found: $1"
}

# ---------------------------------------------------------------------------
# Preflight
# ---------------------------------------------------------------------------

preflight() {
  require curl
  require make
  require cpio
  require gzip
  require tar
  require "${CROSS_COMPILE}gcc"
  # The agent cross-build prefers cargo-zigbuild; cross is the fallback.
  if ! command -v cargo-zigbuild >/dev/null 2>&1 && ! command -v cross >/dev/null 2>&1; then
    die "need cargo-zigbuild or cross to cross-compile the aarch64 musl agent"
  fi
  [[ -f "${DEFCONFIG}" ]] || die "missing defconfig: ${DEFCONFIG}"
}

# ---------------------------------------------------------------------------
# Download + extract
# ---------------------------------------------------------------------------

fetch() {
  local url="$1" dest="$2"
  if [[ -f "${dest}" ]]; then
    log "cached: ${dest}"
    return
  fi
  log "downloading ${url}"
  curl -fSL --retry 3 -o "${dest}.tmp" "${url}"
  mv "${dest}.tmp" "${dest}"
}

build_kernel() {
  local src_dir="${WORK_DIR}/linux-${KERNEL_VERSION}"
  fetch "${KERNEL_URL}" "${WORK_DIR}/${KERNEL_TARBALL}"

  if [[ ! -d "${src_dir}" ]]; then
    log "extracting kernel ${KERNEL_VERSION}"
    tar -C "${WORK_DIR}" -xf "${WORK_DIR}/${KERNEL_TARBALL}"
  fi

  log "configuring kernel (olddefconfig from kernel-arm64.defconfig)"
  cp "${DEFCONFIG}" "${src_dir}/.config"
  make -C "${src_dir}" ARCH="${ARCH}" CROSS_COMPILE="${CROSS_COMPILE}" olddefconfig

  log "building kernel Image (-j${JOBS})"
  make -C "${src_dir}" ARCH="${ARCH}" CROSS_COMPILE="${CROSS_COMPILE}" -j"${JOBS}" Image

  # VZ requires the UNCOMPRESSED Image, not Image.gz.
  local image_src="${src_dir}/arch/arm64/boot/Image"
  [[ -f "${image_src}" ]] || die "kernel Image not produced at ${image_src}"
  cp "${image_src}" "${OUT_DIR}/Image"
  log "kernel Image -> ${OUT_DIR}/Image"
}

# ---------------------------------------------------------------------------
# Static busybox (initramfs fallback tools)
# ---------------------------------------------------------------------------

build_busybox() {
  local src_dir="${WORK_DIR}/busybox-${BUSYBOX_VERSION}"
  fetch "${BUSYBOX_URL}" "${WORK_DIR}/${BUSYBOX_TARBALL}"

  if [[ ! -d "${src_dir}" ]]; then
    log "extracting busybox ${BUSYBOX_VERSION}"
    tar -C "${WORK_DIR}" -xf "${WORK_DIR}/${BUSYBOX_TARBALL}"
  fi

  log "configuring busybox (static defconfig)"
  make -C "${src_dir}" ARCH="${ARCH}" CROSS_COMPILE="${CROSS_COMPILE}" defconfig
  # Force a fully static build so it runs in the bare initramfs.
  sed -i 's/.*CONFIG_STATIC[ =].*/CONFIG_STATIC=y/' "${src_dir}/.config" || true
  if ! grep -q '^CONFIG_STATIC=y' "${src_dir}/.config"; then
    echo 'CONFIG_STATIC=y' >>"${src_dir}/.config"
  fi
  # busybox's bundled kconfig has no `olddefconfig` target (that's a Linux
  # kernel target); normalize the edited .config with a non-interactive
  # `oldconfig`, answering any NEW-symbol prompts with their default.
  yes "" | make -C "${src_dir}" ARCH="${ARCH}" CROSS_COMPILE="${CROSS_COMPILE}" oldconfig

  log "building static busybox (-j${JOBS})"
  make -C "${src_dir}" ARCH="${ARCH}" CROSS_COMPILE="${CROSS_COMPILE}" -j"${JOBS}" busybox

  [[ -f "${src_dir}/busybox" ]] || die "busybox binary not produced"
  cp "${src_dir}/busybox" "${WORK_DIR}/busybox-static"
}

# ---------------------------------------------------------------------------
# Agent (static-musl aarch64) — becomes /init
# ---------------------------------------------------------------------------

build_agent() {
  local target="aarch64-unknown-linux-musl"
  log "adding rust target ${target}"
  rustup target add "${target}" >/dev/null 2>&1 || true

  pushd "${REPO_ROOT}" >/dev/null
  if command -v cargo-zigbuild >/dev/null 2>&1; then
    log "cross-compiling zlayer-vzagent via cargo-zigbuild (${target})"
    cargo zigbuild --release --target "${target}" -p zlayer-vzagent
  else
    log "cross-compiling zlayer-vzagent via cross (${target})"
    cross build --release --target "${target}" -p zlayer-vzagent
  fi
  popd >/dev/null

  local agent_bin="${REPO_ROOT}/target/${target}/release/zlayer-vzagent"
  [[ -f "${agent_bin}" ]] || die "agent binary not produced at ${agent_bin}"
  cp "${agent_bin}" "${WORK_DIR}/zlayer-vzagent"
}

# ---------------------------------------------------------------------------
# Assemble initramfs
# ---------------------------------------------------------------------------

build_initramfs() {
  local root="${WORK_DIR}/initramfs-root"
  rm -rf "${root}"
  mkdir -p "${root}"/{bin,sbin,dev,proc,sys,run,tmp,newroot,lower}

  # /init IS the agent (the kernel execs /init as PID 1).
  install -m 0755 "${WORK_DIR}/zlayer-vzagent" "${root}/init"

  # Static busybox + the applet symlinks the fallback path needs.
  install -m 0755 "${WORK_DIR}/busybox-static" "${root}/bin/busybox"
  for applet in sh mount umount switch_root pivot_root udhcpc ip ifconfig mkdir; do
    ln -sf busybox "${root}/bin/${applet}"
  done
  # udhcpc default lease script (busybox calls /usr/share/udhcpc/default.script
  # by default; provide a minimal one that configures the interface).
  mkdir -p "${root}/usr/share/udhcpc"
  cat >"${root}/usr/share/udhcpc/default.script" <<'EOS'
#!/bin/sh
# Minimal busybox udhcpc lease handler: apply the offered IP + default route.
case "$1" in
  bound|renew)
    ip addr flush dev "$interface" 2>/dev/null
    ip addr add "$ip/${mask:-24}" dev "$interface"
    [ -n "$router" ] && ip route add default via "$router" dev "$interface" 2>/dev/null
    if [ -n "$dns" ]; then
      : >/etc/resolv.conf
      for s in $dns; do echo "nameserver $s" >>/etc/resolv.conf; done
    fi
    ;;
esac
exit 0
EOS
  chmod 0755 "${root}/usr/share/udhcpc/default.script"

  # A couple of essential device nodes in case devtmpfs is delayed.
  # (mknod requires root; tolerate failure — devtmpfs mounts these at runtime.)
  mknod -m 600 "${root}/dev/console" c 5 1 2>/dev/null || true
  mknod -m 666 "${root}/dev/null" c 1 3 2>/dev/null || true

  log "packing initramfs.cpio.gz"
  ( cd "${root}" && find . -print0 \
      | cpio --null -o -H newc 2>/dev/null \
      | gzip -9 ) >"${OUT_DIR}/initramfs.cpio.gz"
  log "initramfs -> ${OUT_DIR}/initramfs.cpio.gz"
}

# ---------------------------------------------------------------------------
# Manifest
# ---------------------------------------------------------------------------

write_manifest() {
  local image_sha initrd_sha
  image_sha="$(sha256 "${OUT_DIR}/Image")"
  initrd_sha="$(sha256 "${OUT_DIR}/initramfs.cpio.gz")"
  cat >"${OUT_DIR}/manifest.json" <<EOF
{
  "kernel_version": "${KERNEL_VERSION}",
  "busybox_version": "${BUSYBOX_VERSION}",
  "arch": "aarch64",
  "image": {
    "file": "Image",
    "sha256": "${image_sha}"
  },
  "initramfs": {
    "file": "initramfs.cpio.gz",
    "sha256": "${initrd_sha}"
  }
}
EOF
  log "manifest -> ${OUT_DIR}/manifest.json"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

main() {
  preflight
  mkdir -p "${OUT_DIR}" "${WORK_DIR}"
  build_agent
  build_busybox
  build_initramfs
  build_kernel
  write_manifest
  log "done. artifacts in ${OUT_DIR}"
  ls -la "${OUT_DIR}" >&2
}

main "$@"
