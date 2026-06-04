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
