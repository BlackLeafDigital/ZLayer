## Phase F — Windows native multi-stage + WSL2 Linux

### Goals

1. Lift the single-stage gate in `WindowsBuilder` so multi-stage WCOW Dockerfiles work natively on a Windows host, including cross-stage `COPY --from=<stage>` with NTFS ACL / reparse / EA preservation.
2. Implement real `manifest_create` / `manifest_add` / `manifest_push` on `HcsBackend` so a Windows host can compose OCI image indexes (and, paired with a Linux/macOS peer that produces the Linux blob, push multi-arch manifest lists).
3. Route `target_os = ImageOs::Linux` on a Windows host through the existing `zlayer-wsl` distro to a Linux `zlayer` instance running the Phase A/B/C native executor, replacing the `BuildError::BuildahNotFound` stub at `crates/zlayer-builder/src/backend/mod.rs:169-173`.
4. Confirm there is no HCN analogue of the buildah/netavark version skew on the build-time RUN path.

### Steps

Each step is ≤ 2 files, ≤ ~250 LOC. Hours are work-time, not wall-clock. Steps marked [WIN-HOST] require running on the MiniWindows CI runner per `docs/windows-ci-runner.md`.

1. **Skeleton: multi-stage IR plumbing on `BuildSkeleton`.** (3 h) — `crates/zlayer-builder/src/windows_builder.rs:413-460` (struct `BuildSkeleton`) + the gate at `:631-645`. Replace `working_layer_chain_dir` / `working_chain` / `image_config` with a `Vec<StageState>` keyed by stage index. Keep the single-stage call path working by treating it as `stages.len() == 1`. No HCS calls in this step; pure refactor + new unit tests on Linux CI.

2. **Skeleton: per-stage base materialisation loop.** (3 h) — Same file, around `:622-720`. Walk all `parsed.stages` and call `pull_and_materialise_base` per stage (cache key by base ref so two stages sharing a base reuse the unpacked chain). Reject only `FROM scratch` and unknown stage names; allow `DockerfileFromTarget::Stage(name)` to resolve to the by-name `StageState`. Tests: a 3-stage parse on Linux CI exercises the resolver without touching HCS.

3. **Per-stage instruction loop.** (3 h) — `windows_builder.rs` `build_image_for_backend` at `:1135-1300`. Iterate stages in order; inside each stage iterate `stage.instructions`. The current state used by `apply_run` / `apply_copy` / `apply_add` becomes `current_stage_state` instead of skeleton-wide fields. Off-Windows: keep the same NotSupported gating for RUN; on Windows the loop already works because the underlying HCS ops are per-layer.

4. **Cross-stage COPY: finalise source stage to an exportable layer chain.** [WIN-HOST] (4 h) — New helper `finalize_stage_for_export(&StageState) -> Result<LayerChain>` in `windows_builder.rs`. After the last instruction of a stage runs, the stage's `working_chain` IS its final RO layer chain (each RUN / COPY already commits one RO layer via `wclayer::import_layer` at `:2802`). Returns the borrow of that chain plus the on-disk `layer_path` of the topmost layer. No new HCS plumbing — just bookkeeping.

5. **Cross-stage COPY: export source via `wclayer::export_layer`.** [WIN-HOST] (4 h) — New helper `export_stage_files(stage: &StageState, export_dir: &Path) -> Result<()>` calling `wclayer::export_layer(top_layer_path, export_dir, parent_chain, "{}")` (`crates/zlayer-agent/src/windows/wclayer.rs:200-217`). The export folder contains the canonical wclayer staging layout — `Files\…` entries with BackupRead-framed payloads, exactly the format `BackupStreamReader` at `crates/zlayer-agent/src/windows/layer.rs:376-409` knows how to read.

6. **Cross-stage COPY: source-path selection.** (3 h) — Pure path logic in `windows_builder.rs`. Implement glob matching against `export_dir/Files/<src>` honouring Dockerfile COPY semantics (single source, multiple sources, directory recursion, trailing-slash dest interpretation). Mirror the existing `resolve_copy_sources` shape at `:1895-1960`. All-Linux unit tests (no HCS).

7. **Cross-stage COPY: BackupStream-preserving write into destination stage.** [WIN-HOST] (5 h) — In a new file `crates/zlayer-builder/src/cross_stage_copy.rs` (≤ 250 LOC). For each selected source file under `export_dir/Files/…`, open it with `BackupStreamReader::open` and stream the raw BackupRead bytes into a `BackupStreamWriter::create_new` under the destination stage's working scratch layer. The framing IS the ACL + reparse + EA payload; preserving it byte-for-byte is exactly what we need. Confirms the ACL-preservation goal because the `BACKUP_SECURITY_DATA` stream rides through unchanged.

8. **Cross-stage COPY: directory recursion + tombstones.** [WIN-HOST] (3 h) — Same new file. Walk the source subtree, skip whiteout / tombstone files that hcsshim's exporter writes (`Files\$Recycle.Bin` style and the `.wh.` prefix used for deletions inside `Hives\…`). Tombstones from an intermediate layer must NOT propagate via cross-stage COPY — that would erase files in the destination's base. Mirrors hcsshim `legacyLayerWriter.Add` deletion-tag handling.

9. **Lift the multi-stage gate.** (2 h) — `windows_builder.rs:637-645` and the mirror at `:1158-1166`. Replace `Err(NotSupported)` with an info-level `tracing` event and the loop from step 3. Update the test at `:4618-4651` to exercise a 2-stage build that succeeds off-Windows up to the point of HCS contact and then surfaces `NotSupported` only from the base-materialise path, not the stage-count path. The unit test at `:3585-3604` (multi-stage `COPY --from` rejection) is rewritten to assert the new positive behaviour.

10. **Manifest list: in-memory model + persistence.** (3 h) — New module `crates/zlayer-builder/src/backend/hcs/manifest_list.rs` (≤ 250 LOC). Define `ManifestList { name, entries: Vec<ManifestListEntry { digest, size, platform: { os, arch, os_version } } > }`. Persist as JSON under `<storage_root>/manifest-lists/<name>.json`. Implement `manifest_create` (write empty list), `manifest_add` (resolve image tag → local OCI manifest digest via the existing local OCI store written by `WindowsBuilder`, append entry), and a `manifest_load`. Pure-Rust, fully unit-testable on Linux CI.

11. **Manifest list: push via existing `zlayer-registry` index path.** (3 h) — `crates/zlayer-builder/src/backend/hcs/mod.rs:197-201`. Replace the `NotSupported` stub with a call into `zlayer_registry::client::push_image_index` (the helper already exists per `crates/zlayer-registry/src/client.rs:1661,1715`). The Windows host pushes ONLY entries whose blobs it has locally; Linux entries are pushed by the Linux peer and then referenced by digest in the index. Document the cross-host workflow in the rustdoc on `manifest_add`.

12. **WSL2 routing: detect_backend wiring.** [WIN-HOST] (3 h) — `crates/zlayer-builder/src/backend/mod.rs:166-179`. Replace the `Linux => Err(...)` arm with a call into a new `WslLinuxBackend::new()`. Use `zlayer_wsl::distro::distro_exists()` (`crates/zlayer-wsl/src/distro.rs:88`) and `zlayer_wsl::setup::ensure_wsl_backend_ready()` (`crates/zlayer-wsl/src/setup.rs:18`) to confirm the `zlayer` distro is installed; otherwise return a typed error pointing the user at `zlayer wsl install` (an existing CLI surface — see `bin/zlayer/src/commands/`).

13. **WSL2 routing: `WslLinuxBackend` implements `BuildBackend`.** [WIN-HOST] (5 h) — New file `crates/zlayer-builder/src/backend/wsl.rs` (≤ 250 LOC). `build_image` shells the context dir into the WSL distro: copy via `\\wsl$\zlayer\…` UNC (use `zlayer_wsl::paths::windows_to_wsl` at `crates/zlayer-wsl/src/paths.rs:39` to translate paths). Invoke `wsl_exec("zlayer", &["build", "--backend=native", ...])` from `crates/zlayer-wsl/src/distro.rs:63`. Stream stdout/stderr lines as `BuildEvent`s into the host TUI. Pull the resulting `BuiltImage` JSON back over stdout. `push_image` / `tag_image` / manifest ops forward similarly. `is_available` runs `distro_exists`.

14. **HCN version-skew probe + RUN-time network attach decision.** [WIN-HOST] (2 h) — `windows_builder.rs` RUN path around `:2580-2750`. Read this for whether the ephemeral compute system attaches an HCN endpoint at all. WCOW build RUN steps default to NAT via the existing host `nat` HCN network created at install time; there is no "build network" the way buildah has on Linux, so the netavark-shaped version-skew bug class does not exist here. Add a startup probe in `HcsBackend::is_available` that calls `zlayer_hns::list_networks` once and warns (not errors) if HCN v2 is unreachable. Document the finding in rustdoc and CHANGELOG.

15. **CHANGELOG + cross-platform unit tests.** (2 h) — Update `CHANGELOG.md` under `[Unreleased]` (per the memory rule, never invent a version). Add a Linux-CI test in `crates/zlayer-builder/tests/` that parses a 3-stage Dockerfile, walks the stage loop until the `NotSupported` base-materialise gate fires, asserts each stage's state transitions, and asserts the `apply_copy` path now accepts `--from=<stage>` instead of rejecting it.

### Multi-stage cross-VHD COPY algorithm

Per-instruction commit (already in place at `windows_builder.rs:2802`) means every stage finishes with an ordered chain of fully-imported RO layers under `<build_dir>/unpacked/<layer-id>/`. The "VHD" framing in the prior scoping is a red herring on Windows — `HcsExportLayer` against an RO layer reads file content + BackupRead framing directly via the layer filter, returning a canonical wclayer staging tree under `export_dir/Files/…`. No raw VHD parsing, no offline-VHD-mount dance.

The algorithm:

1. Source stage finishes; its top layer path is `top := stage.working_chain.last().layer_path`.
2. `wclayer::export_layer(top, export_dir, stage.working_chain (as parent_chain), "{}")` — produces `export_dir/Files/...`.
3. For each selected source path, open with `BackupStreamReader::open` (`crates/zlayer-agent/src/windows/layer.rs:390`).
4. Open the destination scratch layer file with `BackupStreamWriter::create_new` (`:319`).
5. `std::io::copy` between them. The bytes flowing through ARE the `WIN32_STREAM_ID`-framed `BACKUP_SECURITY_DATA` + `BACKUP_DATA` + (if present) `BACKUP_REPARSE_DATA` + `BACKUP_EA_DATA` stream. ACLs ride along by construction.
6. Skip tombstones / Hives delete markers in the source tree.
7. After all files copy, commit the destination scratch into a new RO layer via the existing `wclayer::export_layer` → tar+gzip → `wclayer::import_layer` cycle at `:2752-2803`. That path is unchanged.

This is genuinely simpler than the "cross-VHD" framing suggested because hcsshim's `HcsExportLayer` is the official primitive for reading from a finalised layer.

### WSL2 routing model

- Host-side `WslLinuxBackend` is a thin RPC: serialise BuildOptions → JSON, `wsl.exe -d zlayer -- zlayer build --json-events --context /mnt/host/<path> ...`. The Linux-side `zlayer` is the same binary, built for `x86_64-unknown-linux-gnu`, installed into the distro at `zlayer wsl install` time.
- Context transfer uses `\\wsl$\zlayer\…` UNC paths (`zlayer_wsl::paths` already handles the translation). Buffers stream through the kernel's 9P bridge, not over the network, so there is no need for a dedicated transport.
- Push / pull / manifest ops forward to the Linux daemon and surface its stdout/stderr verbatim.
- Manifest lists composing Linux + Windows blobs: the Windows host writes the index referencing both digests; it has the Windows blob locally and references the Linux digest pushed by the WSL2 instance.

### Files to modify

- `crates/zlayer-builder/src/windows_builder.rs` (gate removal, stage loop, COPY --from dispatch, BuildSkeleton refactor)
- `crates/zlayer-builder/src/backend/hcs/mod.rs` (real manifest ops, WSL availability)
- `crates/zlayer-builder/src/backend/mod.rs:166-179` (WSL routing)
- `CHANGELOG.md`

### Files to create

- `crates/zlayer-builder/src/cross_stage_copy.rs`
- `crates/zlayer-builder/src/backend/hcs/manifest_list.rs`
- `crates/zlayer-builder/src/backend/wsl.rs`
- `crates/zlayer-builder/tests/multi_stage_parse.rs`

### Tests (Windows-host required)

- Two-stage build: stage1 nanoserver writes `C:\artifact.txt`, stage2 nanoserver `COPY --from=0 C:\artifact.txt .` — assert ACL on resulting file equals the source ACL. [WIN-HOST]
- Three-stage build with `COPY --from=builder` AND `COPY --from=tools` interleaved. [WIN-HOST]
- `manifest_create` + two `manifest_add`s (one Windows, one synthesised Linux digest) + `manifest_push` to a local zot registry. [WIN-HOST]
- WSL2 routing: from a Windows host, `zlayer build --platform linux/amd64 .` produces a Linux image that pulls and runs on a Linux peer. [WIN-HOST + Linux peer]
- Negative: stage name typo on `COPY --from=builderr` surfaces the existing `BuildError::stage_not_found`.
- Linux-CI: 3-stage parse + stage state machine without HCS contact.

### Risks (the 80/20 trap)

The prior scoping called this HIGH risk on "cross-VHD BackupStream subtleties." After reading `wclayer.rs` + `backuptar.rs` + `layer.rs` the real picture is:

1. **`HcsExportLayer` is the official primitive — there is no raw VHD parsing.** The ZLayer wrappers already use it for per-RUN commits. Cross-stage COPY is the same export, just consumed differently. Risk: MEDIUM, not HIGH.
2. **Whiteout / tombstone handling is the actual subtlety.** Hcsshim emits delete markers in `Hives\` and as `$wcifs.something` empty entries; failing to filter them propagates "delete this file" into the destination stage and corrupts its base. Mitigation: explicit allowlist of `Files\…` real entries, mirror hcsshim's `legacyLayerWriter.Add` filter.
3. **Long paths in `WinSxS` and deeply nested .NET assembly trees.** The `\\?\` extended-length prefix is already handled by `to_extended_wide` (`layer.rs:47`); cross-stage paths must consistently route through it. Mitigation: every new path crossing the FFI boundary goes through that helper.
4. **`SeBackupPrivilege` / `SeRestorePrivilege` already enabled** (`enable_backup_restore_privileges`, `layer.rs:193`). No new privilege work.
5. **HCN version skew: NOT a problem.** Confirmed: WCOW build RUN uses the host `nat` HCN network created at install time; HCN v2 is in-tree (`Microsoft-Windows-Containers` package) and shipped with the OS, not a separately-versioned daemon like netavark. No analogue of the Linux skew bug.
6. **WSL2 distro lifecycle.** Distro must be present + `zlayer` binary installed in it. If the user has uninstalled the distro mid-build, every op fails with cryptic stderr. Mitigation: `WslLinuxBackend::is_available` runs `distro_exists` + a `zlayer --version` probe before announcing readiness.

### Verification

- `cargo fmt --all` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` green on Linux CI (all stage-loop + manifest-list logic is exercised there).
- `cargo test --workspace --target x86_64-pc-windows-msvc` green on the MiniWindows CI runner; Windows-only tests at `#[cfg(target_os = "windows")]` cover the BackupStream cross-stage COPY paths.
- Manual: build the existing `images/ZImagefile.zlayer-node` (currently single-stage) plus a new multi-stage example under `examples/multi-stage-windows/` and confirm the resulting image runs `cmd.exe /c dir C:\artifact` cleanly.
- WSL2 routing manual test: on the Windows host, `zlayer build --platform linux/amd64 examples/hello-linux/` produces an image identical (by digest) to one built directly on a Linux peer of the same arch.

### Estimated effort

- Total: ~45 work-hours / 5–7 working days for a single engineer with full Windows-host access.
- Prior scoping said 4–6 person-weeks at HIGH risk; the actual primitive (`HcsExportLayer`) is already wrapped, so realistic estimate is 1.5–2 person-weeks at MEDIUM risk. The dominant cost is iteration time on the MiniWindows runner, not novel kernel-API work.
- Critical path: Steps 1 → 3 (refactor) before any cross-stage work; Steps 7 → 8 (BackupStream COPY + tombstone filter) are the highest-leverage tests to write first. Manifest list (10–11) and WSL routing (12–13) are parallelisable with the multi-stage work because they share no files.
