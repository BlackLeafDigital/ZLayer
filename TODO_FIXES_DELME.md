# Install / buildd-clock / ZLayerZQL-sync / overlay-NIC fixes — working tracker

## What this file is

The single source of truth for the in-flight fix batch (2026-06-11 session). Update checkboxes as work lands.
**DELETE THIS FILE when every box is checked and both repos are pushed** — it must not survive past this batch
(`rm TODO_FIXES_DELME.md`, drop it from any commit it accidentally rode into). Like TODO_DELME.md it is a scratch
tracker, not documentation.

Repos: public ZLayer = `/home/zach/github/ZLayer` (branch dev). Private fork = `/home/zach/github/zstack/ZLayerZQL`
(branch main, own repo; sync ONLY via its `scripts/zql/regenerate.py`; the old zlayer-zql `zdb` branch is DEAD).

---

## Problem 1 — buildd VZ-Linux VM clock stuck at 2026-06-07T10:50Z on every spawn (Mac, production)

x509 "certificate not yet valid" on every pull; also why the GHCR buildd image fails its 60s Health=SERVING gate
(probe dials mTLS, stale clock rejects the fresh cert). Root cause: `{data_dir}/vz/linux/kernel/` caches
`Image` + `initramfs.cpio.gz` (the vzagent IS the initramfs `/init`, PID 1); `ensure_linux_kernel`
(`crates/zlayer-agent/src/runtimes/macos_vz_linux.rs`) only checks the files EXIST — never refreshes. The Mac's
cache is a Jun-7 build that survives every zlayer upgrade. The fresh `zlayer.boottime=` cmdline inject is fine.

**Design (user-corrected, final): the vz-linux bundle is just another base image.**
- [ ] CI: `vz-linux-images.yml` packages `Image`+`initramfs.cpio.gz`+`manifest.json` into an OCI image pushed to
      `ghcr.io/blackleafdigital/zlayer/vz-linux:arm64` (+ `:<kernel_version>` tag) — mirror `buildd-image.yml`'s
      GHCR login/push exactly. Keep the existing Forgejo generic-package publish step too.
- [ ] Daemon: `ensure_linux_kernel` pulls that literal ref through the EXISTING `zlayer_registry::ImagePuller`
      + persistent blob cache (digest change → re-extract; cached layers serve offline; populated cache +
      unreachable registry → warn and use cache; empty + unreachable → existing clear error). NO ref
      canonicalization. Env overrides `ZLAYER_VZ_LINUX_KERNEL/_INITRD` still bypass everything.
- [ ] REVERT the uncommitted version-pin edits in `macos_vz_linux.rs` + `images/vz-linux/build.sh`
      (wrong design, superseded by the above; `git diff HEAD` shows them).
- [ ] After first CI publish: flip the new GHCR `vz-linux` package to PUBLIC in the GitHub UI (org packages
      default internal → anonymous pulls 401) and verify with an anonymous token pull.
- [x] vzagent `Msg::SetTime{unix_secs}` (tag 14) + guest settimeofday handler + proto round-trip tests
      (`crates/zlayer-vzagent/src/{proto,main}.rs`, compiles+tests green on Linux).
- [ ] Host-side `push_settime` in `macos_vz_linux.rs` mirroring `push_overlay_agent`: call after Resume in
      `unpause_container` + a per-live-VM periodic tick (~5 min) so ANY long-lived VZ container self-heals.
      NOTE: only works once the Mac's bundle is refreshed (stale Jun-7 agent has no tag 14).
- [x] `ensure_guest_time()` in `buildd_manager.rs`: exec `date -u -s "<host utc>"` + read-back skew check (<60s),
      HARD-FAIL before any build, on both warm-reuse and fresh paths. Works with any agent vintage.
- [x] `ensure_dev_fuse` hardened: Result-returning, 3×500ms retry, final verify of `/dev/fuse` +
      `command -v fuse-overlayfs`, hard-fail naming the active image (kills the cryptic
      `fuse-overlayfs: exit status 1` from the `zlayer-buildd-v2:arm64` fallback).
- [ ] Mac verification (ZArcRunner): `zlayer down zlayer-buildd`; `zlayer build -z ZImagefile -t
      zarcrunner-executor:latest .` → buildd healthy <60s; skew test `zlayer exec zlayer-buildd -- date -s
      '2026-06-07'` then rebuild → self-heals.

## Problem 2 — `install-dev.sh --replace` broke docker on macOS

`daemon install --docker-socket` unconditionally installed docker/docker-compose CLI shims (clobbers Docker
Desktop at /usr/local/bin/docker, or lands off-PATH in ~/.zlayer/bin on Apple Silicon); blind `sleep 3` before
the /var/run/docker.sock takeover.
- [x] Deleted the unconditional shim install from `daemon install` on all 3 platforms — `--docker-socket` enables
      ONLY the socket; shims are exclusively `zlayer docker install`'s job (it has `--no-shim`; install-dev.sh
      already passes it). Uninstall-path shim cleanup kept. (`bin/zlayer/src/commands/daemon.rs`)
- [x] Reverted the misguided `--no-docker-shim` flag (cli.rs back to HEAD).
- [x] install-dev.sh: prints `Mode: replace (...)` in the config block; `sleep 3` → 30×1s `status` readiness
      poll that exits 1 with the right log path BEFORE touching /var/run/docker.sock.
- [ ] Mac verification: `./scripts/install-dev.sh --replace` → prints mode line, daemon-ready gate passes,
      `docker ps` works after install.

## Problem 3 — release artifacts (prod bringup issues #10/#11)

- [x] dev `build.yml`: linux amd64+arm64 tarballs now build+pack `zlayer-overlayd` (musl/darwin/windows
      deliberately excluded); `install.sh` stages it next to zlayer (rm-before-cp fresh-inode); CHANGELOG bullet.
- [x] ZLayerZQL `.forgejo/workflows/daemon-publish.yml`: builds+packs `zlayer-overlayd-zql` in both arch jobs;
      raw curl PUTs replaced with upload-packages.sh-style retry/409-handling/verify for tarball AND .sha256.
      (Fixes: 0.1.64 tarball missing overlayd; sha256 sidecar never published.)

## Problem 4 — data-dir layout (prod issue #12)

- [x] `bin/zlayer/src/migrations.rs`: `layout_version` marker (current=1), legacy-dir backfill, downgrade
      refusal, corrupt-marker refusal, `detect_incompatible_stores` probe-list hook (empty in public build; the
      ZQL fork's migrations patch adds its sqlite-without-zql-store probe), loud fail with exact paths +
      remediation (ZLAYER_DATA_DIR or wipe). Tests green. USER DECISION: no SQLite→ZQL content migration ever
      (ZQL build stays sqlite-free).

## Problem 5 — ZLayerZQL sync (finish + push)

State: regen output sits UNCOMMITTED in `/home/zach/github/zstack/ZLayerZQL` and is audited clean (correct dev
source, ~10-commit gap, zero ZQL storage code touched; the overlay-NAT and self_update test failures were real
at the fork's own HEAD under its own CI and are fixed in-tree).
- [ ] Fix malformed hunk header in `scripts/zql/patches/bin__zlayer__src__commands__self_update.rs.patch`
      (declares `+1,21`, body is 25 new lines → `@@ -1,17 +1,25 @@`); verify `patch -p1 --dry-run` against staging.
- [ ] After the dev push (below): re-run `uv run scripts/zql/regenerate.py --apply --yes` to absorb the new dev
      commit; full gates (`fmt --all` / `clippy --workspace --all-targets -D warnings` / `test --workspace` /
      `build --workspace`).
- [ ] Commit one-liner (`regen: pull from ZLayer dev (...)`) + `git push origin main`.

## Problem 6 — overlay must survive netbird-style meshes; own the base NIC (user directive)

Investigation done (file:line map in session): WG UDP socket binds 0.0.0.0 always
(`zlayer-overlayd/src/server.rs:2549`, `overlay/src/config.rs:88`); `advertise_addr` auto-detect = source IP of
default route (`bin/zlayer/src/daemon.rs:127` `detect_local_ip`, no override on the zero-config serve path at
:460); NAT/STUN re-derives the same way on separate wildcard sockets (`overlay/src/nat/traversal.rs:502`,
`nat/stun.rs:118`); no SO_BINDTODEVICE/IP_BOUND_IF/fwmark anywhere; overlay DNS has NO upstream forwarder
(`overlay/src/dns.rs:373` — containers inherit host resolv.conf, `agent/src/runtimes/docker.rs:499`, so a
netbird `~.` resolved hijack breaks container DNS + STUN hostname resolution); MTU unset on Linux/macOS TUN
(1420 hardcoded Windows-only, `transport.rs:562`) → WG-in-WG silent blackholing; ports are userspace-proxy only,
no kernel forwarding.
Implement (in order):
- [ ] (A) Physical-egress resolver (new helper in `overlay/src/netlink.rs`): read default route, skip
      `wt*/wg*/zl-*/utun*/tun*` interfaces, return the physical NIC + its IP. Feed it into
      `OverlayConfig.local_endpoint` (replace UNSPECIFIED at `overlayd/src/server.rs:2549`),
      `detect_local_ip()` (`daemon.rs:127`), and `discover_local_ip()` (`nat/traversal.rs:502`).
      Pin zlayer-owned NAT/STUN sockets with SO_BINDTODEVICE (Linux) / IP_BOUND_IF (macOS).
      boringtun's own socket can't be pinned today (no fd injection in DeviceConfig) — source-IP selection +
      advertised-endpoint correctness is the realistic scope; document the boringtun limitation in code.
- [ ] (B) `advertise_addr` override on the zero-config serve path (flag/env/config) falling back to (A)'s
      resolver — never the bare 8.8.8.8 trick.
- [ ] (C) Overlay DNS upstream forwarding pinned to explicit resolvers + point container resolv.conf at the
      overlay resolver instead of inheriting the host's; STUN hostnames resolve through the same path.
      Set TUN MTU explicitly on Linux/macOS (netlink currently never sets it).
- [ ] Raw L2 (AF_PACKET/eBPF) explicitly out: nothing half-built exists; (A)+(B)+(C) is the code-shaped scope.
      If deeper raw-traffic ownership is wanted after that, it's a from-scratch design conversation.

## Problem 7 — NOT in these repos (confirmed by exhaustive grep; belongs to the Zatabase repo)

- netbird wt0 `~.` catch-all DNS hijack permanent installer fix (issue #9).
- Installer step calling bare `Command::new("zlayer")` off-PATH (issue #13).

## Push sequence

1. dev: full gates → single commit (A0 remaining + everything [x] above that's dev-side) → `git push origin dev`.
2. ZLayerZQL: patch-header fix → regen --apply → gates → commit → `git push origin main`.
3. Mac verifications (Problems 1+2) via ZArcRunner / install-dev run.
4. **Delete this file.**
