# macOS Apple-Virtualization (VZ) runtime

The VZ runtime (`crates/zlayer-agent/src/runtimes/macos_vz.rs`) runs **ephemeral
native-macOS guest VMs** via Apple's `Virtualization.framework` — the
GitHub-runner / Tart model. It coexists with, and does not replace:

| Runtime | Module | Guest | Selected by |
|---|---|---|---|
| Seatbelt sandbox | `macos_sandbox` | macOS-native processes | `Auto` (primary) / `--runtime mac-sandbox` |
| libkrun micro-VM | `macos_vm` | Linux | composite delegate / `--runtime mac-vm` |
| **Apple Virtualization** | **`macos_vz`** | **full macOS guest VM** | **opt-in only** |

## Selecting it

Under the default composite runtime the VZ delegate is **preferred automatically
for genuine VZ bundles**, and remains explicitly selectable:

- **Auto-detect (default):** when an image's manifest carries the
  `com.zlayer.runtime=vz` annotation — stamped by `zlayer vz build-base` — the
  composite's `select_for` routes it to the VZ runtime (the only runtime that
  can boot such a bundle). This fires *only* for real VZ bundles, so
  Seatbelt-rootfs images and Linux images are unaffected (Linux still routes to
  the libkrun delegate or a peer).
- **Per service (force/opt-out):** label `com.zlayer.isolation: "vz"` to force
  VZ, or `com.zlayer.isolation: "sandbox"` (alias `seatbelt`) to force the
  Seatbelt sandbox even for a VZ-annotated image.
- **Whole node:** `zlayer --runtime mac-vz …` (a standalone `VzRuntime`).

## How it works

Each "container" is a macOS guest VM cloned from a base image bundle:

- **Base bundle** (`{data_dir}/vz/images/{image}/`): `disk.img`,
  `hardware-model.bin` (a `VZMacHardwareModel` `dataRepresentation`), and
  `aux.img`. Provide one by:
  - `pull_image` of an OCI artifact whose layers unpack to those three files
    (Tart-style), or
  - pointing the image reference at a local directory containing them, or
  - **building one** from a macOS `.ipsw` with `zlayer vz build-base` (see
    below).
- **create_container**: APFS `clonefile` CoW of the base `disk.img`, a fresh
  `VZMacMachineIdentifier`, a per-VM locally-administered MAC
  (`VZMACAddress.randomLocallyAdministeredAddress`), an ephemeral SSH keypair
  (`ssh-keygen`), and a serialized `config.json`.
- **start_container**: builds a `VZVirtualMachineConfiguration` (platform =
  hardware model + machine id + aux; `VZMacOSBootLoader`; CPU/RAM clamped to the
  framework's allowed range; a graphics device — required to boot; virtio block
  over the CoW disk; virtio-net + NAT with the per-VM MAC; serial →
  `console.log`), creates the `VZVirtualMachine` on a **dedicated serial
  dispatch queue**, and starts it. The framework's `block2` completion blocks
  are bridged to a std channel so the async runtime can await them.
- **get_container_ip**: parses `/var/db/dhcpd_leases` for the VM's MAC → guest
  IPv4 (the host NAT DHCP server writes the lease once the guest requests one).
- **exec**: SSH into the guest (`admin` by default, override with
  `com.zlayer.vz.user`) using the ephemeral key.
- **pause/unpause**: real `VZVirtualMachine` pause/resume.

## Building base images (`zlayer vz build-base`)

The producer end of the runtime
(`crates/zlayer-agent/src/runtimes/macos_vz_build.rs`) mints a base bundle from
a macOS `.ipsw` restore image:

```bash
# From a local / remote .ipsw, write the bundle to a directory:
zlayer vz build-base --ipsw ~/UniversalMac.ipsw --output ./macos-vz-base

# Fetch the latest host-supported restore image and publish to a registry:
zlayer vz build-base --latest --push ghcr.io/org/zlayer/macos-vz:sequoia
```

It loads the restore image (`VZMacOSRestoreImage`), reads its most-featureful
supported configuration (`VZMacHardwareModel` + minimum CPU/RAM), writes
`hardware-model.bin`, creates a fresh `aux.img`
(`VZMacAuxiliaryStorage initCreatingStorageAtURL:hardwareModel:options:error:`)
and a sparse blank `disk.img`, then runs `VZMacOSInstaller` (~20-40 min) to
install macOS onto the disk. With `--push`, the three files are packed into a
single `tar+zstd` OCI layer (`zlayer-registry::pack::pack_files_tar_zstd`) and
pushed via `ImagePuller::push_artifact`; the manifest carries the routing
annotation `com.zlayer.runtime=vz`.

This requires a **signed** binary (the virtualization entitlement) on
Apple Silicon — build with `make build` (auto-signs on macOS) or run
`scripts/sign-vz.sh` afterward. CI: `.forgejo/workflows/macos-vz-images.yml`
(manual `workflow_dispatch`, since the install is multi-GB and ~30 min). Because
the install needs the entitlement + an `.ipsw`, the end-to-end builder test is
`#[ignore]`d (set `ZLAYER_TEST_IPSW` to run it); the full code path compiles and
the pure helpers are unit-tested. The pushed layer (a compressed multi-GB disk)
is held in memory during the chunked-over-the-wire blob push, so run the build
on a host with adequate RAM.

## Constraints

- **Two VMs per host.** Apple limits concurrent macOS guests to two. A
  process-wide `AtomicU32` gate with an RAII guard enforces it.
- **Thread-safety.** `VZVirtualMachine` is not thread-safe — every call runs on
  its single serial `DispatchQueue`.
- **No host PID / no live metrics.** A VM has no host-visible container PID;
  stats report the configured CPU/RAM allocation.

## Entitlement & code-signing (required to boot)

Booting a VM requires the **`com.apple.security.virtualization`** entitlement on
a code-signed binary. Without it, `VZVirtualMachine::isSupported()` returns
false and `start_container` fails with an actionable error. Sign the `zlayer`
binary with an entitlements plist containing:

```xml
<key>com.apple.security.virtualization</key><true/>
```

CI machines typically lack this entitlement, so the VM-boot integration test is
`#[ignore]`d (it also needs a base bundle via `ZLAYER_VZ_TEST_BUNDLE`). The full
code path still compiles and the pure helpers are unit-tested.

# Linux guests

The sections above describe the **macOS-guest** VZ path (`macos_vz.rs`), which
boots a full macOS guest from a disk-image bundle. ZLayer also has a separate VZ
path that boots a **Linux kernel** to run ordinary OCI Linux containers
(`alpine`, etc.) on Apple Silicon. It lives in
`crates/zlayer-agent/src/runtimes/macos_vz_linux.rs` (`VzLinuxRuntime`), with the
in-guest agent and wire protocol in `crates/zlayer-vzagent`.

It is a third, distinct VZ-family option:

| Path | Module | Guest | Boot loader |
|---|---|---|---|
| macOS-guest VZ | `macos_vz` | full macOS VM | `VZMacOSBootLoader` (disk-image bundle) |
| **Linux-guest VZ** | **`macos_vz_linux`** | **Linux VM (OCI containers)** | **`VZLinuxBootLoader` (kernel `Image` + initramfs)** |
| libkrun micro-VM | `macos_vm` | Linux | libkrun (third-party `libkrun.dylib`) |

Unlike the libkrun runtime it loads **no external dylib** —
`Virtualization.framework` ships with macOS — and unlike the macOS-guest path it
boots a Linux kernel + initramfs rather than a macOS bundle.

## What it does

Each "container" is a lightweight headless Linux VM created via
`Virtualization.framework`'s `VZLinuxBootLoader`. The VM boots a ZLayer-built
arm64 kernel `Image` plus an initramfs whose `/init` is the `zlayer-vzagent`
PID-1 agent; the extracted OCI image rootfs is shared into the guest over
virtiofs and overlaid with a writable tmpfs. The agent then runs the container
entrypoint, streaming its stdout/stderr and exit code back to the host over a
virtio-vsock control channel. The full `Runtime` surface is implemented —
pull/create/start/stop/remove, `exec`, `logs`, `wait`, `kill`, `pause`/`unpause`,
`stats`, container IP, and port mappings.

## Architecture

```text
host (zlayer-agent, VzLinuxRuntime)                guest (Linux VM)
──────────────────────────────────                ────────────────────────────
kernel Image + initramfs.cpio.gz  ── VZLinuxBootLoader ─▶ kernel execs /init
   (agent installed as /init)                              = zlayer-vzagent (PID 1)
virtiofs share (tag "rootfs", RO) ──────────────────────▶ mount virtiofs at /lower
   = extracted OCI image rootfs                            + tmpfs overlay upper
                                                           pivot_root onto /newroot
virtio-net + NAT (per-VM MAC)     ──────────────────────▶ eth0 DHCP → 192.168.64.x
virtio-vsock device               ◀── AF_VSOCK :1024 ───── agent listens on CONTROL_PORT
                                  ── Run/Exec/Signal ────▶
                                  ◀─ Stdout/Stderr/
                                     Started/Exited/Error ─
```

Step by step:

1. **Boot.** `build_config_linux` builds a `VZVirtualMachineConfiguration` on a
   dedicated serial dispatch queue: a generic platform (no machine identifier /
   aux storage), `VZLinuxBootLoader` over the kernel `Image` + `initramfs.cpio.gz`
   with command line `console=hvc0 rootfstag=rootfs rw`, a headless serial
   console wired to `console.log`, a virtiofs share, a virtio-net NAT device, and
   a virtio-vsock device. Linux guests are **uncapped** — the macOS two-VM
   licensing limit does not apply.
2. **rootfs.** A `VZVirtioFileSystemDeviceConfiguration` (tag `rootfs`) shares the
   extracted OCI image rootfs into the guest **read-only**. The agent mounts that
   virtiofs share at `/lower`, layers a writable tmpfs overlay
   (`upperdir`/`workdir` on tmpfs) to form `/newroot`, and `pivot_root`s onto it
   while staying PID 1. The read-only lower means the shared image rootfs is never
   mutated and can be shared across replicas; per-container writes are ephemeral
   in the tmpfs upper.
3. **agent + workload.** The in-guest `zlayer-vzagent` mounts the core
   pseudo-filesystems, brings up `eth0`, `pivot_root`s, then `listen`s on
   `AF_VSOCK` port `1024` (`proto::CONTROL_PORT`). After the VM reaches Running,
   the host connects to that port — the `connectToPort` call is issued on the VM's
   serial queue and its completion block `dup`s the connected fd back over a
   channel — and sends a `Run` message (resolved argv + env + cwd + uid/gid). The
   agent spawns the entrypoint, sends `Started{pid}`, streams `Stdout`/`Stderr`,
   and reports `Exited{code}` (a dedicated single-`waitpid` reaper thread keeps
   PID 1 zombie-free). `exec`, `kill`/`stop`, and signal delivery each open a
   fresh vsock connection to the same port (`Exec` enters the workload's PID
   namespace via `setns`; `Signal` delivers a numeric POSIX signal).
4. **networking.** The virtio-net device uses a `VZNATNetworkDeviceAttachment`
   whose MAC is pinned to the container's per-VM `VZMACAddress`. VZ's userspace
   DHCP server hands the guest a `192.168.64.x` lease; the host resolves it by
   reading `/var/db/dhcpd_leases` keyed by that MAC (a background poller caches
   it at start, 60 s budget; `get_container_ip` falls back to a 15 s live read).
   The NAT subnet is **directly host-reachable**, so `guest_ip:container_port`
   works with no proxy. That guest IP is what the daemon's same-service
   `localhost:<port>` reachability feature consumes to publish
   `127.0.0.1:<port>` → guest. Spec `port_mappings` that pin a fixed (non-zero)
   TCP host port additionally get a tokio loopback listener on
   `127.0.0.1:host_port` that proxies to `guest_ip:container_port` (Docker's
   `-p host:container`), so tooling targeting a stable host port reaches the
   guest even though the NAT IP is ephemeral.

   > **Kernel requirement (load-bearing).** The guest only gets `eth0` if the
   > kernel was built with `CONFIG_NETDEVICES=y` **and** `CONFIG_VIRTIO_NET=y`.
   > `VIRTIO_NET` lives inside the `if NETDEVICES` block and `NETDEVICES` is
   > `default y if UML` (→ `n` on arm64), so listing `CONFIG_VIRTIO_NET=y` alone
   > in the defconfig is silently dropped by `make olddefconfig` — the guest then
   > boots with only `lo` and no DHCP ever happens. `images/vz-linux/build.sh`
   > asserts every load-bearing symbol survived `olddefconfig` for exactly this
   > reason. The in-guest agent picks the NIC by `ARPHRD_ETHER` (`/sys/class/net/
   > <if>/type == 1`), never the kernel's auto-created `sit0` tunnel.
5. **overlay (cross-node mesh).** A VZ guest is a full VM with no host
   netns/PID, so the Linux veth-by-PID overlay attach can't apply. Instead the
   runtime reports `OverlayAttachKind::InGuestVsock`; the service layer asks
   overlayd for a **guest-managed** config (`AttachHandle::GuestManaged` →
   `GuestOverlayConfig`: overlay IP + WireGuard keypair + peer set, with the
   guest's public key registered in the mesh), and pushes it to the guest as a
   `proto::Msg::OverlayConfig` over a fresh vsock connection. The in-guest agent
   brings up a **kernel WireGuard** interface `zl-overlay0` (needs
   `CONFIG_WIREGUARD=y`) via netlink. `get_container_ip` then prefers the overlay
   IP over the NAT lease. On macOS the runtime is a `CompositeRuntime`, which
   forwards `overlay_attach_kind`/`push_overlay_config` to its VZ-Linux delegate.

### Directory layout

```text
{data_dir}/vz/linux/
  images/{sanitized_image}/rootfs/   -- extracted OCI image layers (virtiofs lower)
  kernel/                            -- guest kernel cache (Image + initramfs.cpio.gz)
  cache/                             -- blob/registry cache
  {service}-{replica}/               -- per-container state
    rootfs/                          -- per-container rootfs (clone of base)
    console.log                      -- guest serial console + mirrored workload output
    config.json                      -- serialized ServiceSpec
```

## Selecting it

Under the default composite runtime the Linux-guest VZ runtime is the **default
path for Linux images on macOS** whenever it is available:

- **Auto (default):** on macOS, Linux images route here automatically — whether
  the OS is known from `spec.platform.os = linux` or from the image manifest's
  recorded OS. The libkrun delegate is used only when the VZ Linux runtime is
  absent.
- **Manifest marker:** an image whose manifest carries
  `com.zlayer.runtime=vz-linux` (the `ZLAYER_RUNTIME_LINUX_VZ` registry constant)
  auto-routes here.
- **Per-service force:** label `com.zlayer.isolation: "vz-linux"` forces this
  runtime explicitly.
- **Opt out to libkrun:** label `com.zlayer.isolation: "vm"` (alias `libkrun`)
  forces the libkrun delegate even when the VZ Linux runtime is the default.

(The `com.zlayer.isolation: "vz"` label and the `com.zlayer.runtime=vz`
annotation still route to the **macOS-guest** runtime described above; only the
`-linux` variants reach `VzLinuxRuntime`.)

## The kernel artifact

The guest boots a ZLayer-built arm64 kernel `Image` plus an
`initramfs.cpio.gz`. They are produced by `images/vz-linux/build.sh`, which
cross-compiles an unmodified upstream LTS kernel (pinned by `KERNEL_VERSION`,
configured by `kernel-arm64.defconfig`) and an initramfs whose `/init` is the
static-musl `zlayer-vzagent` agent (bundled with a static busybox for the
`udhcpc` / `mount` fallback path). VZ's `VZLinuxBootLoader` requires the
**uncompressed** raw `Image`, not `Image.gz`.

> **The kernel must be built on Linux.** `build.sh` cross-compiles an arm64
> kernel + static-musl agent; it is not expected to run on the macOS developer
> host. The kernel is GPLv2 source-available — see `images/vz-linux/README.md`
> and `images/vz-linux/KERNEL_SOURCE.md` for the version, source URL, license,
> and exact reproduction steps. CI builds and publishes the artifacts via
> `.forgejo/workflows/vz-linux-images.yml` (opt-in: manual `workflow_dispatch`,
> or a `dev` push whose commit message carries the `[vz-build]` marker, since the
> cross-compile is slow). The workflow publishes `Image`, `initramfs.cpio.gz`,
> and `manifest.json` to a Forgejo generic package keyed by kernel version.

At runtime the host resolves the artifacts in this order (`ensure_linux_kernel`):

1. The dev-override env vars **`ZLAYER_VZ_LINUX_KERNEL`** and
   **`ZLAYER_VZ_LINUX_INITRD`** (both must be set and exist) — point these at a
   local build to iterate without republishing.
2. The on-disk cache **`{data_dir}/vz/linux/kernel/`**, containing the files
   `Image` and `initramfs.cpio.gz`.

If neither yields both artifacts, `start_container` fails with an actionable
error pointing at `images/vz-linux/build.sh`.

## Entitlement & code-signing

Booting any VZ VM requires the **`com.apple.security.virtualization`**
entitlement on a code-signed binary — the same requirement as the macOS-guest
path. It is already declared in `bin/zlayer/zlayer.entitlements`; ad-hoc sign the
binary for local use with `scripts/sign-vz.sh` (or `make build`, which auto-signs
on macOS). Without it, `VZVirtualMachine::isSupported()` returns false and
`start_container` fails with an actionable error.

The Linux-guest path uses VZ's built-in **NAT** networking, which does **not**
require the additional `com.apple.vm.networking` entitlement (that entitlement is
only needed for bridged/host networking attachments, which this runtime does not
use).

## vsock wire protocol

Host and guest speak a tiny length-prefixed protocol
(`zlayer_vzagent::proto`) over the `AF_VSOCK` control connection on port `1024`.
Each frame is `u32 LE length` + `u8 tag` + `postcard` payload. The message types:

| Tag | Message | Direction | Meaning |
|---|---|---|---|
| 1 | `Run { argv, env, cwd, uid, gid }` | host → guest | run the container entrypoint as the primary workload |
| 2 | `Exec { argv, env }` | host → guest | spawn an extra process in the workload's namespaces |
| 3 | `Signal { signum }` | host → guest | deliver a POSIX signal to the workload |
| 4 | `Stdout(Vec<u8>)` | guest → host | a chunk of workload stdout |
| 5 | `Stderr(Vec<u8>)` | guest → host | a chunk of workload stderr |
| 6 | `Started { pid }` | guest → host | the workload process was spawned |
| 7 | `Exited { code }` | guest → host | the process exited (`128 + signum` for signal kills) |
| 8 | `Error { message }` | either | transport- or operation-level error |

The framing helpers (`read_frame`/`write_frame`/`encode`/`decode`) are
codec-agnostic and have no Linux-only dependencies, so the host links the same
`proto` library it shares with the guest.

## Limitations

- **No live CPU/memory metrics.** `Virtualization.framework` exposes no per-VM
  resource accounting, so `get_container_stats` reports the **configured**
  allocation: `memory_limit = ram_mib * 1024 * 1024`, a deliberately non-zero
  `memory_bytes` estimate (a quarter of the limit), and `cpu_usage_usec = 0`.
- **No host-visible container PID.** A guest workload has no host PID, so
  `get_container_pid` returns `None`.
- **Real NAT, not host-socket sharing.** Unlike libkrun's TSI-style transparent
  host-socket sharing, this runtime uses VZ's real NAT networking — the guest
  gets its own `192.168.64.x` IP, directly host-reachable, with optional fixed
  host-port forwarders for stable `-p host:container` bindings.
