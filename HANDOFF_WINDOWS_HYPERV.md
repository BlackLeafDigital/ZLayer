# ZLayer Windows Hyper-V (Part B) — Mac handoff

**Last updated:** 2026-06-02 (Mac→box SSH debug)
**Audience:** future-you, iterating on the Windows box (rsync copy at `C:\src\ZLayer`, Mac drives via SSH)
**Status one-liner:** Guest now DIALS + negotiates (the never-dial bug is fixed). New frontier: the guest inbox GCS flakily CLOSES the bridge (EOF, no HRESULT, VM stays Running) on the cold-start `RpcCreate`/HvSocket. Our bytes match hcsshim `main` exactly; this is a guest-side `vmcomputeagent.exe` fault. All host-side hypotheses exhausted — next step needs in-guest crash capture (heavy).

---

## ⟢ 2026-06-02 update — never-dial FIXED; cold-start RpcCreate is the new wall

**The never-dial bug is SOLVED.** The `gcs.DependOnService` override (strip to `condrv` +
`hvsocketcontrol`, in `build_uvm_registry_changes`, committed `bf5c5b14`) fixed it. The guest
GCS now dials ~1-2.5s after UVM start and **NegotiateProtocol succeeds** every run:
`Result:0, Version:4, Capabilities{SendHostCreateMessage, SendHostStartMessage,
HvSocketConfigOnStartup, SendLifecycleNotifications, SupportedSchemaVersions:[{1,0},{2,1}],
RuntimeOsType:Windows, GuestDefinedCapabilities{NamespaceAddRequest, SignalProcess,
PurgeVSmbCachedHandles}}`.

**NEW failure (the current wall):** after negotiate we send (over the same bridge, serially):
msg2 cold-start `RpcCreate` (null container, `UvmConfig{SystemType:"Container",
TimeZoneInformation:…}` double-encoded as `ContainerConfig`) → msg3 cold-start `RpcStart` →
msg4 `RpcModifySettings` HvSocket ("configureHvSocketForGCS"). The guest **closes the bridge
(reader gets EOF), with NO HRESULT response**, and the **VM stays `State:"Running"`**. It is
**FLAKY**: ~25-30% of UVMs survive Create+Start and die at msg4; the rest die at msg2. The
guest is otherwise alive (it once returned an *error response* to a ModifyServiceSettings
RPC). A hard close with no HRESULT + VM Running = the inbox `vmcomputeagent.exe` **faults
mid-dispatch** (not a protocol-decode rejection, which returns an HRESULT).

### RULE-OUT MATRIX (do NOT re-investigate these)
| Hypothesis | Verdict | Evidence |
|---|---|---|
| Our cold-start/Create/Start/HvSocket **bytes** | CORRECT | byte-for-byte match vs hcsshim `main` `internal/gcs/guestconnection.go::connect`, `prot/protocol.go` (UvmConfig, AnyInString double-encode, null GUID, no SchemaVersion on cold-create), `internal/uvm/start.go::configureHvSocketForGCS` (ParentAddress `894cc2d6…`) |
| **TimeZoneInformation** as the cause | EXONERATED | omit→`0xEF CRITICAL_PROCESS_DIED`; bare `"UTC"`→reject; hcsshim UTC-constant (empty dates)→flaky close; **real host TZ** (Win32 GetDynamicTimeZoneInformation, hcsshim default)→still flaky close |
| **Memory pressure** | NO | box = 15.7 GB RAM, 12.2 GB free, 0 leftover compute systems |
| **My windows-debug instrumentation** | NOT a confounder | bare `--features hcs-runtime,wsl` build fails identically |
| **Guest-init race** (host waits before Create) | NO | `ZLAYER_GCS_COLDSTART_DELAY_MS=1500` made it *worse* (0/3 past Create) |
| **Image/host version mismatch** | NO | host = Server 2025 build **26100** (24H2); swapping nanoserver `ltsc2022`→**`ltsc2025`** (=26100) still fails identically |
| **Writable VSMB exfil share** (§6.B) | DEAD END | HCS rejects `add_vsmb(zlayer-debug)` with PowerOnCold `0x80070057` regardless of `options`/timing (pre- or post-accept) |
| **GCS log forwarding** as a cold-start instrument | DEAD END | guest doesn't advertise log-forwarding support; rejects `StartLogForwarding`; and hcsshim only starts forwarding AFTER cold-start succeeds (catch-22) |
| **HvSocketConfigOnStartup → skip msg4** | hcsshim does NOT skip it | `start.go::configureHvSocketForGCS` is gated only on `OS()=="windows"`, sent unconditionally. (Untested whether skipping helps THIS guest — but won't fix the msg2 deaths.) |
| Protocol version | v4 correct | `protocolVersion=4` is current hcsshim `main`; guest agreed (Version:4) |
| BCD `/bootlog` + ntbtlog | OBSOLETE | targeted the (solved) never-dial/driver-load theory |

### What's committed this session (all green; Mac workspace + box build RC=0)
- `windows-debug` cargo feature (off by default; zlayer-agent→zlayer-gcs forward) — kept in-tree per standing rule.
- GCS protocol parity: `AnyInString` double-encoding; `CreateRequest.ContainerConfig` (was `Settings`); zlayer-hcs schema `skip_serializing_if` omitempty + `Layer.Id` casing.
- cold-start TimeZoneInformation = **real host TZ** via `crate::windows::timezone::host_timezone_information()` (Win32), falling back to hcsshim UTC constant.
- GCS log-forwarding scaffold (host hvsock listener on `WindowsLoggingHvsockServiceID 172dad59` + `ModifyServiceSettings`/`StartLogForwarding`); WER LocalDumps + demand-start `zlayer-dbg` svc (gated). NOTE: log forwarding is a dead instrument for THIS guest (see matrix).
- bridge stage-labeled errors (`negotiation: cold-start Create: bridge closed`) + verbose tracing gated behind `windows-debug`.
- `ZLAYER_GCS_COLDSTART_DELAY_MS` diag knob (default 0).
- Commits on `dev` (NOT pushed): `bf5c5b14` (dial fix), `da563a6f` (instrument+parity+compose), `6939b676` (host TZ).

### REMAINING OPTIONS (all heavy — need a steer)
1. **In-guest crash capture via a SCSI-attached writable VHD** (the one exfil channel HCS won't
   reject like VSMB does). The guest OS onlines SCSI disks at boot independent of GCS; point WER
   `LocalDumps` (or a guest svc dumping the Application/System event log) at it; read the VHD
   offline host-side after the run. Captures the `vmcomputeagent` fault reason. Multi-iteration build.
2. **Guest kernel debugger** (KD over the COM/named-pipe the harness already exposes). Heaviest;
   the COM/`Uefi.Console` path previously broke UVM boot — needs care.
3. **Escalate to hcsshim/Microsoft**: an EXTERNAL GCS host driving the Server 2025 inbox
   `vmcomputeagent.exe` for WCOW may be an under-supported configuration; the flaky cold-start
   crash could be a guest-side bug. Worth a minimal repro + issue.
4. **Cheap remaining experiment** (low confidence): skip msg4 when `HvSocketConfigOnStartup:true`
   (won't fix the msg2 deaths, only the msg4-class ones).

The disciplined read (confirmed by deep hcsshim research): STOP spraying byte-level changes —
the bytes are correct. The fault is inside the guest; capturing it (option 1) is the only way
forward without guesswork.

---

## ⟢ 2026-05-31 PM update — corrections + new iteration model

**Iteration model changed.** We no longer drive from the Mac via rsync. `C:\src\ZLayer`
on the box is now a **real git checkout of `dev`** (converted in place, build cache kept).
SSH in and run the e2e directly for full in-guest visibility:
`ssh MiniWindows@MiniWindows.local` — **remote default shell is PowerShell**, not cmd
(use `;`, `Test-Path`, `$env:`, `Get-ChildItem`; `&`/`&&`/`if exist`/`%VAR%`/`dir /b` all fail).
The `scripts/windows/*.ps1` (launch/status/cleanup) still work; `debug_e2e.py` from the Mac is optional now.

**Corrections to the findings below (this session's evidence overrides the older text):**

1. **The UVM OS boots — confirmed.** Hyper-V-Worker-**Admin** event **18601 "successfully booted
   an operating system"** (+ 18500 "started successfully") fired for all 3 UVMs. The earlier
   "no 18601" worry came from exporting only the *Operational* channel, which doesn't carry 18601.
   So this is purely a **guest-never-dials** problem, not a boot problem.

2. **The "listen-after-start race" theory (§6.A / theory #1) is dead.** Run timestamps prove
   `GcsBridge::listen` (step 3a) precedes `HcsStartComputeSystem` (step 3) by ~0.5 ms, and
   `accept` is awaited after start. Listen-before-start is correct. Do not re-investigate.

3. **`DefaultConnectSecurityDescriptor` is NOT the gap.** Tested adding it to the UVM
   `HvSocketSystemConfig`; e2e still hit 3/3 120s accept timeouts with `bridge_started=0`.
   Reverted. Verified against hcsshim: **WCOW** (`internal/uvm/create_wcow.go:270-274`) sets
   ONLY `DefaultBindSecurityDescriptor` + an empty `ServiceTable`; the lone ServiceTable entry
   is the **logging** svc (`172dad59`), gated on `LogForwardingEnabled` — there is **no** GCS
   (`acef5661`) entry. `DefaultConnectSecurityDescriptor` is **LCOW-only** (host-dials-guest).
   Our doc already matches WCOW. hcsshim's `startExternalGcsListener` just `winio.ListenHvsock`s
   on `(runtimeID, WindowsGcsHvsockServiceID)` — same as ours.

4. **Remaining live theories (all in-guest — need in-guest visibility):**
   - SCM never starts `gcs` (`vmcomputeagent.exe`) — a driver/service dependency
     (`condrv`/`hvsocketcontrol`/`mpssvc`/`netsetupsvc`) fails to load in the NIC-less UVM.
     **Cheap high-probability next test:** re-add the `gcs.DependOnService` override (strip
     `mpssvc`/`netsetupsvc`) — the `encode_multi_sz_utf16le` helper already exists in `hcs.rs`
     (currently `#[allow(dead_code)]`). One e2e tells you.
   - `gcs` starts then exits/crashes before dialing.
   - To SEE which: restore the **6.B in-guest exfil channel** — writable `zlayer-debug` VSMB
     share + a boot-time SCM service that writes `sc query gcs` + `\Windows\ntbtlog.txt`
     (BCD `/bootlog`) into it; read host-side after the timeout via `read_uvm_debug_dump`
     (already in `hcs.rs`). The share + service were removed in B-4 (folded into `e85587ce`);
     restore from that diff. Risk: a create-time writable share previously correlated with a
     `0xEF` bugcheck — if it recurs that is itself a clean signal.

5. **Known-not-blocking:** 5 pre-existing macOS-only `zlayer-builder` unit-test failures
   (`lockfile_path_for_directory` asserts `/var/lib` but `lockfile_path_for` returns
   `dir.join(...)`; 4 `buildah::translate_*` tests). Test-only; not run by the pre-commit hook.

(Original handoff below is still accurate except where the four points above override it.)

---

## 0. Read these in order before doing anything

| Source | Why |
|---|---|
| `BlackLeafDocs/zlayer/windowshcs/gcs-bridge-and-0xEF.md` §6.1–6.11 | The full investigation log. §6.6/6.7 prove the doc is correct; §6.9 lists the 4 open theories. |
| `BlackLeafDocs/zlayer/windowshcs/gcs-bridge-and-0xEF.md` §4 header | Confirms current state pointer ("see §6 for 2026-05-31"). |
| `crates/zlayer-agent/src/runtimes/hcs.rs::build_virtual_machine_doc` | The UVM JSON doc emitter. Now verified field-identical to runhcs's UVM. |
| `crates/zlayer-gcs/src/bridge.rs::cold_start_create_start` | Listen-before-start, accept-after-start sequence. |
| `scripts/windows/debug_e2e.py` | The cycle driver. **Use it. Do not hand-write rsync+ssh loops.** |
| `scripts/windows/launch_e2e.ps1` | What runs on Windows. COM-pipe reader is in here (currently kept, see §6.10 of BlackLeafDocs for why). |
| `/home/zach/github/hcsshim` (on Linux box) and equivalent reference on Mac via `git clone https://github.com/microsoft/hcsshim` | The ground truth for every wire-format question. **Pin to the same tag as the prior clone if you re-clone — the API surface drifted recently.** |

---

## 1. What's NOT the problem (ruled out, do not re-investigate)

| Hypothesis | Status | Killed by |
|---|---|---|
| Wire-format malformation of RpcCreate | ❌ ruled out | run5/run6 byte capture matches `internal/gcs/prot/protocol.go::ContainerCreate` verbatim |
| Wrong `dump_type` (`Mini` vs `Full`) | ❌ ruled out | `Mini` → `HCS_E_INVALID_STATE`; `Full` is the only accepted value |
| Bridge not negotiating | ❌ ruled out | Negotiate frame succeeds; capabilities response received |
| `gcs.exe` missing from extracted layer | ❌ ruled out | §6.8 — 348 files match containerd's snap6 exactly; `gcs` service entry intact in SYSTEM hive |
| Missing api-ms-win-*.dll imports | ❌ ruled out | ApiSets resolved at load by kernel; not real files. Present on every working host too |
| `.vmrs` saying anything diagnostic | ❌ ruled out | undecodable format; vmwp save-on-stop, not a bugcheck channel. Public converters do not exist |
| `0xC0370103` HRESULT | ❌ ruled out | `errVmcomputeOperationPending = 0xC0370103`, **async-pending, NOT failure** (hcsshim `vmcompute.go:51`). Earlier session notes calling this a failure were wrong |
| `DebugOptions` / `GuestCrashReporting` causing crash | ❌ ruled out (B-5 / G.5 cycles) | doc-doc diff with these stripped also fails; runhcs ships them |
| `WindowsLoggingHvsockServiceID` ServiceTable entry presence | ❌ ruled out | runhcs ships exactly this one entry; we match (G.2-revert + G.2-restore) |
| `Uefi.Console="ComPort1"` redirect | ❌ kept OFF | breaks UVM boot; runhcs doesn't set it. The ComPort device itself is fine to keep for future debug |
| Unpacker dropping files (the "1236 vs 1768" panic) | ❌ ruled out (G.6 + B.1) | snap6 file count parity confirmed; hardlink replay landed in `unpacker.rs` |
| Wave 1.x edge_cache mirror impacting Hyper-V path | ❌ unrelated | edge_cache lives in zlayer-overlay; Hyper-V path doesn't touch it |

If a future session re-raises any of the above without **new evidence**, push back.

---

## 2. What IS the problem (current best framing)

UVM boots. SCM comes up. Either SCM never starts `gcs` (`vmcomputeagent.exe`), or it starts and exits/dials a different ServiceID than we host-listen on (`acef5661-84a1-4e44-856b-6245e69f4620`). The host's hvsock `accept()` never returns a frame, so `cold_start_create_start` times out and we tear the UVM down.

Four theories, ranked by likelihood given evidence:

1. **(Most likely) Race condition: host hvsock `listen()` happens *after* HCS Start, guest dials before the bind completes, fails fast with no retry.**
   - Evidence: hcsshim `internal/uvm/start.go` carefully orders listen-before-start; our `bridge.rs::cold_start_create_start` claims to do the same, but a prior session agent audit said "follows the same shape" — that's not the same as "verified by reading both side by side."
   - Action: do that side-by-side audit. Specifically compare:
     - hcsshim `internal/uvm/start.go::Start` (the `accept`-then-`HcsStartComputeSystem` order)
     - hcsshim `internal/gcs/bridge.go::Connect`
     - our `crates/zlayer-gcs/src/bridge.rs::cold_start_create_start`
     - our `crates/zlayer-agent/src/runtimes/hcs.rs::hyperv_create_via_gcs` (where it calls into `bridge.rs` vs where it calls `HcsStartComputeSystem`)

2. **SCM never starts `gcs` because a driver dependency fails to load.**
   - `gcs` `DependOnService = condrv\0hvsocketcontrol\0mpssvc\0netsetupsvc`
   - Action: BCD `/bootlog` mod (see §6 below). Reads `\Windows\ntbtlog.txt` after boot via a writable VSMB share.

3. **`vmcomputeagent.exe` starts, runs, and exits silently (clean `ExitProcess`, no WerFault).**
   - Action: `Devices.HvSocket.ServiceTable` could include an SDDL grant for a custom ServiceID we own, then drop a `RunOnce` registry entry into the SYSTEM hive that probes `Get-ItemProperty HKLM\…\Services\gcs` and writes to a writable VSMB share before SCM gets to `gcs`. Pre-empts the failure window.

4. **`vmcomputeagent.exe` dials a different ServiceID** than we host-listen on.
   - Evidence: weak — string search of `vmcomputeagent.exe` did NOT find `acef5661-…` as plain or raw-byte GUID, but did find `dcc079ae-…` (VSMB) and an unknown `f8615163-…` family. ServiceIDs can be computed at runtime, registry-driven, or string-different.
   - Action: ETW capture of the *guest* (not host) hvsock traffic. Needs the kernel-debugger ServiceID approach to get any in-guest visibility at all.

---

## 3. Mac dev-env bring-up

Mac has `mise`, no `uv`, no `~/.ssh/config`. Run these once:

```bash
# 3.1 — uv (the e2e driver is a /// uv-script ///)
mise use -g uv@latest      # or: curl -LsSf https://astral.sh/uv/install.sh | sh

# 3.2 — ssh shortcut for mini-windows (so the e2e driver's --host default still works)
mkdir -p ~/.ssh && chmod 700 ~/.ssh
cat >> ~/.ssh/config <<'EOF'
Host mini-windows
    HostName 192.168.68.92
    User MiniWindows
    ConnectTimeout 10

Host mini-windows-nb
    HostName 100.96.69.45
    User MiniWindows
    ConnectTimeout 10

Host mini-windows-dns
    HostName miniwindows.net.home
    User MiniWindows
    ConnectTimeout 10
EOF
chmod 600 ~/.ssh/config

# 3.3 — verify auth (uses ~/.ssh/id_ecdsa by default per ssh -G)
ssh mini-windows 'powershell -NoProfile -Command "$env:USERNAME + ` on ` + $env:COMPUTERNAME"'
# expect: MiniWindows on MiniWindows  (or whatever the hostname is now)
```

If that ssh fails because the LAN IP changed: try `mini-windows-nb` (Tailscale `100.96.69.45`) or `mini-windows-dns` (`miniwindows.net.home`). Both routes have been working as of 2026-05-31.

---

## 4. The one command you actually run

From the Mac, inside `~/github/ZLayer`:

```bash
uv run --script scripts/windows/debug_e2e.py \
    --test windows_hcs_hyperv_e2e \
    --timeout-min 20
```

That does, in order:

1. `rsync` the repo to `MiniWindows@192.168.68.92:C:/src/ZLayer/` (excludes `target/`, `.git`, etc.)
2. `cleanup_hcn.ps1` over SSH (clear leftover HCN endpoints + overlay nets)
3. `launch_e2e.ps1 -Test windows_hcs_hyperv_e2e` in a detached `cmd.exe` (survives SSH disconnect)
4. Poll `status_e2e.ps1` every 30s until the sentinel file appears or timeout
5. SCP the artifacts back to `~/.cache/zlayer-debug/run-<UTC timestamp>/`:
   - `stdout.log` — full cargo output
   - `rc` — exit code
   - `pid` — wrapper PID
   - HCS doc JSONs (UVM + container)
   - HCN endpoint dumps
   - COM-pipe reader output if it captured anything
6. Assemble `report.md` summarizing what happened

**Flags to know:**

| Flag | Purpose |
|---|---|
| `--no-rsync` | Re-run the same code without re-uploading. Idempotence check. |
| `--host MiniWindows@<other-ip>` | Override host. Useful if LAN IP changed. |
| `--test <other_test_name>` | E.g. `composite_dispatch_e2e` for the Part A (process-isolated + HCN overlay IP) suite. |
| `--timeout-min 5` | Short window for fast-fail iterations. |
| `--report-dir <path>` | Override `~/.cache/zlayer-debug/`. **Never use `/tmp` — it's tmpfs (RAM) and you'll lose iteration history.** |

**Cycle time:** ~3–5 min per run (compile on Windows + boot the UVM). Build incrementally — don't `cargo clean` between runs.

---

## 5. What to grep for in the report

```bash
# Successful host listen, no accept ever returns
grep -E "hvsock.*listen|listening on" ~/.cache/zlayer-debug/run-*/stdout.log

# The timeout that proves nobody dialed in
grep -E "WSAETIMEDOUT|WSA 10060|gcs.*timeout|accept.*timeout" ~/.cache/zlayer-debug/run-*/stdout.log

# Whether we got past Negotiate (we should — that part works)
grep -E "Negotiate|negotiated v4|capabilities" ~/.cache/zlayer-debug/run-*/stdout.log

# What the UVM JSON looked like (compare against ~/work/runhcs-real.json from G.6)
ls -la ~/.cache/zlayer-debug/run-*/hcs-docs/

# COM-pipe captures (currently kept enabled; usually 0 bytes because no Console redirect)
ls -la ~/.cache/zlayer-debug/run-*/com1.bin 2>/dev/null
```

**A successful run** (when we finally make one) will show:
1. host bind/listen at `acef5661-…` BEFORE `HcsStartComputeSystem`
2. `HcsStartComputeSystem` returns success
3. accept returns within ~5s
4. Negotiate exchange
5. RpcCreate of the container
6. RpcStart
7. Container PID printed
8. test asserts hit and pass

We currently get stuck between steps 2 and 3.

---

## 6. The three open diagnostic channels — pick one

These are mutually exclusive in terms of next-iteration scope. Pick the cheapest one first.

### 6.A — Listen-before-start ordering audit (no Windows work needed)

**Cost:** 0 e2e runs to start. Pure code reading on the Mac.
**Action:**
1. Open all four files in a wide editor:
   - `~/github/hcsshim/internal/uvm/start.go`  (clone fresh on Mac if needed)
   - `~/github/hcsshim/internal/gcs/bridge.go`
   - `~/github/ZLayer/crates/zlayer-gcs/src/bridge.rs`
   - `~/github/ZLayer/crates/zlayer-agent/src/runtimes/hcs.rs` (`hyperv_create_via_gcs`)
2. Write line-by-line what each does between `HcsCreate` and `HcsStart`. Specifically: when is `winio.ListenHvsock` (Go) / `HvSockListener::bind` (Rust) called, when is `HcsStartComputeSystem` called, when is `accept` called.
3. **If our order is wrong**, fix and re-run e2e. Cheap.
4. **If our order matches hcsshim**, eliminate theory (1) and move to 6.B or 6.C.

### 6.B — BCD `/bootlog` + writable VSMB exfil (diagnostic depth: medium)

**Cost:** 1 scaffolding session (~half a day) + ~5 e2e iterations.
**Goal:** confirm/deny theory (2) — whether all of `condrv`, `hvsocketcontrol`, `mpssvc`, `netsetupsvc` load.
**Action:**
1. Add code to `crates/zlayer-agent/src/runtimes/hcs.rs::build_uvm_registry_changes` (or a new sibling) that, after `ProcessUtilityImage` mounts the scratch VHDX, offline-edits the guest's BCD store on the scratch:
   ```
   bcdedit /store <scratch>\EFI\Microsoft\Boot\BCD /set {default} bootlog Yes
   ```
2. Restore the `zlayer-debug` writable VSMB share (was removed in B-4 — re-add via the same hot-attach path hcsshim uses for layer shares; ordering matters, hot-attach AFTER boot).
3. Add a RunOnce SYSTEM-hive entry that copies `C:\Windows\ntbtlog.txt` to the writable share on first user-mode SCM tick.
4. Run e2e, fetch ntbtlog.txt, read which drivers/services loaded vs failed.
5. If `condrv` or `hvsocketcontrol` is in the failed list → that's the bug.

### 6.C — In-guest kernel debugger over hvsock (diagnostic depth: maximum)

**Cost:** 1 day scaffolding + windbg license/install on Mac (or run windbg on a Windows VM).
**Goal:** see exactly what `vmcomputeagent.exe` does after SCM starts it.
**Action:**
1. Add a second ServiceTable entry in `Devices.HvSocket.ServiceTable` for the kernel debugger ServiceID (look it up in hcsshim — there's a constant).
2. Offline-edit the BCD store to enable kernel debugging over hvsock with that ServiceID.
3. Boot UVM, attach windbg from host (or from the Mac through a Windows VM).
4. Set a breakpoint on `vmcomputeagent!main` (or the C++ class entrypoint visible in the binary's ASCII strings: `ComputeService::Management::*`).
5. Step.

**This is the nuclear option.** Use 6.A and 6.B first.

---

## 7. The loop-closing action when it finally works

Once a Windows container actually starts via `windows_hcs_hyperv_e2e`:

1. **Refresh the zql hcs.rs override** (currently the only entry in `scripts/zql/.drift-allowlist`):
   ```
   cd ~/github/zlayer-zql                                # branch: zdb
   uv run scripts/zql/regenerate.py --source ~/github/ZLayer
   # confirm: OVERRIDE DRIFT: 0 drifted
   ```
   If `hcs.rs` still drifts after the fix lands, run a rust-expert agent over JUST that file with `crates/zlayer-agent/src/runtimes/hcs.rs` + its `.baselines/` sibling. Same shape as the Wave 1/2/3 refreshes that landed 2026-05-31.

2. **Remove `crates/zlayer-agent/src/runtimes/hcs.rs` from `scripts/zql/.drift-allowlist`**. The CI gate at `scripts/zql/overlay/.forgejo/workflows/regen-from-dev.yml` will then enforce no drift on hcs.rs going forward.

3. **Update CHANGELOG.md** under `[Unreleased]` with whatever the fix turned out to be. Concrete, not vague.

4. **Workspace checks** (entire workspace, never `-p <crate>`):
   ```
   cd ~/github/ZLayer
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   cargo build --workspace
   ```

5. **Single-line commit**, no body, no Co-Authored-By:
   ```
   git commit -m "fix(hcs): <concrete one-liner about what made the guest dial>"
   ```

6. **Do not push** unless explicitly asked.

---

## 8. House rules (ironclad, see `~/.claude/CLAUDE.md`)

- **NEVER `git stash`** (any variant). Use `git diff HEAD <path>` for pre-edit comparisons.
- **Never canonicalize local image tags.** `zlayer build -t NAME:TAG` stores under literal `NAME` — never run through `oci_client::Reference`.
- **Never hand-edit Cargo.toml deps.** Use `cargo add`/`cargo remove`/`cargo update -p`.
- **Never hardcode versions** — `gh release list -R <repo>`, `cargo search`, etc.
- **All checks GREEN, workspace-wide.** Never `-p <crate>`.
- **CHANGELOG: `[Unreleased]` header**, never invent versions.
- **Commits: one-liner only**, no bodies, no trailers.
- **Don't push without asking.**
- **Foreground agents only**, model `opus`, one small task each, verify after every one. Never `run_in_background=true`.
- **/tmp is RAM** (tmpfs). Sockets there fine; state, databases, caches, rootfs — never. Default report-dir is `~/.cache/zlayer-debug/` for a reason.
- **zlayer-zql lives on branch `zdb`** of the same Forgejo repo. Push with `git push origin zdb`, not `dev`. Files under `scripts/zql/overrides/...` are STICKY — editing only the generated copy gets wiped on next regen.
- **Never defer / "out of scope."** Finish the whole task or stop and state the concrete blocker.

---

## 9. State of the world as of this handoff

| Component | Status |
|---|---|
| ZLayer `dev` | green (fmt/clippy/test/build all pass workspace-wide). Latest commit on origin/dev: `e85587ce feat: Track A edge_cache API + Track B unpacker hardlink + Hyper-V Phase G doc compliance + COM-pipe scaffolding`. Mac may have local commits `a80d22d3` / `f16477f0` / `0b180574` (install-dev for mac) ahead of origin/dev. |
| zlayer-zql `zdb` | green. Latest commit on origin/zdb: `4e18a5cc fix(zql): clippy doc-markdown + format-collect + large_futures + compose project-prefixed build tags`. CI drift gate landed; `hcs.rs` is the one allowed drift. |
| BlackLeafDocs `main` | up to date. `gcs-bridge-and-0xEF.md` §6 has the full 2026-05-31 investigation log. |
| Hyper-V Windows containers (Part B) | **Not working.** Negotiate succeeds; guest never dials post-Negotiate. See §2 of this doc. |
| Process-isolated Windows containers + HCN overlay (Part A) | **Working.** 5/5 tests, real IP `10.220.99.2`, runs twice cleanly. |
| Linux containers (Part 0) | Working, unchanged. |

---

## 10. Pending tasks (filtered to "what to actually do next")

| ID | What | Why it matters |
|---|---|---|
| #69 (in-progress) | Validate `d89a81d2` e2e + iterate to frame#1 | The whole point of this handoff. |
| #57 (pending) | Zatabase: implement `zserver` caller (`edge_cache.rs` + `manager.rs` wire) | Track A is half-landed — ZLayer side ships, zserver side doesn't call it yet. |
| #60 (pending) | B.1-bis: investigate why hardlink fix didn't recover files (OCI tar inspection) | Diagnostic; not currently blocking anything because §6.8 confirmed file-count parity from a different angle. |
| #45 (in-progress) | zserver Wave 1.3.3.6 (edge_cache caller) blocked on ZLayer API surface | Same as #57 — different framing. |
| #46 (pending) | zserver Wave 1.3.3.7: 3-node integration test | Blocked on #45/#57. |
| #38 (pending) | Phase H: revert COM-pipe scaffolding | Currently kept for future debug; not blocking anything. Revert only if it bloats the doc later. |

**Do #69 first.** Everything else can wait.

---

## 11. If you get totally stuck

The fastest path to unsticking yourself, in this order:

1. **Re-read §6.6 of BlackLeafDocs/zlayer/windowshcs/gcs-bridge-and-0xEF.md** — that's where the doc-equality proof lives. It will remind you that the JSON is correct and stop you from re-investigating the doc.
2. **Re-run `--no-rsync` mode** to rule out a transient Windows-side issue (host's HCN service hung, leftover overlay net, etc.).
3. **Ask the user for a windbg session.** The kernel-debugger ServiceID approach (§6.C) is the last channel; user can drive it interactively if needed.
4. **Don't trust this doc blindly.** Memory rot is real. If something here disagrees with the current code, **the code wins.** Update this doc.

End of handoff.
