## Phase C — BuildKit-style cache

### Goals

Replace the implicit "let buildah handle it" caching with a first-class content-addressable build cache living under `${ZLAYER_DATA_DIR}/build-cache/`, plus full `--mount=type=cache` semantics and OCI-registry cache export/import (`--cache-to=type=registry`, `--cache-from=type=registry`).

Concretely:

1. Every Dockerfile/ZImagefile instruction that produces a layer gets a deterministic SHA256 cache key. On hit, the executor copies the cached layer descriptor into the new image instead of re-running.
2. `RUN --mount=type=cache,target=...,id=...,sharing=...` mounts a persistent host directory whose contents survive across builds AND does NOT factor into the step's cache key (per BuildKit docs: "Contents of the cache directories persists between builder invocations without invalidating the instruction cache" — https://docs.docker.com/reference/dockerfile/#run---mounttypecache).
3. Local cache store on disk reuses ZLayer's existing `PersistentBlobCache` (sled-backed CAS, LRU eviction) at `crates/zlayer-registry/src/persistent_cache.rs:28`.
4. Cache-to-registry exports a BuildKit-compatible OCI cache manifest (mode=min by default, mode=max as opt-in) so other CI nodes can `--cache-from` it.

Phase C does NOT implement parallel DAG solving (Phase D), `type=secret`/`type=ssh` (Phase G), or multi-arch cache sharing.

### Steps

Each step is one agent, ≤2 files, ≤~250 LOC. Effort in hours assumes a focused agent + verification round.

1. **Define `CacheKey` + `StepHashInput` types in `crates/zlayer-builder/src/cache/key.rs` (new file).** Struct holding `parent_layer_digest: Option<String>`, `instruction_kind`, `canonical_text: String`, `mounts: Vec<MountKeyPart>`, `source_digest: Option<String>`, `arg_env: BTreeMap<String,String>`. `impl CacheKey { pub fn finalize(&self) -> String }` returns `sha256:<hex>`. Unit tests for stable ordering. **2h.**

2. **Add `pub mod cache` to `crates/zlayer-builder/src/lib.rs` and re-export `CacheKey`, `BuildCacheStore`.** Single-file edit. **0.5h.**

3. **Write `crates/zlayer-builder/src/cache/store.rs` — `BuildCacheStore` wrapping `PersistentBlobCache`.** Two namespaces: `step/<sha>` -> layer descriptor JSON (`{digest, size, media_type, diff_id}`); `layer/<sha>` -> compressed blob. Reuses `zlayer_registry::persistent_cache::PersistentBlobCache::open()` (`crates/zlayer-registry/src/persistent_cache.rs:43`) and the `BlobCacheBackend` trait (`crates/zlayer-registry/src/cache.rs:33`). **3h.**

4. **Add `ZLayerDirs::build_cache()` helper in `crates/zlayer-paths/src/lib.rs`.** Returns `{data_dir}/build-cache`. Mirrors the existing `cache()` helper at line 290 and `images()` at line 377. **0.5h.**

5. **Implement canonical text for `RunInstruction` in `crates/zlayer-builder/src/cache/canonical.rs`.** Inputs: shell-vs-exec form, normalized whitespace within heredocs preserved verbatim, mount specs in sorted order by `(type, target, id)`, ARG/ENV referenced via `${VAR}` substituted to their effective values from the build env. Citation: cache-invalidation list at https://docs.docker.com/reference/dockerfile/#run (ARG/ENV/heredoc/mount all invalidate). **4h.**

6. **Canonical text for `CopyInstruction` / `AddInstruction`.** Inputs: sorted source paths, dest, `--chown`, `--chmod`, `--from`, and the *content digest* of the resolved source tree (see step 7). Citation: BuildKit cache-invalidates RUN when COPY/ADD before it changes (same URL). **3h.**

7. **Implement source-context content digest in `crates/zlayer-builder/src/cache/contenthash.rs`.** Mirror BuildKit's `cache/contenthash` (https://github.com/moby/buildkit/tree/master/cache/contenthash — SHA256, merkle-style, per-file records keyed by `convertPathToKey(path)` containing file-type + content digest + symlink target; mtime/uid/gid NOT hashed). Walk the build context honoring `.dockerignore`. **6h.**

8. **Canonical text for `EnvInstruction`, `WorkdirInstruction`, `UserInstruction`, `ExposeInstruction`, `LabelInstruction`, etc. — all the metadata-only instructions** that still feed `parent_layer_digest` to the next step. Trivial: kind + sorted key=value. **2h.**

9. **Wire `CacheKey` computation into the `Builder` driver in `crates/zlayer-builder/src/builder.rs` around the per-instruction execution loop (currently buildah-backed at line ~831).** Each instruction first computes `CacheKey::finalize`, then queries `BuildCacheStore::get_step(key)`. On hit -> skip exec, append cached layer descriptor to image. On miss -> execute, push resulting layer + descriptor into store keyed by `key`. **4h.**

10. **Implement `MountSpec` cache-key contribution** in `crates/zlayer-builder/src/cache/canonical.rs`. Per BuildKit: `type`, `target`, `id` (defaults to `target` — https://docs.docker.com/reference/dockerfile/#run---mounttypecache), `sharing`, `ro`, `from`, `source`, `mode`, `uid`, `gid` ALL feed the key. But the *contents* of the cache dir do NOT. **2h.**

11. **Implement persistent cache-mount host directories in `crates/zlayer-builder/src/cache/mount_volumes.rs`.** Path: `{build-cache}/mounts/{sha256(id_or_target)}/`. Sharing semantics: `shared` (concurrent OK), `locked` (fs2 flock), `private` (per-build tempdir copied from canonical). Wire into the executor's bind-mount setup. The existing `RunMount::Cache` AST at `crates/zlayer-builder/src/dockerfile/instruction.rs:397` already carries `id` + `sharing` + `readonly`; the buildah path at `crates/zlayer-builder/src/buildah/mod.rs:373` is what we're replacing for the native executor. **5h.**

12. **Cache-from/cache-to CLI surface.** Add `--cache-from <ref>` and `--cache-to <ref>` flags to `bin/zlayer/src/commands/build.rs` (currently relies on buildah's flags). Plumb into `BuildOptions` at `crates/zlayer-builder/src/builder.rs:288`. **2h.**

13. **`CacheRegistryExporter` in `crates/zlayer-builder/src/cache/registry_export.rs`.** Emits an OCI image manifest (preferred over index per https://docs.docker.com/build/cache/backends/registry/, `image-manifest=true` for ECR compatibility) whose layers are the cached step blobs and whose config JSON enumerates `cache_keys -> blob_digest` mappings. Reuses `zlayer_registry::oci_export::OciManifest` (`crates/zlayer-registry/src/oci_export.rs:159`) and `OciDescriptor` (line 111). Mode=min ships only final-stage cached layers; mode=max ships every step. **5h.**

14. **`CacheRegistryImporter` in `crates/zlayer-builder/src/cache/registry_import.rs`.** Pulls the cache manifest, populates `BuildCacheStore` with the mappings, lazily fetches blobs on hit. Graceful 404 (per docs: "If the `--cache-from` target doesn't exist, then the cache import step will fail, but the build continues" — https://docs.docker.com/build/cache/backends/registry/). **3h.**

15. **LRU pruning + `zlayer build cache prune` subcommand.** `PersistentBlobCache` already has eviction (`crates/zlayer-registry/src/persistent_cache.rs:300`); expose `--max-size`, `--keep-since`, `--all` flags on a new subcommand under `bin/zlayer/src/commands/build.rs`. Default max size: 20 GiB. **3h.**

### Cache key spec

`CacheKey::finalize` = `"sha256:" + hex(sha256(serialize(StepHashInput)))` where `serialize` is canonical JSON (sorted keys, no whitespace). `StepHashInput` fields, in order:

- `version: u32` — bump on algorithm changes so old caches invalidate cleanly.
- `parent: String` — parent layer's diff_id (`sha256:...`); empty string for the FROM-base layer's first child.
- `kind: String` — `"RUN"` / `"COPY"` / `"ADD"` / `"ENV"` / `"WORKDIR"` / etc.
- `canonical: String` — per-instruction (see below).
- `mounts: Vec<MountKeyPart>` — sorted by `(type, target, id)`.
- `source_digest: Option<String>` — present only for COPY/ADD.
- `platform: String` — `linux/amd64`, `linux/arm64`, etc. Phase D will widen this.

**Per-instruction `canonical` definition:**

- **RUN** (shell form): `"sh\x00" + arg_substitute(cmd_string)`. (Exec form): `"exec\x00" + json_array(argv)`. Heredoc bodies are appended verbatim after `\x00heredoc\x00`. ARG and ENV references are substituted to their *resolved* values from the build env before hashing — this matches BuildKit's documented invalidation rule that ARG/ENV changes invalidate RUN (https://docs.docker.com/reference/dockerfile/#run).
- **COPY**: `"copy\x00" + sorted(srcs).join("\x01") + "\x00" + dest + "\x00" + chown.unwrap_or("") + "\x00" + chmod.unwrap_or("") + "\x00" + from.unwrap_or("")`. `source_digest` carries the merkle hash of the source tree.
- **ADD**: like COPY but `kind = "ADD"` and for HTTP/Git URLs we hash `(url, declared_checksum_or_etag)` instead of walking a tree. If no checksum is declared and the source is a URL, we MUST refetch and hash on every build (this matches Docker behavior).
- **ENV / LABEL**: `kind + "\x00" + sorted_kv_string`. ENV also implicitly affects all subsequent RUN cache keys via the substitute step above.
- **WORKDIR / USER / EXPOSE / SHELL / STOPSIGNAL**: `kind + "\x00" + value`.
- **CMD / ENTRYPOINT / HEALTHCHECK / VOLUME**: metadata-only, no layer, but contribute to the final image config digest (not the step cache).
- **ARG**: never produces a layer; affects downstream canonical via substitution.

**`MountKeyPart` fields** (all contribute, contents do NOT):

```
{ type, target, id_or_target, sharing, readonly, from, source, mode, uid, gid }
```

per https://docs.docker.com/reference/dockerfile/#run---mounttypecache.

**Honest uncertainty**:

- BuildKit's exact byte layout for `cache/contenthash` is not documented prose; we read https://github.com/moby/buildkit/tree/master/cache/contenthash (SHA256, merkle, per-file records keyed by null-byte-encoded paths, mode/uid/gid/mtime NOT hashed, symlink target hashed). Our reimplementation matches that and is documented in-tree; we do NOT promise byte-for-byte cache compatibility with BuildKit's local cache (registry-cache format is the interop point).
- BuildKit's heredoc, here-string, and line-continuation normalization rules are not fully documented prose. We treat the post-parse, pre-shell-exec command string verbatim — same bytes the executor runs.
- BuildKit's `SOURCE_DATE_EPOCH` handling (https://github.com/moby/buildkit/blob/master/docs/build-repro.md) is **not** in this phase; if set, we pass it through to the executor unchanged but do NOT factor it into cache keys (matches BuildKit, which uses it for output timestamps, not invalidation).

### Mount=cache spec

Authoritative reference: https://docs.docker.com/reference/dockerfile/#run---mounttypecache.

| Option | Default | Cache-key contribution | Host-side behavior |
| --- | --- | --- | --- |
| `target` | required | yes | bind target inside container |
| `id` | value of `target` | yes — drives volume identity | `{build-cache}/mounts/{sha256(id)}/` |
| `sharing` | `shared` | yes | `shared`: concurrent; `locked`: fs2 flock; `private`: copy-on-build |
| `ro` / `readonly` | `false` | yes | bind ro |
| `from` | empty dir | yes | source image/stage layer rootfs |
| `source` | root of `from` | yes | subpath of `from` |
| `mode` | `0755` | yes | chmod on new dir creation |
| `uid` / `gid` | `0` / `0` | yes | chown on new dir creation |

Existing AST at `crates/zlayer-builder/src/dockerfile/instruction.rs:397` already covers `target`, `id`, `sharing`, `readonly`. **Missing fields**: `from`, `source`, `mode`, `uid`, `gid` — Step 11 extends the enum and the parser at `crates/zlayer-builder/src/dockerfile/parser.rs`. The buildah default of `sharing=locked` at line 472 of `instruction.rs` is wrong per upstream docs (Docker default is `shared`); fix it in Step 11 too.

### Cache-to-registry format

Per https://docs.docker.com/build/cache/backends/registry/:

- Push target: `--cache-to=type=registry,ref=<host>/<repo>:<tag>[,mode=min|max][,image-manifest=true]`.
- Media types: OCI (`application/vnd.oci.image.manifest.v1+json` for manifest, `application/vnd.oci.image.config.v1+json` for config, `application/vnd.oci.image.layer.v1.tar+gzip` or `+zstd` for layers).
- Default = image index; with `image-manifest=true`, single manifest (required for ECR).
- `mode=min` (default): only final-stage cached layers shipped. `mode=max`: every step.
- Config payload (BuildKit format, replicated by us): JSON with a `cacheKeys` array mapping `step_cache_key -> layer_descriptor_index`. We mirror this format so `docker buildx build --cache-from` against a ZLayer-pushed cache works (and vice-versa for mode=min — mode=max compatibility is best-effort, not promised).
- 404 on `--cache-from`: log warning, continue build.

### Files to create

- `crates/zlayer-builder/src/cache/mod.rs`
- `crates/zlayer-builder/src/cache/key.rs`
- `crates/zlayer-builder/src/cache/canonical.rs`
- `crates/zlayer-builder/src/cache/contenthash.rs`
- `crates/zlayer-builder/src/cache/store.rs`
- `crates/zlayer-builder/src/cache/mount_volumes.rs`
- `crates/zlayer-builder/src/cache/registry_export.rs`
- `crates/zlayer-builder/src/cache/registry_import.rs`
- `crates/zlayer-builder/tests/cache_key_stability.rs`
- `crates/zlayer-builder/tests/cache_mount_sharing.rs`
- `crates/zlayer-builder/tests/cache_registry_roundtrip.rs`

### Files to modify

- `crates/zlayer-builder/src/lib.rs` — add `pub mod cache`.
- `crates/zlayer-builder/src/builder.rs` — wire cache lookups into the per-instruction loop (`BuildOptions` at line 288; `build_args` at line 831).
- `crates/zlayer-builder/src/dockerfile/instruction.rs` — extend `RunMount::Cache` to carry `from`, `source`, `mode`, `uid`, `gid`; fix `sharing` default from `Locked` (line 528) to `Shared`.
- `crates/zlayer-builder/src/dockerfile/parser.rs` — parse the new mount fields.
- `crates/zlayer-paths/src/lib.rs` — add `build_cache()` helper next to `cache()` (line 290).
- `bin/zlayer/src/commands/build.rs` — add `--cache-from`, `--cache-to` flags; new `cache prune` subcommand.
- `bin/zlayer/src/cli.rs` — register the new subcommand.
- `CHANGELOG.md` — under `[Unreleased]`.

### Reuse from existing tree

- **`PersistentBlobCache`** (`crates/zlayer-registry/src/persistent_cache.rs:28`) — sled-backed CAS with LRU eviction at line 300. Use as the storage primitive; do NOT introduce `cacache` (would duplicate functionality already in-tree).
- **`BlobCacheBackend`** trait (`crates/zlayer-registry/src/cache.rs:33`) — abstract over local vs S3 cache backends; `s3_cache.rs` already exists for remote.
- **`compute_digest`** (`crates/zlayer-registry/src/cache.rs:310`) — SHA256 helper.
- **OCI types** (`crates/zlayer-registry/src/oci_export.rs:58-181`) — `OciLayout`, `OciIndex`, `OciDescriptor`, `OciManifest`. Reuse for the cache manifest format.
- **`LayerUnpacker`** (`crates/zlayer-registry/src/unpack.rs:157`) — needed when materializing a cached layer onto rootfs during `--cache-from` hits.
- **`ZLayerDirs`** (`crates/zlayer-paths/src/lib.rs:6`) — add `build_cache()` here per the "utility crates own derived state" rule.
- **Existing `RunMount` AST** (`crates/zlayer-builder/src/dockerfile/instruction.rs:386`) — extend, don't replace.

### Tests

- `cache_key_stability.rs`: same Dockerfile + same context => identical key across machines; reordering ARG flags but same effective env => same key; whitespace inside heredoc preserved => different key.
- `contenthash.rs` unit tests: same tree, different `mtime`/`uid` => same digest; added empty file => different; symlink target change => different.
- `cache_mount_sharing.rs`: `shared` allows concurrent writers; `locked` serializes; `private` snapshots.
- `cache_registry_roundtrip.rs`: export to local OCI registry (use `zlayer_registry::local_registry::LocalRegistry`), import back, hit rate = 100% on identical Dockerfile.
- `cache_hit_skips_exec.rs`: instrument the executor to count exec calls; second build of unchanged Dockerfile => zero RUN execs.
- Negative: ENV change between builds => RUN re-execs even though command text unchanged.
- Negative: 404 on `--cache-from` logs warning and continues.

### Risks (the 80/20 trap)

1. **Cache-key correctness** is the entire ballgame. False hits ship wrong layers; false misses ruin perf. Per scoping ("Moby spent 5 years getting cache-key semantics right"), expect post-MVP bugs around heredocs, ARG-scoping (build ARG vs FROM-scope ARG), and `${VAR:-default}` substitution. Mitigation: a corpus of ~30 reference Dockerfiles snapshotted into `tests/fixtures/` whose cache keys are pinned and reviewed.
2. **Contenthash compatibility with BuildKit** — we follow the documented merkle scheme but don't promise byte-identical local-cache interop. Registry-cache (mode=min) is the interop contract and is tested by roundtrip with `docker buildx`.
3. **Cache-mount data corruption under `sharing=shared`** — concurrent writers to the same host dir. Mitigation: documented as best-effort per upstream docs ("another build may overwrite the files or GC may clean it"). Workloads needing serialization must use `sharing=locked`.
4. **CAS unbounded growth** — `PersistentBlobCache::evict_if_needed` already handles LRU. Default 20 GiB cap; expose via `zlayer build cache prune`.
5. **Mode=max registry-cache compatibility with buildx** — not promised; mode=min is the interop guarantee.
6. **Lock contention on `sharing=locked`** — `fs2::FileExt::lock_exclusive` blocks indefinitely. Add a configurable timeout (default 10 min) to avoid stuck builds.
7. **Scoping overlap with Phase D**: parallel DAG solving will re-order step execution; the cache key spec here is per-step and order-agnostic (parent layer digest threads through), so D can layer on top without changing the format. Document this contract in `cache/key.rs` so Phase D doesn't violate it.

### Verification

- `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo build --workspace`.
- Manual: `zlayer build examples/zimage-rust -t demo:1 && zlayer build examples/zimage-rust -t demo:2` — second build should report 100% cache hits and finish in <2s.
- Manual: edit one RUN line, rebuild — only that step and downstream steps should re-execute.
- Manual: `zlayer build ... --cache-to=type=registry,ref=localhost:5000/cache:latest` then on a fresh `${ZLAYER_DATA_DIR}` `zlayer build ... --cache-from=type=registry,ref=localhost:5000/cache:latest` — full cache hit.
- Interop: `docker buildx build --cache-from=type=registry,ref=...` against the ZLayer-pushed cache (mode=min) — should hit.

### Estimated effort

~45h of focused agent work across the 15 steps (sum of per-step estimates). At one agent per step plus verification + integration debugging, **3 person-weeks** is realistic; the original MED-HIGH 3-4-week estimate holds. The contenthash + cache-key correctness work (Steps 5, 6, 7) is the highest-risk concentration — budget extra debugging time there.
