# ZLayer Native Builder (`ZLAYER_BUILDER.md`)

Tracking document for replacing the buildah dependency with a native Rust OCI build executor. Updated by the orchestrating session after each agent group lands.

**Status legend**: ⬜ not started · 🟡 in progress · ✅ done · 🔵 blocked

> Workstream tags use `ZB-` prefix. "Phase" and "Wave" are deliberately not used (collision with shipped Phase E/F entries in `CHANGELOG.md`, and noise). CHANGELOG is for actual releases only. Per-step IDs: `ZB-A.1`, `ZB-A.2`, etc.

## Context

ZLayer currently shells out to **buildah** (Go) for the actual layer-execution step of `zlayer build`. The parser, ZImagefile schema, pipeline executor, registry client, OCI manifest writer, and macOS sandbox builder are all already Rust-native. The Linux gap is the only remaining buildah dependency; macOS emits non-OCI single-rootfs blobs; Windows already has the most complete native HCS-driven OCI builder of the three platforms (`crates/zlayer-builder/src/windows_builder.rs`, 4,699 lines).

**Why replace buildah**: the buildah 1.44.0 / netavark 1.17.2 version skew that's been biting builds on the user's host. The pain is the dependency on a Go daemon-adjacent stack that ZLayer cannot version-lock or fix from the Rust side. Going native eliminates the failure mode and lets the builder and orchestrator share `libcontainer` (youki on Linux), HCS (Windows), and Apple's Virtualization.framework (macOS).

**Scope**: all workstreams ZB-0 through ZB-G. Full BuildKit parity in Rust.

**Total effort estimate**: ~14–19 focused person-weeks (calendar ~3–4 months with parallel agent dispatch).

## Decisions made

1. **Naming**: workstreams are `ZB-0` / `ZB-A` … `ZB-G`. Drop "Phase" / "Wave". CHANGELOG is only for releases.
2. **macOS substrate**: switch to **Virtualization.framework via `objc2-virtualization 0.3.2`** with an in-repo thin safe wrapper. libkrun (`crates/zlayer-agent/src/runtimes/macos_vm.rs`) is the tactical bridge — kept in place during migration, then ripped out in a cleanup pass after ~2 weeks of green VZ operation. As of 2026-06, every first-party macOS container effort (`apple/container` 0.12.3, `Containerization` 0.33.3-prerelease, vfkit, lima, colima) is on VZ; libkrun (1.18.1) is HVF-only and is the deliberate outlier. We follow Apple's investment direction.
3. **SBOM strategy**: both. Native `cyclonedx-bom`-based emitter is the default; optional `--sbom-engine=syft` shells out to syft (Apache-2.0 is permissive — does NOT bleed into ZLayer codebase or downstream user code; bundling the binary only requires shipping syft's `LICENSE`+`NOTICE` files alongside).
4. **SBOM + provenance default**: ON (`--sbom=cyclonedx` + `--provenance=min`). Matches ko/docker-buildx-attest. Better supply-chain posture out of the box.

## Open questions still pending (none block ZB-0)

1. **Intel-Mac support in ZB-E**: arm64-only initially. Intel VZ works but Rosetta cross-arm64 is a separate workstream. Acceptable?
2. **Provenance signing (cosign)**: ~1 additional week. Fold into ZB-G or defer to ZB-G+1?
3. **Inline cache format**: BuildKit-compatible (reverse-engineer Moby's `containerimage.buildinfo`) or zlayer-native?
4. **Move ZLayer/zlayer-zql to `~/github/zstack/`?**: orthogonal organizational question. Recommendation: no, not yet.

## Sequencing

```
ZB-0 (types + scaffolding, ~13h)
    ↓
ZB-A (Linux native single-stage, 5–7 weeks)
    ↓
ZB-B (Linux multi-stage, 1.3 weeks)
    ↓
(parallelizable from here on)
    ├── ZB-C (BuildKit cache, 3 weeks)
    ├── ZB-D (multi-arch, 1.5–2 weeks)
    ├── ZB-E (macOS VZ Linux-in-VM, 2 weeks)
    └── ZB-F (Windows multi-stage + WSL2 routing, 1.5–2 weeks)
    ↓
ZB-G (SBOM/provenance/advanced mounts/full cache matrix, 4–5 weeks)
```

## Cross-cutting rules

- **Types-and-API FIRST**: every workstream that introduces new types schedules them in `zlayer-types` BEFORE consumers.
- **Use agents for ALL fixes**: even one-line clippy / doc cleanups after a wave go to a follow-up agent.
- **Never hand-edit manifests**: every `Cargo.toml` change goes through `cargo add` / `cargo remove` / `cargo update`.
- **Never hardcode versions**: look up real current via `gh release list` / `cargo search` / WebFetch / perplexity at implementation time.
- **No `git stash`** anywhere, ever.
- **CHANGELOG uses `[Unreleased]`**: never invent a version number.
- **Commit style**: one-liner subject only, no body, no Co-Authored-By trailers.
- **No background work**: every agent runs in foreground. Parallel foreground agents in one message are fine.
- **ALL CHECKS GREEN workspace-wide** after every agent wave: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo build --workspace`. Never `-p <crate>`.

---

## ZB-0 — Foundation: types, traits, scaffolding

**Status: ⬜ not started · Effort: 10–13 agent-hours · Risk: low**

| # | Status | Task | Files | Commit |
|---|---|---|---|---|
| ZB-0.1 | ⬜ | Document `BuildBackend` trait gaps in `docs/builder/zb-0-trait-gaps.md` | docs only | — |
| ZB-0.2 | ⬜ | Add `BuilderBackendKind` enum to new `crates/zlayer-types/src/builder.rs` | `crates/zlayer-types/src/builder.rs` (new), `crates/zlayer-types/src/lib.rs` | — |
| ZB-0.3 | ⬜ | Add `LayerCommitMetadata`, `CacheKey`, `BuildStepResult`, `CacheStatus` | same file | — |
| ZB-0.4 | ⬜ | Widen `BuildBackend` trait with `kind()`, `commit_layer()`, `lookup_cached_layer()` default-impl methods + `BuildError::Unsupported` | `crates/zlayer-builder/src/backend/mod.rs`, `error.rs` | — |
| ZB-0.5 | ⬜ | Extend `detect_backend()` for `native-youki|native-hcs|native-sandbox|linux-in-vm` | `crates/zlayer-builder/src/backend/mod.rs` | — |
| ZB-0.6 | ⬜ | Add `backend` + `progress_protocol` to `BuildRequest`; `backend_used` to `BuildStatus` | `crates/zlayer-types/src/api/build.rs` | — |
| ZB-0.7 | ⬜ | Add `backend_override` to `BuildOptions` + `--backend <kind>` CLI flag | `crates/zlayer-builder/src/builder.rs`, `bin/zlayer/src/cli.rs`, `commands/build.rs` | — |
| ZB-0.8 | ⬜ | Lay down `backend/native/{linux,macos,windows,linux_in_vm}/mod.rs` stubs | 4 new files + 1 modified | — |
| ZB-0.9 | ⬜ | Workspace dep audit (no edits) | none — audit only | — |
| ZB-0.10 | ⬜ | Migration / fallback strategy doc | `docs/builder/native-executor-migration.md` (new) | — |

Full scoping detail: [`docs/builder/zb-0-scoping.md`](docs/builder/zb-0-scoping.md).

---

## ZB-A — Linux native single-stage executor (youki + overlayfs)

**Status: ⬜ not started · Effort: 5–7 person-weeks · Risk: medium-high**

| # | Status | Task | Commit |
|---|---|---|---|
| ZB-A.1 | ⬜ | `YoukiBackend` skeleton + selector wiring | — |
| ZB-A.2 | ⬜ | overlayfs mount helper (`MNT_DETACH` on Drop) | — |
| ZB-A.3 | ⬜ | Base-image materialisation (reuse `ImagePuller` + `LayerUnpacker`) | — |
| ZB-A.4 | ⬜ | `DigestWriter<W>` streaming sha256 | — |
| ZB-A.5 | ⬜ | **Upperdir-to-OCI-tar emitter with whiteout translation** (highest risk) | — |
| ZB-A.6 | ⬜ | **Per-step youki sandbox executor** (highest risk) | — |
| ZB-A.7 | ⬜ | Build context tar + bind path (`.dockerignore` honored) | — |
| ZB-A.8 | ⬜ | Single-RUN step driver | — |
| ZB-A.9 | ⬜ | COPY / ADD step driver | — |
| ZB-A.10 | ⬜ | Metadata-only instruction handlers (env/workdir/user/cmd/entrypoint/expose/label/volume/stopsignal/healthcheck/shell/onbuild) | — |
| ZB-A.11 | ⬜ | Image config + manifest assembly | — |
| ZB-A.12 | ⬜ | Top-level `build_image` orchestrator | — |
| ZB-A.13 | ⬜ | `push_image`/`tag_image`/`manifest_*` via `oci-client` | — |
| ZB-A.14 | ⬜ | `BuildBackend` impl complete + tests (root-gated integration) | — |
| ZB-A.15 | ⬜ | CLI `--backend youki` + docs | — |

Full scoping detail: [`docs/builder/zb-a-scoping.md`](docs/builder/zb-a-scoping.md).

---

## ZB-B — Linux multi-stage builds + `COPY --from=<stage>`

**Status: ⬜ not started · Effort: ~24 agent-hours / 1.3 person-weeks · Risk: low**

| # | Status | Task | Commit |
|---|---|---|---|
| ZB-B.1 | ⬜ | `StageRootfsRegistry` (triple-indexed by name/index/`Stage::identifier()`) | — |
| ZB-B.2 | ⬜ | `COPY --from=<stage>` dispatch | — |
| ZB-B.3 | ⬜ | `--target` stage truncation | — |
| ZB-B.4 | ⬜ | External `COPY --from=<image>` pull + materialise | — |
| ZB-B.5 | ⬜ | External vs stage disambiguation | — |
| ZB-B.6 | ⬜ | Stage cleanup + `--keep-stages` debug flag | — |
| ZB-B.7 | ⬜ | Per-stage TUI events | — |
| ZB-B.8 | ⬜ | Tests (10 named scenarios + property test) | — |

Full scoping detail: [`docs/builder/zb-b-scoping.md`](docs/builder/zb-b-scoping.md).

---

## ZB-C — BuildKit-style cache + `--mount=type=cache`

**Status: ⬜ not started · Effort: ~45 agent-hours / 3 person-weeks · Risk: med-high (cache-key correctness)**

| # | Status | Task | Commit |
|---|---|---|---|
| ZB-C.1 | ⬜ | `CacheKey` + `StepHashInput` types | — |
| ZB-C.2 | ⬜ | Wire `pub mod cache` + re-exports | — |
| ZB-C.3 | ⬜ | `BuildCacheStore` wrapping `PersistentBlobCache` | — |
| ZB-C.4 | ⬜ | `ZLayerDirs::build_cache()` helper | — |
| ZB-C.5 | ⬜ | **Canonical text for RUN** (heredoc preservation, mount sorting, ARG/ENV substitution) | — |
| ZB-C.6 | ⬜ | Canonical text for COPY/ADD with content digest from ZB-C.7 | — |
| ZB-C.7 | ⬜ | **Source-context content digest** (BuildKit `cache/contenthash` port) | — |
| ZB-C.8 | ⬜ | Metadata-instruction canonical text | — |
| ZB-C.9 | ⬜ | Wire cache lookups into per-instruction loop | — |
| ZB-C.10 | ⬜ | `MountSpec` cache-key contribution | — |
| ZB-C.11 | ⬜ | Persistent cache-mount host dirs; fix `CacheSharing::default()` `Locked` → `Shared` | — |
| ZB-C.12 | ⬜ | `--cache-from`/`--cache-to` CLI flags | — |
| ZB-C.13 | ⬜ | `CacheRegistryExporter` (OCI image manifest with `image-manifest=true`) | — |
| ZB-C.14 | ⬜ | `CacheRegistryImporter` (graceful 404) | — |
| ZB-C.15 | ⬜ | LRU pruning + `zlayer build cache prune` subcommand | — |

Full scoping detail: [`docs/builder/zb-c-scoping.md`](docs/builder/zb-c-scoping.md).

---

## ZB-D — Multi-architecture builds (`linux/amd64,linux/arm64`)

**Status: ⬜ not started · Effort: ~60 agent-hours / 1.5–2 person-weeks · Risk: low**

| # | Status | Task | Commit |
|---|---|---|---|
| ZB-D.1 | ⬜ | Audit existing multi-platform plumbing; design doc | — |
| ZB-D.2 | ⬜ | CLI: comma-separated `--platform`; new `TargetPlatform` in `zlayer-types` | — |
| ZB-D.3 | ⬜ | `BuildOptions.platforms: Vec<TargetPlatform>` | — |
| ZB-D.4 | ⬜ | Binfmt detection (`/proc/sys/fs/binfmt_misc/qemu-<arch>`) | — |
| ZB-D.5 | ⬜ | Wire binfmt check into native single-platform build | — |
| ZB-D.6 | ⬜ | Per-platform build fan-out via `JoinSet` | — |
| ZB-D.7 | ⬜ | Native `manifest_create` + `manifest_add` (port from sandbox backend) | — |
| ZB-D.8 | ⬜ | Native `manifest_push` (port from sandbox) | — |
| ZB-D.9 | ⬜ | Auto-construct manifest list after multi-platform build | — |
| ZB-D.10 | ⬜ | Cross-compile escape hatch (`BUILDPLATFORM`/`TARGETPLATFORM`) | — |
| ZB-D.11 | ⬜ | E2E test: dual-platform build + push | — |
| ZB-D.12 | ⬜ | CHANGELOG + `docs/multi-arch-builds.md` | — |

Full scoping detail: [`docs/builder/zb-d-scoping.md`](docs/builder/zb-d-scoping.md).

---

## ZB-E — macOS Linux-in-VM build path (Virtualization.framework)

**Status: ⬜ not started · Effort: ~50 agent-hours / 2 person-weeks · Risk: medium**

Substrate: `objc2-virtualization 0.3.2` + in-repo thin safe wrapper. libkrun stays as tactical fallback via `MACOS_BUILDER_VM_SUBSTRATE=libkrun` until VZ proves out, then ripped out in a separate cleanup pass.

| # | Status | Task | Commit |
|---|---|---|---|
| ZB-E.1 | ⬜ | New `crates/zlayer-macos-vz/` crate with `objc2-virtualization` wrapper | — |
| ZB-E.2 | ⬜ | `BuilderVm` lifecycle (state machine, owns `VZVirtualMachine`) | — |
| ZB-E.3 | ⬜ | Idle timeout + graceful shutdown (VZ `requestStop()`) | — |
| ZB-E.4 | ⬜ | `bin/zlayer-builder-agent` guest binary (vsock JSON RPC) | — |
| ZB-E.5 | ⬜ | Guest rootfs assembly + `default_builder_vm_rootfs_path()` | — |
| ZB-E.6 | ⬜ | Host-side vsock client (length-prefixed JSON) | — |
| ZB-E.7 | ⬜ | `LinuxInVmBackend` implementing `BuildBackend` | — |
| ZB-E.8 | ⬜ | `detect_backend` routing on macOS host | — |
| ZB-E.9 | ⬜ | `ImageOs::MacOS` variant + routing matrix | — |
| ZB-E.10 | ⬜ | virtiofs context transfer (with 30-min concurrency spike) | — |
| ZB-E.11 | ⬜ | `MACOS_BUILDER_VM_SUBSTRATE={vz,libkrun}` env var; libkrun fallback | — |
| ZB-E.12 | ⬜ | E2E smoke test gated `ZLAYER_E2E=1` + macOS host | — |
| ZB-E.13 | ⬜ | CHANGELOG + docs (VZ substrate, virtiofs, migration window) | — |
| ZB-E.14 | ⬜ | Investigate `apple/container` for borrowable patterns; document | — |

Full scoping detail: [`docs/builder/zb-e-scoping.md`](docs/builder/zb-e-scoping.md) (composite document — Section A is the substrate investigation, Section B is the superseded libkrun-based scoping).

---

## ZB-F — Windows native multi-stage + WSL2 Linux routing

**Status: ⬜ not started · Effort: ~45 agent-hours / 1.5–2 person-weeks · Risk: medium**

[WIN-HOST] tasks require the MiniWindows CI runner per `docs/windows-ci-runner.md`.

| # | Status | Task | Commit |
|---|---|---|---|
| ZB-F.1 | ⬜ | Refactor `BuildSkeleton` → `Vec<StageState>` | — |
| ZB-F.2 | ⬜ | Per-stage base materialisation loop | — |
| ZB-F.3 | ⬜ | Per-stage instruction loop in `build_image_for_backend` | — |
| ZB-F.4 | ⬜ | [WIN-HOST] Finalize source stage to exportable layer chain | — |
| ZB-F.5 | ⬜ | [WIN-HOST] `export_stage_files` via `wclayer::export_layer` | — |
| ZB-F.6 | ⬜ | Source-path selection / glob matching | — |
| ZB-F.7 | ⬜ | [WIN-HOST] Cross-stage COPY via `BackupStreamReader`/`Writer` | — |
| ZB-F.8 | ⬜ | [WIN-HOST] Directory recursion + tombstone filtering | — |
| ZB-F.9 | ⬜ | Lift multi-stage gate; rewrite test as positive assertion | — |
| ZB-F.10 | ⬜ | Manifest list in-memory model + persistence | — |
| ZB-F.11 | ⬜ | Manifest list push via existing `push_image_index` | — |
| ZB-F.12 | ⬜ | [WIN-HOST] WSL2 routing wired into `detect_backend` | — |
| ZB-F.13 | ⬜ | [WIN-HOST] `WslLinuxBackend` impl (context via UNC, `wsl_exec`) | — |
| ZB-F.14 | ⬜ | [WIN-HOST] HCN version-skew probe in `is_available` | — |
| ZB-F.15 | ⬜ | CHANGELOG + cross-platform unit tests | — |

Full scoping detail: [`docs/builder/zb-f-scoping.md`](docs/builder/zb-f-scoping.md).

---

## ZB-G — SBOM, SLSA provenance, advanced mounts, cache matrix

**Status: ⬜ not started · Effort: ~53–55 agent-hours / 4–5 person-weeks · Risk: medium**

| # | Status | Task | Commit |
|---|---|---|---|
| ZB-G.1 | ⬜ | Types in `zlayer-types`: `SbomFormat`, `SbomEngine`, `SbomDescriptor`, `ProvenancePredicate`, `BuildAttestation` | — |
| ZB-G.2 | ⬜ | API surface: `sbom`/`sbom_engine`/`provenance` fields | — |
| ZB-G.3 | ⬜ | Cache spec parser completion (`type=s3`, `type=local`, `type=inline`) | — |
| ZB-G.4 | ⬜ | Secret mount executor (tmpfs, cache key hashes id only) | — |
| ZB-G.5 | ⬜ | SSH mount executor (`SSH_AUTH_SOCK` forwarding) | — |
| ZB-G.6a | ⬜ | **Native SBOM generator** (default; `cyclonedx-bom`, partial day-one coverage) | — |
| ZB-G.6b | ⬜ | **Syft engine adapter** (`--sbom-engine=syft`) | — |
| ZB-G.7 | ⬜ | SLSA v1.0 provenance generator (in-toto Statement) | — |
| ZB-G.8 | ⬜ | Referrer push (Referrers API + tag fallback on 404) | — |
| ZB-G.9 | ⬜ | Inline cache embedding (OCI manifest annotations) | — |
| ZB-G.10 | ⬜ | S3 / local cache backend wiring | — |
| ZB-G.11 | ⬜ | CLI surface (`--sbom`, `--sbom-engine`, `--provenance`, `--secret`, `--ssh`) | — |
| ZB-G.12 | ⬜ | Tests + CHANGELOG (incl. cross-engine equivalence test) | — |

Full scoping detail: [`docs/builder/zb-g-scoping.md`](docs/builder/zb-g-scoping.md).

---

## Verification protocol

After EVERY agent group completes:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

ALL FOUR green across the entire workspace. Failures in unrelated crates are still our responsibility. Never `-p <crate>`.

## Update protocol

After each agent group, the orchestrating session:

1. Reads the changed files to verify the agent's claims.
2. Runs the four verification commands above.
3. Updates the relevant rows in this file: status marker (`⬜` → `🟡` → `✅`), commit hash in the rightmost column.
4. Adds any newly discovered follow-up tasks as new rows in the appropriate workstream section.
5. Commits this file alongside the implementation changes when the workstream's tracking changes are non-trivial; otherwise updates it in its own one-line commit (`chore: update ZLAYER_BUILDER.md for ZB-A.5`).
