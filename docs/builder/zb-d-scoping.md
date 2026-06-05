## Phase D — Multi-architecture builds

Native Rust replacement for `buildah` must build OCI images for non-host architectures (`linux/amd64`, `linux/arm64`, eventually more), wire QEMU binfmt emulation as the fallback, allow cross-compile escape hatches, assemble an OCI image index (manifest list), and push it to common registries.

Phases A–C land the single-platform native executor. Phase D layers multi-platform orchestration on top.

### Goals

1. `zlayer build --platform=linux/amd64,linux/arm64 -t REPO:TAG .` produces one OCI image manifest per platform plus an `application/vnd.oci.image.index.v1+json` manifest list referencing them, all served through the native backend (no buildah).
2. Cross-arch builds emulate via QEMU binfmt when the host arch differs from the target. ZLayer detects whether binfmt is registered and emits an actionable error (with the exact privileged-container install command) when it is not.
3. A cross-compile escape hatch lets language runtimes (Rust, Go) skip emulation by running the compile stage natively and only placing the foreign-arch artifact into the final layer.
4. `manifest_push` uploads the index to OCI-compliant registries (ECR, GHCR, GAR, Quay, Harbor, Docker Hub), with documented workarounds for the known incompatibilities.
5. Pipeline path (`PipelineExecutor::build_wave`) keeps working — it already calls `build_multiplatform_image` (`crates/zlayer-builder/src/pipeline/executor.rs:682`) but currently routes through `manifest_push` on the buildah backend; once the native backend implements those trait methods, the pipeline path "just works."

### Steps

Each step is one focused agent task (≤ 2 files, ~200 LOC each).

1. **Audit existing multi-platform plumbing & write design note** (3h). Read `bin/zlayer/src/commands/build.rs:30,44` (the `--platform` flag is already parsed as `Option<String>` and split into a single `ImageOs`), `crates/zlayer-builder/src/builder.rs:389-391` (`BuildOptions.platform: Option<String>`), and `crates/zlayer-builder/src/pipeline/executor.rs:514-520` (`effective_platforms`). Produce design doc capturing: (a) `--platform` today accepts ONE value; CLI must accept comma-separated list and lower into `Vec<String>`; (b) `BuildOptions.platform` becomes `Vec<String>` or a sibling `platforms: Vec<String>` for multi-platform mode; (c) call-site fan-out happens in `builder.rs::build()`, not in each backend. Files: design doc only, no code.

2. **CLI: accept comma-separated `--platform`** (4h). Update `bin/zlayer/src/cli.rs` (already has `platform: Option<String>` at line 517-518) — keep the string flag, parse `linux/amd64,linux/arm64` in `bin/zlayer/src/commands/build.rs:44-50` into `Vec<TargetPlatform>` with a new `TargetPlatform { os: ImageOs, arch: String, variant: Option<String> }` struct (or reuse one from `zlayer-types` per the user's memory rule about shared types). Existing test `test_cli_build_platform_flag_linux_amd64` at `cli.rs:4486` extends naturally to a multi-value test. Files: `bin/zlayer/src/commands/build.rs`, `crates/zlayer-types/src/platform.rs` (new, small).

3. **Plumb `Vec<TargetPlatform>` into `BuildOptions`** (3h). Add `BuildOptions.platforms: Vec<TargetPlatform>` to `crates/zlayer-builder/src/builder.rs:389`, keep deprecated `platform: Option<String>` as a `Vec<TargetPlatform>` adapter for now. Files: `crates/zlayer-builder/src/builder.rs`.

4. **Binfmt detection helper** (5h). New module `crates/zlayer-builder/src/backend/native/binfmt.rs`. Function `pub async fn detect_qemu_for_arch(arch: &str) -> BinfmtStatus` that reads `/proc/sys/fs/binfmt_misc/qemu-<arch>` (or `qemu-x86_64`/`qemu-aarch64`), parses `enabled`, `interpreter`, and the `F` (fix-binary) flag per the kernel docs (https://docs.kernel.org/admin-guide/binfmt-misc.html). Returns `Registered { interpreter, fix_binary: bool } | NotRegistered | NotApplicable` (host == target). Error message for `NotRegistered` includes the recommended install command (the tonistiigi/binfmt privileged-container approach). Files: `crates/zlayer-builder/src/backend/native/binfmt.rs`, `crates/zlayer-builder/src/error.rs` (new `BinfmtNotRegistered` variant).

5. **Wire binfmt check into the native single-platform build** (3h). In the native backend's `build_image` (introduced in Phase A), before forking into the target rootfs, call `detect_qemu_for_arch(target_arch)` if `target_arch != host_arch`. If `NotRegistered`, return `BuildError::BinfmtNotRegistered` with the actionable install instruction. If `Registered` but `fix_binary == false`, log a warning that emulation will not work inside the unprivileged container (per the kernel doc: without `F`, the interpreter must exist in the rootfs). Files: `crates/zlayer-builder/src/backend/native/mod.rs` (or wherever Phase A lands).

6. **Per-platform build fan-out** (8h). In `crates/zlayer-builder/src/builder.rs`, after `BuildOptions.platforms.len() > 1`, drive one native build per platform in a `JoinSet`, each writing to a distinct content-addressed location in the local store. Aggregate the per-platform `BuiltImage` records into a new `MultiPlatformBuild { entries: Vec<(TargetPlatform, BuiltImage)> }`. Reuse the wave logic but for one image × N platforms (the pipeline executor's `build_multiplatform_image` at `crates/zlayer-builder/src/pipeline/executor.rs:936` already does this against the buildah backend — port that orchestration to the single-image CLI path). Files: `crates/zlayer-builder/src/builder.rs`.

7. **Native `manifest_create` + `manifest_add`** (8h). Implement the two trait methods on the native backend, modeled on the working sandbox implementation (`crates/zlayer-builder/src/backend/sandbox.rs:391-624`). Use the local-store layout (manifests dir under data dir, per-manifest `index.json` + `blobs/` subdir). The `Platform` struct already exists in `crates/zlayer-registry` (`crates/zlayer-registry/src/client.rs:9`). Files: `crates/zlayer-builder/src/backend/native/manifest.rs`, `crates/zlayer-builder/src/backend/native/mod.rs`.

8. **Native `manifest_push`** (6h). Port `sandbox::manifest_push` (`crates/zlayer-builder/src/backend/sandbox.rs:650-723`) to the native backend. It reads the on-disk index, pushes every layer+config+manifest blob through `ImagePuller`, then calls `push_image_index_to_registry` (`crates/zlayer-registry/src/client.rs:1678`). All the registry plumbing is already there — this is glue. Files: `crates/zlayer-builder/src/backend/native/manifest.rs`.

9. **Auto-construct the manifest list after multi-platform build** (4h). In `builder.rs::build()`, when `platforms.len() > 1`, after step 6 completes, call `manifest_create(tag)` then `manifest_add(tag, per_platform_tag)` for each. Returns a single `BuiltImage` whose digest is the index digest. Files: `crates/zlayer-builder/src/builder.rs`.

10. **Cross-compile escape hatch (ZImagefile + Dockerfile)** (8h). Honor two existing buildkit-style ARGs in the Dockerfile/ZImagefile IR: `BUILDPLATFORM` (host) and `TARGETPLATFORM` (target). Expose them to `RUN` instructions as build args automatically. Document in the ZImagefile schema how to use them to pick a cross-compile toolchain (e.g. `RUN cargo build --target ${TARGETPLATFORM:-x86_64-unknown-linux-gnu}`). No backend change needed — just argument injection. Files: `crates/zlayer-builder/src/builder.rs` (build-args injection), `crates/zlayer-builder/src/zimage/parser.rs` (docs/comment).

11. **End-to-end test: dual-platform build + index push to local registry** (6h). New `crates/zlayer-builder/tests/multiarch_native.rs`. Build a tiny scratch image for `linux/amd64,linux/arm64`, assert two per-platform manifests exist, assert the index references both with correct `platform.architecture` values, push to a local OCI-distribution registry (CNCF distribution image in a container, ephemeral). Skip with `#[ignore]` on CI hosts that lack binfmt. Files: `crates/zlayer-builder/tests/multiarch_native.rs`.

12. **CHANGELOG + docs** (2h). Document the `--platform` multi-value syntax, the binfmt-setup error and install command, the `BUILDPLATFORM`/`TARGETPLATFORM` args, and the registry compatibility caveats. Files: `CHANGELOG.md`, `docs/multi-arch-builds.md` (new).

Total: ~60 agent-hours ≈ 1.5–2 person-weeks.

### Files to create

- `crates/zlayer-types/src/platform.rs` — shared `TargetPlatform` struct
- `crates/zlayer-builder/src/backend/native/binfmt.rs` — kernel binfmt_misc probe
- `crates/zlayer-builder/src/backend/native/manifest.rs` — native `manifest_create/add/push`
- `crates/zlayer-builder/tests/multiarch_native.rs` — E2E test
- `docs/multi-arch-builds.md` — operator-facing setup guide

### Files to modify

- `bin/zlayer/src/cli.rs` (line 517-518) — extend `--platform` test coverage
- `bin/zlayer/src/commands/build.rs` (line 30, 44) — parse comma-separated platforms
- `crates/zlayer-builder/src/builder.rs` (line 389-391, 1187-1190) — `platforms: Vec<TargetPlatform>` and the fan-out
- `crates/zlayer-builder/src/backend/native/mod.rs` — wire binfmt check into single-platform build (Phase A produces this file)
- `crates/zlayer-builder/src/error.rs` — `BinfmtNotRegistered` variant
- `CHANGELOG.md` — under `[Unreleased]`

### Manifest list (OCI index) spec

Per https://github.com/opencontainers/image-spec/blob/main/image-index.md, the JSON shape is:

```json
{
  "schemaVersion": 2,
  "mediaType": "application/vnd.oci.image.index.v1+json",
  "manifests": [
    {
      "mediaType": "application/vnd.oci.image.manifest.v1+json",
      "digest": "sha256:<per-platform-manifest-digest>",
      "size": <bytes>,
      "platform": {
        "architecture": "amd64",
        "os": "linux",
        "variant": null,
        "os.version": null,
        "os.features": null
      }
    },
    { /* arm64 entry */ }
  ],
  "artifactType": null,
  "annotations": null
}
```

Required: `schemaVersion=2`, `mediaType`, `manifests[]` (may be empty but property must be present). Per-entry required: `mediaType`, `digest`, `size`. `platform.architecture` and `platform.os` are required when `platform` is present. `variant` matters for ARM (`v7` for ARMv7, `v8` for ARMv8 — though `arm64` typically omits it). The `OciImageIndex` / `ImageIndexEntry` / `Platform` types already exist in `crates/zlayer-registry` (`crates/zlayer-registry/src/client.rs:9`) and `push_image_index_to_registry` (`crates/zlayer-registry/src/client.rs:1678`) already serializes and uploads them — Phase D consumes this, does not extend it.

### QEMU binfmt setup story

Two ways the host can have QEMU registered:

1. **Distro-packaged `qemu-user-static`** drops binaries at `/usr/bin/qemu-<arch>-static` and writes entries to `/proc/sys/fs/binfmt_misc/qemu-<arch>` via a sysvinit/systemd unit. The `F` (fix-binary) flag is essential — it tells the kernel to open the interpreter once and clone the open fd for every invocation. Without `F`, the kernel re-resolves the interpreter path at exec time, and the path `/usr/bin/qemu-aarch64-static` won't exist inside an unprivileged container's rootfs (kernel docs: https://docs.kernel.org/admin-guide/binfmt-misc.html).

2. **Privileged container** (e.g. `docker run --privileged --rm tonistiigi/binfmt --install all` — version intentionally unpinned; the agent must look up `gh release list -R tonistiigi/binfmt --limit 5` before suggesting a tag). Mounts `/proc/sys/fs/binfmt_misc` writable and writes the entries with the `F` flag, plus bind-mounts the qemu binaries into a host path the kernel can re-open. This is the recommended path for CI runners.

**ZLayer responsibility**: probe `/proc/sys/fs/binfmt_misc/qemu-<arch>` at build time. If `enabled` and `F` flag set → proceed. If `enabled` and no `F` flag → warn loudly: emulation will fail inside the unprivileged build container unless the qemu binary is copied into the build rootfs (we can offer to do this automatically as a follow-up). If missing → return `BinfmtNotRegistered` with copy-pasteable install command. ZLayer does **not** install binfmt automatically — that requires root + `--privileged` semantics and is operator policy, not build-time policy.

### Registry compatibility matrix

| Registry | Supports OCI index | Known quirks | Workaround needed |
|---|---|---|---|
| AWS ECR (private) | Yes, since 2020-05 (https://aws.amazon.com/about-aws/whats-new/2020/05/ecr-now-supports-manifest-lists-for-multi-architecture-images/) | Image scanning does NOT support `application/vnd.oci.image.index.v1+json` — only the Docker `manifest.list.v2+json` mediaType (https://github.com/docker/setup-buildx-action/issues/186). Push succeeds; scan silently skips. | If ECR scan is required, push with Docker mediaType (`manifest.list.v2+json`) instead. We expose a `--manifest-format=docker` flag deferred to a follow-up. |
| AWS ECR Public | Yes | Same scan limitation. | Same as above. |
| GHCR (GitHub) | Yes, fully OCI-compliant | None observed for index push. Older buildkit cache-image index references were non-compliant (https://gitlab.com/gitlab-org/container-registry/-/issues/407), but that affects cache, not final images. | None for normal use. |
| GAR (Google Artifact Registry) | Yes, fully OCI-compliant | Strict: rejects manifest lists that reference layer descriptors instead of manifests (per the gitlab.com/gitlab-org/container-registry issue above; GAR was named as one of the strict registries). | Make sure index entries always have `mediaType: application/vnd.oci.image.manifest.v1+json`, never a layer mediaType. We do this naturally. |
| Quay.io | Yes since ~late 2021 (https://github.com/containers/buildah/issues/3550) | Server does not echo back `mediaType` on the index it stores (https://github.com/containers/buildah/issues/4395). Push works; round-tripping the field is lossy. | None at push time. Tools that re-pull may need to tolerate missing `mediaType`. |
| Harbor | Yes since v2.0 | Pull-through proxy historically rejected OCI mediaTypes from upstream (https://github.com/goharbor/harbor/issues/15837). Direct push to Harbor is fine; the issue is Harbor-as-pull-cache for upstream OCI images. | None for direct push. Document for users who push to Harbor proxy caches. |
| Docker Hub | Yes | None observed for OCI index. Accepts both Docker manifest list v2 and OCI image index. | None. |

### Tests

- Unit: `binfmt.rs` parser handles `enabled` / `flags F` / missing file, with fixture `/proc` snapshots.
- Unit: CLI `--platform=linux/amd64,linux/arm64` parses into `Vec<TargetPlatform>` of length 2 (extend `cli.rs:4486` patterns).
- Unit: `BuildOptions.platforms.len() > 1` triggers the multi-platform branch (mock backend asserting `manifest_create` was called).
- Integration: build amd64+arm64 of a scratch image on an x86_64 host with binfmt installed; assert both manifests written to local store, index references both.
- Integration: push to a local CNCF distribution registry container; pull back the index, assert `manifests[].platform.architecture` round-trips.
- Negative: build amd64+arm64 with no binfmt → expect `BinfmtNotRegistered` error containing the install command string.

### Risks

- **Binfmt on CI runners.** GitHub Actions Linux runners do not register foreign-arch binfmt by default. CI matrix tests must install binfmt (via the privileged-container approach) as a setup step or be marked `#[ignore]`. The ZLayer CI runbook (`docs/windows-ci-runner.md` exists for Windows — we need a Linux equivalent) must call this out.
- **Cross-arch glibc + ld-linux quirks.** Some qemu-user versions mis-handle `clone()` flags used by modern glibc; rare but real. Document the QEMU version floor when we encounter it (do not hardcode without a real reproducer).
- **Layer cache poisoning across arches.** Phase B's content-addressed cache must key on `target_platform` for every step, or an amd64 layer will be served to an arm64 build. Mitigation: cache key already includes the FROM-image digest, which differs per arch; verify in Phase B's design that intermediate layers are scoped per platform.
- **ECR scan silent skip.** Users may assume their multi-arch image is scanned when it is not. Document prominently. Future flag `--manifest-format=docker` lets ECR users opt into the Docker mediaType.
- **`fix_binary` flag missing.** Distro packages occasionally ship binfmt entries without the `F` flag. Native build inside an unprivileged container will fail when the kernel tries to re-resolve `/usr/bin/qemu-*-static` from inside the rootfs. We detect and warn but cannot fix without copying the static binary into the build rootfs (follow-up).

### Verification

After Phase D lands:

```bash
cargo fmt --all && \
cargo clippy --workspace --all-targets -- -D warnings && \
cargo test --workspace && \
cargo build --workspace
```

All four green across the workspace. Then run the integration test against a real registry the user controls (Quay or GHCR sandbox repo): push, then `crane manifest <ref>` (or equivalent) and assert the index has both architectures with the expected digests. Existing pipeline path (`zlayer pipeline` with `platforms: [linux/amd64, linux/arm64]` in `ZPipeline.yaml`) must continue to work end-to-end — the pipeline executor calls `manifest_push` via the trait, so once the native backend implements those methods (steps 7–8), the existing pipeline tests should pass on the native backend too.

### Estimated effort

~60 agent-hours of focused work, decomposable into 12 small agents as listed above. Roughly 1.5–2 person-weeks of calendar time, matching the prior medium-risk estimate. Risk concentrates in steps 4–5 (binfmt detection — kernel-API-adjacent, host-dependent) and step 11 (E2E test requires a real binfmt-enabled host + a local registry container, both flaky on CI). All other steps are mechanical ports of the existing sandbox backend's implementation, which already works end-to-end against real registries.
