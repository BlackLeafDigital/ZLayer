# ZB-E scoping: macOS Linux-in-VM build path

> **Composite document.** Section A is the substrate decision investigation (June 2026). Section B is the original ZB-E scoping built on a libkrun substrate; the libkrun-substrate-specific content is superseded by Section A's verdict but VM-lifecycle and file-transfer detail there remains useful (those concerns are substrate-agnostic).

## Decision summary

- **Substrate: Virtualization.framework via `objc2-virtualization 0.3.2`** with an in-repo thin safe wrapper.
- **Tactical bridge**: keep `crates/zlayer-agent/src/runtimes/macos_vm.rs` libkrun lifecycle code in place while the VZ wrapper proves out via `MACOS_BUILDER_VM_SUBSTRATE=libkrun` fallback env var; after ~2 weeks of green VZ operation, rip libkrun lifecycle out in a separate cleanup pass.
- **Step plan**: see the master `ZLAYER_BUILDER.md` ZB-E section for the 14 agent-sized tasks. The libkrun-based 12-step plan in Section B below is superseded.

---

# Section A — macOS virtualization substrate investigation (June 2026)

Scope: read-only research. No edits, no code changes. Versions verified via `gh release list` / crates.io API on 2026-06-02; Apple's developer docs are JS-rendered SPAs so raw HTTP could not confirm deprecation banners — flagged inline where it matters.

---

## 1. Apple `container` and the `Containerization` Swift package

- Repo: https://github.com/apple/container — Swift, Apache-2.0, ~26.7k stars. Latest release **0.12.3 (2026-04-30)**; trajectory is fast: 0.6.0 (Oct 2025) → 0.12.3 (Apr 2026), with the early-2026 series (0.8 → 0.12) landing roughly monthly.
- Underlying library: https://github.com/apple/containerization (Swift, ~8.6k stars, latest **0.33.3-prerelease (2026-06-01)**). README states explicitly: *"Containerization is written in Swift and uses Virtualization.framework on Apple silicon."* It is NOT Hypervisor.framework. Apple silicon only; macOS 26 + Xcode 26 required for the package itself (`container` CLI ships prebuilt with a wider compat surface).
- Architecture per Containerization README: one lightweight Vz VM per container, optimized Linux kernel (≥6.14.9 tested), `vminitd` init speaking gRPC over **vsock**, ext4 rootfs blocks, per-container IP (no port forwarding), Rosetta 2 for linux/amd64 on ARM. Sub-second boot claimed.
- Image format: **standard OCI** — `ContainerizationOCI` module handles registry pull/push and image manipulation. Not Apple-proprietary.
- Rust consumability: **no.** It is a Swift Package consumed via SwiftPM; the public API surface is Swift classes (`LinuxContainer`, `LinuxProcess`, etc.). Crossing into Rust would mean either (a) shelling out to the `container` CLI, (b) bridging via a Swift→C ABI shim you write, or (c) bypassing it entirely and going straight to Virtualization.framework via objc2-virtualization.
- Verdict for ZLayer builder VM: **not a library option.** Useful as a *reference architecture* (the vminitd-over-vsock control plane is exactly the pattern you'd want), but you would not link it.

## 2. Virtualization.framework state and Rust bindings

- Apple-blessed path. Every Apple-authored Linux-on-macOS effort in 2025–2026 (`container`, `Containerization`, all WWDC 2025/2026 sample code) targets Virtualization.framework, not Hypervisor.framework. I could not load https://developer.apple.com/documentation/virtualization via plain HTTP to confirm a literal "preferred over Hypervisor" banner — **I don't know** the exact wording; find out by opening the page in a real browser or via Xcode docs. The behavioral evidence (Apple's own first-party container tooling using only Vz) is unambiguous regardless.
- Rust bindings — confirmed on crates.io 2026-06-02:
  - **`objc2-virtualization` 0.3.2** (madsmtm/objc2, 2025-10-04). Auto-generated from Apple headers by objc2's `header-translator`; tracks the framework's full Obj-C surface (`VZVirtualMachine`, `VZVirtualMachineConfiguration`, `VZLinuxBootLoader`, `VZVirtioFileSystemDeviceConfiguration`, `VZVirtioSocketDeviceConfiguration`, etc.). This is the canonical low-level binding.
  - **`arcbox-vz` 0.1.6** (2026-03-12) — "Safe Rust bindings for Apple's Virtualization.framework." Higher-level safe wrapper sitting on top of objc2.
  - **`vfrust` 0.1.0** (2026-04-07) — "Rust library for macOS Virtualization.framework VM management." Newer, smaller download count (58).
  - **`vmette` 0.2.0** (2026-06-01, chamuka-inc) — "Run untrusted agents in a hardware-isolated Linux microVM on macOS — a security boundary built on Apple's Virtualization.framework." Very fresh, agent-VM oriented.
  - Older `applevisor` 1.0.0 / `applevisor-sys` 1.0.0 (2026-01-13) are **Hypervisor.framework** bindings, not Vz — don't confuse them.
- Maturity: `objc2-virtualization` itself is solid (objc2 ecosystem is the de-facto Apple FFI standard and madsmtm pushes it aggressively). The *safe* wrappers (`arcbox-vz`, `vfrust`, `vmette`) are all <1.0, all 2026, all small. Expect to either pick one and contribute fixes, or build a thin safe wrapper of your own on `objc2-virtualization`.
- Send/Sync friction: VZ objects are Obj-C `id`s — `Retained<T>` from objc2 is not `Send` by default. You will end up either pinning the VM lifecycle to one thread or wrapping in `Mutex<SendWrapper<_>>`. This is the standard objc2 tax.
- Verdict for ZLayer builder VM: **strongest forward bet.** Same substrate Apple is investing in, real Rust bindings exist, and the feature set you need (virtio-vsock for control + virtio-fs for context staging + virtio-blk for image layers) is all first-class in VZ.

## 3. libkrun current state

- Latest: **libkrun 1.18.1 (2026-05-20)**. Active — 1.14.0 (Jun 2025) → 1.15 (Aug 2025) → 1.16 (Oct 2025) → 1.17.x (Q1 2026) → 1.18.x (Q2 2026). Releases every 1–3 months.
- README (current `main`) verbatim: *"libkrun is a dynamic library that allows programs to easily acquire the ability to run processes in a partially isolated environment using KVM Virtualization on Linux and **HVF on macOS/ARM64**."* HVF = Hypervisor.framework. **libkrun has NOT adopted Virtualization.framework as a backend** — it is its own VMM that talks the HVF syscalls directly. That's the whole point of libkrun (minimal VMM, no Apple-framework dependency on the device-model side).
- Feature set still strong: virtio-vsock + TSI (the unique selling point — no virtual NIC needed, sockets transparently proxied), virtio-fs, virtio-gpu (Venus + native-context), virtio-blk, virtio-net w/ passt/gvproxy. SEV/TDX variants for confidential compute on Linux.
- Rust bindings: **no published `krun-sys` crate on crates.io** (confirmed; the name returns nothing). Today's ZLayer uses `dlopen` against the libkrun C ABI from `crates/zlayer-agent/src/runtimes/macos_vm.rs` — that's the standard integration pattern (crun, krunkit, muvm all do roughly the same in C/C++).
- Verdict for ZLayer builder VM: **still viable, especially because you already ship it.** But you are pinned to Hypervisor.framework long-term, and Apple's own tooling diverging onto Vz is a slow-burning compatibility / mindshare risk (kernel-feature gaps, virtio-fs perf differences, Rosetta 2 integration shortcuts, future macOS versions deprioritizing HVF improvements).

## 4. vfkit / krunvm

- **vfkit 0.6.3 (2026-01-09)**, crc-org. Go CLI. *"Command-line tool to start VMs on macOS … using the macOS Virtualization framework."* Used by minikube ≥1.35, Podman ≥5, crc, ovm. So it's the de-facto "Vz-based microVM launcher" before Apple's own `container` showed up. Architecture: Go binary wraps Vz via the Code-Hex/vz Go bindings; you drive it via CLI args + a REST API on a Unix socket; not a Rust library. Same caveat as `container` — you'd shell out, not link.
- **krunvm 0.2.6 (2026-02-09)**, containers org. Thin CLI on top of libkrun + buildah for "run an OCI image as a microVM." Shares libkrun's HVF-on-macOS limitation. Useful as a *prior-art* reference for the exact "OCI image → builder VM" path you're scoping. Not library-consumable from Rust beyond what libkrun already gives you.
- Verdict: neither is a substrate choice — they're *peers* to what ZLayer would build. vfkit is the closest analogue if you go Vz; krunvm is the closest if you stay on libkrun.

## 5. Lima / Colima / OrbStack substrate

- **Lima 2.1.2 (2026-06-01)** — pluggable. Drivers: `vz` (Virtualization.framework, default on macOS 13+), `qemu` (fallback / Intel), and experimental `wsl2`. Trajectory has been Vz-first for ~2 years.
- **Colima 0.10.1 (2026-02-22)** — wraps Lima; inherits Lima's `vz` default.
- **OrbStack** — closed source, not in this list. Public statements have been Virtualization.framework + heavy custom kernel/networking work; **I don't know** their current internals as of 2026-06; find out by reading OrbStack's blog or asking their team — not relevant as a substrate for ZLayer anyway since it's not consumable.
- Verdict: the entire desktop-runtime ecosystem on macOS has converged on Vz. libkrun is the odd one out, and intentionally so (it serves Linux-on-Linux primarily, with macOS as a secondary target).

## 6. Apple's deprecation stance

- No public deprecation of Hypervisor.framework that I can confirm via the tools I have. The framework still ships, still gets bugfixes in macOS point releases, and `applevisor`-style bindings still work.
- Direction of investment, however, is unambiguous: every new Apple sample, framework feature (Rosetta-for-Linux, paravirtualized graphics, nested virt on M3+, signed-disk-image attestation), and first-party tool (`container`, `Containerization`) lands on Vz, not HVF. HVF is the "raw KVM-equivalent" layer; Vz is the productized layer Apple wants third parties on.
- **I don't know** whether Apple has signaled deprecation in a WWDC 2026 session or a header comment — find out by grepping the macOS 26 SDK headers (`hv.h`, `hv_vm.h`) for `__API_DEPRECATED` and by checking WWDC 2026 platform-state-of-the-union notes.

## 7. Best fit for ZLayer's builder VM (vsock control + virtio-fs context)

Decision matrix:

| Substrate | vsock | virtio-fs | Rust lib | Lifecycle ergo | macOS-forward | Send/Sync |
|---|---|---|---|---|---|---|
| libkrun (current) | yes (+ TSI) | yes | C via dlopen (existing ZLayer path) | simple C API, single-process | declining (HVF only) | wrap C handle in `Send` newtype — easy |
| objc2-virtualization (raw) | yes (`VZVirtioSocketDevice`) | yes (`VZVirtioFileSystemDevice`) | yes (canonical) | Obj-C lifecycle, retain/release | strongest | `Retained<T>` not Send — real friction |
| arcbox-vz / vfrust / vmette | yes (via VZ) | yes (via VZ) | yes (safe wrappers) | better than raw objc2 | strongest | wrapper-defined; varies |
| apple/container CLI | yes | yes | shell-out only | subprocess + JSON | strongest | n/a (process boundary) |
| vfkit CLI | yes | yes | shell-out only | subprocess + REST UDS | strongest | n/a |

Concrete trade-offs:

- **Stay on libkrun**: zero migration cost, you already have `dlopen` plumbing in `macos_vm.rs`, TSI gives you working networking with no `passt` sidecar, GPU forwarding is there for free if you ever want it. You lose: alignment with Apple's direction, easy Rosetta-for-Linux, and you carry the libkrun bundled-kernel + libkrunfw blob (extra ~10–20MB) in the macOS distribution.
- **Move to objc2-virtualization directly**: maximum control, no shell-out, no extra process, futureproof. Costs: a real chunk of Obj-C-binding work, you ship your own Linux kernel build (Kata's `vmlinux.container` is a sensible default — Apple's own Containerization README recommends it), you write the vsock-init agent yourself (analogous to Apple's `vminitd` but in Rust), and you eat the `Retained<T>: !Send` ergonomics tax.
- **Move to objc2-virtualization via a safe wrapper** (arcbox-vz, vfrust, or vmette): cheaper than raw, but all three are sub-1.0 single-author crates as of 2026-06; commit-bus-factor risk. Read each one's source before adopting; expect to upstream patches.
- **Shell out to `container` or vfkit**: fastest to ship, but adds a runtime dependency the user must install, and you give up programmatic control over context-transfer (virtio-fs share lifecycles, snapshotting, etc.). Wrong fit for a builder daemon that wants tight feedback loops.

Recommendation, ranked:

1. **Strategic: build on `objc2-virtualization` with a thin in-repo safe wrapper** (own it, don't depend on a sub-1.0 third-party wrapper). Reuse Kata's `vmlinux.container` and write a small Rust vsock-init. This is the "where Apple is going" answer and matches the architecture Apple validated in `Containerization`.
2. **Tactical bridge: keep libkrun for the builder VM right now**, file a tracking issue to migrate, and start the wrapper work in parallel. libkrun 1.18.x is healthy; nothing urgent forces a move this quarter.
3. **Do not** adopt arcbox-vz / vfrust / vmette as load-bearing deps without reading their source and committing to vendor/fork if needed.
4. **Do not** shell out to apple/container or vfkit for the builder path — wrong abstraction for a daemon that needs to stream context and harvest layers programmatically.

### Things explicitly not known

- Exact wording of any Apple deprecation banner on Hypervisor.framework — Apple's docs are JS-rendered and could not load via `curl`. Confirm in Xcode 26 docs viewer or `hv.h`.
- OrbStack's internal substrate as of 2026-06.
- Whether libkrun has any unmerged-but-active branch experimenting with a VZ backend (didn't grep the repo's branches/PRs). Worth a `gh pr list -R containers/libkrun --search 'virtualization'` if it matters.

---

# Section B — Original libkrun-based ZB-E scoping (SUPERSEDED for substrate; VM-lifecycle detail still applicable)

> The substrate decision has moved to Virtualization.framework (Section A). The libkrun symbol additions in step 1 are obsolete; the rest of the lifecycle, file-transfer, and agent-binary design is substrate-agnostic and informs the VZ-based plan in `ZLAYER_BUILDER.md` ZB-E.


> Naming caveat: the CHANGELOG already uses "Phase E" for the Windows HCN-overlay work (`CHANGELOG.md:1242`) and "Phase F" for the composite Windows runtime (`CHANGELOG.md:1238`). This scoping document inherits the letter from the user's brief; before merge we should rename to something unambiguous (e.g. **Phase M-macOS-VM-Builder** or **Phase OCI-2-macOS**) to avoid collision with the shipped phases. Flagged as an open question below.

### Goals

- On macOS hosts, `zlayer build` targeting a Linux image must produce real OCI images (manifest + config + gzipped layer tarballs) without requiring `buildah`, Docker Desktop, or any non-Apple-distributed dependency at runtime beyond a one-time install of the VM helper.
- Reuse the existing `crates/zlayer-agent/src/runtimes/macos_vm.rs:1-1626` libkrun VM lifecycle code as the substrate, rather than introducing a parallel `Virtualization.framework` lifecycle path. (See *Risks* — the user brief assumed Virtualization.framework, but the codebase has already standardised on libkrun.)
- Keep `crates/zlayer-builder/src/sandbox_builder.rs:1-2706` as the runtime-only path for macOS-native (non-OCI) builds; document that emitting OCI layers from sandbox-exec is deferred.
- Warm VM reused across builds; idle-shutdown after configurable timeout; clean teardown on `zlayer daemon stop`.
- Honest assessment up front: libkrun's `krun_start_enter` is a blocking, single-shot entrypoint with no exec channel (`crates/zlayer-agent/src/runtimes/macos_vm.rs:40,1312,1330`). A "warm builder VM with an in-VM `zlayer-builder` daemon" requires either (a) adding vsock + a guest agent listener that owns the build loop, or (b) accepting one-shot VMs per build. We pick (a) explicitly and call out the libkrun symbols we need to add to the FFI wrapper.

### Steps

Each step is one agent-sized task, ≤ 2 files, ≤ ~200 LOC, with stated effort in hours.

1. **Extend libkrun FFI wrapper with vsock + virtio-fs symbols** (3h)
   - Add `krun_add_vsock_port`, `krun_add_virtiofs_volume` (and any missing `krun_set_passt_fd` if we end up needing real networking) to `LibKrun` in `crates/zlayer-agent/src/runtimes/macos_vm.rs:125-149,164-233`. libkrun 1.18.1 (`gh release list -R containers/libkrun`) exposes these. Stop short of using them; just expose the symbols + safe `unsafe fn` wrappers.
   - Files: `crates/zlayer-agent/src/runtimes/macos_vm.rs` only.

2. **New `BuilderVm` lifecycle type** (4h)
   - New module `crates/zlayer-agent/src/runtimes/macos_builder_vm.rs` owning a long-lived VM separate from the per-container `VmContainer` map in `macos_vm.rs:241-266`. State: `Idle | Booting | Ready { vsock_port, last_used } | ShuttingDown`. Spawn/teardown methods only — no build logic yet.
   - Files: new file + 1-line `pub mod` add in `crates/zlayer-agent/src/runtimes/mod.rs`.

3. **Idle timeout + graceful shutdown wiring** (3h)
   - Background `tokio::task` polls `last_used`; if older than `ZLAYER_BUILDER_VM_IDLE_SECS` (default 600), tear the VM down. Hook teardown into the daemon-stop path in `bin/zlayer/src/commands/daemon.rs` (find via grep — listed under "Files to modify").
   - Files: `crates/zlayer-agent/src/runtimes/macos_builder_vm.rs`, `bin/zlayer/src/commands/daemon.rs`.

4. **Guest-side builder agent binary** (6h, 2 files)
   - New `bin/zlayer-builder-agent` (NOT in workspace? — verify against `bin/zlayer-desktop` precedent which is excluded; `bin/zlayer/src/cli.rs:180` already mentions VM runtime). It's a small Rust binary that runs inside the VM, listens on vsock, and on each request shells out to `youki` + Rust OCI-layer code from Phase A. For *this* phase we only stub: bind vsock, accept JSON request `{op: "ping"}`, reply `{ok: true}`, log to stderr.
   - Files: `bin/zlayer-builder-agent/Cargo.toml`, `bin/zlayer-builder-agent/src/main.rs`.

5. **Guest rootfs assembly** (4h)
   - Build a minimal Alpine/Debian-based rootfs tar that contains the guest agent binary + busybox + `/sbin/init` (a tiny shell script that starts the agent). Ship as a build-time artifact under `images/zlayer-builder-vm/`. Use the existing ZImagefile/buildah pipeline on Linux CI to produce it (no macOS-host bootstrap required). On macOS host, ship as part of the `zlayer` macOS bottle / installer payload, extracted to `~/.zlayer/builder-vm/rootfs/`.
   - Files: `images/zlayer-builder-vm/ZImagefile`, `crates/zlayer-paths/src/lib.rs` (add `default_builder_vm_rootfs_path()`).

6. **Host-side vsock client** (3h)
   - On macOS, libkrun exposes vsock as a Unix socket on the host (per vfkit docs at https://github.com/crc-org/vfkit/blob/main/doc/usage.md — "macOS does not have host support for `AF_VSOCK` sockets so the vsock port will be exposed as a unix socket on the host"). Client: `tokio::net::UnixStream` framed with length-prefixed JSON. Implement `BuilderAgentClient::{ping, build}` skeleton.
   - Files: new `crates/zlayer-builder/src/backend/macos_vm_client.rs`.

7. **`LinuxInVmBackend` implementing `BuildBackend`** (5h)
   - New module `crates/zlayer-builder/src/backend/linux_in_vm.rs`. Implements all 7 trait methods (`build_image`, `push_image`, `tag_image`, `manifest_create`, `manifest_add`, `manifest_push`, `is_available`, `name`) from `crates/zlayer-builder/src/backend/mod.rs:87-124`. For this phase, `push_image` / `manifest_*` proxy to the in-VM agent which calls into the Phase A executor; `build_image` is the meaty one. Use `BuilderAgentClient` from step 6.
   - Files: `crates/zlayer-builder/src/backend/linux_in_vm.rs`, `crates/zlayer-builder/src/backend/mod.rs` (add `#[cfg(target_os = "macos")] pub mod linux_in_vm;` + `pub use`).

8. **`detect_backend` routing change** (2h)
   - Replace the current "buildah-if-present-else-sandbox" macOS branch (`crates/zlayer-builder/src/backend/mod.rs:181-200`) with: `target_os == Linux` → `LinuxInVmBackend` (unconditional, since Phase A's executor lives in-VM); `target_os == Windows` → unchanged error; macOS-native sandbox is now only picked when `ZLAYER_BACKEND=sandbox` is forced or when the user has explicitly requested a macOS image (we need a new `ImageOs::MacOS` variant — see step 9).
   - Files: `crates/zlayer-builder/src/backend/mod.rs` only.

9. **`ImageOs::MacOS` variant + routing** (2h)
   - Add `MacOS` to the `ImageOs` enum (`crates/zlayer-builder/src/backend/mod.rs:51-80`) so the routing table can distinguish "build a Linux image on macOS host (→ VM)" from "build a macOS image on macOS host (→ sandbox, non-OCI)". Update the FromStr table, the parse test (`crates/zlayer-builder/src/backend/mod.rs:218-233`), and the routing match in `detect_backend`. Linux + Windows hosts return an error for `ImageOs::MacOS`.
   - Files: `crates/zlayer-builder/src/backend/mod.rs`.

10. **Build-context transfer over virtio-fs** (5h)
    - The build context is potentially many MB. JSON-over-vsock is wrong; mount the host's context dir read-only into the guest at `/zlayer/context`. Use the `krun_add_virtiofs_volume` symbol added in step 1. Output layer staging is the reverse: the guest writes layer tarballs to a host-writable virtio-fs mount at `/zlayer/out`, host reads them out for registry push.
    - Per perplexity research (https://github.com/crc-org/vfkit/blob/main/doc/usage.md and the FOSDEM 2023 vfkit slides at https://archive.fosdem.org/2023/schedule/event/govfkit/), virtio-fs is the documented, reliable choice on Apple's Virtualization.framework. libkrun also exposes virtio-fs natively. 9p is not supported by Apple's framework. NFS would be a kludge over a virtio-net interface. **Decision: virtio-fs.**
    - Files: `crates/zlayer-builder/src/backend/linux_in_vm.rs`, `crates/zlayer-agent/src/runtimes/macos_builder_vm.rs` (mount setup in `BuilderVm::start`).

11. **End-to-end smoke test (gated `#[ignore]`)** (3h)
    - `crates/zlayer-builder/tests/macos_vm_build_e2e.rs`. Boots the builder VM, calls `ping`, then runs a trivial `FROM scratch / COPY hello /hello` build, asserts the output OCI image manifest is valid and the layer contains the file. `#[ignore]`-gated; documented in the test file's module comment that it requires `ZLAYER_E2E=1` + libkrun installed.
    - Files: 1 new test file.

12. **Honest docs + CHANGELOG entry** (1h)
    - Append `## [Unreleased]` entry to `CHANGELOG.md` (per the global rule: never invent a version number). Document: new backend, virtio-fs context transfer, vsock control channel, idle-shutdown behavior, the deferred macOS-native OCI emission, and the libkrun (not Virtualization.framework) substrate.
    - Files: `CHANGELOG.md`.

Total estimate: **~41 hours of focused work** = ~1 person-week if it goes smoothly, more realistically **2 person-weeks** with libkrun symbol surprises and virtio-fs quirks (see *Risks*).

### VM lifecycle decisions

- **Warm VM, one shared builder.** A single long-lived builder VM is started lazily on the first Linux build request; subsequent builds reuse it. The per-container `VmRuntime` (`macos_vm.rs:291-305`) is **not** reused — that one creates one VM per container and is purpose-built for runtime, not build, workloads. A separate `BuilderVm` keeps lifetimes disentangled.
- **Idle shutdown.** `ZLAYER_BUILDER_VM_IDLE_SECS` (default 600s). Configurable via daemon config later.
- **Hard shutdown on daemon stop.** Builder VM teardown hooks into the existing daemon-stop path. libkrun's `krun_start_enter` blocks the thread it runs on; we kill via `pthread_kill` of `SIGTERM` to the worker thread (same pattern `VmContainer::vm_thread` uses at `macos_vm.rs:259`).
- **Single builder, no concurrent builds inside one VM (for now).** Concurrent builds within a single VM require multi-tenant isolation work (cgroups, separate context mounts) that's a v2 concern. For v1, serialize build requests at the `LinuxInVmBackend` layer via a `tokio::sync::Mutex`.
- **No Intel-Mac path in this phase.** libkrun on Intel works (per the existing `macos_vm.rs:31` comment), but virtio-fs + Rosetta + arm64-emulation cross-arch builds are a separate problem. Document Intel as "Linux/amd64 images only, no Rosetta arm64 cross-build" in the user docs.

### File transfer mechanism choice

**Choice: virtio-fs for build context + output layers, vsock for control plane.**

| Mechanism | Status on libkrun/Apple Virt.framework | Verdict |
|-----------|----------------------------------------|---------|
| virtio-fs | First-class in Apple framework; libkrun supports `krun_add_virtiofs_volume`; vfkit production usage documented at https://github.com/crc-org/vfkit/blob/main/doc/usage.md | **Picked** for bulk data |
| 9p        | Not supported by Apple's framework (vfkit slides FOSDEM 2023 confirm only virtio-{net,blk,fs,vsock,serial,rng,balloon}) | Not viable |
| vsock     | Supported; exposed on macOS host as Unix socket (vfkit docs, citation above) | **Picked** for JSON control RPC |
| NFS-over-virtio-net | Works but kludgy, adds an in-VM nfsd | Avoided |
| TCP API over virtio-net | Would require allocating an IP and routing; libkrun's TSI (`macos_vm.rs:286-290,108-110`) gives transparent socket impersonation but is per-process inside the guest, not for the builder daemon | Avoided |

Caveat I want to flag honestly: I have **not personally verified** that libkrun 1.18.1's virtio-fs path is stable for read+write workloads with macOS-side concurrent reads. Perplexity citations cover vfkit/Virtualization.framework, not libkrun specifically. Step 10 should start with a 30-minute spike where we mount a host dir, write 100 small files from the guest, and read them concurrently from the host. If that fails, fall back to vsock-streamed tar archives (slower but proven).

### Files to create

- `crates/zlayer-agent/src/runtimes/macos_builder_vm.rs` — builder VM lifecycle (step 2).
- `crates/zlayer-builder/src/backend/linux_in_vm.rs` — `BuildBackend` impl (step 7).
- `crates/zlayer-builder/src/backend/macos_vm_client.rs` — vsock RPC client (step 6).
- `bin/zlayer-builder-agent/Cargo.toml`, `bin/zlayer-builder-agent/src/main.rs` — guest agent (step 4).
- `images/zlayer-builder-vm/ZImagefile` — guest rootfs build recipe (step 5).
- `crates/zlayer-builder/tests/macos_vm_build_e2e.rs` — e2e smoke test (step 11).

### Files to modify

- `crates/zlayer-agent/src/runtimes/macos_vm.rs:74-233` — extend FFI wrapper (step 1).
- `crates/zlayer-agent/src/runtimes/mod.rs` — add `pub mod macos_builder_vm;` (step 2).
- `crates/zlayer-builder/src/backend/mod.rs:24-216` — add module decl, `ImageOs::MacOS`, routing changes (steps 7–9).
- `crates/zlayer-paths/src/lib.rs` — `default_builder_vm_rootfs_path()` (step 5). [Mandated by user memory: derived-path helpers live on `ZLayerDirs`.]
- `bin/zlayer/src/commands/daemon.rs` — shutdown hook (step 3).
- `CHANGELOG.md` — `[Unreleased]` entry (step 12).

### Tests

- **Unit:** `BuilderVm` state machine transitions; `BuilderAgentClient` framing; `ImageOs::MacOS` FromStr + routing matrix.
- **Integration (`#[ignore]`, gated on `ZLAYER_E2E=1` + macOS host):** boot builder VM, ping, build trivial image, validate manifest.
- **Workspace checks:** `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo check --workspace` must all be green per the project's `CLAUDE.md` "ALL CHECKS GREEN" rule. The macOS-gated modules need to compile-test under `--target x86_64-apple-darwin` and `aarch64-apple-darwin` via the existing `check-macos` CI job (`CHANGELOG.md:1238` confirms it exists).

### Risks

1. **Virtualization.framework vs libkrun mismatch with the brief.** The user brief assumed Virtualization.framework via `objc2-virtualization`. The repo has already standardised on libkrun (`macos_vm.rs:1-8`). `objc2-virtualization` exists at https://github.com/madsmtm/objc2/tree/main/framework-crates/objc2-virtualization but is auto-generated bindings — Send/Sync and delegate handling are still rough, and we'd be doing a second VM-lifecycle implementation parallel to libkrun. **Recommend: stay on libkrun for now, document the choice, leave the door open to swap.** This needs user sign-off (see open questions).
2. **libkrun no graceful shutdown.** `krun_start_enter` blocks until the entrypoint exits (`macos_vm.rs:42`). Cancelling = killing the worker thread. Acceptable for a builder VM (worst case: stranded virtio-fs mount, cleaned up at next daemon start) but not pretty. Mitigation: the guest agent listens for a "shutdown" vsock message and `exit 0`s the entrypoint, letting `krun_start_enter` return cleanly.
3. **virtio-fs concurrency on libkrun unverified.** See file-transfer section. Spike before committing.
4. **Apple's `container` tool overlap.** https://github.com/apple/container exists (Apache-2.0, currently at 0.12.3 per `gh release list -R apple/container --limit 3`). It builds OCI images on macOS via Virtualization.framework. Using it as a library is **not** feasible (it's a Swift application, no FFI surface, no published library crate). Borrowing its design ideas is fine, but we are not vendoring or shelling out to it. Documenting the comparison so reviewers don't ask.
5. **Build-time-only `youki` dependency in guest rootfs.** The guest agent shells to `youki` for the OCI runtime portion of the build (RUN-step exec). `youki` is a Rust binary already used elsewhere in this codebase — fine. But the guest rootfs needs to bundle it; step 5 should pin its version via a normal `cargo install` or release-asset pull, not by hand-editing manifests.
6. **Deferred: macOS-native OCI emission.** Explicitly out of scope. Tracked in CHANGELOG as "future work: emit OCI layers from `SandboxImageBuilder` for `ImageOs::MacOS` targets so users can ship macOS-native containers to other macOS hosts." No `#[allow(dead_code)]` placeholders, no stub functions — just documentation.

### Open questions for the user

1. **Phase letter collision.** "Phase E" already shipped (Windows HCN overlay, `CHANGELOG.md:1242`). Rename to **Phase OCI-2-macOS** or similar? Need your call.
2. **Virtualization.framework vs libkrun.** Brief assumed Virtualization.framework, repo uses libkrun. Stay on libkrun (recommended) or build a parallel Virtualization.framework path?
3. **Intel Mac support.** This phase ships arm64-only. Acceptable? (Intel libkrun works but Rosetta cross-build for arm64 Linux images is a separate workstream.)
4. **Builder VM concurrency.** v1 serialises builds inside the VM via a Mutex. Acceptable, or do you want concurrent in-VM builds (significantly more work — multi-tenant cgroup + mount isolation)?
5. **Guest agent distribution.** Ship the prebuilt rootfs tar as part of the `zlayer` macOS installer, or download on first use (signed, checksummed)? First is faster but bloats the installer; second adds first-build latency + a trust-on-first-download concern.

### Verification

Per the project rule (`CLAUDE.md`: "ALL CHECKS GREEN — ENTIRE WORKSPACE"), every step ends with `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo check --workspace`. On macOS hosts, additionally `cargo check --workspace --target aarch64-apple-darwin`. The `#[ignore]` e2e test is the manual smoke-test gate.

Honest verification limit: I cannot run the e2e on this Linux host. The macOS check needs to happen on the existing macOS CI runner or on a developer's Apple Silicon box before merge. No way around that — flagging it explicitly rather than pretending CI alone will catch a libkrun virtio-fs regression.

### Estimated effort

- **Steps total:** ~41 hours focused.
- **Calendar:** 1.5–2 person-weeks with normal interruptions, libkrun-symbol spelunking, and the virtio-fs spike.
- **Confidence:** Medium. The two unknowns are (a) libkrun virtio-fs read+write concurrency, and (b) how cleanly we can kill the blocking `krun_start_enter` thread on shutdown. Both are addressable but could each add a day if they misbehave.
- **Risk class vs. prior estimate:** The original "2-3 person-weeks, medium risk" estimate stands. Drops to the lower end (~1.5 weeks) if we accept the libkrun substrate (no `objc2-virtualization` adoption); rises toward the upper end if the user wants a parallel Virtualization.framework path.
