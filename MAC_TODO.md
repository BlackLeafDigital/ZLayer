# MAC_TODO — macOS work for the Mac agent

Two tracks: **(A) rootless daemon install** + **(B) the Apple-Virtualization (VZ) runtime**. Do both
exhaustively. NO stubs/`todo!()`/deferrals. After EVERY change: `cargo fmt --all` → `cargo clippy
--workspace --all-targets -- -D warnings` → `cargo test --workspace` → `cargo build --workspace`, all
GREEN (never `-p`). Mirror shared-crate changes to `/Users/zach/GitHub/zlayer-zql` + bump its
CHANGELOG. Update `CHANGELOG.md` (real next version; currently 0.52.6 — use 0.52.7+). NEVER hand-edit
Cargo.toml — use `cargo add`.

---

## STATUS (updated) — Track A DONE, Track B foundation in; VZ runtime is the remaining build

Local commits on `dev` (NOT pushed):
- `54c2fbf6` — RAFT: opt-in affinity placement + spec propagation (cluster_scaling/cluster_upgrade).
- `a3f75df5` — **Track A macOS rootless install — VERIFIED LIVE** (gui/$uid Agent, no GroupName,
  health 200, PPID=1/survives shell, overlay change-gate skips w/ no sudo when unchanged, dual-domain
  uninstall, EX_CONFIG/exit-78 diagnostics, socket-probe fix, install-dev.sh full-`~/.zlayer` reclaim).
- `e79acf9e` — **Track A Linux rootless** (`systemctl --user` unit, rootless youki + Delegate=yes,
  overlay change-gate, `loginctl enable-linger` tip). **cfg(linux) TYPE-checked only by Linux CI** —
  macOS can't compile libseccomp, only syntax/clippy. Windows = concrete SCM-admin blocker (documented).
- `686f395b` — Track B deps added.
- `9696f6d4` — **Track B DONE: Apple-Virtualization (VZ) runtime.** Full `Runtime` impl in
  `crates/zlayer-agent/src/runtimes/macos_vz.rs` (~1100 lines) via objc2-virtualization: VM config +
  lifecycle on a serial DispatchQueue, block2→channel completion bridging, SSH exec, dhcpd-lease IP,
  pause/resume, 2-VM AtomicU32 gate. Integration: RuntimeType::MacVz (cli), RuntimeConfig::MacVz
  (lib + create_runtime), config.rs map, CompositeRuntime::with_vz_delegate + `com.zlayer.isolation=vz`
  routing. 7 helper unit tests pass; VM-boot test `#[ignore]` (needs the
  com.apple.security.virtualization entitlement + a base bundle). `docs/macos-vz-runtime.md`. CHANGELOG
  0.52.10. fmt+clippy+test+build green (one flaky network sandbox-build e2e in zlayer-builder, passes in
  isolation — unrelated).

**ALL RAFT + MAC WORK DONE locally. Remaining = housekeeping:** mirror Track A + Track B into
zlayer-zql (daemon.rs/cli.rs/config.rs/composite.rs/lib.rs/macos_vz.rs/install-dev.sh; zql can't compile
locally — private `zql` registry dep) + bump zql CHANGELOG; then push + dispatch raft-e2e via the
forgejo-actions sidecar (verifies RAFT 5/5 AND that the Track A Linux + Track B cfg blocks type-check on
CI). Windows daemon stays admin-required (concrete SCM blocker).

## ALREADY DONE THIS SESSION (do not redo — context only)
- **Daemon startup DEADLOCK fixed (`cfd18bff`, pushed to dev, mirrored to zql).** `zlayer serve` never
  bound its API listener when stderr is non-TTY (launchd/CI). Cause: `51383c54` moved the observability
  console layer stdout→stderr, colliding with `serve`'s `install_stderr_redirect_to_tracing` (dup2 fd 2
  → pipe → re-emit as `tracing::error!` → feedback loop → deadlock on the global stderr mutex). Fix in
  `crates/zlayer-observability/src/logging.rs`: daemon arms (file-writer present == `serve`) route
  console to **stdout** (disjoint from the fd-2 capture); CLI arms keep stderr (clean `ps --format json`
  stdout). Verified: serve → `/health/ready` in ~4s (was never).
- **overlayd dial bounded** (`OverlaydClient::connect_with_attempts`, no trailing sleep; OverlayManager
  uses 6 attempts ~2.5s; `setup_global_overlay` connects once up front) so a dead/missing overlayd
  degrades fast instead of stalling ~35s. Same commit.
- **The local machine was already unblocked manually** (booted out both services, removed both plists,
  `chown -R zach ~/.zlayer`, removed stale root-owned socket/log). So the daemon now STARTS even as the
  (still-wrong) root system daemon.

---

## TRACK A — rootless daemon install, CROSS-PLATFORM (macOS + Linux + Windows)

> **USER REQUIREMENT (explicit):** the rootless install must apply to **Linux and Windows too** — if
> the overlay does NOT need to be updated, the install must NOT require root, on every platform. Root is
> acquired ONLY when the overlay (or another component that genuinely needs root) must change. This is
> the SAME `bin/zlayer/src/commands/daemon.rs` file (three per-OS `install()`/`uninstall()`/
> `install_overlayd_service()` blocks: macOS ~887/1015, Linux ~1741/2195, Windows ~2999/3141), so do all
> three OS arms here. The Windows agent owns HCS; THIS track owns the install/privilege rework for all
> platforms.
>
> **Linux caveat to INVESTIGATE (don't fake it):** the Linux main daemon historically runs containers
> via youki + cgroups/namespaces and installs a **system** systemd unit (`/etc/systemd/system`) +
> copies the binary to `/usr/local/bin` — all of which need root independent of the overlay. To be
> genuinely rootless when the overlay is unchanged you must determine whether ZLayer supports a **rootless**
> Linux daemon (rootless youki / user namespaces + cgroup-v2 delegation + a `systemctl --user` unit in
> `~/.config/systemd/user` + data under `~/.zlayer`). If it does → make that the rootless path and
> elevate only for the overlay-change step (and for the one-time system bits if the user opts into a
> system install). If the container runtime genuinely cannot run without root and rootless-youki is not
> supported, SURFACE that as the concrete blocker (per the user's rule) rather than pretending — but
> first verify against the codebase (`crates/zlayer-agent` runtime init, `zlayer daemon install` Linux
> arm, any existing rootless/uid-map handling). Windows: determine whether a per-user service is
> feasible vs the SCM `LocalSystem` requirement; same change-gate principle.

### The architecture (the user was EXPLICIT — get this right; applies to ALL platforms)
- The main daemon writes only to `~/.zlayer` (user-owned) + a user socket → it MUST install as a
  **rootless per-user launchd Agent** (`gui/$uid`, `~/Library/LaunchAgents`), never as root.
- **The overlay (`zlayer-overlayd`) is the ONLY thing that needs root** (it owns the WireGuard/utun
  adapter, a system resource → `/Library/LaunchDaemons`, root).
- **Acquire root ONLY when the overlay has CHANGED, and ONLY for the overlay step.** If the overlay is
  unchanged, do NOT touch it and do NOT prompt for root. A daemon-only upgrade → zero sudo prompts.
  > THIS IS THE PART THE PREVIOUS ATTEMPT GOT WRONG: it skipped overlay whenever non-root. WRONG. The
  > gate is **"did the overlay change?"**, NOT "are we root?". If unchanged → skip (no root). If
  > changed → that step needs root → ask for it (surgical sudo).

### The "did the overlay change?" gate (ALL non-root reads)
Implement `overlayd_service_is_current(data_dir, staged_overlayd_bin) -> bool`:
1. Installed plist `/Library/LaunchDaemons/com.zlayer.overlayd.plist` (world-readable) string-equals the
   freshly-rendered plist; AND
2. `sha256(staged zlayer-overlayd binary) == sha256(installed binary)` (the path in the installed
   plist's `ProgramArguments[0]`); AND
3. service is loaded: `launchctl print system/com.zlayer.overlayd` exits 0 (works read-only).
All three true → overlay **unchanged** → SKIP, no root. Any false → overlay changed/missing → elevate
JUST the overlayd step (a dedicated internal subcommand re-exec'd under `sudo`, e.g. `sudo zlayer daemon
_install-overlayd --data-dir … --overlayd-bin …`, OR sudo-wrapped plist write + `launchctl bootstrap
system`) and prompt: "overlay networking needs root to register its system service". Persist
`{binary_sha256, plist_sha256}` to a **user-owned** `~/.zlayer/overlayd-service.json` as a fast-path
cache (on-disk plist+binary remain source of truth). `--version` is useless (`zlayer-overlayd --version`
returns constant `0.0.0-dev`) — the binary sha is the real identity. cargo is content-addressed, so a
main-daemon-only rebuild leaves `target/*/zlayer-overlayd` byte-identical → sha matches → no prompt.

### Code changes (`bin/zlayer/src/commands/daemon.rs` unless noted)
1. **Dispatch elevation** (`handle_daemon`, ~198-221): REMOVE the blanket up-front
   `ensure_root_or_reexec` for Install/Uninstall/Start/Stop/Restart/Reset/Migrate on ALL platforms. The
   daemon-management actions must run as the user by default; root is acquired LATER and SURGICALLY,
   only when the change-gate (below) says a root-needing component (the overlay, or — only if the user
   opts into a system install / where the runtime truly requires it — the Linux system unit / Windows
   SCM service) must change. macOS: fully rootless main daemon. Linux/Windows: rootless when the overlay
   is unchanged AND no system-level component needs (re)writing; see the Linux caveat above.
2. **macOS `install()`** (~1015): runs as the user → user Agent. Drop `<key>GroupName</key>` from the
   plist when non-root (a per-user Agent has no shared unix group; emitting `GroupName=zlayer` makes
   `launchctl bootstrap` fail **EX_CONFIG** because the group only exists when created by root via
   `dseditgroup`). Gate `ensure_zlayer_group_macos(data_dir)` behind `is_root` (it shells `dseditgroup`,
   needs root). **THE GROUPNAME/dseditgroup TRAP is why a naive rootless change re-breaks the install —
   handle it.**
3. **overlayd install** (`install_overlayd_service` macOS ~887, called from install() ~1336): replace
   with the change-gate above — skip when unchanged, surgically elevate when changed. Do NOT install a
   user-agent overlayd (it would crash-loop failing to create the utun).
4. **Ownership reconciliation / migration off the legacy root install:** detect a legacy root
   `system/com.zlayer.daemon` (the old wrong home) → boot it out + `rm /Library/LaunchDaemons/
   com.zlayer.daemon.plist` (root; fold into the one surgical elevation) → `chown -R $SUDO_USER` the
   whole `~/.zlayer` tree. Today only `registry/cache/bundles` get chowned (`ensure_zlayer_group_macos`
   ~887/2027) + `secrets` in `scripts/install-dev.sh:322`. Remove stale root-owned `run/*.sock` +
   `logs/daemon.log*`.
5. **`uninstall()`** (~1385): clean BOTH the `gui/$uid` Agent AND any legacy `system` daemon AND the
   overlayd system service — regardless of current euid — so orphans can't survive a `--replace`.
6. **`wait_for_daemon_ready()` (~3927) + `get_daemon_failure_context()` (~3983):** on
   connection-refused-until-timeout, NAME the cause instead of the generic "Daemon failed to start
   within 45s": crash-looping launchd job (`launchctl print gui/$uid/<label> | grep "last exit code"`
   → `78`=EX_CONFIG ⇒ "log/socket not writable by run user"), a same-label service in the other domain,
   or a non-writable `StandardErrorPath`. Probe the socket the daemon was actually configured with
   (thread `--socket` through), not just `default_socket_path()`.
7. **`scripts/install-dev.sh`:** delete the false "No sudo on macOS — installs into ~/Library/
   LaunchAgents" comment (line ~368) — it WAS false because `daemon install` self-elevated; correct
   once Track A lands. Keep `sudo install` only for the `/usr/local/bin` binary copy. Reclaim ownership
   of the FULL `~/.zlayer` (not just `secrets`).

### Verify Track A
Clean slate (`launchctl bootout gui/$(id -u)/com.zlayer.daemon; sudo launchctl bootout
system/com.zlayer.daemon; rm -f ~/Library/LaunchAgents/com.zlayer.daemon.plist; sudo rm -f
/Library/LaunchDaemons/com.zlayer.daemon.plist; sudo chown -R $(id -un) ~/.zlayer; rm -f
~/.zlayer/run/*.sock ~/.zlayer/logs/daemon.log*`) → `cargo build -p zlayer` → `./scripts/install-dev.sh
--replace` (no sudo) → main daemon installs as a `gui/$uid` Agent with NO sudo prompt, reaches
`/health/ready`; `~/.zlayer` uniformly user-owned; exactly one `com.zlayer.daemon` (user); `launchctl
print gui/$uid/com.zlayer.daemon` shows it running (no EX_CONFIG). Re-run `--replace` twice: no orphans,
and NO overlayd sudo prompt when the overlay binary/plist are unchanged.

---

## TRACK B — macOS Apple-Virtualization (VZ) runtime

New `crates/zlayer-agent/src/runtimes/macos_vz.rs`: ephemeral native-macOS VMs via Virtualization.
framework (GitHub-runner / Tart model), COEXISTING with the Seatbelt sandbox (selectable, NOT a
replacement; NOT libkrun, NOT Linux-on-mac — `runtimes/macos_vm.rs` is the libkrun/Linux-guest runtime;
mirror its LAYOUT only).

### Crate versions (RESOLVED 2026-06-03 via `cargo add --dry-run`; re-confirm at impl time)
`objc2 0.6.4`, `objc2-foundation 0.3.2`, `objc2-virtualization 0.3.2` (123 features default — the VZ
classes are available without manually enabling each), `block2 0.6.2`, `dispatch2 0.3.1`. Plus `russh`
(latest via `cargo add`) for SSH exec, or shell out to system `ssh`. Add under
`[target.'cfg(target_os = "macos")'.dependencies]` in `crates/zlayer-agent/Cargo.toml` (~148, next to
`dirs`/`libloading`) via `cargo add --target 'cfg(target_os="macos")'`.

### Integration points (file:line)
- `Runtime` trait: `crates/zlayer-agent/src/runtime.rs:1062`. Implement EVERY required method; override
  `kill_container` (validate_signal at runtime.rs:1765; SIGKILL/SIGTERM→force stop), `pause_container`/
  `unpause_container` (VZ pause/resumeWithCompletionHandler — cheap real win), `tag_image`→Unsupported.
  Leave `exec_pty` default Unsupported, `get_container_port_override` default `Ok(None)` (per-VM
  netstack). `ContainerAuthContext` at runtime.rs:1789; pause/unpause defaults at :1569/:1579.
- `RuntimeType` enum: `bin/zlayer/src/cli.rs:168` (add `MacVz` after `MacVm` at :182, cfg macos).
- `RuntimeConfig`/`MacVzConfig`: `crates/zlayer-agent/src/lib.rs` (enum :119, `MacVm` variant :143,
  `create_runtime` macOS arms :283-311). VZ is opt-in ONLY — never selected by `Auto` (Seatbelt stays
  macOS primary/composite-primary).
- Builder: `bin/zlayer/src/config.rs:31`. Module + re-export: `runtimes/mod.rs:135/160`, `lib.rs:74`.
- Per-service label routing: `runtimes/composite.rs:167` (`select_for`) — route
  `spec.labels.get("com.zlayer.isolation") == "vz"` to the VZ delegate.
- Mirror layout from `runtimes/macos_vm.rs` (NOT its libkrun FFI): struct shape, `container_dir_name`
  ("{service}-{replica}"), `vm_dir`/`images_dir`, `sanitize_image_name`, `parse_memory_to_mib`,
  `safe_vcpu_count`, `resolve_entrypoint`. Per-container dir `{data_dir}/vz/{service}-{replica}/` with
  `disk.img` (APFS `clonefile` CoW of base), `aux.img`, `machine_id.bin`, `config.json`, `console.log`,
  `ssh_key`.

### Hard parts (implement concretely, no hand-waving)
- **Image:** OCI pull via `zlayer-registry` ImagePuller (Tart `ghcr.io/cirruslabs/macos-*` bundles) OR
  `.ipsw` restore via `VZMacOSRestoreImage`/`VZMacOSInstaller`. Base bundle = disk + `VZMacHardwareModel`
  data + base aux.
- **Platform config:** `VZMacPlatformConfiguration` with hardware model (from base) + FRESH per-VM
  `VZMacMachineIdentifier` + `VZMacAuxiliaryStorage` (created in create_container). Boot loader =
  `VZMacOSBootLoader` (native macOS guest). Graphics device required to boot. Virtio block (disk.img),
  virtio net + `VZNATNetworkDeviceAttachment` (MAC = `VZMACAddress`), serial → `console.log` via
  `VZFileHandleSerialPortAttachment`, virtio-fs share from `spec.storage`.
- **Threading:** `VZVirtualMachine` is NOT thread-safe — one serial `dispatch2::DispatchQueue` per VM;
  all VZ calls on it; bridge completion `block2` blocks to tokio `oneshot`.
- **exec via SSH** (russh): poll guest:22, run entrypoint over SSH → console.log; `exec` returns
  `(exit, stdout, stderr)`. Ephemeral keypair in create_container.
- **get_container_ip:** parse `/var/db/dhcpd_leases` for the VM's MAC → guest IPv4 (NOT localhost).
- **2-macOS-VM-per-host limit:** `vm_count: Arc<AtomicU32>` gate with an RAII guard so early `?` returns
  don't leak the count (mirror the libkrun GPU `AtomicBool` at macos_vm.rs:304/833/877).
- **Entitlement:** `com.apple.security.virtualization` + code-signing required to boot a VM. Add
  `docs/macos-vz-runtime.md`. If CI lacks the entitlement, gate VM-boot integration tests `#[ignore]`
  but STILL write the full code path.

### Tests
Unit-test pure helpers (sanitize_image_name, parse_memory_to_mib, safe_vcpu_count, vCPU/RAM clamping vs
`VZVirtualMachineConfiguration.minimum/maximumAllowed*`, the 2-VM gate transitions, a `/var/db/
dhcpd_leases` parser fixture, MAC formatting). VM-boot tests `#[ignore]`.

> NOTE: subagents are denied `cargo` at the policy layer — the Mac agent must run in a context where
> `cargo` works (the main interactive session), or all four green gates are unverifiable.
