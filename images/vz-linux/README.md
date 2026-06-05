# `images/vz-linux` — Linux guest kernel + initramfs for the macOS VZ runtime

This directory builds the **Linux guest** that ZLayer's macOS Apple-
Virtualization (VZ) runtime boots — via `VZLinuxBootLoader` — to run OCI **Linux**
containers on Apple Silicon. The host side lives in `crates/zlayer-agent`
(`runtimes/macos_vz_linux.rs`); the in-guest PID1 agent and the vsock wire
protocol live in `crates/zlayer-vzagent`.

## What this produces

`build.sh` emits three artifacts into `$OUT_DIR`
(default `target/vz-linux/`, or `$ZLAYER_DATA_DIR/vz-linux` if that env var is
set):

| File | Description |
| --- | --- |
| `Image` | **Uncompressed** arm64 kernel image. VZ's `VZLinuxBootLoader` requires a raw `Image`, **not** `Image.gz`. |
| `initramfs.cpio.gz` | gzip `newc` cpio initramfs. Its `/init` **is** the `zlayer-vzagent` PID1 agent (static-musl aarch64), bundled with a static busybox for the `udhcpc` / `mount` / `switch_root` fallback path. |
| `manifest.json` | Kernel + busybox versions and the SHA-256 of `Image` and `initramfs.cpio.gz`. |

## How the guest boots

1. The macOS host configures a `VZVirtualMachine` with `VZLinuxBootLoader`
   pointed at `Image` + `initramfs.cpio.gz`, a virtio-fs share tagged `rootfs`
   (the OCI rootfs), a virtio-net NAT device, and a virtio-vsock device.
2. The kernel execs the initramfs `/init` — the `zlayer-vzagent` agent — as
   **PID 1**.
3. The agent mounts `/proc`, `/sys`, devtmpfs, tmpfs; mounts the virtiofs
   `rootfs` share read-only at `/lower`; layers a writable tmpfs overlay to form
   `/newroot`; brings up `eth0` (kernel `ip=dhcp` and/or busybox `udhcpc`);
   `pivot_root`s onto `/newroot` while staying PID 1; then listens on
   `AF_VSOCK` port **1024** for the host control connection.
4. The host drives the workload over vsock using the
   [`zlayer_vzagent::proto`] protocol (run / exec / signal, with stdout/stderr
   streaming and exit-code reporting).

## How the host consumes the artifacts

The host fetches `Image` + `initramfs.cpio.gz` (published by the
`vz-linux-images` CI workflow) into its data directory. For local development
you can point the runtime at a freshly-built pair without republishing, via:

| Env var | Overrides |
| --- | --- |
| `ZLAYER_VZ_LINUX_KERNEL` | Absolute path to the `Image` to boot. |
| `ZLAYER_VZ_LINUX_INITRD` | Absolute path to the `initramfs.cpio.gz` to boot. |

## Building locally

> Cross-compiles an arm64 kernel and a static-musl aarch64 agent. Intended to
> run on a **Linux** host (or CI runner). It is not expected to run on the macOS
> developer host.

```sh
# Requires: an aarch64 GCC cross toolchain (aarch64-linux-gnu-gcc), make, cpio,
# gzip, curl, and either cargo-zigbuild or cross for the agent.
./images/vz-linux/build.sh

# Pin a different kernel / busybox:
KERNEL_VERSION=6.18.34 BUSYBOX_VERSION=1.38.0 ./images/vz-linux/build.sh

# Custom output directory:
OUT_DIR=/tmp/vz-out ./images/vz-linux/build.sh
```

## GPLv2

The kernel `Image` is built from unmodified upstream source. See
[`KERNEL_SOURCE.md`](./KERNEL_SOURCE.md) for the version, source URL, license,
and exact reproduction steps. The configuration delta from a stock arm64
defconfig is fully contained in
[`kernel-arm64.defconfig`](./kernel-arm64.defconfig).
