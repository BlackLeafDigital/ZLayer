# TODO: Close the HCS Windows-builder gaps

> Working doc for sub-agent dispatch. Phased plan A → B → C → D, with cross-phase parallelism. Delete this file once all phases ship.

## Context

The macOS `crates/zlayer-builder/src/sandbox_builder.rs` translates `apt`/`apk` package commands to Homebrew because Seatbelt is a security sandbox running on the macOS host, not a Linux container runtime. Windows HCS is a **real** container runtime — `RUN choco install foo` runs natively in a Windows guest — so no Chocolatey-translation layer is needed.

ZLayer already builds on Windows (MiniWindows runner, `docs/windows-ci-runner.md`), but only for **single-stage, never-cached, no-preflight, no-pre-baked-toolchain-image** builds. This doc breaks the four real gaps into sub-agent-sized tasks.

## Confirmed decisions

1. **Phase order:** A → B → C → D, sequentially as phase milestones. Within and across phases, parallelize where the dep graph permits.
2. **Multi-stage `COPY --from` strategy:** **hybrid** — mount prior stage scratch layers read-only for the active build (Phase A); separately, support exporting stage diffs to long-lived layer blobs for cache reuse across builds (Phase C, used heavily by Phase D toolchain images).
3. **Phase D toolchain images:** `zlayer-windows-rust` and `zlayer-windows-node`. **No `zlayer-windows-python`** — boringtun overlay should work on Windows the same way Netbird's userspace WireGuard does, so Python tooling rides on overlay-mounted shared infra. If overlay-on-Windows turns out broken, that's a separate workstream.
4. **Cache key granularity:** instruction-level Docker-style: `SHA-256(parent_layer_digests || stage_idx || instruction_idx || canonical_instruction || copy_source_digests)`.

## Solid foundation already in place (do not rebuild)

- **Per-RUN ephemeral compute systems** with UUID system IDs and explicit teardown (`backend/hcs/exec.rs:75, 88-107`).
- **No host bind-mounts into guest** (`exec.rs:171-175`, `mapped_directories: Vec::new()`).
- **COPY/ADD path-traversal guards** (`exec.rs:316-334`, `342-359`).
- **Per-build unique directory + per-instruction unique system ID** (`mod.rs:237, 623-637`).
- **Base-image blob cache via `ImagePuller` + `zlayer_registry::CacheType`** (`mod.rs:160-172`).

## Cross-phase parallelism map

```
                Phase A (multi-stage)         Phase B (preflight)        Phase C (cache)            Phase D (images)
                ===========================   =======================    ========================   ========================
Time → A.1 ─┬─ A.2 ─┬─ A.5 ─┬─ A.7 ─ A.8     B.1 (independent)         C.1 design (parallel A)    D.1, D.2 author (par. A)
            │       ├─ A.3 ─┤                  └─ B.2 ─┬─ B.4 ─ B.6                                  └─ D.3 ─ D.4 ─ D.5
            │       ├─ A.4 ─┤                          └─ B.3                C.2, C.3, C.4 ─ C.5 ─┬─ C.7
            │       └─ A.6 (par. A.5)                                                              └─ C.6 (par. C.5)
            └─ docs touched by all phases
```

Tasks in the same column are sequential within their phase. Tasks across columns at the same row can run in parallel sub-agents.

## Sub-agent dispatch rules (per `CLAUDE.md`)

- **Model:** every Agent invocation MUST use `model: "opus"` (Opus 4.6).
- **Foreground only:** never `run_in_background: true`. Parallel foreground agents in a single message are fine.
- **One task per agent:** 1-2 files maximum. No giant multi-file rewrites.
- **Verify after each agent:** read the changed files, confirm the diff matches the prompt, then run the workspace gates: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo build --workspace`. Every failure across the workspace is the orchestrator's responsibility, even in untouched crates.
- **Never `git stash`** in agent prompts. Include the no-stash rule near the top of every agent prompt.
- **Windows E2E gate is separate:** `cargo test -p zlayer-builder --test windows_build_e2e -- --ignored` runs only on the MiniWindows runner; orchestrator dispatches via Forgejo workflow rather than locally.

---

# Phase A — Multi-stage Windows builds

**Goal:** Lift the `stages.len() != 1` rejection at `backend/hcs/mod.rs:206-214` and make `FROM ... AS builder` + `COPY --from=builder` work end-to-end on a Windows host.

**Why first:** Largest user-visible feature win. Unblocks the Microsoft-recommended `servercore`-builder → `nanoserver`-final pattern. Required by Phase D toolchain images.

**Estimated scope:** Medium. ~6 sub-agent commits, plus tests.

**Files affected:**
- `crates/zlayer-builder/src/backend/hcs/mod.rs` (dispatch loop, single-stage gate)
- `crates/zlayer-builder/src/backend/hcs/scratch.rs` (per-stage scratch provisioning)
- `crates/zlayer-builder/src/backend/hcs/exec.rs` (`COPY --from` resolution)
- `crates/zlayer-builder/src/backend/hcs/commit.rs` (final-stage-only commit)
- `crates/zlayer-builder/src/windows/deps.rs` (per-stage validator audit)
- `crates/zlayer-builder/tests/windows_build_e2e.rs` (replace rejection test, add positive multi-stage test)

## Task A.1 — Refactor `prepare_base_chain` into a per-stage primitive

**Files:** `crates/zlayer-builder/src/backend/hcs/scratch.rs`
**Depends on:** nothing
**Parallelizable with:** B.1, C.1, D.1, D.2
**What:** Today `scratch.rs` assumes one base chain per build. Split it into:
- `pub async fn provision_stage_scratch(&self, build_id: &str, stage_idx: usize, base_image: &ImageReference) -> Result<StageScratch>` — returns a struct holding `parent_layer_paths: Vec<PathBuf>`, `scratch_root: PathBuf`, `unpacked_root: PathBuf`, `base_image_config: ImageConfigJson`.
- Each stage gets its own subdirectory: `storage_root/builds/<build_id>/stages/<idx>/{parent-chain,scratch,unpacked-root}`.
- Privilege enablement (`enable_backup_restore_privileges`) moves into a once-per-build helper.

**Acceptance:**
- Single-stage path keeps working — refactor is internal-only.
- New `StageScratch` struct documented with per-field semantics.
- All existing tests in `windows_build_e2e.rs` and unit tests still pass on MiniWindows.

**Sub-agent prompt outline:**
> opus 4.6 agent. Work on `crates/zlayer-builder/src/backend/hcs/scratch.rs` only. Do not touch any other file. Never `git stash`. Refactor the existing `prepare_base_chain` flow into a per-stage primitive named `provision_stage_scratch` that takes `(build_id, stage_idx, base_image)` and returns a new `StageScratch` struct with the four fields named above. Keep the existing single-stage call site (`mod.rs:279-291`) working — adjust the call to pass `stage_idx = 0`. Run `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo build --workspace` and report back with the diff and gate results.

## Task A.2 — Multi-stage dispatch loop in `mod.rs`

**Files:** `crates/zlayer-builder/src/backend/hcs/mod.rs`
**Depends on:** A.1
**Parallelizable with:** A.3, A.4, A.6
**What:** Replace the rejection at lines 206-214 with a stage-iteration loop. Each iteration calls `provision_stage_scratch`, runs the stage's instructions, and tracks the resulting scratch handle in a `Vec<StageScratch>`. Only the **final** stage produces an OCI commit; intermediate stages' scratches stay alive until the build completes, then get destroyed.

**Acceptance:**
- Two-stage dockerfile dispatches without error.
- Orchestrator owns `Vec<StageScratch>` and tears them down in reverse order on success or failure.
- Build IDs and per-stage directories don't collide.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/src/backend/hcs/mod.rs` only, no `git stash`. Replace the single-stage rejection at lines 206-214 with a stage iteration loop that calls `scratch::provision_stage_scratch` per stage and collects results into `Vec<StageScratch>`. Only the final stage commits to OCI. Pass the `Vec` reference into `exec::execute_instructions` so `COPY --from` can resolve. Add stage-level teardown on Drop or explicit cleanup. Run the full workspace gates and report.

## Task A.3 — `COPY --from=<stage>` resolution in `exec.rs`

**Files:** `crates/zlayer-builder/src/backend/hcs/exec.rs`
**Depends on:** A.1
**Parallelizable with:** A.2, A.4, A.6
**What:** Lift the rejection at `exec.rs:513-518`. Implement resolution by:
- Looking up the named (or indexed) prior stage in the `&[StageScratch]` slice passed in from A.2.
- Reading source files through that stage's `scratch_root` path (WCIFS already attached during the build).
- Re-using the existing `resolve_dest_in_scratch` for destination handling.
- Path-escape guard against the prior stage's scratch root, identical in spirit to the build-context guard.

**Acceptance:**
- `COPY --from=<name>` and `COPY --from=<index>` both work.
- `COPY --from=<missing>` returns a clear `BuildError` listing available stage names.
- Path-traversal attempts (`COPY --from=builder ..\\..\\windows\\system32\\config\\sam dst`) are rejected.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/src/backend/hcs/exec.rs` only, no `git stash`. Implement `COPY --from=<stage>` by accepting a `prior_stages: &[StageScratch]` parameter on the relevant exec functions. Resolve sources against the prior stage's `scratch_root`, reject path escapes the same way `resolve_src_in_context` does. Replace the rejection at lines 513-518. Add a `BuildError::UnknownCopyFromStage { name, available: Vec<String> }` variant if needed. Run workspace gates and report.

## Task A.4 — Per-stage `ImageConfigBuilder`, only-final-stage commits

**Files:** `crates/zlayer-builder/src/backend/hcs/commit.rs`
**Depends on:** A.1
**Parallelizable with:** A.2, A.3, A.6
**What:** Each stage maintains its own `ImageConfigBuilder` initialized from that stage's base-image config. Earlier stages' builders accumulate metadata for the duration of the stage but their output is **discarded**. Only the final stage's builder produces the manifest + config + layer blob.

**Acceptance:**
- A two-stage dockerfile produces exactly one OCI image, derived from the final stage's base, with metadata only from the final stage's instructions.
- Final-stage `os.version` is taken from the final stage's base, not from any builder stage.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/src/backend/hcs/commit.rs` only, no `git stash`. Make `ImageConfigBuilder` instantiable per-stage. Document via doc-comment that intermediate-stage builders are discarded. The orchestrator in `mod.rs` (already changed in A.2) passes only the final stage's builder to commit. Run workspace gates and report.

## Task A.5 — Wire dispatch and end-to-end smoke

**Files:** `crates/zlayer-builder/src/backend/hcs/mod.rs` (final wiring)
**Depends on:** A.1, A.2, A.3, A.4
**Parallelizable with:** A.6
**What:** Glue task. Compose the new `provision_stage_scratch`, the multi-stage exec loop, the per-stage builders, and `COPY --from`. Add structured `BuildEvent::StageStart`/`BuildEvent::StageEnd` emissions for the TUI.

**Acceptance:**
- A two-stage build emits two `StageStart`/`StageEnd` event pairs.
- Final image runs (`zlayer run` against the produced OCI image) on MiniWindows.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/src/backend/hcs/mod.rs` only, no `git stash`. Compose the multi-stage pipeline assuming A.1-A.4 are merged. Emit `BuildEvent::StageStart { idx, name }` and `BuildEvent::StageEnd { idx, name, success }`. Run workspace gates and report.

## Task A.6 — Per-stage validator audit in `windows/deps.rs`

**Files:** `crates/zlayer-builder/src/windows/deps.rs`
**Depends on:** nothing (validator already iterates stages)
**Parallelizable with:** A.2, A.3, A.4, A.5
**What:** Confirm the existing per-stage `choco`/`winget` nanoserver validator is still correct now that multi-stage actually runs end-to-end. Add regression unit tests for: (a) servercore-builder + nanoserver-final passes, (b) nanoserver-only with choco fails, (c) two-builder-stages where one uses choco on nanoserver fails.

**Acceptance:** New unit tests in `windows/deps.rs` cover the three scenarios.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/src/windows/deps.rs` only, no `git stash`. Read the existing validator. Add the three unit tests described in TODO_WINDOWSDELME.md A.6. Run workspace gates and report.

## Task A.7 — Positive multi-stage E2E test

**Files:** `crates/zlayer-builder/tests/windows_build_e2e.rs`
**Depends on:** A.5
**Parallelizable with:** A.8
**What:** Replace the rejection assertion at lines 131-175 with a positive test: builder stage installs Node via `choco install nodejs -y` on `servercore:ltsc2022`, final stage `FROM nanoserver:ltsc2022` does `COPY --from=builder "C:\\Program Files\\nodejs\\" "C:\\app\\nodejs\\"` and sets `CMD ["C:\\app\\nodejs\\node.exe", "--version"]`. Test asserts the produced image runs and prints a Node version.

**Acceptance:** Test passes on MiniWindows under `--ignored`.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/tests/windows_build_e2e.rs` only, no `git stash`. Replace the rejection test at lines 131-175 with the positive multi-stage test described in TODO_WINDOWSDELME.md A.7. Keep the test gated `#[ignore]`. Run `cargo build --workspace --tests` and report.

## Task A.8 — Negative tests for multi-stage edge cases

**Files:** `crates/zlayer-builder/tests/windows_build_e2e.rs`
**Depends on:** A.5
**Parallelizable with:** A.7
**What:** Add tests for: missing `--from` stage name, self-referential `--from`, path-escape via `COPY --from=builder ..\\..`, and unrelated-stage path that doesn't exist in the source stage.

**Acceptance:** Each test asserts the specific error variant.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/tests/windows_build_e2e.rs` only, no `git stash`. Add the four negative tests described in TODO_WINDOWSDELME.md A.8. All `#[ignore]`-gated. Run `cargo build --workspace --tests` and report.

---

# Phase B — Preflight checks and clear errors

**Goal:** Replace the lying `is_available() -> true` at `mod.rs:482-488` with a real probe, plus a structured preflight at the start of `build_image` that turns opaque HCS HRESULT failures into actionable user errors.

**Why second:** Independent of A. Highest UX-per-line-of-code ratio. End users on a fresh Windows machine without Hyper-V get a clear "enable Windows Containers feature" message instead of a hex error code. (MiniWindows is already configured, so this primarily benefits new users / fresh environments.)

**Estimated scope:** Small. ~3 sub-agent commits.

**Files affected:**
- `crates/zlayer-builder/src/backend/hcs/mod.rs` (real `is_available` probe)
- `crates/zlayer-builder/src/backend/hcs/preflight.rs` (NEW)
- `crates/zlayer-builder/src/error.rs` (new error variant)

## Task B.1 — Real `is_available()` HCS probe

**Files:** `crates/zlayer-builder/src/backend/hcs/mod.rs` (just the `is_available` function, lines 482-488)
**Depends on:** nothing
**Parallelizable with:** All of Phase A, C.1, D.1, D.2
**What:** Replace the unconditional `true` with a probe that creates an `Operation` handle via `zlayer_hcs::operation::Operation::create()` and immediately drops it. Cache the result in `OnceLock<bool>` so subsequent calls are free. Return `false` and log the reason on probe failure.

**Acceptance:** On a Windows host without HCS (Hyper-V disabled), `is_available()` returns `false` and the backend dispatch in `backend/mod.rs` falls through to a clear error rather than crashing in `build_image`.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/src/backend/hcs/mod.rs` only, no `git stash`. Replace the body of `is_available` (lines 482-488) with a real `Operation::create` probe cached in `OnceLock<bool>`. Log a warning with the underlying error on failure. Run workspace gates and report.

## Task B.2 — `preflight.rs` module with structured checks

**Files:** `crates/zlayer-builder/src/backend/hcs/preflight.rs` (NEW)
**Depends on:** B.1, B.3
**Parallelizable with:** A.x, C.x, D.x
**What:** New module with one entry point `pub async fn preflight_windows_host() -> Result<(), BuildError>`. Checks in order:
1. **Windows version ≥ 10 1809 / Server 2019** — query via `GetVersionExW` or registry. Fix hint: "ZLayer requires Windows 10 1809+ or Windows Server 2019+. Update Windows."
2. **HCS handle creation** — calls B.1's probe. Fix hint: "Enable Windows Containers feature: `Enable-WindowsOptionalFeature -Online -FeatureName Containers`. Hyper-V is also required for isolated containers: `Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All`."
3. **`SeBackupPrivilege` + `SeRestorePrivilege` available** — wraps `zlayer_agent::windows::layer::enable_backup_restore_privileges` (already exists). Fix hint: "Run zlayer as Administrator, or grant SeBackupPrivilege/SeRestorePrivilege via Local Security Policy."
4. **Network reach to `mcr.microsoft.com`** OR a base-image already present locally in the registry blob cache — at least one must succeed. Fix hint: "Cannot reach mcr.microsoft.com and no Windows base image is cached locally. Pull a base image manually with `zlayer pull mcr.microsoft.com/windows/nanoserver:ltsc2022`, or check your network/proxy."

**Acceptance:** Each failed check returns `BuildError::WindowsHostUnready { component, fix_hint }` (B.3) with the specific component and hint. Successful preflight returns `Ok(())`.

**Sub-agent prompt outline:**
> opus 4.6, create `crates/zlayer-builder/src/backend/hcs/preflight.rs` and add `mod preflight;` to `backend/hcs/mod.rs`. Implement the four checks listed in TODO_WINDOWSDELME.md B.2. Reuse `zlayer_agent::windows::layer::enable_backup_restore_privileges` for check #3. Use `reqwest` (already a dep) for the network probe with a 3s timeout and retry once. Run workspace gates and report.

## Task B.3 — `BuildError::WindowsHostUnready` variant

**Files:** `crates/zlayer-builder/src/error.rs`
**Depends on:** nothing
**Parallelizable with:** B.1, B.2 (B.2 will need this committed first)
**What:** Add the variant to `BuildError`:
```rust
#[error("Windows host is not ready: {component} — {fix_hint}")]
WindowsHostUnready { component: String, fix_hint: String },
```

**Acceptance:** Variant compiles, downstream `Result` flow accepts it, no clippy warnings.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/src/error.rs` only, no `git stash`. Add the `WindowsHostUnready { component: String, fix_hint: String }` variant to `BuildError` with the `thiserror` message format shown in TODO_WINDOWSDELME.md B.3. Run workspace gates and report.

## Task B.4 — Wire preflight into `HcsBackend::build_image` entry

**Files:** `crates/zlayer-builder/src/backend/hcs/mod.rs`
**Depends on:** B.2, B.3
**Parallelizable with:** B.5
**What:** Call `preflight::preflight_windows_host().await?` as the first line of `HcsBackend::build_image`. Cache the result in `OnceCell<()>` so re-invocations are free.

**Acceptance:** Build fails fast with a structured error if any preflight check fails. Successful preflight is a no-op on subsequent builds in the same process.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/src/backend/hcs/mod.rs` only, no `git stash`. Add `preflight::preflight_windows_host()` as the first call in `build_image` and cache success in `OnceCell<()>`. Run workspace gates and report.

## Task B.5 — Per-component fix-hint copy review

**Files:** `crates/zlayer-builder/src/backend/hcs/preflight.rs`
**Depends on:** B.2
**Parallelizable with:** B.4
**What:** Sanity-check each fix hint against `docs/windows-ci-runner.md` so user-facing language matches the CI runbook. Cross-link from runbook to preflight error strings.

**Acceptance:** Hints in the code match the runbook's "first-time setup" section verbatim where applicable.

## Task B.6 — Tests for each preflight failure case

**Files:** `crates/zlayer-builder/src/backend/hcs/preflight.rs` (`#[cfg(test)]` mod) and possibly a new `tests/windows_preflight.rs`
**Depends on:** B.4
**Parallelizable with:** nothing in B (last task)
**What:** Mock each check independently and assert the error component + fix hint substring. Refactor each check into a function that takes its dependencies as trait objects so tests can inject mocks.

**Acceptance:** Four unit tests, one per preflight check, each asserting the right component string.

**Sub-agent prompt outline:**
> opus 4.6, add tests to `crates/zlayer-builder/src/backend/hcs/preflight.rs` `#[cfg(test)]` block. Refactor each check into a function that takes its dependencies as trait objects so the tests can inject mocks. Four tests, one per check. Run workspace gates and report.

---

# Phase C — Inter-build cache (parent-chain reuse + instruction-level layer cache)

**Goal:** Wire the dead `unpacked_root` field at `scratch.rs:55-59` and add Docker-style instruction-level caching so an unchanged second build is dominated by I/O reads, not RUN re-execution.

**Why third:** Depends conceptually on multi-stage being correct (Phase A) since cache keys must include stage context. Big perf win — Phase D toolchain images will be impractical without it (every CI run re-installing Rust toolchain in a 5GB servercore image is unworkable).

**Estimated scope:** Medium-large. ~5-6 sub-agent commits, careful design pass first.

**Files affected:**
- `crates/zlayer-builder/src/backend/hcs/scratch.rs` (parent-chain reuse)
- `crates/zlayer-builder/src/backend/hcs/commit.rs` (post-build registration)
- `crates/zlayer-builder/src/backend/hcs/cache.rs` (NEW — key derivation, store)
- `crates/zlayer-builder/src/backend/hcs/mod.rs` (cache hit/miss in dispatch loop)
- `bin/zlayer/src/cli.rs` (`--no-cache`, `--cache-from` flags)
- `crates/zlayer-builder/src/tui/events.rs` or similar (cache events)

## Task C.1 — Cache key spec design doc

**Files:** Inline doc-comment at top of (yet-to-be-created) `crates/zlayer-builder/src/backend/hcs/cache.rs`
**Depends on:** nothing
**Parallelizable with:** all of A, B, D.1, D.2
**What:** No code yet — just the spec. Document:
- **Layer-cache key:** `SHA-256(parent_layer_digest_chain || stage_idx || instruction_idx || canonical_instruction || copy_source_digests)` where `canonical_instruction` is a normalized representation that strips comments/whitespace and resolves ARG values.
- **Parent-chain key:** `SHA-256(ordered list of parent layer digests)`. Used to cache the unpacked-on-disk parent chain.
- **Cache invalidation:** explicit `--no-cache` flag; ARG changes invalidate from that instruction forward; COPY source content hash invalidates that COPY and everything after.
- **Storage layout:**
  - `storage_root/parent-chains/<chain-digest>/` — unpacked parent chain, gc-eligible after N days.
  - `storage_root/layer-cache/<key>/{layer.tar.gz,config.json,metadata.json}` — instruction-level cached layers.
- **Concurrent access:** file-lock on each cache entry directory; readers fall through to miss on lock contention rather than block.

**Acceptance:** Doc comment exists at the top of `cache.rs` (which can be a stub at this point). Reviewable independently.

**Sub-agent prompt outline:**
> opus 4.6, create `crates/zlayer-builder/src/backend/hcs/cache.rs` with ONLY a top-of-file `//!` doc comment containing the spec from TODO_WINDOWSDELME.md C.1. Module body can be `pub fn _placeholder() {}` or similar — actual logic is C.2. Run `cargo build --workspace` and report.

## Task C.2 — `cache.rs` key derivation and store

**Files:** `crates/zlayer-builder/src/backend/hcs/cache.rs`
**Depends on:** C.1
**Parallelizable with:** C.3, C.4
**What:** Implement:
- `pub struct CacheKey { hex: String }` with `pub fn derive(...)` matching the C.1 spec.
- `pub struct LayerCacheStore { root: PathBuf }` with `lookup(&self, key: &CacheKey) -> Option<CachedLayer>`, `store(&self, key: CacheKey, layer: CachedLayer) -> Result<()>`, file-locking.
- `pub struct ParentChainStore { root: PathBuf }` with similar `lookup` / `store`.
- A canonical-instruction normalizer: strip comments, normalize whitespace, resolve `ARG` substitution.

**Acceptance:** Unit tests cover key stability (same input → same key) and key sensitivity (any field change → different key).

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/src/backend/hcs/cache.rs` only, no `git stash`. Implement `CacheKey`, `LayerCacheStore`, `ParentChainStore`, and the canonical-instruction normalizer per TODO_WINDOWSDELME.md C.2. Use `fs2` or `fd-lock` for the file lock (check `Cargo.lock` for an existing dep). Add ~10 unit tests covering key stability and sensitivity. Run workspace gates and report.

## Task C.3 — Wire `unpacked_root` reuse in `scratch.rs`

**Files:** `crates/zlayer-builder/src/backend/hcs/scratch.rs`
**Depends on:** C.2 (for `ParentChainStore`)
**Parallelizable with:** C.4
**What:** In `provision_stage_scratch` (from A.1):
- Compute parent-chain digest from the manifest layers.
- Look up `ParentChainStore::lookup(chain_digest)`. If present, hardlink/clone the unpacked tree into the per-build directory.
- On miss, do the existing unpack and call `ParentChainStore::store` afterward.
- Remove the `#[allow(dead_code)]` on `unpacked_root`.

**Acceptance:** Second build with the same base image skips the unpack step. Wall-clock measured improvement on a typical `servercore` build of ≥30s.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/src/backend/hcs/scratch.rs` only, no `git stash`. Wire `ParentChainStore` (from C.2) into `provision_stage_scratch`. Use Windows hardlinks (`std::os::windows::fs::symlink_dir` or `MKLINK /J` via `std::process`) for the unpacked-root reuse — confirm WCIFS handles them. Remove the `#[allow(dead_code)]` at lines 55-59. Run workspace gates and report.

## Task C.4 — Layer blob registration in `commit.rs`

**Files:** `crates/zlayer-builder/src/backend/hcs/commit.rs`
**Depends on:** C.2
**Parallelizable with:** C.3
**What:** After a successful build, register each produced layer in `LayerCacheStore` keyed by the cache key derived for the instruction range that produced it. Each instruction-level layer becomes its own cache entry.

**Acceptance:** A second build with the same instruction stream finds cached layers and skips RUN re-execution.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/src/backend/hcs/commit.rs` only, no `git stash`. After successful commit, register each layer in `LayerCacheStore` keyed by `CacheKey::derive` from C.2. Run workspace gates and report.

## Task C.5 — Cache hit/miss detection in dispatch loop

**Files:** `crates/zlayer-builder/src/backend/hcs/mod.rs`
**Depends on:** C.2, C.3, C.4
**Parallelizable with:** C.6
**What:** Before running each instruction, derive its cache key, look up in `LayerCacheStore`. On hit: apply the cached layer to the current scratch (extract the cached `layer.tar.gz` into the scratch root via the existing layer-import path), emit `BuildEvent::CacheHit`, skip the `RUN` execution. On miss: run the instruction normally, emit `BuildEvent::CacheMiss`, after RUN succeeds capture the diff and store via C.4's flow.

**Acceptance:**
- Identical second build is dominated by cache lookup + extract, not RUN re-execution.
- Modifying a single RUN line invalidates only that RUN and all downstream steps.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/src/backend/hcs/mod.rs` only, no `git stash`. In the per-instruction dispatch loop, derive `CacheKey`, look up in `LayerCacheStore`, on hit apply the cached layer and skip RUN; on miss run normally and store. Emit `BuildEvent::CacheHit`/`CacheMiss`. Run workspace gates and report.

## Task C.6 — `--no-cache` and `--cache-from` CLI flags

**Files:** `bin/zlayer/src/cli.rs` and the relevant `commands/build.rs` plumbing
**Depends on:** C.5
**Parallelizable with:** C.5 (flag plumbing can be drafted before C.5 lands)
**What:** Add `--no-cache` (Boolean) and `--cache-from <ref>` (`Vec<String>` of registry refs to seed the cache from). Match Docker's semantics. Plumb into the build options struct passed to the backend.

**Acceptance:** `zlayer build --no-cache .` rebuilds from scratch. `zlayer build --cache-from registry/img:tag .` pulls the referenced image's layers into the local cache before building.

**Sub-agent prompt outline:**
> opus 4.6, `bin/zlayer/src/cli.rs` and `bin/zlayer/src/commands/build.rs` only, no `git stash`. Add `--no-cache` and `--cache-from` flags matching Docker's semantics. Run workspace gates and report.

## Task C.7 — Perf benchmark + correctness E2E

**Files:** `crates/zlayer-builder/tests/windows_build_e2e.rs`
**Depends on:** C.5, C.6
**Parallelizable with:** C.8
**What:** Add `#[ignore]`-gated tests:
- Build a moderate dockerfile twice; assert second run is ≥3× faster wall-clock.
- Modify the last RUN; assert only it (and the commit) re-executes.
- `--no-cache` forces full rebuild.

**Acceptance:** All three tests pass on MiniWindows.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/tests/windows_build_e2e.rs` only, no `git stash`. Add the three `#[ignore]`-gated cache tests from TODO_WINDOWSDELME.md C.7. Run `cargo build --workspace --tests` and report.

## Task C.8 — TUI events for cache hit/miss

**Files:** `crates/zlayer-builder/src/tui/` (find the events enum and renderer)
**Depends on:** C.5
**Parallelizable with:** C.7
**What:** Add `BuildEvent::CacheHit` and `BuildEvent::CacheMiss` to the events enum. Render in the TUI as `↑ CACHED` / `· building`. Include them in the `--no-tui` line-mode output.

**Acceptance:** TUI shows cache hits visibly. JSON event stream includes them.

---

# Phase D — Windows toolchain images (`zlayer-windows-rust`, `zlayer-windows-node`)

**Goal:** Ship pre-baked `servercore`-with-toolchain images that downstream user builds can `FROM` so they skip multi-minute `choco install rust` cycles. Image-pipeline parity with the existing Linux `zlayer-node` image.

**Why fourth:** Depends on multi-stage (Phase A) so the images themselves can be built with `servercore`-bake → `nanoserver`-slim patterns where appropriate. Depends on caching (Phase C) for CI to be tolerable. **Skipping `zlayer-windows-python`** per user direction — the boringtun overlay should make Python tooling shareable cross-platform via the network, the same way Netbird's userspace WireGuard works on Windows. If overlay-on-Windows turns out broken, that's a separate workstream.

**Estimated scope:** Medium. ~5 sub-agent commits + CI plumbing.

**Files affected:**
- `images/Dockerfile.zlayer-windows-rust` (NEW)
- `images/Dockerfile.zlayer-windows-node` (NEW)
- `images/ZImagefile.zlayer-windows-rust` (NEW)
- `images/ZImagefile.zlayer-windows-node` (NEW)
- `ZPipeline.yaml` (add Windows targets with platform constraints)
- `.forgejo/workflows/build.yml` (add a Windows pipeline-build job)
- `images/README.md` (document Windows images)

## Task D.1 — `zlayer-windows-rust` image

**Files:** `images/Dockerfile.zlayer-windows-rust`, `images/ZImagefile.zlayer-windows-rust`
**Depends on:** none for authoring; A and C for being practically buildable in CI
**Parallelizable with:** D.2, all of A, B, C
**What:** Two-stage Dockerfile:
- Stage `installer`: `FROM mcr.microsoft.com/windows/servercore:ltsc2022`. Installs `chocolatey`, then `choco install rust visualstudio2022buildtools git protoc -y`. Sets `RUSTFLAGS`, `CARGO_HOME=C:\\cargo`, `RUSTUP_HOME=C:\\rustup` so they're stable across builds.
- Stage `final`: `FROM mcr.microsoft.com/windows/servercore:ltsc2022`. `COPY --from=installer C:\\cargo C:\\cargo`, `COPY --from=installer C:\\rustup C:\\rustup`, `COPY --from=installer "C:\\Program Files\\Git" "C:\\Program Files\\Git"`, etc. `ENV PATH=...`. `WORKDIR C:\\workspace`.
- Why two stages: the installer stage produces ~8GB of cache/temp artifacts not needed in the final image. The final stage carries only the toolchains.

**Acceptance:** Image builds on MiniWindows. `rustc --version` (via zlayer run) prints a Rust version.

**Sub-agent prompt outline:**
> opus 4.6, create `images/Dockerfile.zlayer-windows-rust` and `images/ZImagefile.zlayer-windows-rust`. Two-stage as described in TODO_WINDOWSDELME.md D.1. Use `ltsc2022` base. Set `CARGO_HOME=C:\\cargo`, `RUSTUP_HOME=C:\\rustup`. ZImagefile mirrors the Dockerfile in YAML. Run `cargo build --workspace` and report.

## Task D.2 — `zlayer-windows-node` image

**Files:** `images/Dockerfile.zlayer-windows-node`, `images/ZImagefile.zlayer-windows-node`
**Depends on:** none for authoring
**Parallelizable with:** D.1, all of A, B, C
**What:** Two-stage Dockerfile mirroring D.1's pattern but with `nodejs-lts`, `pnpm`, `git`. `CMD ["node.exe"]` or `["pwsh.exe"]` as appropriate.

**Acceptance:** Same as D.1, with `node --version` and `pnpm --version`.

**Sub-agent prompt outline:**
> opus 4.6, create `images/Dockerfile.zlayer-windows-node` and `images/ZImagefile.zlayer-windows-node`. Two-stage mirroring D.1 but with `choco install nodejs-lts pnpm git -y`. Run `cargo build --workspace` and report.

## Task D.3 — `ZPipeline.yaml` Windows targets with platform constraints

**Files:** `ZPipeline.yaml`
**Depends on:** D.1, D.2
**Parallelizable with:** D.4, D.5
**What:** Add `images.zlayer-windows-rust` and `images.zlayer-windows-node` entries with `platforms: ["windows/amd64"]`. Mark them `skip_on_unsupported_host: true` so Linux/macOS dev runs of `zlayer pipeline` don't fail. Add tags `${REGISTRY}/zlayer-windows-rust:${VERSION}` and `:latest`.

**Acceptance:** `zlayer pipeline -f ZPipeline.yaml --set VERSION=dev` on a Windows runner builds both new images. Same command on Linux skips them with an explanatory log.

**Sub-agent prompt outline:**
> opus 4.6, `ZPipeline.yaml` only. Add the two new image entries per TODO_WINDOWSDELME.md D.3 with `platforms: ["windows/amd64"]` and `skip_on_unsupported_host: true`. Confirm the pipeline schema supports `skip_on_unsupported_host` — if not, this becomes a small spec change in `crates/zlayer-builder/src/pipeline/`. Run workspace gates and report.

## Task D.4 — `.forgejo/workflows/build.yml` Windows pipeline job

**Files:** `.forgejo/workflows/build.yml`
**Depends on:** D.3
**Parallelizable with:** D.5
**What:** Extend `build-windows-amd64` (or add a new job `build-windows-images`) to run `zlayer pipeline -f ZPipeline.yaml --no-tui --set VERSION=$VERSION` on the MiniWindows runner. Push to the registry on tag builds.

**Acceptance:** CI run on a tag pushes new `:latest` and `:<version>` tags for both Windows images.

**Sub-agent prompt outline:**
> opus 4.6, `.forgejo/workflows/build.yml` only. Add a Windows pipeline job per TODO_WINDOWSDELME.md D.4. Use the existing `windows-latest` label. Report the workflow YAML.

## Task D.5 — Smoke test: user `FROM zlayer-windows-rust` Dockerfile

**Files:** `crates/zlayer-builder/tests/windows_build_e2e.rs`
**Depends on:** D.3, D.4
**Parallelizable with:** D.4
**What:** Add an `#[ignore]`-gated test that builds a tiny user dockerfile `FROM zlayer/zlayer-windows-rust:dev` + `RUN cargo new helloworld` + `WORKDIR helloworld` + `RUN cargo build --release`. Asserts the build succeeds in <2 minutes (the toolchain image already has Rust pre-installed).

**Acceptance:** Test passes on MiniWindows.

**Sub-agent prompt outline:**
> opus 4.6, `crates/zlayer-builder/tests/windows_build_e2e.rs` only. Add the `#[ignore]`-gated smoke test from TODO_WINDOWSDELME.md D.5. Run `cargo build --workspace --tests` and report.

---

# Phase E — Cleanup deferred ops (opportunistic, post-D)

**Goal:** Pick off the smaller deferred items.

**Estimated scope:** Each task is ~1 sub-agent commit. Do them when convenient.

## Task E.1 — `ADD <url>` support
**Files:** `crates/zlayer-builder/src/backend/hcs/{mod,exec}.rs`
Replace the rejection at `mod.rs:526-532` with a `reqwest`-based download into the scratch layer. Mirror the Linux behavior in the buildah backend.

## Task E.2 — `RUN --mount=type=bind,...` (BuildKit-style mounts)
**Files:** `crates/zlayer-builder/src/backend/hcs/exec.rs`
Replace the rejection at `exec.rs:57-68`. Translate bind mounts to WCIFS-mapped paths in the compute system's `mapped_directories`. Cache mounts can use a host-side directory mapped read-write; type=secret stays unsupported for now.

## Task E.3 — `push_image` / `tag_image` / manifest ops
**Files:** `crates/zlayer-builder/src/backend/hcs/mod.rs`
Lines 439-479. These return `NotSupported` because the registry client routes around them. Either keep that contract and document it, or wire them as thin wrappers over the registry client. User decision.

---

# Verification gates (per phase)

Per `CLAUDE.md` global instructions, **every commit** must pass on the entire workspace:

```
cargo fmt --all                                                      # FIRST, every time
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

**Windows-specific gates** (require MiniWindows runner):

```
cargo test -p zlayer-builder --test windows_build_e2e -- --ignored
cargo test -p zlayer-builder --test hcs_backend_e2e -- --ignored
```

**Per-phase milestone:**

| Phase | Milestone test | Where it runs |
|-------|----------------|---------------|
| A | A.7 (multi-stage positive E2E) passes | MiniWindows runner |
| B | B.6 (preflight unit tests) + manual non-HCS smoke | Local + manual VM |
| C | C.7 (perf benchmark + correctness) passes | MiniWindows runner |
| D | D.5 (user-Dockerfile-FROM-toolchain smoke) passes | MiniWindows runner |

**Per-task acceptance:** every sub-agent must report back the diff and a green `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo build --workspace`. The orchestrator reads the diff, verifies it matches the prompt, and runs `cargo test --workspace` before dispatching the next agent.

---

# Dispatch cheat sheet

When you sit down to start work, in order:

1. **Open this file**, find the next undone task per the dep map.
2. **Spawn an Agent** with `model: "opus"`, foreground, single-task scoped to the files listed.
3. **Read the changed files** after the agent reports — do NOT trust agent output blindly.
4. **Run gates locally:** `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo build --workspace`.
5. **For Windows-only tasks:** push the branch and run the Forgejo `windows-latest` job, or shell into MiniWindows to run `cargo test -p zlayer-builder --test windows_build_e2e -- --ignored`.
6. **Update this file** — strike through completed tasks, add notes on anything that surprised you.
7. **Commit** with a single-purpose message; do not bundle unrelated tasks.

Parallelism: when launching multiple agents from the parallelism map, send all tool calls in **one message** (per the harness's parallel-agent guidance).
