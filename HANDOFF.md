# HANDOFF — RAFT + HCS ship-blockers, plus install-ownership / macOS-VZ / ZArcRunner

**Updated:** 2026-06-03. **Author:** Claude (this session). Everything I know right now, with file
references + how to continue. Companion docs: `HANDOFF_WINDOWS_HYPERV.md` (the HCS/GCS saga in full),
and the master plan `/Users/zach/.claude/plans/please-investigate-the-handoff-windows-h-dapper-matsumoto.md`.

---

## 0. Mission

Two things gate shipping ("stopping us from existing"): **RAFT** (`raft-e2e` CI green, all 5 cluster
suites) and **HCS** (the Windows HCS runtime — both `windows-hcs-e2e` + `windows-hcs-hyperv-e2e`
green). In parallel (user said "everything in parallel"): fix the macOS **install-ownership** bug,
add a macOS **Apple-Virtualization (VZ)** isolation tier (additive to Seatbelt, NOT replacing it,
NOT libkrun, NOT Linux-on-mac), and finish **ZArcRunner** (a ZLayer-native Forgejo Actions runner
that replaces the bare-metal act_runner so every CI job runs in a ZLayer container per-OS).

Execution style the user wants: investigate per task with agents, then waves of small per-task
agents; exhaustive, no deferrals; if blocked, surface the concrete blocker and keep going; choose the
most complete solution when forced.

---

## 1. Repos / branches / current HEADs

| Repo | Path | Branch | HEAD (this session) | Pushed? |
|---|---|---|---|---|
| ZLayer (Rust, primary) | `/Users/zach/GitHub/ZLayer` | `dev` | `51383c54` | **yes** (origin/dev in sync) |
| zlayer-zql (proprietary mirror) | `/Users/zach/GitHub/zlayer-zql` | `zdb` | `469f712a` | **NO — 10 commits ahead of origin/zdb, intentionally unpushed** |
| ZArcRunner (Go runner) | `/Users/zach/GitHub/ZArcRunner` | `main` | `188c140` | **NO — local only** |
| ForgejoRunner (act fork) | `/Users/zach/GitHub/ForgejoRunner` | (patched act_runner) | n/a | reference only |

Origin = Forgejo `https://forge.blackleafdigital.com/BlackLeafDigital/ZLayer.git` (token-credentialed).
Commit markers: `[np]` (no-publish) / `[fast]`; NEVER `[skip ci]` (no-op here).

**⚠ Commit hygiene note:** ZLayer commit **`51383c54`** has message "fix(cli): route console tracing
to stderr" but a `git add -A` accidentally BUNDLED the HCS instrumentation into it too — it actually
contains: `logging.rs` (the real stderr fix, +12), `crates/zlayer-agent/src/runtimes/hcs.rs` (+362,
the svcdump + container_logs work), `crates/zlayer-hcs/src/process.rs` (+215, HCS pipe capture),
`crates/zlayer-hcs/Cargo.toml` (+2, windows features), `CHANGELOG.md`. It's pushed; don't rewrite.
The `hcs.rs`/`process.rs` parts are `#[cfg(windows)]` so they're excluded from the Linux raft build
(harmless to raft) but are **only mac-build-verified, NOT Windows-compile-verified** (see §4).

---

## 2. The runner fleet + CI mechanics (critical, hard-won)

- CI runs on a **Forgejo Actions** instance. The `e2e.yml` workflow is **`workflow_dispatch`-only**
  (pushes do NOT trigger it; pushes DO trigger `ci.yaml`). Dispatch via `mcp__forgejo__dispatch_workflow`
  (`{owner: BlackLeafDigital, repo: ZLayer, workflow_filename: e2e.yml, ref: dev, inputs:{test: raft-e2e}}`).
- **Runner fleet** (Komodo servers; `nvidia`-labelled = GPU + privileged-capable):
  - `GitRunner` = server **162.55.187.112** (aka "gitworker"), labels `ubuntu-latest,...` — does NOT
    reliably grant `--privileged` to job containers. This is the stock act_runner.
  - `MiniBeast`, `BeastPC` = labels include `nvidia` **AND `ubuntu-latest`** — GPU hosts, privileged.
  - `MiniWindows` = the Windows runner (`windows-latest`, server 2025 build 26100).
  - `MacRunner` = `macos-latest,arm64`.
  - **THE FLAKE ROOT:** `ubuntu-latest` matches GitRunner AND the nvidia boxes, so `runs-on:
    ubuntu-latest` was a privilege lottery. The raft job is now pinned **`runs-on: nvidia`**.
- **act `--privileged`/`--cgroupns` drop:** stock upstream act_runner DROPS the workflow's
  `--privileged`/`--cgroupns=host` `container.options`. `ForgejoRunner` (the fork at
  `/Users/zach/GitHub/ForgejoRunner`, patch `9ef0cfc`, vendors `code.forgejo.org/forgejo/runner`)
  fixes that — **but per its `SITREP-2026-05-25.md` the patch was NEVER deployed to the runners**.
  We worked around it in CI (cgroup-prep step + nvidia pin) rather than deploying the fork.
- **CI logs are UNREADABLE over REST** on this Forgejo version (`mcp__forgejo__get_action_job_logs`
  / `list_run_job_logs` → 404). The Postgres-backed **forgejo-actions MCP** reads them, but it is
  **configured in `~/.claude.json` yet NOT loading this session** (no `mcp__forgejo-actions__*` tools
  surface). Workaround used all session: **curl the sidecar directly** at
  `https://forge-actions.blackleafdigital.com/mcp` (streamable-HTTP JSON-RPC: initialize → capture
  `mcp-session-id` header → notifications/initialized → tools/call). Bearer = vault item
  **"Claude Code MCP: forgejo-actions"** (`FORGEJO_ACTIONS_MCP_API_KEY`,
  `2e3ee0adb9bd541e19ab2af1297b8667871211322aac462d1892c27299742f4b`; also in `~/.config/bld/.env`).
  Tools: `list_runs` (repo MUST be `owner/name`), `get_run_detail {run_id, include_steps}`,
  `tail_job_log {run_id, job_id, grep, tail_lines, strip_ansi, strip_timestamps}`, `monitor_run`,
  `dispatch_workflow`, `cancel_run`. `run_id` = global action_run id (e.g. 10991), distinct from the
  per-repo run_number (#1695). **Fix for next session: reinstall the forgejo-actions MCP from
  BlackLeafServices + restart Claude.**
- **ci-step-status cache gotcha:** jobs fingerprint their source paths via `ci-step-status@main`; a
  matching prior fingerprint marks the step "completed" and SKIPS it → the job shows **`success` with
  ~0.1–0.5m duration** WITHOUT actually running. Many `windows-hcs-e2e` "success" rows are these
  cache-skips, NOT real runs. Real runs take minutes. Don't trust short-duration "success".
- Each push triggers `ci.yaml` (full workspace lint/test) which competes for the single ubuntu
  runner; cancel it (`cancel_run`) to let a dispatched e2e through.

---

## 3. RAFT track — `raft-e2e` (★ship-blocker)

5 suites: `cluster_3node`, `cluster_failover`, `cluster_node_upgrade` (pure consensus — **GREEN** for
a while now), `cluster_scaling`, `cluster_upgrade` (need real workload containers). The job had
**never passed**; I peeled a multi-layer onion (each fix committed on `dev`):

1. **overlayd not built in CI** → `serve` self-spawns `zlayer-overlayd` (a separate binary); absent →
   ~35s dial stall > 30s `/health/ready` → bootstrap fail. Fix: build `-p zlayer -p zlayer-overlayd`
   in `.forgejo/workflows/e2e.yml` + `.github/workflows/e2e.yml` + `run-suite.py` build_phase; added
   `crates/zlayer-overlayd` to the raft fingerprint; bumped per-node health wait 30→60s. (`530d2058`)
2. **Linux-only compile error** in `zlayer-overlayd` exposed once CI built it:
   `crates/zlayer-overlayd/src/server.rs` `IpNet::contains(ip)` must be `contains(&ip)` + an unused
   import. (`95e1f04f`)
3. **Unqualified images** rejected by the registry → qualified the 4 `cluster-specs/*.yaml` to
   `docker.io/library/nginx:…` + `_norm_image` tolerant compare in `run-suite.py`. (`530d2058`/`35c20e5e`)
4. **cgroups** — job container lands at the cgroup-v2 root with `/sys/fs/cgroup` read-only when not
   privileged. Added a cgroup-prep step (`remount,rw` + delegate `/zlayer-e2e` + export
   `ZLAYER_CGROUP_PARENT`) (`a99d238f`) AND pinned `runs-on: nvidia` (`a391ab41`).
5. **Stabilization `healthy=false`** — containers ran but `wait_for_stabilization` never saw them
   healthy. ROOT CAUSE: `crates/zlayer-agent/src/service.rs` (~615-695) only bridged the health
   monitor into the `health_states` map (which stabilization reads) INSIDE `if let (Some(proxy),
   Some(ip))`; in CI there's no reachable `effective_ip` → no bridge → forever unhealthy. **Fix:
   register the health_states bridge UNCONDITIONALLY** (proxy backend update stays gated). Specs
   declare a host-side `command:"true"` health check. (`78c671ee`)
6. **JSON pollution** (the LAST layer) — after #5 the deploy reaches `ready (3/3 healthy)`, but the
   suite's `json.loads(`zlayer ps --containers --format json` stdout)` died with `JSONDecodeError:
   Extra data` because the **observability console tracing layer wrote INFO `{"timestamp":...}` lines
   to STDOUT**, polluting the JSON. **Fix: all 6 console `fmt::layer().with_writer(io::stdout)` →
   `io::stderr` in `crates/zlayer-observability/src/logging.rs`** (daemon still logs to file/journal;
   stdout = data, stderr = logs). (`51383c54`) Verified locally: `ps --format json` stdout has 0
   timestamp lines.

**STATE:** all 6 fixes are committed + pushed to `dev` (HEAD `51383c54`). On run #1697 (`78c671ee`,
pre-stderr) the containers genuinely ran 3/3 and the ps JSON showed 3 `state:"running"` entries — the
ONLY failure was the JSON parse, now fixed. **A raft-e2e dispatch on `51383c54` is the verification
that flips it to 5/5** (run #1699 / id 11103 was dispatched then the user interrupted — re-dispatch
or check it). If a further layer appears (e.g. `cluster_scaling`'s spread-across-≥2-nodes assertion —
note in #1697 all 3 replicas landed `node_id:"1"`, so spread may need attention), fix it; done = 5/5.
**Critical files:** `service.rs` (health bridge), `stabilization.rs`, `health.rs` (`Command` runs
`sh -c` host-side), `logging.rs`, `bin/zlayer/src/commands/ps.rs` (the `image`/`state` fields, added
`530d2058`), `crates/zlayer-manager/tests/e2e/{scripts/run-suite.py,cluster-specs/*.yaml}`.

---

## 4. HCS track — Windows runtime (★ship-blocker; see `HANDOFF_WINDOWS_HYPERV.md` for full saga)

- `windows-hcs-hyperv-e2e` FAILS every real run (~9 min). `windows-hcs-e2e` (process-iso) "success"
  rows are cache-skips (unproven).
- **GCS cold-start saga (the blocker):** never-dial was solved by stripping `gcs.DependOnService`
  (`bf5c5b14`), but that's the WRONG fix — the cold-start `RpcCreate` now flakily faults the guest
  `vmcomputeagent.exe` (bridge closes, no HRESULT, VM stays Running; ~25-30% survive Create+Start).
  Real root: `mpssvc`/`netsetupsvc` don't reach Running in the stripped nanoserver UVM (drivers load,
  configs/DLLs present → a runtime service-start failure of unknown cause). The rule-out matrix
  (TZ/memory/image/NIC/bytes all exonerated) is in `HANDOFF_WINDOWS_HYPERV.md`.
- **NEW this session — the svcdump diagnostic IS NOW IMPLEMENTED** (bundled in `51383c54`):
  `ZLAYER_GCS_SVCDUMP=1` (`#[cfg(feature="windows-debug")]`, default off) in
  `crates/zlayer-agent/src/runtimes/hcs.rs`: (a) injects host `sc.exe` →
  `uvm.os_files_dir()\Windows\System32\sc.exe` (~hcs.rs:1559); (b) registers an auto-start no-deps
  `zlayer-svcdump` service in `build_uvm_registry_changes()` (~hcs.rs:2709) that loops
  `sc queryex mpssvc|netsetupsvc|BFE|RpcSs|nsi|gcs` + `sc qc` → `C:\zlayer-dbg\svcdump.txt`;
  (c) on failure it mounts the kept scratch ON-BOX (no offline detach) via `mount_and_read_svcdump`
  (~hcs.rs:2543) and folds `svcdump.txt` into the `CreateFailed` error + echoes to stderr (so a
  foreground `--nocapture` run shows it LIVE). Also wired `container_logs` (was `Unsupported` stub at
  ~hcs.rs:3819→4055) via real HCS pipe capture in `crates/zlayer-hcs/src/process.rs`
  (`ComputeProcess::create_capturing_blocking` + `drain_pipe`).
  **⚠ NOT Windows-compile-verified** — only mac-build (which excludes `cfg(windows)`) + `cargo check
  --target x86_64-pc-windows-msvc -p zlayer-hcs` (green); the full workspace windows check is blocked
  by `ring`/`aws-lc-sys` needing the Windows SDK C headers (no cross-C toolchain on the mac). It must
  be compiled on the box.
- **Existing windows-debug toggles (all implemented):** `ZLAYER_GCS_BOOTLOG`, `ZLAYER_GCS_KD`,
  `ZLAYER_GCS_STOCK_DEPS` (omit the DependOnService strip = stock hcsshim parity),
  `ZLAYER_GCS_COLDSTART_DELAY_MS`, `ZLAYER_KEEP_UVM_ON_FAILURE`. Log-forwarding scaffold in
  `crates/zlayer-gcs/src/log_forward.rs` (guest may not support it).
- **The user will debug HCS INTERACTIVELY at the Windows machine** (monitor plugged in), NOT via
  SSH-detached + offline-mount. The command to run on the box (build first with the features):
  ```powershell
  $env:ZLAYER_GCS_STOCK_DEPS=1; $env:ZLAYER_GCS_SVCDUMP=1; $env:ZLAYER_KEEP_UVM_ON_FAILURE=1
  cargo test --features hcs-runtime,wsl,windows-debug --package zlayer-agent `
    --test windows_hcs_hyperv_e2e windows_hcs_hyperv_smoke_create_start_stop_remove -- --ignored --nocapture
  ```
- **WHAT'S LEFT for HCS done:** (1) compile the bundled instrumentation on the box (fix any
  windows-only compile errors). (2) Run svcdump → read the `WIN32_EXIT_CODE`/`SERVICE_EXIT_CODE` for
  `mpssvc`/`netsetupsvc` (126=missing DLL, 1068/1075=dep, 1058=disabled, 1053=timeout) → NAME the
  cause. (3) Inject the missing dep/config into the UVM offline. (4) **Restore the stock `gcs`
  DependOnService** (drop the strip in `build_uvm_registry_changes`). (5) Verify both Windows jobs
  green. (6) CHANGELOG + zql mirror (NOTE: `hcs.rs` is on the zql `.drift-allowlist` — it intentionally
  lags `dev` during the GCS arc; mirror via `scripts/zql/regenerate.py`, not by hand). The CI test
  entry points: `crates/zlayer-agent/tests/{windows_hcs_e2e.rs, windows_hcs_hyperv_e2e.rs}`; jobs in
  `.forgejo/workflows/e2e.yml` (`windows-hcs-e2e` ~line 909, `windows-hcs-hyperv-e2e` ~1014; build
  `--features hcs-runtime,wsl`, runner `windows-latest`/MiniWindows).

---

## 5. macOS install-ownership track (NOT started in code; investigated)

Symptom: a prior `sudo` install left `~/.zlayer` root-owned; a later non-root `zlayer daemon install`
registers a launchd USER agent that can't write `daemon.log` → launchd `EX_CONFIG (78)` crash-loop.
**Real code:** `bin/zlayer/src/commands/daemon.rs` — `install()` (macOS@~1015 / Linux@~2186 /
Windows@~3132) and `install_overlayd_service()` (macOS@~887 / Linux@~1741 / Windows@~2999) create the
log/run dirs but NEVER reconcile ownership of the data/log/run tree to the run-user (`ensure_zlayer_
group{,_macos}` @~1857/2027 only chown registry/cache/bundles). `launchd_context()`@~787 picks
root→system-daemon vs non-root→user-agent. `wait_for_daemon_ready()`@~3927 HIDES the PermissionDenied
case. **Fix shape:** before writing the plist/unit + bootstrapping, create the FULL tree and set+verify
it owned-by/writable-by the run-user (macOS/Linux: SUDO_USER/invoking-user `chown -R` the whole
`~/.zlayer`, not just build dirs; overlayd install must create its log_dir; Windows: ACL for the
service account); and make `wait_for_daemon_ready` surface (not swallow) the failure.
**Already done:** `scripts/install-dev.sh` — fixed the bash-3.2 `set -u` empty-array `unbound
variable` crash (use `${arr[@]+"${arr[@]}"}`), root-owned `~/.zlayer/secrets` reclaim, and `tmp` dir
(committed in `78c671ee`). The deeper daemon.rs ownership fix is the remaining Track-3 work.

---

## 6. macOS Apple-Virtualization (VZ) track (NOT started; researched design)

Goal: new runtime = ephemeral native-macOS VMs via Virtualization.framework (the GitHub-runner /
Tart / lume model), COEXISTING with the Seatbelt sandbox (selectable; not a replacement). NOT
libkrun, NOT Linux-on-mac (the user was emphatic: macOS = Seatbelt by design; `macos_vm.rs` is
libkrun/Linux-guest and stays OUT).
**Design (from research):** use `objc2` + **`objc2-virtualization`** crates — **verify latest
versions with `cargo add`/`cargo search` at impl time, don't hardcode**. Tart (Cirrus, open source) is
the reference + image source (OCI-wrapped `.bundle`). New `crates/zlayer-agent/src/runtimes/macos_vz.rs`
mirroring the `VmRuntime` shape (`runtimes/macos_vm.rs`) for image/dir/log layout (but NOT its libkrun
FFI). Implement the `Runtime` trait (`crates/zlayer-agent/src/runtime.rs:1062-1587`):
create/start(thread)/stop(+timeout)/remove, **exec via SSH** into the guest (Tart model; `exec_pty`
default-Unsupported is fine), logs, workspace via virtio-fs, `get_container_ip`. Add `RuntimeConfig::
MacVz(MacVzConfig)` + dispatch in `crates/zlayer-agent/src/lib.rs::create_runtime` so Seatbelt stays
primary and VZ is chosen by an explicit isolation knob/label. Honor Apple's **2-macOS-VM-per-host**
limit (AtomicU32). Hard parts: base macOS image sourcing/signing, SSH bootstrap, virtio-fs path
translation. No `todo!()` — if a hardware/image constraint genuinely blocks it, surface that concrete
blocker.

---

## 7. ZArcRunner track (Go) — the runner replacing bare-metal act

`/Users/zach/GitHub/ZArcRunner`. It polls Forgejo (`runner.v1` protocol), dispatches each task into a
ZLayer-managed container running a `task-executor`, streams logs back, cleans up. ~75-80% of `act`
fidelity, ~13.5k LOC, 11 passing E2E scenarios. Source-of-truth: `TODO_MAC_RUNNER.md` (8 slices).
**Done this session (committed locally on `main`):**
- `25bd920` — executor fidelity (`pkg/taskexecutor/`): `timeout-minutes` enforcement (per-step +
  per-job, float, default 360), real status fns (`if: failure()/cancelled()/always()`), local
  `uses: ./path`, composite-action `outputs:`, Windows `shell: cmd` (new `pkg/taskexecutor/shell.go`).
  All `go build/vet/lint/test` green.
- `188c140` — Slice 2 node_selector/platform: `pkg/supervisor/config.go` (NodeSelector +
  ClusterEndpoint), `pkg/zlayer/{types.go,convert.go,placement.go}`, `pkg/listener/dispatcher.go`,
  `scripts/register.{sh,ps1}` flags. **BLOCKER surfaced:** the ZLayer Go SDK fields
  (`CreateContainerRequest.NodeSelector/Platform`) exist in ZLayer dev (`clients/go` @ `e85587ce`)
  but are NOT in any `go get`-able published version (GitHub mirror max `v0.11.23` predates them).
  The facade→SDK marshaling is gated behind build tag **`zlayersdk_placement`** (no-op stub by
  default; real impl in `pkg/zlayer/placement_sdk.go`). **To activate on the wire: publish a
  `clients/go` tag carrying the fields, then `go get …@<tag>` + build `-tags zlayersdk_placement`.**
- Earlier (pre-session, on `main`): Slice 1 (ZLayer-Secrets registration token, `3753f76`).
**Remaining slices (3-8):** per-OS workspace/executor images (`images/`, wire `release.yml`),
`zarcrunner dispatch run` (Slice 4: `pkg/zlayertar`, `pkg/dispatch/local`, `cmd/zarcrunner/dispatch.go`),
`scripts/fleet/join-node.sh` (Slice 5) + a `gitworker-cleanup.sh`, `zarcrunner dispatch workflow`
(Slice 6), `cmd/zarcrunner-mcp` (Slice 7), dispatch-error→Forgejo-task-log + `docs/FLEET.md` + fleet
rollout/retire act (Slice 8). Open items also in `TODO_MAC_RUNNER.md` §4.
**Per-OS isolation reality (verified):** Linux youki 🟢; macOS Seatbelt 🟢 (native jobs) + VZ tier
(new, §6); Windows HCS process-iso 🟡 (works, logs now wired) + WSL2 delegate for Linux-on-Windows.
ZArcRunner depends on ZLayer's `zlayer-docker` Docker-API shim being complete (it is — verified) if
ever pointing act at it, but ZArcRunner uses the ZLayer REST API directly.

---

## 8. Access / infra / secrets

- **Vault (vaultwarden MCP, `mcp__vaultwarden__*`):** forgejo-actions bearer (item "Claude Code MCP:
  forgejo-actions"); forge.blackleafdigital.com web login (item `e6216457`, user `zachhandley`, has
  2FA — TOTP not stored, so web-UI log access via puppeteer dead-ends).
- **SSH:** the GitRunner host (162.55.187.112) **rejects** both the Mac agent key and the ssh-mcp
  managed key. The Windows box = `Admin@miniwindows.net.home` (pwsh default; `C:\src\ZLayer` is a
  credentialed git checkout — commit+push from the box). The ssh-mcp managed pubkey:
  `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEKJLGcaph2Ymj1aAcgINX+9h6/TDBgcTN+7u56UcAa5 ssh-mcp-managed`.
- **Komodo MCP** (`mcp__komodo__*`): the fleet servers (GitRunner=162.55.187.112, DedicatedAlpha,
  BlackLeafCloud=178.63.67.34 runs forgejo, etc.). act_runner on GitRunner is NOT a Komodo stack.
- Mac is **bash 3.2.57** (`/bin/bash`) — empty-array `${arr[@]}` under `set -u` errors; use
  `${arr[@]+"${arr[@]}"}`.

---

## 9. Standing rules (do NOT violate)

- Mirror shared ZLayer crate changes to **zlayer-zql** + bump its CHANGELOG (it's `zdb`, proprietary,
  currently 10 commits unpushed). `hcs.rs` is on zql's `.drift-allowlist` during the GCS arc — mirror
  via `scripts/zql/regenerate.py`, not by hand. **zdb is SECRET** — never document zdb specifics in
  any checked-in file (this handoff is in ZLayer/dev → keep it generic).
- Rust: ALWAYS `cargo fmt --all` + `clippy --workspace --all-targets -- -D warnings` + `test
  --workspace` + `build --workspace`, ALL green, never `-p <crate>`. Go: `gofmt -l .` (empty) +
  `go build/vet ./...` + `golangci-lint run` + `go test ./...`.
- NEVER hand-edit dependency manifests (use `cargo add`/`go get`). NEVER `git stash`. Look up real
  latest dep versions. Update `CHANGELOG.md` (use the real upcoming version, e.g. `0.52.4`, never
  `[Unreleased]`).
- Windows-debug instrumentation is feature-gated + KEPT in-tree (it keeps getting deleted+recreated —
  do not delete it).

---

## 10. Immediate next actions

1. **RAFT (gating):** verify raft-e2e on `51383c54` (run #1699 / id 11103 was dispatched). If 5/5 →
   RAFT DONE. If `cluster_scaling` spread (replicas all on node 1) or another layer fails, fix it.
   (CI logs via the forgejo-actions sidecar curl — see §2; or fix the MCP.)
2. **HCS (gating):** on the Windows box — build `--features hcs-runtime,wsl,windows-debug` (fix any
   windows-only compile errors in the bundled `51383c54` instrumentation), run the svcdump command
   (§4), name why `mpssvc`/`netsetupsvc` fail, inject the missing piece, restore stock `gcs` deps,
   get both Windows jobs green.
3. **install-ownership (Track 3):** implement the daemon.rs tree-ownership fix (§5).
4. **macOS VZ (Track 4):** build `runtimes/macos_vz.rs` (§6).
5. **ZArcRunner (Track 5):** Slices 3-8 (§7); publish a `clients/go` tag to unblock node_selector wire.
6. **zlayer-zql:** decide whether to push the 10 unpushed `zdb` commits.
7. **Infra:** reinstall the forgejo-actions MCP from BlackLeafServices + restart Claude so CI logs are
   first-class again (stop curl-ing the sidecar).

**Master plan (full task breakdown):**
`/Users/zach/.claude/plans/please-investigate-the-handoff-windows-h-dapper-matsumoto.md`.
