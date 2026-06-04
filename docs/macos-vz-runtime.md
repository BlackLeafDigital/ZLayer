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

VZ is **never** chosen by `Auto`. Opt in either way:

- **Whole node:** `zlayer --runtime mac-vz …` (a standalone `VzRuntime`).
- **Per service:** under the default composite runtime, label the service
  `com.zlayer.isolation: "vz"`. The composite's `select_for` routes only those
  services to the VZ delegate; everything else is unaffected.

## How it works

Each "container" is a macOS guest VM cloned from a base image bundle:

- **Base bundle** (`{data_dir}/vz/images/{image}/`): `disk.img`,
  `hardware-model.bin` (a `VZMacHardwareModel` `dataRepresentation`), and
  `aux.img`. Provide one by:
  - `pull_image` of an OCI artifact whose layers unpack to those three files
    (Tart-style), or
  - pointing the image reference at a local directory containing them, or
  - restoring from a macOS `.ipsw` (canonical `VZMacOSInstaller` flow).
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
