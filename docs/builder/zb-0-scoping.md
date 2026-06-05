## Phase 0 — Foundation: types, traits, scaffolding

### Goals

Land the **type-system and trait scaffolding** that the native OCI executor (Phases A–G) will plug into, without yet writing any executor logic. Three concrete deliverables:

1. Widen the existing `BuildBackend` trait (`crates/zlayer-builder/src/backend/mod.rs:87-124`) so a native executor can express layer-commit metadata, content-addressable cache lookups, and manifest-list assembly without reaching back into `BuildahBackend` privates.
2. Move every cross-crate wire type (`BuilderBackendKind`, `LayerCommitMetadata`, `CacheKey`, `BuildStepResult`, plus widened `BuildRequest` / `BuildStatus`) into `crates/zlayer-types/` per the memory rule "Types-and-API FIRST in sequence".
3. Pick a sub-module vs new-crate layout for the native executor, register the new `ZLAYER_BACKEND` values, and keep buildah selectable as the default fallback for the entire A–F runway.

This phase ships **no executor code**. Phase A is the first phase that actually builds a layer natively.

### Steps

**0.1 — Audit & document the current `BuildBackend` trait gaps.** (1–2 h, low risk.)
Read `crates/zlayer-builder/src/backend/mod.rs:87-124`. Document in code comments which methods are buildah-CLI-shaped vs OCI-spec-shaped. Concretely: `manifest_create / manifest_add / manifest_push` (lines 111–117) presume a buildah-style local manifest store, and there is no method for "commit a layer tarball with this digest" or "look up a cached layer by `CacheKey`". No edits in this step — produce a written gap list checked into `docs/builder/phase-0-trait-gaps.md`.

**0.2 — Add `BuilderBackendKind` enum to `zlayer-types`.** (1 h, low risk.)
New file: `crates/zlayer-types/src/builder.rs`. Public enum:
```rust
pub enum BuilderBackendKind { Buildah, NativeYouki, NativeHcs, NativeSandbox, LinuxInVm }
```
Plus `FromStr` / `Display` / serde (`rename_all = "kebab-case"`). Register the module in `crates/zlayer-types/src/lib.rs` (after line 244 where the other `pub mod` lines live). This is **not** the same as the existing `ImageOs` enum (`backend/mod.rs:51-59`); `ImageOs` is the *target image* OS, `BuilderBackendKind` is the *executor*. Both live side-by-side.

**0.3 — Add `LayerCommitMetadata`, `CacheKey`, `BuildStepResult` to `zlayer-types`.** (2 h, medium risk — schema choices ripple into A–C.)
Same file `crates/zlayer-types/src/builder.rs`. Each type derives `Debug, Clone, Serialize, Deserialize, utoipa::ToSchema` to match the existing API DTO convention (`crates/zlayer-types/src/api/build.rs:11,34,54`).
- `LayerCommitMetadata { digest: String /* sha256:... */, diff_id: String, size: u64, media_type: String, created_by: String, created_at: chrono::DateTime<Utc> }`
- `CacheKey { instruction_hash: String, base_layer_diff_id: Option<String>, context_hashes: Vec<(PathBuf, String)>, mount_keys: Vec<String> }` with a `pub fn fingerprint(&self) -> String` returning a stable `sha256:` digest.
- `BuildStepResult { instruction_index: usize, cache_status: CacheStatus, layer: Option<LayerCommitMetadata>, duration_ms: u64 }` + `enum CacheStatus { Hit, Miss, Bypassed }`.

Risk: if the shape is wrong, Phase C cache work has to migrate it. Mitigation: keep fields additive (`#[serde(default)]` on every optional).

**0.4 — Widen `BuildBackend` trait with default-impl methods.** (2 h, medium risk.)
File: `crates/zlayer-builder/src/backend/mod.rs`. Add three methods with provided default impls so `BuildahBackend` is unaffected:
```rust
fn kind(&self) -> zlayer_types::builder::BuilderBackendKind;
async fn commit_layer(&self, _ctx: &Path, _meta: &LayerCommitMetadata) -> Result<()> {
    Err(BuildError::Unsupported("commit_layer not implemented for this backend".into()))
}
async fn lookup_cached_layer(&self, _key: &CacheKey) -> Result<Option<LayerCommitMetadata>> { Ok(None) }
```
The existing `manifest_*` methods stay as-is for Phase 0; Phase D widens them to take a parsed manifest-list IR instead of buildah-style string names. Add a `BuildError::Unsupported(String)` variant in `crates/zlayer-builder/src/error.rs` if absent.

**0.5 — Extend `detect_backend` for native variants.** (1 h, low risk.)
File: `crates/zlayer-builder/src/backend/mod.rs:143-216`. Add new `ZLAYER_BACKEND` values: `"native-youki"`, `"native-hcs"`, `"native-sandbox"`, `"linux-in-vm"`. Each currently returns `BuildError::Unsupported("native <X> backend is not yet implemented — Phase A/E/F")`. The host×target matrix at lines 130–134 stays Buildah-default; switching the default to `native-youki` happens in Phase A's last step, not here. Update the `ZLAYER_BACKEND` doc table at lines 130–134 accordingly.

**0.6 — Add `backend` field to `BuildRequest` DTOs.** (1 h, low risk.)
File: `crates/zlayer-types/src/api/build.rs`. Add to both `BuildRequest` (line 12) and `BuildRequestWithContext` (line 99):
```rust
#[serde(default)]
pub backend: Option<BuilderBackendKind>,
```
Plus an optional `pub progress_protocol: Option<String>` field reserved for Phase A's structured progress events (rendezvous keyword: `"buildkit-v1"` vs `"buildah-legacy"`). Update the `From<BuildRequestWithContext>` impl at line 122. `BuildStatus` (line 35) gains an optional `pub backend_used: Option<BuilderBackendKind>` field so clients can see which executor actually ran.

**0.7 — Wire the new field through `BuildOptions` and CLI.** (1–2 h, low risk.)
File: `crates/zlayer-builder/src/builder.rs:288-432`. Add `pub backend_override: Option<zlayer_types::builder::BuilderBackendKind>` to `BuildOptions`, default `None`. In `bin/zlayer/src/commands/build.rs` (the existing `backend` selection lives near line 110, see grep hit "backend is detected for the correct OS up front"), respect the override before calling `detect_backend`. CLI surface: add `--backend <kind>` flag to `zlayer build` (`bin/zlayer/src/cli.rs`).

**0.8 — Pick scaffolding location and lay down empty modules.** (1 h, low risk.)
**Decision: sub-modules under `crates/zlayer-builder/src/backend/native/`, not a new crate.** Rationale:
- `BuildBackend`, `Dockerfile` IR (`crates/zlayer-builder/src/dockerfile/`), `RegistryAuth`, `BuiltImage` (`builder.rs:233,250`) all live in `zlayer-builder`. A separate crate would either pull them all out or force a circular dep.
- Phases A–F each touch a single `native::{linux,macos,windows,linux_in_vm}` sub-module; a single crate keeps the build graph flat.
- The existing `backend/hcs/` precedent (already a sub-module) sets the pattern.
Create empty `mod.rs` stubs at `crates/zlayer-builder/src/backend/native/{linux,macos,windows,linux_in_vm}/mod.rs`, each with a single `pub struct PlaceholderBackend;` + `#[allow(dead_code)]` so the crate still compiles. Register `pub mod native;` in `backend/mod.rs`.

**0.9 — Workspace dep audit.** (30 min, low risk.)
`Cargo.toml:46+`: confirmed already vendored at workspace level — `oci-spec = "0.8"` (line 143), `oci-client = "0.15"` (line 145), `sha2 = "0.10"` (line 157), `tar = "0.4"` (line 185), `flate2 = "1"` (line 186), `nix` (line 193). `libcontainer` is per-crate in `zlayer-agent` only (`crates/zlayer-agent/Cargo.toml:133`, the `zlayer-libcontainer 0.6.1-zlayer.5` fork behind the `youki-runtime` feature). `zstd` is transitively in the lockfile but **not** declared as a workspace dep — it must be added at implementation time via `cargo add --workspace zstd` when Phase A needs it; do **not** hand-edit `Cargo.toml`. `cacache` and any content-addressable-storage crate selection is deferred to Phase C; the existing `zlayer-registry::cache::BlobCacheBackend` trait (`crates/zlayer-registry/src/cache.rs:31`) and `PersistentBlobCache` (`persistent_cache.rs:361`) already implement digest-keyed put/get and should be reused before reaching for a new crate.

**0.10 — Migration / fallback strategy doc.** (1 h, low risk.)
Document in `docs/builder/native-executor-migration.md`:
- **Selection precedence**: explicit `--backend` flag → `ZLAYER_BACKEND` env var → daemon config `[builder] default_backend = …` → built-in default per host×target.
- **Built-in default** stays `Buildah` for all of Phases 0–F; the flip happens at the top of Phase G (or whenever the user explicitly accepts the cutover) — never silently in 0.
- **Fallback**: if the selected native backend returns `BuildError::Unsupported`, the dispatcher logs a `tracing::warn!` and re-runs against buildah only if `ZLAYER_BACKEND_FALLBACK=buildah` is set. No silent fallback — failed native builds must be visible.
- **Existing installs**: untouched. `ZLAYER_BACKEND` unset = buildah, exactly the current behaviour.

### Files to create

- `crates/zlayer-types/src/builder.rs` — `BuilderBackendKind`, `LayerCommitMetadata`, `CacheKey`, `BuildStepResult`, `CacheStatus`.
- `crates/zlayer-builder/src/backend/native/mod.rs` plus `linux/`, `macos/`, `windows/`, `linux_in_vm/` sub-stubs.
- `docs/builder/phase-0-trait-gaps.md` (step 0.1).
- `docs/builder/native-executor-migration.md` (step 0.10).

### Files to modify

- `crates/zlayer-types/src/lib.rs` — register `pub mod builder;` after the existing module list at line 244.
- `crates/zlayer-types/src/api/build.rs` — add `backend` / `progress_protocol` / `backend_used` fields.
- `crates/zlayer-builder/src/backend/mod.rs` — widen trait (0.4), extend `detect_backend` (0.5), register `native` module.
- `crates/zlayer-builder/src/builder.rs` — `BuildOptions.backend_override` field + `Default` impl update.
- `crates/zlayer-builder/src/error.rs` — `BuildError::Unsupported(String)`.
- `bin/zlayer/src/cli.rs` — `--backend` flag on the `build` subcommand.
- `bin/zlayer/src/commands/build.rs` — pass override into `detect_backend`.

### New crates / deps

None at workspace level in Phase 0. All required crates (`oci-spec`, `oci-client`, `sha2`, `tar`, `flate2`, `nix`) are already vendored. `zstd`, `cacache`, and any executor-specific deps are added by Phase A/C via `cargo add --workspace <crate>` when they're actually needed — never hand-edited.

### Tests

- Unit test in `crates/zlayer-types/src/builder.rs`: `BuilderBackendKind` serde round-trip (kebab-case), `CacheKey::fingerprint()` stability (same inputs → same digest; field reordering does not change result).
- Unit test in `crates/zlayer-builder/src/backend/mod.rs`: each new `ZLAYER_BACKEND` value (`native-youki`, `native-hcs`, `native-sandbox`, `linux-in-vm`) routes to `BuildError::Unsupported` with a message naming the phase.
- Unit test in `crates/zlayer-builder/src/builder.rs`: `BuildOptions::default().backend_override.is_none()`.
- Integration smoke: `zlayer build . -t test:0 --backend buildah` still works exactly as today (no behavior change for the default path).

### Risks

- **Schema-shape risk for `CacheKey`** (0.3): if Phase C decides cache keys need additional dimensions (e.g. platform, env-allowlist hash), every persisted cache entry from earlier phases is invalidated. Mitigation: include `pub schema_version: u32` field (default 1) and check it at lookup time.
- **`BuildBackend` trait stability**: any new method without a default impl breaks every external implementor. Every method added in 0.4 has a default impl.
- **Visibility of `BuildError`**: it lives in `zlayer-builder`, not `zlayer-types`. The new `Unsupported` variant cannot be returned across the API boundary as-is. For Phase 0 the API surface only sees `BuildStatus { status: Failed, error: Some(string) }`, so this is fine — Phase A will revisit.

### Verification

Run, from the repo root, in this order (per `CLAUDE.md` "ALL CHECKS GREEN" rule):
```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```
All four must ring green across the entire workspace. Manual smoke: `cargo run -p zlayer -- build examples/<simple-dockerfile> -t smoke:0` succeeds with no `--backend` flag (default buildah path unchanged); same command with `--backend native-youki` returns the explicit "Phase A" `Unsupported` error.

### Dependencies on other phases

- **Phase A** consumes 0.2 / 0.3 / 0.4 / 0.5 / 0.8 directly; it lights up `native::linux::YoukiBackend` and flips `--backend native-youki` from `Unsupported` to a real builder.
- **Phase C** consumes 0.3 (`CacheKey`, `LayerCommitMetadata`, `CacheStatus`) and the widened `BuildBackend::lookup_cached_layer` / `commit_layer` methods from 0.4.
- **Phase D** widens 0.4's `manifest_*` methods (deliberately deferred — buildah-shaped today, IR-shaped after D).
- **Phase E** / **Phase F** consume 0.5's `linux-in-vm` and `native-hcs` `ZLAYER_BACKEND` slots and the empty sub-modules from 0.8.
- **Phase G** consumes nothing new from 0 directly but reads the migration doc (0.10) to time the default-backend cutover.

Total effort estimate, Phase 0 only: **10–13 hours of agent-driven implementation** across 10 small sequential agents (one per step). Estimates are guesses based on file sizes and the no-executor-logic constraint; honest unknown on 0.3 schema iteration.
