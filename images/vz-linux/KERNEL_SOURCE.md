# Linux kernel source & GPLv2 compliance — ZLayer VZ-Linux guest

The `Image` artifact produced by [`build.sh`](./build.sh) in this directory is a
Linux kernel compiled from **unmodified upstream source**. This document
satisfies the GNU General Public License v2 written-offer / source-availability
obligations for the binary we ship.

## Kernel version

- **Version:** `6.18.34` (longterm / LTS line `6.18.x`)
- **Source tarball:** `linux-6.18.34.tar.xz`
- **Canonical download URL:**
  <https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.18.34.tar.xz>
- **Upstream project:** <https://www.kernel.org/>

The exact version is pinned by the `KERNEL_VERSION` variable in `build.sh` and
recorded (with the binary's SHA-256) in the generated `manifest.json`. The
newest non-EOL LTS patch level is verified at build time — see the comment above
`KERNEL_VERSION` in `build.sh` for the `kernel.org/releases.json` query.

## License

The Linux kernel is licensed under the **GNU General Public License, version 2**
(with the syscall-boundary user-space exception). The full license text ships
inside the source tarball at `COPYING` / `LICENSES/`. ZLayer redistributes the
kernel binary under the same terms.

## How to reproduce the binary

The complete corresponding source is the upstream tarball above; no ZLayer
patches are applied. To reproduce the shipped `Image`:

```sh
# 1. Obtain the identical source (verify against kernel.org signatures):
curl -fSLO https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.18.34.tar.xz

# 2. Build with the configuration and steps in this directory:
KERNEL_VERSION=6.18.34 ./images/vz-linux/build.sh
```

The configuration applied to the source is
[`kernel-arm64.defconfig`](./kernel-arm64.defconfig) in this directory, expanded
via `ARCH=arm64 make olddefconfig`. Together, `kernel-arm64.defconfig` +
`build.sh` + the pinned upstream tarball fully reproduce the binary `Image`.

## Modifications

**None.** ZLayer applies no patches to the kernel source. Only the build-time
`.config` (this directory's defconfig) differs from a stock `arm64` defconfig,
and that delta is fully contained in `kernel-arm64.defconfig`.
