## Phase A — Linux native single-stage executor

### Goals

Stand up a `YoukiBackend: BuildBackend` that, on Linux, executes single-stage Dockerfiles end-to-end without invoking the `buildah` CLI. Concretely:

- Materialise the base image as a stack of OCI layer rootfs trees on disk under the build's data dir, addressed by diff-id.
- For each Dockerfile instruction in order: mount an overlayfs on top of the current layer stack with a fresh upperdir/workdir, execute the instruction inside that upper, then commit the upperdir as a new OCI tar layer (with proper whiteouts) into the local registry blob store.
- Assemble a v1 OCI manifest + image config referencing those layers and store it via `LocalRegistry` so existing pull/run/push paths see the image identically to a buildah-built one.
- Cover the Dockerfile instructions enumerated below (single-stage only, `COPY --from=<stage>` is Phase B).
- Honour `ZLAYER_BUILDER_BACKEND=youki` to select this backend; default stays `buildah` until Phase A is green.

Out of scope here (other phases): multi-stage, `--mount=type=cache|secret|ssh`, QEMU/multi-arch, macOS, Windows, and the `BuildBackend` trait extension (which is Phase 0 and already in the trait surface at `crates/zlayer-builder/src/backend/mod.rs:87`).

### Steps

Each step is one agent task. Function signatures are first-pass — implementers may refine but should not widen the contract without re-scoping.

**A.1 — Add `YoukiBackend` skeleton + selector wiring.** Files: `crates/zlayer-builder/src/backend/mod.rs`, `crates/zlayer-builder/src/backend/youki/mod.rs` (new). Add `#[cfg(target_os = "linux")] pub mod youki;`. Define `pub struct YoukiBackend { data_dir: PathBuf, registry: LocalRegistry }` and stub `impl BuildBackend` returning `Err(BuildError::Unsupported)` for all methods. Extend `detect_backend()` (`backend/mod.rs:143`) to honour `ZLAYER_BUILDER_BACKEND=youki` (Linux only). ~100 LOC. Low risk. Depends on: none. 2h.

**A.2 — overlayfs mount helper (`overlay.rs`).** File: `crates/zlayer-builder/src/backend/youki/overlay.rs`. `pub struct Overlay { mountpoint: PathBuf, upperdir: PathBuf, workdir: PathBuf }`. `pub fn mount(lowerdirs: &[&Path], upperdir: &Path, workdir: &Path, mountpoint: &Path) -> Result<Overlay>` using `nix::mount::mount` with `MsFlags::empty()` and data `"lowerdir=L1:L2:...,upperdir=...,workdir=..."`. `impl Drop` calls `nix::mount::umount2` with `MNT_DETACH`. `nix` is already vendored with the `mount` feature (`Cargo.toml:193`). ~120 LOC + unit tests requiring root (gate behind `#[ignore]` or use `unshare(CLONE_NEWUSER|CLONE_NEWNS)`). Medium risk (root + cleanup races). Depends on: A.1. 4h.

**A.3 — Base-image materialisation.** File: `crates/zlayer-builder/src/backend/youki/base.rs`. `async fn materialise_base(registry: &LocalRegistry, image: &ImageReference) -> Result<Vec<PathBuf>>` — pulls the manifest (reuse `ImagePuller` from `zlayer-registry`, see `sandbox_builder.rs:912-981` for the reference call sequence), then for each layer descriptor: if a directory `<data_dir>/youki-layers/<diff_id>/` already exists skip, else fetch the blob via `registry.get_blob`, decompress + untar into that dir using `LayerUnpacker::new(diff_dir).unpack_layer(&blob, media_type)` (`crates/zlayer-registry/src/unpack.rs:189`). Returns the ordered list of materialised lowerdirs. ~150 LOC. Medium risk (layer caching key correctness). Depends on: A.1. 4h.

**A.4 — sha256 streaming digest writer.** File: `crates/zlayer-builder/src/backend/youki/digest.rs`. `pub struct DigestWriter<W: Write> { inner: W, hasher: Sha256, bytes: u64 }` impl `Write`, plus `pub fn finalize(self) -> (W, String /* sha256:... */, u64)`. Mirror the streaming pattern in `sandbox_builder.rs:316-324` but on a `Write` adapter, not after-the-fact. `sha2` is at `Cargo.toml:157`. ~60 LOC. Low risk. Depends on: A.1. 1.5h.

**A.5 — Upperdir-to-OCI-tar emitter with whiteout translation.** File: `crates/zlayer-builder/src/backend/youki/commit.rs`. `pub fn commit_upperdir(upperdir: &Path, out: &mut impl Write) -> Result<()>`. Walks `upperdir`, and for each entry: a character-device with major=0 minor=0 (overlayfs whiteout) emits a tar entry named `.wh.<basename>` with empty content; the xattr `trusted.overlay.opaque="y"` on a directory emits an extra `.wh..wh..opq` entry inside that directory; everything else writes through. Use `tar::Builder` (`Cargo.toml:185`) wrapping a `DigestWriter` (A.4) wrapping a `GzEncoder` (`flate2`, `Cargo.toml:186`) wrapping the file. The OCI spec for whiteouts is at `https://github.com/opencontainers/image-spec/blob/main/layer.md` — confirmed: `.wh.<name>` for file/dir deletion and `.wh..wh..opq` inside a dir for opaque whiteout; the existing unpacker treats these correctly at `unpack.rs:320-335`. Read the overlayfs whiteout xattr via `nix::sys::stat::lstat` (S_IFCHR + rdev==0) for the char-dev case, and `xattr` crate for `trusted.overlay.opaque` (need to `cargo add xattr` at impl time). ~200 LOC. High risk (this is the trickiest piece — get fuzz/property tests). Depends on: A.4. 8h.

**A.6 — Per-step youki sandbox executor.** File: `crates/zlayer-builder/src/backend/youki/exec.rs`. `pub async fn run_step(rootfs: &Path, command: &[String], env: &[(String, String)], workdir: &Path, user: Option<&str>, network: RunNetwork) -> Result<i32>`. Writes a minimal `config.json` (OCI runtime spec) into a temp bundle dir alongside the rootfs, then drives `libcontainer::container::ContainerBuilder::new(id, SyscallType::Linux).with_root_path(...).as_init(&bundle).build()` exactly like `crates/zlayer-agent/src/runtimes/youki.rs:1005-1090` does, but tuned for one-shot exec (process exits → tear down). Wait for exit via `Container::wait()`; capture stdout/stderr through pre-opened pipes wired into the spec's `process.terminal=false` + `stdio` paths. ~250 LOC. High risk. Depends on: A.1. 12h.

**A.7 — Build context tar + bind path.** File: `crates/zlayer-builder/src/backend/youki/context.rs`. `pub fn prepare_context(context: &Path, excludes: &[String]) -> Result<PathBuf>` — copies the context honouring `.dockerignore` into a per-build scratch dir under `<data_dir>/builds/<build_id>/context/`. For RUN steps that need `--mount=type=bind,from=context` we'll bind-mount this scratch dir read-only into the rootfs at A.6's spec time. For COPY/ADD we read from this scratch dir directly. Buildah hides this behind its own context tar, so we replicate the model rather than the implementation. ~120 LOC. Low risk. Depends on: A.1. 3h.

**A.8 — Single-RUN step driver.** File: `crates/zlayer-builder/src/backend/youki/step.rs` (consolidates A.2 + A.5 + A.6 for RUN). `async fn run_run_step(state: &mut BuildState, run: &RunInstruction) -> Result<LayerDescriptor>`. Allocates `upper-N/`, `work-N/`, `merged-N/` under the build dir; mounts overlay; spawns youki container with `merged-N/` as rootfs; on success, commits `upper-N/` to a gzipped tar via A.5; computes the uncompressed diff-id by re-streaming through a second `Sha256` (OCI requires both digest-on-disk and diff-id-uncompressed); registers the blob via `registry.put_blob`; unmounts overlay. ~180 LOC. High risk (orchestration glue). Depends on: A.2, A.5, A.6. 6h.

**A.9 — COPY / ADD step driver.** File: `crates/zlayer-builder/src/backend/youki/step.rs` (same module as A.8). `async fn run_copy_step(state, copy) -> Result<LayerDescriptor>` and `async fn run_add_step(state, add) -> Result<LayerDescriptor>`. No container needed — overlay mount, then `std::fs` walk from the context (or URL fetch for ADD) into `upper-N/`, applying `--chown`/`--chmod` semantics via `nix::unistd::{chown, fchmodat}`. ADD URL fetch: use existing HTTP client (look up at impl time — there is already `reqwest` somewhere in the workspace, confirm before `cargo add`). ADD tar auto-extract: use `tar::Archive` over the fetched/local archive when source is a local file with a recognised suffix (mirror `sandbox_builder.rs:1923-2030` for the detection list). Commit upperdir via A.5. ~200 LOC. Medium risk. Depends on: A.2, A.5, A.7. 6h.

**A.10 — Metadata-only instruction handlers.** File: `crates/zlayer-builder/src/backend/youki/meta.rs`. Pure functions that mutate an `ImageConfig` (`crates/zlayer-registry/src/image_config.rs:32`): `apply_env`, `apply_workdir`, `apply_user`, `apply_cmd`, `apply_entrypoint`, `apply_expose`, `apply_label`, `apply_volume`, `apply_stopsignal`, `apply_healthcheck`, `apply_shell`. No layer emitted; these only touch the manifest's config blob (see parity table). ARG handling stays in the existing `expand_variables` / `substitute_args` pipeline (`sandbox_builder.rs:1879`) — Phase A only wires the existing IR consumer. ~150 LOC. Low risk. Depends on: A.1. 3h.

**A.11 — Image config + manifest assembly.** File: `crates/zlayer-builder/src/backend/youki/assemble.rs`. `fn assemble_image(layers: &[LayerDescriptor], config: ImageConfig, history: Vec<History>) -> (OciManifest, /* config_blob */ Vec<u8>)`. Computes the config JSON, its sha256, then constructs an `OciManifest` (`crates/zlayer-registry/src/oci_export.rs:158`) with `config` pointing at the config blob and `layers` listing each compressed layer descriptor. Push config + manifest into the registry via `registry.put_blob` + `registry.put_manifest`. ~120 LOC. Low risk. Depends on: A.8, A.9, A.10. 3h.

**A.12 — Top-level `build_image` orchestrator.** File: `crates/zlayer-builder/src/backend/youki/mod.rs`. Wire all the pieces: materialise base (A.3), iterate the single stage's instructions calling A.8/A.9/A.10, assemble (A.11), tag via `registry.tag_manifest`. Send `BuildEvent`s on the `event_tx` channel parallel to what `BuildahBackend::build_image` does (`backend/buildah.rs:334`). Return `BuiltImage`. Reject multi-stage with `BuildError::Unsupported { reason: "multi-stage requires Phase B" }`. ~200 LOC. High risk (integration). Depends on: A.3, A.8, A.9, A.10, A.11. 6h.

**A.13 — Push / tag / manifest_create / manifest_add / manifest_push.** File: `crates/zlayer-builder/src/backend/youki/mod.rs`. Push: reuse `oci-client` (`Cargo.toml:145`) — no buildah dependency. Tag: just `registry.tag_manifest`. Manifest list: build an `OciIndex` (`oci_export.rs:77`) with platform descriptors. ~150 LOC. Medium risk. Depends on: A.12. 4h.

**A.14 — `BuildBackend` impl complete + unit tests for each instruction handler.** Files: `crates/zlayer-builder/src/backend/youki/mod.rs` + a new `crates/zlayer-builder/tests/youki_*.rs` set. Smoke tests gated `#[cfg(target_os = "linux")]` and `#[ignore]` (root required). At minimum: empty `FROM scratch` → built image; `FROM alpine + RUN echo` → built image; `FROM alpine + COPY hello.txt /` → file present in layer; whiteout property test (create file in layer 1, delete in layer 2, confirm `.wh.` emission). ~200 LOC. Low–medium risk. Depends on: A.12, A.13. 6h.

**A.15 — CLI/UX: `--backend youki` flag + docs in `bin/zlayer/src/commands/build.rs`.** Add a CLI flag that sets `ZLAYER_BUILDER_BACKEND` for the call site. Update `--help`. ~30 LOC. Low risk. Depends on: A.12. 1h.

Total: ~2030 LOC headline (above the 1500 estimate — the whiteout emitter and youki exec are the cost drivers).

### Files to create

- `crates/zlayer-builder/src/backend/youki/mod.rs`
- `crates/zlayer-builder/src/backend/youki/overlay.rs`
- `crates/zlayer-builder/src/backend/youki/base.rs`
- `crates/zlayer-builder/src/backend/youki/digest.rs`
- `crates/zlayer-builder/src/backend/youki/commit.rs`
- `crates/zlayer-builder/src/backend/youki/exec.rs`
- `crates/zlayer-builder/src/backend/youki/context.rs`
- `crates/zlayer-builder/src/backend/youki/step.rs`
- `crates/zlayer-builder/src/backend/youki/meta.rs`
- `crates/zlayer-builder/src/backend/youki/assemble.rs`
- `crates/zlayer-builder/tests/youki_smoke.rs`

### Files to modify

- `crates/zlayer-builder/src/backend/mod.rs` — register module + extend `detect_backend()` env-var branch (lines around 143).
- `crates/zlayer-builder/src/lib.rs` — re-export `YoukiBackend` behind `#[cfg(target_os = "linux")]` next to `BuildahBackend` at line 273.
- `crates/zlayer-builder/Cargo.toml` — `cargo add xattr` (look up current version at impl time) under `[target.'cfg(target_os = "linux")'.dependencies]`. Pull `libcontainer` + `nix` + `sha2` + `flate2` + `tar` + `zlayer-registry` in via the workspace.
- `bin/zlayer/src/commands/build.rs` — `--backend` flag plumbing.

### Reusable code from existing tree

- `crates/zlayer-registry/src/unpack.rs:55-83` — media-type classification (`is_linux_layer`) drives our compatibility filter when materialising base layers (A.3).
- `crates/zlayer-registry/src/unpack.rs:84-87, 320-335` — confirms `.wh.` / `.wh..wh..opq` semantics in the consumer direction; A.5 emits the inverse.
- `crates/zlayer-registry/src/unpack.rs:189` — `LayerUnpacker::unpack_layer` is called as-is to materialise base layers.
- `crates/zlayer-registry/src/oci_export.rs:77, 110, 158` — `OciIndex`, `OciDescriptor`, `OciManifest` structs are the manifest writer surface for A.11/A.13.
- `crates/zlayer-registry/src/cache.rs:310` — `compute_digest` for one-shot digesting; A.4's streaming variant complements but does not replace.
- `crates/zlayer-registry/src/image_config.rs:32` — `ImageConfig` struct A.10/A.11 mutate.
- `crates/zlayer-builder/src/sandbox_builder.rs:1264-1585` — exemplar instruction-execution structure (env construction, workdir resolution, error mapping). The mechanism differs (sandbox-exec vs youki) but the shape carries over.
- `crates/zlayer-builder/src/sandbox_builder.rs:1675-1845` — ADD URL fetch + archive auto-extract patterns; A.9 reimplements the same predicates against `is_extractable_archive_name` (sandbox_builder.rs:1935).
- `crates/zlayer-builder/src/sandbox_builder.rs:1894` — `resolve_user_name(user, rootfs_dir)` reads `etc/passwd` from the rootfs — reusable for USER lookups in A.6/A.10.
- `crates/zlayer-agent/src/runtimes/youki.rs:1005-1090` — canonical `ContainerBuilder::new(...).as_init(&bundle).build()` invocation in `spawn_blocking`; A.6 follows the same shape with a one-shot lifecycle instead of a persistent agent container.
- `crates/zlayer-agent/src/bundle.rs` — `BundleBuilder::new(...).write_config(id, spec)` is overkill for A.6 (`ServiceSpec` doesn't match the build step model), so A.6 will likely hand-write a minimal `config.json` instead of reusing this. Confirm during A.6 — don't bend `BundleBuilder` to fit.

### Per-Dockerfile-instruction parity table

| Instruction | Existing reference | Algorithm | New code needed (LOC est) | Notes |
|---|---|---|---|---|
| `FROM` | `backend/buildah.rs:157-239` (`resolve_base_image`) + `sandbox_builder.rs:653-839` (`setup_base_image`) | Resolve registry → pull manifest → materialise layers into per-diff-id dirs (A.3). `FROM scratch` → empty `lowerdirs`. | 0 in A; A.3 covers materialisation | Multi-stage `FROM stage AS name` deferred to Phase B. |
| `RUN` | `backend/buildah.rs:543-615` (translator + retries) + `sandbox_builder.rs:1264-1585` (exec) | Overlay mount (A.2) → youki container (A.6) over `merged-N/` → commit `upper-N/` via A.5. | ~180 (A.8) | Retries: replicate `options.retries+1` loop from buildah backend at line 530. Default `--mount=type=cache|secret|ssh` is Phase C/G — Phase A errors out cleanly. |
| `COPY` (local context only) | `backend/buildah.rs:461-503` (resolution) + `sandbox_builder.rs:1590-1675` (execution) | Overlay mount → fs-walk from context (A.7) into `upper-N/`, apply `--chown`/`--chmod`/`--exclude`. Commit via A.5. | ~100 of the 200 in A.9 | `--from=<stage>` rejected with `Unsupported` (Phase B); `--from=<external-image>` rejected with `Unsupported` (Phase C). `--link` accepted as a hint, ignored functionally (still produces a layer; the layer-as-overlay model already gets cache wins). |
| `ADD` (local files + URL + tar extract) | `sandbox_builder.rs:1675-1845` | Overlay mount → if URL, fetch via http client → if archive (gzip/bzip2/xz/zip tar suffix) auto-extract into `upper-N/` → else copy. `--checksum=sha256:...` validated against the fetched bytes. | ~100 of the 200 in A.9 | `--keep-git-dir` accepted no-op for git-style sources (rare in Phase A scope). |
| `WORKDIR` | `backend/buildah.rs:621-623` (state tracking) | Mutate `ImageConfig.working_dir`. Also pre-create the dir inside the next overlay upper so subsequent RUN steps don't fail. | ~15 in A.10 | — |
| `ENV` | parser handles parsing | Mutate `ImageConfig.env` (append `KEY=VALUE`). Substitutions in subsequent instructions reuse existing `expand_variables` (`crates/zlayer-builder/src/dockerfile/variable.rs`). | ~10 in A.10 | — |
| `ARG` | `sandbox_builder.rs:1879` (`substitute_args`) | Tracked in build-state `HashMap<String, String>`, merged with `options.build_args`. Not baked into image config (matches Docker semantics). | ~15 in A.10 | `ARG` declared before `FROM` (global ARGs) handled identically — they're already in the parser IR. |
| `CMD` | parser IR | Mutate `ImageConfig.cmd` to the `ShellOrExec` payload. | ~10 in A.10 | — |
| `ENTRYPOINT` | parser IR | Mutate `ImageConfig.entrypoint`. Per OCI spec, ENTRYPOINT clears CMD if both are unspecified for the next image. | ~15 in A.10 | — |
| `EXPOSE` | parser IR | Insert `"<port>/<proto>": {}` into `ImageConfig.exposed_ports`. | ~10 in A.10 | — |
| `LABEL` | parser IR | Merge into `ImageConfig.labels`. | ~10 in A.10 | — |
| `USER` | `sandbox_builder.rs:1894` (`resolve_user_name`) | Mutate `ImageConfig.user`; A.6 reads this and passes uid/gid into the youki process spec. Name→uid lookup reads `etc/passwd` from the *current* materialised rootfs (lowerdir stack). | ~30 in A.10 (lookup) | — |
| `VOLUME` | parser IR | Insert into `ImageConfig.volumes`. | ~5 in A.10 | — |
| `STOPSIGNAL` | parser IR | Set `ImageConfig.stop_signal`. | ~5 in A.10 | — |
| `HEALTHCHECK` | parser IR | Set `ImageConfig.healthcheck`. | ~20 in A.10 | — |
| `SHELL` | `backend/buildah.rs:539-544` comment | Set `ImageConfig.shell`; A.8 reads this when assembling shell-form RUN commands. | ~10 in A.10 | — |
| `ONBUILD` | parser IR (`Instruction::Onbuild(Box<Instruction>)`) | Phase A: append the inner instruction text to `ImageConfig.config.OnBuild`. Triggers fire at downstream-image build time, which is correctly out of scope for the executor itself. | ~10 in A.10 | — |

### Tests

- **Unit (in-tree, no root):** `digest.rs` round-trip; `commit.rs` whiteout-emission table tests (synthesize char-dev mknod via `nix::sys::stat::mknod` in a tmpfs the test owns — gate `#[ignore]` for env without privileges); `meta.rs` config-mutation tests; `context.rs` `.dockerignore` matching.
- **Integration (root-gated, `#[ignore]`):** `tests/youki_smoke.rs` covering: scratch build; alpine + single RUN; alpine + COPY local file; alpine + ADD URL with `--checksum`; whiteout property test (file created in layer 1, deleted in layer 2, confirm `.wh.<name>` appears in the layer 2 tar and the consumer unpacker (`LayerUnpacker`) reproduces the deletion); ENV/WORKDIR/USER propagate to the next RUN; CMD/ENTRYPOINT show up in the final manifest config blob.
- **Cross-backend parity:** `tests/youki_vs_buildah.rs` — build the same fixture Dockerfile with both backends, compare the final manifest's layer count and the diff-id list. Skip if buildah isn't installed.

### Risks

- **overlayfs requires root or user-namespace rootless setup.** Rootless overlayfs is kernel-version-gated (5.11+). Mitigation: detect kernel version on `YoukiBackend::is_available`, refuse with a clear error on older kernels; document the requirement.
- **Whiteout encoding** is the bug-prone part. The unpacker is the inverse of what we emit, so we get a round-trip test for free, but real-world layers also use opaque whiteouts in subtle places (e.g. when a directory is replaced). Property test against a corpus of `apk add` / `apt install` upper dirs.
- **youki one-shot vs persistent.** The agent's youki path is built for long-lived containers with stdio piped to files. For build steps we want synchronous exec with streamed stdout/stderr to the `BuildEvent` channel. May need to write a thinner wrapper than `agent/runtimes/youki.rs` provides; I don't yet know whether `Container::wait()` exposes exit-code cleanly in the pinned fork — verify in A.6.
- **Layer cache key.** The existing `instruction.cache_key()` (`dockerfile/instruction.rs:157`) doesn't account for the resolved base-layer diff-ids. Phase A should compute `cache_key = sha256(parent_diff_id || instruction.cache_key())` to be safe; finer-grained content-addressed caching is a Phase C concern.
- **`COPY` ownership/permission edge cases.** macOS sandbox builder uses host paths (no chroot). On Linux we're walking into an overlay upper that's owned by root; `--chown=user:group` must resolve names against the layer's `etc/passwd`, which means we touch the rootfs before the chown.
- **Out of estimate.** 4–6 person-weeks at 1500 LOC was the original sizing — my ~2030 LOC estimate suggests the bottom of the range is unrealistic; budget 5–7 weeks.
- I don't know whether the pinned `zlayer-libcontainer 0.6.1-zlayer.5` fork has any patches that affect one-shot exec semantics — verify during A.6.

### Verification

- `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo build --workspace` all green on Linux.
- `ZLAYER_BUILDER_BACKEND=youki cargo run -p zlayer -- build images/zlayer-base -t zlayer/youki-test:0.1` produces a manifest pullable by an unrelated tool (skopeo or `zlayer run`).
- Smoke build of one real ZImagefile (`images/ZImagefile.zlayer-node`) under both backends; diff the final image filesystem (mount each rootfs, run `diff -ruN`) — expect equality for content, allowing tar-metadata differences (mtime, uid 0 vs configured).
- All `#[ignore]`d root-gated tests pass when run as root in CI.

### Estimated effort

5–7 person-weeks, medium-to-high risk. The original 4–6w / 1500 LOC sizing under-counts the overlayfs whiteout emitter (A.5) and one-shot youki executor (A.6), which together are ~430 LOC and ~20h of focused work. Total LOC ~2030 across new code (excluding tests).
