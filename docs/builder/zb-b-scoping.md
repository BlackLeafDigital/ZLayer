## Phase B — Linux multi-stage builds

### Goals

Phase B lights up multi-stage Dockerfiles on the **Phase A native Rust executor** (the one replacing buildah). End state:

- `FROM <base> AS <name>` parses, builds, and persists a stage rootfs addressable by both `name` and numeric `index`.
- `COPY --from=<stage>` resolves against earlier stages of the same build (the in-memory `stage_rootfs_map`).
- `COPY --from=<external-image>` pulls the foreign image via `zlayer-registry::ImagePuller`, unpacks its layers into an ephemeral rootfs, and copies from there.
- `--target=<stage>` truncates the linear stage list (semantics already implemented for buildah at `crates/zlayer-builder/src/backend/buildah.rs:134-150`; lift it onto the native executor).
- Intermediate stages live under a build-scoped cache dir and are torn down after a successful build unless `--keep-stages` is passed (debug aid).
- Stages are built **linearly** (Phase B). The instruction-DAG analysis required for parallel/dedup is explicitly Phase C — call out the boundary in code with a `// Phase C:` tag, not `TODO`.

### Steps

Each agent task: ≤ 2 files, ≤ ~200 LOC, leaves the workspace `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.

- **B.1 — Stage rootfs registry** (3h). Lift the `stage_rootfs_map: HashMap<String, PathBuf>` pattern from `sandbox_builder.rs:393` into a real type `StageRootfsRegistry` in the Phase A executor module (likely `crates/zlayer-builder/src/native/stages.rs`). Indexes by `name` AND stringified `index` AND `Stage::identifier()` (matches the triple-insert at `sandbox_builder.rs:541-545`). Owns the `${ZLAYER_DATA_DIR}/build-cache/stages/<build-id>/<stage-id>/` directory layout and cleans up on drop.
- **B.2 — `COPY --from=<stage>` dispatch** (4h). In the Phase A copy executor, branch on `CopyInstruction.from` (parser already populates this, `dockerfile/parser.rs:303-307`): if the value matches a registered stage in `StageRootfsRegistry`, set `source_root` to that stage's rootfs (mirror of `sandbox_builder.rs:1604-1610`). Translate the source path relative to the stage's WORKDIR if it isn't absolute (buildah backend already does this at `backend/buildah.rs:469-483` — port the logic, do not call into the buildah module).
- **B.3 — `--target` stage truncation** (2h). Port `BuildahBackend::resolve_target_stages` (`backend/buildah.rs:134-150`) onto the native executor. Reads `BuildOptions.target` (already wired through, `builder.rs:305`). Errors with `BuildError::StageNotFound` from `error.rs:46` when the target doesn't exist.
- **B.4 — External `COPY --from=<image>` pull + materialise** (6h). New helper `materialise_external_copy_from(image_ref, cache_dir) -> PathBuf`: parses via `ImageReference::from_str`, calls `ImagePuller::pull_image` (`zlayer-registry/src/client.rs:1398`), unpacks layers via `LayerUnpacker` (`zlayer-registry/src/unpack.rs:157`) into `${build-cache}/external/<sha256-of-ref>/rootfs`, returns the path. Cache by digest so a multi-line `COPY --from=ghcr.io/x:tag` block doesn't re-pull (mirrors the `pulled_external_images: HashSet` at `backend/buildah.rs:374-375`).
- **B.5 — External vs stage disambiguation** (2h). When `CopyInstruction.from` is set, look up the registry first; if absent, parse via `ImageReference::from_str` and route to B.4. This matches the case-1/case-2 split documented at `backend/buildah.rs:444-459`. Bare names like `builder` are already promoted to `Stage` at parse time (`parser.rs:201-205`), so this branch is unambiguous.
- **B.6 — Stage cleanup + `--keep-stages` flag** (2h). Add `BuildOptions.keep_stages: bool`. On successful build, `StageRootfsRegistry::drop` recursively removes the build-id dir unless `keep_stages` is set. Reuse the `chmod -R u+w` then `rm -rf` dance from `sandbox_builder.rs:566-592` (Go's module cache leaves read-only files that defeat `remove_dir_all`).
- **B.7 — Per-stage TUI events** (1h). The TUI already accepts `StageStarted` / `StageComplete` (`sandbox_builder.rs:441-547`). Confirm the Phase A executor emits them, including for stages skipped by `--target`.
- **B.8 — Tests** (4h). See `### Tests`.

Total: **24h of agent work** (~3 person-days). Leaves 2-4 days of integration + flake-debugging buffer inside the 1-2-week budget.

### Files to modify

- `crates/zlayer-builder/src/native/executor.rs` (Phase A entrypoint — TBD path)
- `crates/zlayer-builder/src/native/stages.rs` (**new** — B.1)
- `crates/zlayer-builder/src/native/copy.rs` (B.2, B.5)
- `crates/zlayer-builder/src/native/external.rs` (**new** — B.4)
- `crates/zlayer-builder/src/builder.rs` (B.3 wiring `target`, B.6 `keep_stages` field)
- `crates/zlayer-builder/src/error.rs` (extend `BuildError` if any new variants needed; `StageNotFound` already exists at line 46)
- `crates/zlayer-types/src/api/build.rs` (only if `BuildId` needs to grow — investigate first)

### New types in zlayer-types

Per the rule "types `use`d by 2+ crates live in `crates/zlayer-types`": almost everything here is internal to `zlayer-builder` and stays there. The only candidate is:

- `BuildId(String)` — already partly present (`build_id: String` at `zlayer-types/src/api/build.rs:82` and `client.rs:86`). If B.1's `StageRootfsRegistry` needs a typed key and the API returns the same ID, promote `BuildId` to a real newtype here. Otherwise keep the `String` shape and revisit in Phase C.

No new shared enums or structs are forced by Phase B alone. `DockerfileFromTarget` and `Stage` are Dockerfile-IR types and correctly live in `zlayer-builder` (not consumed elsewhere).

### Stage graph algorithm

Phase B = **linear**. Algorithm:

1. Parse Dockerfile (already done; produces `Vec<Stage>` ordered by file position — `parser.rs:269-272`).
2. If `options.target` is `Some(t)`, truncate to `[0..=index_of(t)]` (B.3).
3. For `i in 0..stages.len()`:
   1. Resolve base. If `Stage::base_image == DockerfileFromTarget::Stage(name)`, look up `StageRootfsRegistry[name]` and `copy_directory_recursive` it into the new stage rootfs (mirrors `sandbox_builder.rs:663-679`). If `Scratch`, create empty. If `Image`, pull via Phase A's existing base-image setup.
   2. Execute the stage's instructions in order (Phase A executor).
   3. After the last instruction, register the rootfs in `StageRootfsRegistry` under the stage's name (if any), its index (always), and `Stage::identifier()` (matches the triple-insert in `sandbox_builder.rs:541-545`).
4. After the loop, the *last* stage's rootfs is the build output. Earlier stages' rootfs dirs are deleted (B.6).

**No DAG analysis, no parallel execution, no dead-stage elimination.** Those are Phase C. The relevant boundary in code: a comment "`// Phase C: parallel/DAG dispatch starts here`" at the head of the `for stage in stages` loop.

Cycle detection is unnecessary in Phase B: `Stage::base_image == Stage(name)` can only reference stages that have already been parsed (parser writes `known_stage_names` at `parser.rs:181-211` *before* checking subsequent FROMs). The parser cannot produce a forward reference; the orchestrator's monotonic-index loop guarantees acyclicity.

### `COPY --from=` semantics table

| Source form                                  | Resolution                                                                                                       | Edge cases                                                                                              | Test coverage needed                                                                              |
|----------------------------------------------|------------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------|
| `--from=<stage-name>`                        | Lookup in `StageRootfsRegistry` by name (B.1). Source path resolved against source stage's recorded WORKDIR.     | Stage exists but is empty (allowed); name shadows an image (parser already promotes — `parser.rs:201`). | `test_copy_from_named_stage`, `test_copy_from_stage_with_workdir_relative_source`.               |
| `--from=<index>`                             | Lookup by stringified index in `StageRootfsRegistry` (Docker permits `COPY --from=0`).                           | Negative index (reject); index ≥ `stages.len()` (reject with `StageNotFound`).                          | `test_copy_from_numeric_index`, `test_copy_from_out_of_range_index_errors`.                       |
| `--from=<external-image>` (e.g. `alpine`)    | Bare unqualified — **parser already promotes to `Stage`** if a same-named stage exists. Else: external pull B.4. | `alpine` with no prior `AS alpine` stage → registry pull of `docker.io/library/alpine:latest`.          | `test_copy_from_bare_unknown_treated_as_image`.                                                   |
| `--from=ghcr.io/org/img:tag`                 | Always external (qualified ref). Pull via `ImagePuller::pull_image`, unpack via `LayerUnpacker`, copy.           | Manifest not found; auth required; layer extraction fails; symlink-escape in tar.                      | `test_copy_from_external_qualified_pulls_once`; mocked-registry integration test.                 |
| `--from=img@sha256:...`                      | External, by digest. Same path as tagged-external but skips `Newer` policy revalidation.                         | Bad digest format; digest mismatch after pull.                                                          | `test_copy_from_external_by_digest`.                                                              |
| `COPY --from=X /abs/src /dst`                | `source_root = stages[X].rootfs`, `source_path = source_root + "/abs/src"`.                                      | Source missing — error with full path (current behavior `sandbox_builder.rs:1631-1639`).                | `test_copy_from_missing_source_errors_with_path`.                                                 |
| `COPY --from=X relative/src /dst`            | Prefix with source stage's WORKDIR (default `/`). Mirrors buildah backend at `backend/buildah.rs:469-483`.       | Source stage never set WORKDIR (default to `/`); relative src escapes WORKDIR via `..`.                 | `test_copy_from_relative_src_uses_source_workdir`, `test_copy_from_relative_src_rejects_dotdot`. |
| `COPY --from=X /src dst` (no trailing slash) | `dst` is relative to current stage's WORKDIR; `is_dir_destination` heuristic from `sandbox_builder.rs:1601`.     | Single file vs multiple sources changes dest semantics.                                                 | `test_copy_from_relative_dst_uses_current_workdir`.                                               |
| `COPY --from=X /dir/ /dst/`                  | Directory copy; recursive.                                                                                       | Permission preservation; ownership via `--chown`.                                                       | `test_copy_from_directory_recursive`, `test_copy_from_with_chown_chmod`.                          |

Docker spec confirmation (`https://docs.docker.com/build/building/multi-stage/`): default target is the *last* stage, `--from=<index>` is valid, `--from=<image>` triggers an implicit pull, and `COPY --from` of a missing file is a build error. All consistent with the rows above.

### Tests

Live in `crates/zlayer-builder/tests/multi_stage.rs` (new) + unit tests in the modules touched.

1. `test_two_stage_builder_pattern` — golang builder → alpine runtime, assert final rootfs contains only the binary.
2. `test_named_stage_referenced_by_name_and_index` — `COPY --from=builder` and `COPY --from=0` resolve to the same rootfs.
3. `test_target_stops_at_named_stage` — `--target=builder` stops before the runtime stage; output is the builder rootfs.
4. `test_target_unknown_stage_errors` — `BuildError::StageNotFound`.
5. `test_copy_from_external_image_pulls_once` — two `COPY --from=ghcr.io/x:tag` lines, mocked registry asserts a single pull.
6. `test_copy_from_external_missing_source_errors` — pulled image lacks the requested path; error includes the path.
7. `test_scratch_final_stage_with_copy_from_builder` — `FROM scratch` + `COPY --from=builder` produces a minimal image (parser already handles this — `parser.rs:669-687`).
8. `test_stage_workdir_propagates_to_copy_from_relative_src` — builder sets `WORKDIR /src`; `COPY --from=builder app /` resolves to `/src/app` in the builder rootfs.
9. `test_intermediate_stages_cleaned_after_success` — after build, only the final image dir remains under `${ZLAYER_DATA_DIR}/build-cache/`.
10. `test_keep_stages_preserves_intermediates` — with `keep_stages = true`, all stage rootfs dirs remain on disk.
11. `test_forward_stage_reference_errors` — Dockerfile referencing a not-yet-declared stage name → parser-level rejection (already partly handled by the post-hoc promotion logic at `parser.rs:201`; add an explicit negative test).
12. Property test: random valid 2-5-stage Dockerfiles always produce a non-empty final rootfs when each stage's instructions are no-ops.

### Risks

- **Layer extraction symlink-escape**: `LayerUnpacker` (`zlayer-registry/src/unpack.rs:157`) is shared with base-image pull, so any escape would already exist in Phase A. Phase B widens the attack surface to arbitrary external `COPY --from` refs. Mitigation: confirm Phase A's tar extraction sanitises `../`, hardlinks, and absolute symlinks; add a regression test extracting a malicious image into the external-stage cache.
- **Cache poisoning across builds**: external `--from` images cached by tag (not digest) could be hijacked. Mitigation: cache key is `sha256(fully-qualified-ref + manifest-digest-after-pull)`, not `sha256(ref)`. Matches the `Newer` policy buildah uses today (`backend/buildah.rs:308-309`).
- **Disk usage**: each intermediate stage is a full rootfs copy. A 3-stage build with a 1 GB base layer = 3 GB on disk. Mitigation: document the cost, plan a Phase C hardlink/overlay step. Do not solve in Phase B.
- **Go module cache read-only files**: known issue (`sandbox_builder.rs:350-368`). B.6 must use the `chmod -R u+w` then `rm -rf` pattern, not `tokio::fs::remove_dir_all`.
- **`Stage::identifier()` collision**: two stages indexed `0` and `1`, no names → identifiers `"0"` and `"1"`. A `COPY --from=0` and a `COPY --from=1` both work. But: a Dockerfile that names a stage `"1"` (`FROM x AS 1`) collides with the implicit index-1 stage. Mitigation: at parse time, reject stage names that parse as `usize` (small parser tweak — flag for B.1 to verify; current parser does NOT reject this).
- **External-image pull during build = network dependency**: same as base-image pull. Honour `BuildOptions.pull` (`Never` / `Newer` / `Always`).

### Verification

After each agent (per global rules):

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

All four green across the entire workspace, no `-p` scoping. End-to-end smoke test: build `examples/multi-stage/Dockerfile` (create if absent — a golang → scratch pattern) and assert the final image runs.

### Estimated effort

**1-2 person-weeks**, confirmed. Breakdown:

- Agent-driven implementation: **24h** (B.1 + B.2 + B.4 are the hot path).
- Test authoring + flake debugging: **12h**.
- Workspace clippy/build fallout in adjacent crates: **8h** (budget for unrelated breakage per the all-green rule).
- Integration with Phase A executor seams that may still be in flux: **8h**.

Total ≈ 52h ≈ 1.3 person-weeks. Risk-adjusted ceiling: 2 weeks if Phase A's executor module layout is renamed mid-flight.
