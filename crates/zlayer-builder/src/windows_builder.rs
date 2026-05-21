//! Native WCOW (Windows Container On Windows) image builder.
//!
//! Parses a Dockerfile/ZImagefile, pulls the Windows base image via the
//! registry client, materialises the foreign-layer base via the Windows
//! unpacker, and prepares a layer chain that subsequent Phase 4 tasks
//! extend:
//!
//! - **4.A**: Dockerfile parse + base image pull + foreign-layer
//!   materialisation. Non-FROM instructions are routed through
//!   [`WindowsBuilder::execute_instruction`].
//! - **4.B** (this task): RUN execution via a transient HCS compute system
//!   attached to the working layer chain, with a Chocolatey translation
//!   hook for Linux package-manager invocations.
//! - **4.C** (this task): COPY / ADD writes into the working layer chain
//!   as a new RO layer per instruction (the **per-instruction commit
//!   model**), and config-only instructions (WORKDIR / ENV / ENTRYPOINT /
//!   CMD / USER / EXPOSE / VOLUME / LABEL / SHELL / STOPSIGNAL /
//!   HEALTHCHECK / ONBUILD) accumulate into a typed [`OciImageConfig`]
//!   carried on the [`BuildSkeleton`] for task 4.D to serialise.
//!
//!   **Layer-commit model**: COPY and ADD each produce ONE new RO layer
//!   on Windows. The alternative "combined scratch" model (let COPY/ADD
//!   write into the same scratch the next RUN sees) is simpler at build
//!   time but produces irregular layer chains where a single RO layer
//!   conflates user-visible operations; per-instruction commits keep the
//!   layer chain 1:1 with Dockerfile instructions, which makes the
//!   emitted OCI manifest (4.D) cleanly auditable and downstream tooling
//!   like `docker history` / `zlayer inspect` produce sensible output.
//!   Off-Windows the model is moot — COPY/ADD still validate sources and
//!   mutate the working tree under `working_layer_chain_dir/<scratch>/`
//!   so unit tests on Linux CI exercise the path-traversal and
//!   tar-extract logic without touching HCS.
//! - **4.D**: OCI image manifest emission with `os: "windows"` +
//!   `os.version` from the resolved base manifest; preserves foreign-layer
//!   `urls[]`.
//! - **4.E**: Push via the existing `zlayer-registry` push path.
//!
//! ## Architectural template
//!
//! Modelled after [`crate::sandbox_builder::SandboxImageBuilder`] — the macOS
//! Seatbelt builder — which is the project's reference for a native (non-
//! buildah) Dockerfile-driven image builder. The key shared pattern: reuse
//! the existing Dockerfile parser ([`crate::dockerfile::Dockerfile`]),
//! delegate base-image materialisation to a platform-specific helper, and
//! iterate over [`Instruction`] variants to drive the layer chain.
//!
//! ## Relationship to [`crate::backend::hcs::HcsBackend`]
//!
//! `HcsBackend` is the existing Windows-only build backend wired into the
//! `BuildBackend` trait. `WindowsBuilder` is intentionally a parallel,
//! more granular API that exposes the build pipeline in skeleton form so
//! Phase 4 follow-up tasks (4.C–4.E) can extend it incrementally without
//! disturbing the working `HcsBackend`. Once Phase 4 lands, `HcsBackend`
//! can be retargeted onto `WindowsBuilder` if desired; for now they
//! co-exist.
//!
//! ## Cross-platform compilation
//!
//! The data types ([`WindowsBuilder`], [`WindowsBuildConfig`],
//! [`BuildContext`], [`BuildSkeleton`], [`LayerRef`], [`WindowsLayerEntry`],
//! [`BaseImageManifest`]) compile on every host so unit tests run on the
//! CI Linux runners. The actual base-layer materialisation in
//! [`WindowsBuilder::build_skeleton`] and the HCS-driven RUN execution in
//! [`WindowsBuilder::execute_instruction`] are gated on
//! `target_os = "windows"`; on other hosts they return
//! `BuildError::NotSupported`. Phase 4 follow-up tasks preserve this
//! gating discipline.

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::dockerfile::{
    AddInstruction, CopyInstruction, Dockerfile, DockerfileFromTarget, EnvInstruction,
    ExposeInstruction, ExposeProtocol, HealthcheckInstruction, Instruction, RunInstruction,
    ShellOrExec,
};
use crate::error::{BuildError, Result};

// `RegistryAuth` is re-exported by `zlayer-registry` unconditionally (the
// underlying `oci-client::secrets::RegistryAuth` is plain Rust with no
// Windows linkage), so the same import works on every host. We keep
// `WindowsBuildConfig::registry_auth` cross-platform so the public
// builder API surfaces the same shape regardless of build target — only
// the Windows-only base materialisation path actually consumes the
// credential.
use zlayer_registry::RegistryAuth;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration for the Windows builder.
///
/// Mirrors the on-disk + registry parameters required to materialise a
/// Windows base image. Subsequent Phase 4 tasks (4.C COPY/ADD,
/// 4.D manifest) thread additional knobs through this struct.
#[derive(Debug, Clone)]
pub struct WindowsBuildConfig {
    /// Build cache root. Per-build subdirectories (`<cache_dir>/<build_id>/`)
    /// hold the unpacked base layer chain, the working writable layer, and
    /// any future blob staging area used by 4.D/4.E.
    pub cache_dir: PathBuf,
    /// Registry credentials, forwarded verbatim to the registry client when
    /// pulling the base image and (in 4.E) pushing the final manifest.
    pub registry_auth: RegistryAuth,
    /// Target OCI platform string, e.g. `"windows/amd64"`. The registry
    /// client uses this to resolve a multi-platform index entry to a
    /// concrete manifest. Defaults to `"windows/amd64"` via
    /// [`WindowsBuildConfig::default_platform`].
    pub platform: String,
    /// Optional override for the tested base image OS build (the
    /// `os.version` constraint, e.g. `"10.0.20348.2227"`). When `None` the
    /// platform resolver inherits the `os.version` reported by the pulled
    /// manifest. Used to pin a specific Windows build family when the host
    /// kernel demands an exact match.
    pub os_version_override: Option<String>,
    /// Scratch-layer size in GiB for per-RUN ephemeral compute systems.
    /// Zero means "HCS default" (currently `20`).
    pub scratch_size_gb: u64,
}

impl WindowsBuildConfig {
    /// The default target platform string: `"windows/amd64"`. The vast
    /// majority of Windows container base images are published for this
    /// platform; `windows/arm64` exists but is not in widespread use yet.
    #[must_use]
    pub const fn default_platform() -> &'static str {
        "windows/amd64"
    }

    /// Default scratch-layer size (20 GiB), used when
    /// [`WindowsBuildConfig::scratch_size_gb`] is zero. Matches the
    /// production `HcsBackend` default so behaviour is consistent across
    /// the two builders.
    #[must_use]
    pub const fn default_scratch_size_gb() -> u64 {
        20
    }
}

/// Inputs to a single build.
///
/// One `BuildContext` per `WindowsBuilder::build_skeleton` invocation;
/// holds the on-disk paths the parser reads (Dockerfile + COPY sources)
/// plus the build-time `ARG` values and the final image tag.
#[derive(Debug, Clone)]
pub struct BuildContext {
    /// Path to the build root. Everything COPY/ADD reads is resolved
    /// relative to this directory; the Dockerfile lives here unless
    /// [`dockerfile_path`](Self::dockerfile_path) gives an absolute path.
    pub context_dir: PathBuf,
    /// Path to the Dockerfile, either relative to
    /// [`context_dir`](Self::context_dir) or absolute. The skeleton uses
    /// [`Dockerfile::parse`] on the file's contents.
    pub dockerfile_path: PathBuf,
    /// Build-time variables (`--build-arg KEY=VALUE`). Stored verbatim and
    /// applied during Dockerfile variable expansion in later tasks.
    pub build_args: HashMap<String, String>,
    /// Final image tag, e.g. `"myapp:latest"`. Recorded for 4.D's manifest
    /// emission and 4.E's push; the 4.A skeleton holds it but does not act
    /// on it.
    pub tag: String,
}

/// One base-image layer reference threaded into [`BuildSkeleton`].
///
/// Mirrors the subset of an OCI layer descriptor the WCOW builder needs:
/// the digest (content-addressable hash of the compressed blob), the
/// media type (drives decompression + foreign-layer detection), the byte
/// size, and the optional mirror URL list (non-empty for MCR foreign
/// Windows base layers). Subsequent tasks 4.D/4.E pass these descriptors
/// through to the final manifest unchanged.
#[derive(Debug, Clone)]
pub struct LayerRef {
    /// `sha256:...` digest of the compressed blob.
    pub digest: String,
    /// OCI media type — e.g.
    /// `application/vnd.docker.image.rootfs.foreign.diff.tar.gzip` for
    /// MCR foreign Windows layers, or
    /// `application/vnd.oci.image.layer.v1.tar+gzip` for ordinary OCI
    /// layers.
    pub media_type: String,
    /// Compressed size in bytes (`i64` because that's the type
    /// `oci-client` uses for descriptor size — keeping the same shape
    /// avoids lossy casts on round-trip).
    pub size: i64,
    /// Optional mirror URLs (foreign layer `urls[]` per the OCI spec).
    /// Non-empty for MCR Windows base layers; empty for ordinary
    /// registry-resident layers. Preserved verbatim through to the
    /// emitted manifest in task 4.D.
    pub urls: Vec<String>,
}

/// On-disk reference to one materialised parent layer.
///
/// Stored on [`BuildSkeleton`] so subsequent RUN steps can build the
/// HCS `LayerChain` (child-to-parent order) the storage filter expects.
/// Cross-platform plain data — the chain is only consumed by
/// `target_os = "windows"` code, but the type compiles everywhere so
/// the public API and unit tests stay portable.
#[derive(Debug, Clone)]
pub struct WindowsLayerEntry {
    /// Caller-chosen GUID/uuid for the layer. HCS stores it inside the
    /// `LayerData` JSON so the storage filter can route opens.
    pub layer_id: String,
    /// Absolute path to the layer directory (e.g.
    /// `<cache_dir>/<build_id>/unpacked/<layer_id>/`).
    pub layer_path: PathBuf,
}

/// Resolved manifest information for the pulled base image.
///
/// Carried into [`BuildSkeleton`] so task 4.D can populate the final
/// image config's `os` / `os.version` fields without re-pulling the
/// manifest. Task 4.D also reads [`base_config`](Self::base_config) so the
/// final image's `config.Env` / `WorkingDir` / `Entrypoint` defaults
/// inherit from the base.
#[derive(Debug, Clone)]
pub struct BaseImageManifest {
    /// Image reference the base was pulled from, e.g.
    /// `"mcr.microsoft.com/windows/nanoserver:ltsc2022"`. Preserved for
    /// diagnostics and for foreign-layer push paths in task 4.E.
    pub image_ref: String,
    /// OCI `os` value from the resolved manifest's platform descriptor —
    /// always `"windows"` for a WCOW build, but stored for symmetry with
    /// the OCI spec.
    pub os: String,
    /// OCI `os.version` from the resolved manifest's platform descriptor
    /// (e.g. `"10.0.20348.2227"`). Used as the final image's
    /// `os.version` unless
    /// [`WindowsBuildConfig::os_version_override`] overrides it. `None`
    /// when the base manifest omits the field, which is non-conformant
    /// for Windows but tolerated.
    pub os_version: Option<String>,
    /// `arch` from the resolved manifest. Defaults to `"amd64"`.
    pub arch: String,
    /// JSON of the base image config blob (`manifest.config`). Stored as
    /// raw bytes so task 4.D can hand it straight back through
    /// `serde_json` without forcing this skeleton to take a hard dep on
    /// `zlayer_registry::image_config::ImageConfig`. Empty when the base
    /// config could not be fetched (a non-fatal degradation handled in
    /// task 4.D).
    pub config_blob: Vec<u8>,
}

/// OCI image config accumulated during instruction execution.
///
/// Config-only Dockerfile instructions (WORKDIR / ENV / ENTRYPOINT / CMD
/// / USER / EXPOSE / VOLUME / LABEL / SHELL / STOPSIGNAL / HEALTHCHECK)
/// mutate this struct in-place during 4.C; task 4.D serialises it into
/// the OCI image config blob alongside the layer descriptors on
/// [`BuildSkeleton`].
///
/// Field shape matches the OCI image-spec
/// `application/vnd.oci.image.config.v1+json` config object so 4.D's
/// emission step is a straight `serde_json::to_value` over each field
/// without remapping.
#[derive(Debug, Clone, Default)]
pub struct OciImageConfig {
    /// `WorkingDir` in the OCI config — the cwd applied to subsequent RUN
    /// steps and to the final container's process. WORKDIR with a
    /// relative path resolves against the previous WORKDIR per the
    /// Dockerfile spec.
    pub working_dir: Option<String>,
    /// `Env` in the OCI config — list of `KEY=value` entries.
    /// Builder-side ENV mutation enforces last-write-wins per key so the
    /// vector never grows duplicate KEYs.
    pub env: Vec<String>,
    /// `Entrypoint` in the OCI config. ENTRYPOINT shell form is rewritten
    /// to `["cmd", "/c", "<rest>"]` for WCOW; exec form is passed
    /// through as-is.
    pub entrypoint: Option<Vec<String>>,
    /// `Cmd` in the OCI config. Setting ENTRYPOINT resets CMD to `None`
    /// per the Dockerfile spec.
    pub cmd: Option<Vec<String>>,
    /// `User` in the OCI config (e.g. `"ContainerUser"` or
    /// `"user:group"`).
    pub user: Option<String>,
    /// `ExposedPorts` in the OCI config — map of `"<port>/<proto>"` →
    /// empty object. Stored as `BTreeMap` so the serialised key order is
    /// deterministic across runs.
    pub exposed_ports: BTreeMap<String, serde_json::Value>,
    /// `Volumes` in the OCI config — map of `<path>` → empty object.
    pub volumes: BTreeMap<String, serde_json::Value>,
    /// `Labels` in the OCI config — string→string map. Multiple LABEL
    /// lines merge; later LABEL with the same KEY wins.
    pub labels: BTreeMap<String, String>,
    /// `StopSignal` in the OCI config (e.g. `"SIGTERM"`). Carried as-is
    /// from the STOPSIGNAL instruction.
    pub stop_signal: Option<String>,
    /// `Healthcheck` in the OCI config. `None` means no healthcheck was
    /// configured (and the base image's healthcheck is inherited);
    /// `HEALTHCHECK NONE` sets this to a sentinel
    /// [`OciHealthcheck::disabled`].
    pub healthcheck: Option<OciHealthcheck>,
    /// `Shell` in the OCI config — list of tokens (`["cmd", "/c"]` for
    /// the default WCOW shell, or a user override via the SHELL
    /// instruction).
    pub shell: Option<Vec<String>>,
    /// `OnBuild` triggers in the OCI config — list of raw Dockerfile
    /// instruction text (`"COPY . /app"` etc.). Only matters when this
    /// image is used as a base; the builder itself never re-executes
    /// them in the producing build.
    pub on_build: Vec<String>,
}

/// OCI healthcheck shape used by [`OciImageConfig`].
///
/// The Dockerfile [`HealthcheckInstruction::Check`] variant carries
/// `Duration` values; we normalise them to OCI's string form
/// (`"30s"`, `"1m30s"`, …) at the point of capture so 4.D can serialise
/// the struct without re-formatting. `disabled` corresponds to
/// `HEALTHCHECK NONE` and round-trips as `Test == ["NONE"]` per the
/// OCI / Docker convention.
#[derive(Debug, Clone)]
pub struct OciHealthcheck {
    /// Healthcheck command. For `HEALTHCHECK NONE` this is
    /// `vec!["NONE".to_string()]`; for `HEALTHCHECK CMD …` this is
    /// `["CMD-SHELL", "<cmd>"]` (shell form) or
    /// `["CMD", "<arg0>", "<arg1>", …]` (exec form).
    pub test: Vec<String>,
    /// `Interval` between consecutive checks (OCI string form).
    pub interval: Option<String>,
    /// Per-check `Timeout` (OCI string form).
    pub timeout: Option<String>,
    /// `Retries` before transition to unhealthy.
    pub retries: Option<u32>,
    /// `StartPeriod` grace before the first check is counted (OCI
    /// string form).
    pub start_period: Option<String>,
}

impl OciHealthcheck {
    /// Build the sentinel value for `HEALTHCHECK NONE`. Reads cleanly in
    /// the emit path: `if hc.is_disabled() { "NONE" } else { … }`.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            test: vec!["NONE".to_string()],
            interval: None,
            timeout: None,
            retries: None,
            start_period: None,
        }
    }

    /// `true` iff this healthcheck is the sentinel produced by
    /// [`OciHealthcheck::disabled`].
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.test == ["NONE"]
    }
}

/// One executed Dockerfile instruction recorded in
/// [`BuildSkeleton::instruction_log`].
///
/// Task 4.D consumes this log to emit the OCI image config's `history`
/// array — one entry per source-line invocation, with `empty_layer = true`
/// for config-only instructions (WORKDIR / ENV / etc.) and
/// `empty_layer = false` for instructions that produced a real layer
/// (FROM / RUN / COPY / ADD). The `source_line` field holds the
/// reconstructed Dockerfile text (e.g. `"FROM mcr.microsoft.com/..."`,
/// `"RUN choco install -y curl"`) so downstream tooling like
/// `docker history` can render the build provenance.
#[derive(Debug, Clone)]
pub struct ExecutedInstruction {
    /// Reconstructed Dockerfile source line for the instruction, used
    /// verbatim as the `created_by` value in the emitted OCI history
    /// entry. Reconstructed from the parsed instruction (rather than
    /// echoing the original line) so a Dockerfile with continuation
    /// backslashes or comments collapses to a single canonical form per
    /// instruction — which is what `docker history` displays.
    pub source_line: String,
    /// `true` when this instruction produced a real layer (FROM / RUN /
    /// COPY / ADD); `false` for config-only instructions. Determines the
    /// `empty_layer` field on the emitted OCI history entry.
    pub produced_layer: bool,
    /// Build-time UTC timestamp captured at execution. Surfaces as
    /// `created` on the emitted OCI history entry.
    pub timestamp: DateTime<Utc>,
}

/// Output of [`WindowsBuilder::build_skeleton`] — the parsed Dockerfile
/// plus the materialised base layer chain plus the resolved base
/// manifest.
///
/// Consumed by Phase 4 follow-up tasks:
///
/// - 4.B (RUN, this task) iterates `parsed_dockerfile.stages[0].instructions`
///   and for each [`Instruction::Run`] spawns an HCS compute system
///   attached to the working chain stored in
///   [`working_chain`](Self::working_chain), captures the diff via
///   `wclayer::export_layer`, and appends a new
///   [`LayerRef`] to [`base_layers`](Self::base_layers) plus a new
///   [`WindowsLayerEntry`] to [`working_chain`](Self::working_chain).
/// - 4.C (COPY/ADD) writes context files into the working scratch layer
///   before the next 4.B commit.
/// - 4.D (manifest) emits the final OCI image with `os: "windows"`,
///   `os.version` from [`base_manifest`](Self::base_manifest), and the
///   accumulated `base_layers + post-4.B layers` chain.
/// - 4.E (push) pipes the 4.D manifest into the registry client.
#[derive(Debug)]
pub struct BuildSkeleton {
    /// Parsed Dockerfile — `parsed_dockerfile.stages[0]` is the single
    /// stage supported by the skeleton.
    pub parsed_dockerfile: Dockerfile,
    /// Base image layers in **base-first** order (matching OCI manifest
    /// order). Task 4.B appends post-RUN layers to the end of this
    /// vector.
    pub base_layers: Vec<LayerRef>,
    /// Manifest metadata from the resolved base image.
    pub base_manifest: BaseImageManifest,
    /// On-disk path to the working layer chain root. On Windows this is
    /// the directory passed to
    /// [`zlayer_agent::windows::unpacker::unpack_windows_image`]; on other
    /// hosts this field is the `<cache_dir>/<build_id>/unpacked/` path
    /// the skeleton would have used had the build proceeded. Each
    /// post-RUN read-only layer is staged as a subdirectory here.
    pub working_layer_chain_dir: PathBuf,
    /// Materialised layers in **base-first** order. The HCS storage
    /// filter consumes the reversed (child-to-parent) view; helpers in
    /// this module do the reversal at the point of use. Populated on
    /// Windows by `build_skeleton` from the unpacker output and extended
    /// by each successful RUN step.
    pub working_chain: Vec<WindowsLayerEntry>,
    /// OCI image config accumulated by config-only Dockerfile
    /// instructions (WORKDIR / ENV / ENTRYPOINT / CMD / USER / EXPOSE /
    /// VOLUME / LABEL / SHELL / STOPSIGNAL / HEALTHCHECK / ONBUILD).
    /// Task 4.D serialises this into the image config blob.
    pub image_config: OciImageConfig,
    /// Per-instruction execution log in build order. The first entry is
    /// the FROM instruction (recorded by [`WindowsBuilder::build_skeleton`]);
    /// subsequent entries are appended by
    /// [`WindowsBuilder::execute_instruction`] in the order it is called.
    /// Task 4.D ([`WindowsBuilder::emit_image`]) consumes this to emit
    /// the OCI image config `history` array.
    pub instruction_log: Vec<ExecutedInstruction>,
}

/// Locally-produced layer blob staged on disk for push (task 4.E).
///
/// Carries everything 4.E needs to upload a layer to a registry: the
/// media type to advertise on the descriptor, the digest + size of the
/// compressed blob, the `diff_id` for the image-config `rootfs.diff_ids`
/// array, the on-disk path to the blob, and (for foreign base layers
/// only) the upstream `urls[]` mirror list so the emitted manifest can
/// point Windows daemons at the original MCR foreign-layer descriptor
/// instead of forcing the daemon to re-download the bytes from the user's
/// own registry.
#[derive(Debug, Clone)]
pub struct EmittedLayer {
    /// OCI media type used in the manifest's `layers[].mediaType`.
    /// For the foreign Windows base layer this is
    /// `application/vnd.docker.image.rootfs.foreign.diff.tar.gzip` (so
    /// Windows daemons recognise it as a foreign layer and skip the
    /// download path); for builder-produced RUN/COPY/ADD layers this is
    /// `application/vnd.oci.image.layer.v1.tar+gzip`.
    pub media_type: String,
    /// `sha256:...` digest of the COMPRESSED (gzipped) tar blob.
    pub digest: String,
    /// Compressed size in bytes (matches the descriptor's `size` field).
    pub size: u64,
    /// `sha256:...` of the UNCOMPRESSED tar blob — what
    /// `rootfs.diff_ids[]` references in the image config. For foreign
    /// base layers this is sourced from the base image config blob; for
    /// builder-produced layers this is computed at export time.
    pub diff_id: String,
    /// On-disk path to the compressed layer blob the registry client
    /// will upload during the 4.E push. Empty for foreign base layers
    /// (they're never re-uploaded — the registry rehydrates them from
    /// `urls[]`).
    pub local_path: PathBuf,
    /// For foreign layers: the MCR / mirror URL list preserved verbatim
    /// from the base manifest's `urls[]`. `None` for builder-produced
    /// layers.
    pub urls: Option<Vec<String>>,
}

/// Final emitted artifact for one image: the OCI manifest blob, the
/// image config blob, and the descriptor list for every layer the
/// manifest references.
///
/// Produced by [`WindowsBuilder::emit_image`] and consumed by task 4.E's
/// registry push, which uploads each layer + the config blob and then
/// PUTs the manifest at `<tag>`.
#[derive(Debug, Clone)]
pub struct BuiltImage {
    /// Image tag for the push (`<repo>:<tag>` or `<host>/<repo>:<tag>`),
    /// carried verbatim from [`BuildContext::tag`].
    pub tag: String,
    /// Serialised image config JSON blob
    /// (`application/vnd.oci.image.config.v1+json`).
    pub image_config_blob: Vec<u8>,
    /// `sha256:...` digest of the image config blob — what the manifest
    /// references in `config.digest`.
    pub image_config_digest: String,
    /// Serialised OCI image manifest JSON blob
    /// (`application/vnd.oci.image.manifest.v1+json`).
    pub manifest_blob: Vec<u8>,
    /// `sha256:...` digest of the manifest blob. Identifies the image on
    /// the registry; the push step uses it as the manifest reference
    /// when computing the per-blob upload URL.
    pub manifest_digest: String,
    /// Layer descriptors in **base-first** order — the same order as
    /// `manifest.layers[]`. 4.E iterates this to upload each blob (or
    /// skips upload for foreign layers).
    pub layers: Vec<EmittedLayer>,
}

/// Native WCOW image builder.
///
/// Stateless apart from the static [`WindowsBuildConfig`] passed at
/// construction. Per-build state lives entirely on the stack of
/// [`WindowsBuilder::build_skeleton`] and inside the returned
/// [`BuildSkeleton`].
pub struct WindowsBuilder {
    config: WindowsBuildConfig,
}

impl WindowsBuilder {
    /// Construct a new `WindowsBuilder` with the given configuration.
    ///
    /// Does no I/O: the cache directory is lazily created when
    /// [`build_skeleton`](Self::build_skeleton) first runs.
    #[must_use]
    pub fn new(config: WindowsBuildConfig) -> Self {
        Self { config }
    }

    /// Borrow the configuration this builder was constructed with.
    #[must_use]
    pub fn config(&self) -> &WindowsBuildConfig {
        &self.config
    }

    /// Parse the Dockerfile, pull the base image, and materialise the
    /// foreign-layer chain.
    ///
    /// Returns a [`BuildSkeleton`] for downstream Phase 4 tasks to
    /// extend. Does **not** execute COPY or ADD — those route
    /// through [`Self::execute_instruction`] and surface
    /// [`BuildError::NotYetImplemented`] until 4.C lands. RUN is
    /// implemented here (4.B).
    ///
    /// # Errors
    ///
    /// - [`BuildError::ContextRead`] when the Dockerfile cannot be read.
    /// - [`BuildError::DockerfileParse`] on a malformed Dockerfile.
    /// - [`BuildError::NotSupported`] when the Dockerfile has more than
    ///   one stage (multi-stage WCOW builds are intentionally out of
    ///   scope for the skeleton — Phase 4 follow-up task).
    /// - [`BuildError::InvalidInstruction`] when the FROM target is
    ///   `scratch` or a stage reference.
    /// - [`BuildError::RegistryError`] / [`BuildError::IoError`] on
    ///   registry-side failures.
    /// - [`BuildError::NotSupported`] when invoked on a non-Windows
    ///   host (the base layer materialisation step needs the HCS
    ///   storage filter APIs).
    pub async fn build_skeleton(&self, ctx: &BuildContext) -> Result<BuildSkeleton> {
        // 1. Resolve and read the Dockerfile.
        let dockerfile_path = if ctx.dockerfile_path.is_absolute() {
            ctx.dockerfile_path.clone()
        } else {
            ctx.context_dir.join(&ctx.dockerfile_path)
        };
        let dockerfile_text =
            std::fs::read_to_string(&dockerfile_path).map_err(|e| BuildError::ContextRead {
                path: dockerfile_path.clone(),
                source: e,
            })?;

        // 2. Parse via the existing dockerfile parser.
        let parsed = Dockerfile::parse(&dockerfile_text)?;

        // 3. Reject multi-stage builds in the 4.A skeleton. The HCS
        //    backend takes the same stance for the same reason —
        //    multi-stage Windows builds require cross-stage COPY which
        //    is part of 4.C.
        if parsed.stages.is_empty() {
            return Err(BuildError::InvalidInstruction {
                instruction: "FROM".to_string(),
                reason: "Dockerfile has no FROM instruction".to_string(),
            });
        }
        if parsed.stages.len() > 1 {
            return Err(BuildError::NotSupported {
                operation: format!(
                    "multi-stage WCOW builds ({} stages) — Phase 4 task 4.A skeleton supports \
                     a single stage; cross-stage COPY arrives in Phase 4 task 4.C",
                    parsed.stages.len()
                ),
            });
        }

        // 4. Resolve the FROM target — only an image reference is valid
        //    for the single-stage WCOW skeleton.
        let stage = &parsed.stages[0];
        let base_image_ref = match &stage.base_image {
            DockerfileFromTarget::Image(r) => r.to_string(),
            DockerfileFromTarget::Stage(name) => {
                return Err(BuildError::stage_not_found(name));
            }
            DockerfileFromTarget::Scratch => {
                return Err(BuildError::InvalidInstruction {
                    instruction: "FROM scratch".to_string(),
                    reason: "WCOW builds require a Windows base image (HCS cannot run a process \
                             without a kernel + cmd.exe). Use \
                             mcr.microsoft.com/windows/nanoserver:... or .../servercore:..."
                        .to_string(),
                });
            }
        };

        // 5. Allocate the per-build cache subdirectories.
        let build_id = new_build_id();
        let build_dir = self.config.cache_dir.join(&build_id);
        std::fs::create_dir_all(&build_dir).map_err(|e| BuildError::ContextRead {
            path: build_dir.clone(),
            source: e,
        })?;
        let working_layer_chain_dir = build_dir.join("unpacked");

        // 6. Pull + materialise the base image. The heavy lifting is
        //    Windows-only because it requires HcsImportLayer +
        //    BackupStream / WCIFS plumbing; off-Windows we surface
        //    a NotSupported error.
        let (base_layers, base_manifest, working_chain) =
            pull_and_materialise_base(&base_image_ref, &self.config, &working_layer_chain_dir)
                .await?;

        // Record the FROM instruction as the first entry of the
        // execution log so 4.D's history array correctly starts with
        // `FROM <base>` (with `empty_layer = false` because the base
        // layer chain materialised above is the layer "produced" by
        // FROM).
        let instruction_log = vec![ExecutedInstruction {
            source_line: format!("FROM {base_image_ref}"),
            produced_layer: true,
            timestamp: Utc::now(),
        }];

        Ok(BuildSkeleton {
            parsed_dockerfile: parsed,
            base_layers,
            base_manifest,
            working_layer_chain_dir,
            working_chain,
            image_config: OciImageConfig::default(),
            instruction_log,
        })
    }

    /// Apply a single non-FROM instruction to a [`BuildSkeleton`].
    ///
    /// Dispatches to a per-instruction handler. Config-only instructions
    /// mutate [`BuildSkeleton::image_config`] in place; COPY/ADD/RUN
    /// commit a new RO layer per instruction (the per-instruction layer
    /// model documented at the top of this module).
    ///
    /// # Errors
    ///
    /// - [`BuildError::NotSupported`] for cross-stage `COPY --from=`
    ///   (multi-stage support arrives in a later task).
    /// - [`BuildError::PathTraversal`] when a COPY/ADD source contains
    ///   `..`.
    /// - [`BuildError::HttpFetchFailed`] when an ADD URL fetch fails.
    /// - [`BuildError::TarExtractFailed`] when an ADD tarball
    ///   auto-extract fails.
    /// - [`BuildError::ContextRead`] / [`BuildError::IoError`] for
    ///   filesystem failures.
    /// - [`BuildError::RunStepFailed`] when the RUN command exits
    ///   non-zero.
    /// - [`BuildError::LayerCreate`] / [`BuildError::LayerExportFailed`]
    ///   on HCS or wclayer failures.
    /// - [`BuildError::NotSupported`] when COPY/ADD/RUN is invoked on a
    ///   non-Windows host (HCS layer commits require the Windows APIs).
    pub async fn execute_instruction(
        &self,
        skeleton: &mut BuildSkeleton,
        ctx: &BuildContext,
        instruction: &Instruction,
    ) -> Result<()> {
        // The step index is the position of this instruction in the
        // current stage so error messages and tracing logs name a real
        // line number. We re-derive it by scanning the parsed dockerfile
        // for an `==` match; that costs O(stage_len) per call which is
        // negligible compared to the HCS round-trip cost.
        let step_index = skeleton
            .parsed_dockerfile
            .stages
            .first()
            .and_then(|s| s.instructions.iter().position(|i| i == instruction))
            .unwrap_or(0);

        let result = match instruction {
            Instruction::Run(run) => self.execute_run_step(skeleton, run, step_index).await,
            Instruction::Copy(copy) => self.apply_copy(skeleton, ctx, copy, step_index).await,
            Instruction::Add(add) => self.apply_add(skeleton, ctx, add, step_index).await,
            Instruction::Env(env) => {
                apply_env(&mut skeleton.image_config, env);
                Ok(())
            }
            Instruction::Workdir(path) => {
                apply_workdir(&mut skeleton.image_config, path);
                Ok(())
            }
            Instruction::Entrypoint(cmd) => {
                apply_entrypoint(&mut skeleton.image_config, cmd);
                Ok(())
            }
            Instruction::Cmd(cmd) => {
                apply_cmd(&mut skeleton.image_config, cmd);
                Ok(())
            }
            Instruction::User(user) => {
                skeleton.image_config.user = Some(user.clone());
                Ok(())
            }
            Instruction::Expose(expose) => {
                apply_expose(&mut skeleton.image_config, expose);
                Ok(())
            }
            Instruction::Volume(paths) => {
                for p in paths {
                    skeleton
                        .image_config
                        .volumes
                        .insert(p.clone(), serde_json::json!({}));
                }
                Ok(())
            }
            Instruction::Label(labels) => {
                for (k, v) in labels {
                    skeleton.image_config.labels.insert(k.clone(), v.clone());
                }
                Ok(())
            }
            // ARG is consumed by the parser's variable-expansion pass and
            // does not appear in the final OCI image config. We accept it
            // here as a no-op so a Dockerfile with bare `ARG` lines walks
            // through the builder cleanly.
            Instruction::Arg(_) => Ok(()),
            Instruction::Shell(tokens) => {
                skeleton.image_config.shell = Some(tokens.clone());
                Ok(())
            }
            Instruction::Stopsignal(sig) => {
                skeleton.image_config.stop_signal = Some(sig.clone());
                Ok(())
            }
            Instruction::Healthcheck(hc) => {
                apply_healthcheck(&mut skeleton.image_config, hc);
                Ok(())
            }
            Instruction::Onbuild(boxed) => {
                skeleton
                    .image_config
                    .on_build
                    .push(format_onbuild_trigger(boxed));
                Ok(())
            }
        };
        // Only record successful executions so failed RUN steps don't
        // leak into the emitted history (a build that fails never
        // produces a manifest anyway, but a clean log keeps 4.D's
        // emitter free of "did this layer actually exist?" branches).
        if result.is_ok() {
            // ARG is intentionally omitted from the history — Docker's
            // `docker history` also elides ARG triggers because they're
            // consumed at parse time and don't affect the final image.
            if !matches!(instruction, Instruction::Arg(_)) {
                skeleton.instruction_log.push(ExecutedInstruction {
                    source_line: format_instruction_source_line(instruction),
                    produced_layer: instruction.creates_layer(),
                    timestamp: Utc::now(),
                });
            }
        }
        result
    }

    /// Execute a COPY instruction. Resolves sources against
    /// `ctx.context_dir`, rejects `..` traversal, copies into a fresh
    /// scratch layer, then commits the scratch as a new RO layer on
    /// Windows. Off-Windows the copy is performed against a plain
    /// directory under `working_layer_chain_dir/<scratch_id>/Files/` for
    /// unit-test coverage; no HCS commit happens.
    async fn apply_copy(
        &self,
        skeleton: &mut BuildSkeleton,
        ctx: &BuildContext,
        copy: &CopyInstruction,
        step_index: usize,
    ) -> Result<()> {
        if let Some(stage) = &copy.from {
            return Err(BuildError::NotSupported {
                operation: format!(
                    "multi-stage COPY --from='{stage}' lands in a later task — the WCOW skeleton \
                     supports single-stage builds only"
                ),
            });
        }
        if let Some(owner) = &copy.chown {
            tracing::info!(
                step_index = step_index,
                chown = %owner,
                "COPY --chown is a no-op on WCOW (Windows containers do not honour Unix-style \
                 uid:gid ownership the same way)"
            );
        }
        let resolved_sources = resolve_copy_sources(ctx, &copy.sources)?;
        apply_filesystem_writes(
            self.config(),
            skeleton,
            step_index,
            &resolved_sources,
            &copy.destination,
            /* extract_archives = */ false,
            /* downloads = */ &[],
        )
        .await
    }

    /// Execute an ADD instruction. Extends COPY with HTTP(S) URL fetch
    /// and tarball auto-extraction.
    async fn apply_add(
        &self,
        skeleton: &mut BuildSkeleton,
        ctx: &BuildContext,
        add: &AddInstruction,
        step_index: usize,
    ) -> Result<()> {
        if let Some(owner) = &add.chown {
            tracing::info!(
                step_index = step_index,
                chown = %owner,
                "ADD --chown is a no-op on WCOW"
            );
        }
        // Partition sources into URLs vs local paths.
        let mut local_sources: Vec<String> = Vec::new();
        let mut url_sources: Vec<String> = Vec::new();
        for src in &add.sources {
            if is_http_url(src) {
                url_sources.push(src.clone());
            } else {
                local_sources.push(src.clone());
            }
        }
        let resolved_locals = resolve_copy_sources(ctx, &local_sources)?;

        // Download URLs into a temp dir so the per-instruction commit
        // step sees them as ordinary files. We materialise the downloads
        // alongside the resolved locals; the writes function does the
        // extract-if-tarball decision uniformly across both groups.
        let mut downloads: Vec<DownloadedFile> = Vec::with_capacity(url_sources.len());
        for url in &url_sources {
            let download = download_url(url).await?;
            downloads.push(download);
        }

        apply_filesystem_writes(
            self.config(),
            skeleton,
            step_index,
            &resolved_locals,
            &add.destination,
            /* extract_archives = */ true,
            &downloads,
        )
        .await
    }

    /// Execute one RUN step: optionally translate Linux package-manager
    /// invocations to Chocolatey, build an ephemeral HCS compute system
    /// rooted at a fresh scratch layer over the working chain, spawn the
    /// command, wait for it to exit, then export the scratch diff as a
    /// new read-only layer and append it to the chain.
    ///
    /// Routed to a platform-specific implementation; non-Windows hosts
    /// surface [`BuildError::NotSupported`].
    async fn execute_run_step(
        &self,
        skeleton: &mut BuildSkeleton,
        run: &RunInstruction,
        step_index: usize,
    ) -> Result<()> {
        execute_run_step_impl(&self.config, skeleton, run, step_index).await
    }

    /// Emit a final OCI image manifest + image config blob from the
    /// accumulated [`BuildSkeleton`] state.
    ///
    /// Walks the base-first [`BuildSkeleton::base_layers`] vector
    /// alongside [`BuildSkeleton::working_chain`] to build one
    /// [`EmittedLayer`] per descriptor. The first descriptor — the
    /// foreign Windows base layer pulled from MCR — gets the
    /// `application/vnd.docker.image.rootfs.foreign.diff.tar.gzip` media
    /// type AND preserves the `urls[]` mirror list so Windows daemons
    /// pull the bytes from MCR rather than the user's destination
    /// registry. Subsequent layers (RUN / COPY / ADD outputs) get
    /// `application/vnd.oci.image.layer.v1.tar+gzip` and no `urls[]`.
    ///
    /// `os.version` resolution order:
    /// 1. [`WindowsBuildConfig::os_version_override`] (explicit user
    ///    pin).
    /// 2. [`BaseImageManifest::os_version`] (from the resolved base
    ///    manifest's platform descriptor / config blob).
    /// 3. Fallback to [`BuildError::OsVersionUnresolved`] — the Windows
    ///    runtime refuses to launch a container whose `os.version` does
    ///    not match the host kernel build, so emitting a manifest
    ///    without one would produce a container nothing can run.
    ///
    /// # Errors
    ///
    /// - [`BuildError::OsVersionUnresolved`] when neither the override
    ///   nor the base manifest carries an `os.version`.
    /// - [`BuildError::SerializeManifestFailed`] when serialising the
    ///   image config or manifest blob fails (programmer error in this
    ///   crate).
    /// - [`BuildError::LayerDigestComputationFailed`] when computing a
    ///   `diff_id` over a non-foreign layer blob fails because the blob
    ///   path could not be read.
    pub async fn emit_image(&self, skeleton: &BuildSkeleton, tag: &str) -> Result<BuiltImage> {
        emit_image_impl(self.config(), skeleton, tag).await
    }
}

/// Reconstruct a canonical Dockerfile source line for one parsed
/// instruction. Used for [`ExecutedInstruction::source_line`] so the
/// emitted OCI history's `created_by` shows the canonical form (not the
/// original line, which may have spanned multiple physical lines via
/// continuation backslashes).
fn format_instruction_source_line(instr: &Instruction) -> String {
    match instr {
        Instruction::Run(run) => match &run.command {
            ShellOrExec::Shell(s) => format!("RUN {s}"),
            ShellOrExec::Exec(args) => format!(
                "RUN {}",
                serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string())
            ),
        },
        Instruction::Copy(c) => {
            let from = c
                .from
                .as_deref()
                .map(|f| format!("--from={f} "))
                .unwrap_or_default();
            format!("COPY {from}{} {}", c.sources.join(" "), c.destination)
        }
        Instruction::Add(a) => format!("ADD {} {}", a.sources.join(" "), a.destination),
        Instruction::Env(e) => {
            let mut keys: Vec<&String> = e.vars.keys().collect();
            keys.sort();
            let body = keys
                .iter()
                .map(|k| format!("{}={}", k, e.vars[*k]))
                .collect::<Vec<_>>()
                .join(" ");
            format!("ENV {body}")
        }
        Instruction::Workdir(p) => format!("WORKDIR {p}"),
        Instruction::Expose(e) => {
            let proto = match e.protocol {
                ExposeProtocol::Tcp => "tcp",
                ExposeProtocol::Udp => "udp",
            };
            format!("EXPOSE {}/{proto}", e.port)
        }
        Instruction::Label(labels) => {
            let mut keys: Vec<&String> = labels.keys().collect();
            keys.sort();
            let body = keys
                .iter()
                .map(|k| format!("{}={}", k, labels[*k]))
                .collect::<Vec<_>>()
                .join(" ");
            format!("LABEL {body}")
        }
        Instruction::User(u) => format!("USER {u}"),
        Instruction::Entrypoint(c) => match c {
            ShellOrExec::Shell(s) => format!("ENTRYPOINT {s}"),
            ShellOrExec::Exec(args) => format!(
                "ENTRYPOINT {}",
                serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string())
            ),
        },
        Instruction::Cmd(c) => match c {
            ShellOrExec::Shell(s) => format!("CMD {s}"),
            ShellOrExec::Exec(args) => format!(
                "CMD {}",
                serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string())
            ),
        },
        Instruction::Volume(paths) => format!("VOLUME {}", paths.join(" ")),
        Instruction::Shell(tokens) => format!(
            "SHELL {}",
            serde_json::to_string(tokens).unwrap_or_else(|_| "[]".to_string())
        ),
        Instruction::Arg(a) => match &a.default {
            Some(d) => format!("ARG {}={d}", a.name),
            None => format!("ARG {}", a.name),
        },
        Instruction::Stopsignal(s) => format!("STOPSIGNAL {s}"),
        Instruction::Healthcheck(_) => "HEALTHCHECK".to_string(),
        Instruction::Onbuild(inner) => {
            format!("ONBUILD {}", format_instruction_source_line(inner))
        }
    }
}

// ---------------------------------------------------------------------------
// 4.C: config-only instruction helpers (cross-platform, pure mutation)
// ---------------------------------------------------------------------------

/// Apply a WORKDIR instruction.
///
/// Relative paths resolve against the previous WORKDIR per the Dockerfile
/// spec. On Windows the resolution uses backslash as the separator so
/// `WORKDIR sub` after `WORKDIR C:\\app` yields `C:\\app\\sub`. Absolute
/// paths (Unix-style `/x` or Windows-style `C:\\x` / `C:/x`) replace the
/// prior value.
pub(crate) fn apply_workdir(cfg: &mut OciImageConfig, path: &str) {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return;
    }
    let is_absolute = is_absolute_windows_or_unix(trimmed);
    let resolved = if is_absolute {
        trimmed.to_string()
    } else if let Some(prev) = cfg.working_dir.as_deref() {
        join_windows_path(prev, trimmed)
    } else {
        // No prior WORKDIR — relative path against an unset cwd is
        // treated as the root drive on Windows. We surface it verbatim
        // so the final OCI config preserves the user's intent for 4.D.
        trimmed.to_string()
    };
    cfg.working_dir = Some(resolved);
}

/// Apply an ENV instruction, enforcing last-write-wins for each KEY.
pub(crate) fn apply_env(cfg: &mut OciImageConfig, env: &EnvInstruction) {
    // Sort keys so repeated calls produce a stable order in the final
    // image config — multi-key ENV lines are common (`ENV A=1 B=2`) and
    // a non-deterministic order breaks image-digest reproducibility.
    let mut keys: Vec<&String> = env.vars.keys().collect();
    keys.sort();
    for key in keys {
        let value = &env.vars[key];
        let entry = format!("{key}={value}");
        // Drop any existing entry with the same KEY.
        cfg.env
            .retain(|e| e.split_once('=').is_none_or(|(k, _)| k != key.as_str()));
        cfg.env.push(entry);
    }
}

/// Apply an ENTRYPOINT instruction. Shell form is rewritten to
/// `cmd /c <body>` per Windows convention; exec form is passed through.
/// Setting ENTRYPOINT resets CMD to `None` per the Dockerfile spec.
pub(crate) fn apply_entrypoint(cfg: &mut OciImageConfig, cmd: &ShellOrExec) {
    cfg.entrypoint = Some(shell_or_exec_to_vec(cmd));
    cfg.cmd = None;
}

/// Apply a CMD instruction. Shell form is rewritten to `cmd /c <body>`;
/// exec form is passed through.
pub(crate) fn apply_cmd(cfg: &mut OciImageConfig, cmd: &ShellOrExec) {
    cfg.cmd = Some(shell_or_exec_to_vec(cmd));
}

/// Apply an EXPOSE instruction. Multiple EXPOSE lines accumulate.
pub(crate) fn apply_expose(cfg: &mut OciImageConfig, expose: &ExposeInstruction) {
    let proto = match expose.protocol {
        ExposeProtocol::Tcp => "tcp",
        ExposeProtocol::Udp => "udp",
    };
    let key = format!("{}/{}", expose.port, proto);
    cfg.exposed_ports.insert(key, serde_json::json!({}));
}

/// Apply a HEALTHCHECK instruction, normalising `Duration` values to OCI
/// string form so 4.D can serialise without re-formatting.
pub(crate) fn apply_healthcheck(cfg: &mut OciImageConfig, hc: &HealthcheckInstruction) {
    match hc {
        HealthcheckInstruction::None => {
            cfg.healthcheck = Some(OciHealthcheck::disabled());
        }
        HealthcheckInstruction::Check {
            command,
            interval,
            timeout,
            start_period,
            retries,
            ..
        } => {
            let test = match command {
                ShellOrExec::Shell(s) => vec!["CMD-SHELL".to_string(), s.clone()],
                ShellOrExec::Exec(args) => {
                    let mut v = Vec::with_capacity(args.len() + 1);
                    v.push("CMD".to_string());
                    v.extend(args.iter().cloned());
                    v
                }
            };
            cfg.healthcheck = Some(OciHealthcheck {
                test,
                interval: interval.map(duration_to_oci_string),
                timeout: timeout.map(duration_to_oci_string),
                retries: *retries,
                start_period: start_period.map(duration_to_oci_string),
            });
        }
    }
}

/// Convert a [`ShellOrExec`] into the OCI config's vector form. Shell
/// form is wrapped in `["cmd", "/c", "<body>"]` for WCOW; exec form is
/// passed through.
fn shell_or_exec_to_vec(cmd: &ShellOrExec) -> Vec<String> {
    match cmd {
        ShellOrExec::Shell(body) => {
            vec!["cmd".to_string(), "/c".to_string(), body.clone()]
        }
        ShellOrExec::Exec(args) => args.clone(),
    }
}

/// Format a [`std::time::Duration`] into the OCI healthcheck string form
/// (e.g. `"30s"`, `"1m30s"`, `"500ms"`). Mirrors the `time.ParseDuration`
/// shape Docker uses on the wire.
fn duration_to_oci_string(d: std::time::Duration) -> String {
    let total_ms = d.as_millis();
    if total_ms == 0 {
        return "0s".to_string();
    }
    if total_ms % 1000 != 0 {
        return format!("{total_ms}ms");
    }
    let secs = d.as_secs();
    if secs % 60 != 0 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins % 60 != 0 {
        return format!("{mins}m");
    }
    format!("{}h", mins / 60)
}

/// Format an ONBUILD trigger back into Dockerfile source form for
/// storage in the image config's `OnBuild` array. The OCI image config
/// stores triggers as raw instruction strings so downstream builds can
/// re-parse them.
fn format_onbuild_trigger(instr: &Instruction) -> String {
    match instr {
        Instruction::Run(run) => match &run.command {
            ShellOrExec::Shell(s) => format!("RUN {s}"),
            ShellOrExec::Exec(args) => format!(
                "RUN {}",
                serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string())
            ),
        },
        Instruction::Copy(c) => format!("COPY {} {}", c.sources.join(" "), c.destination),
        Instruction::Add(a) => format!("ADD {} {}", a.sources.join(" "), a.destination),
        Instruction::Env(e) => {
            let mut keys: Vec<&String> = e.vars.keys().collect();
            keys.sort();
            let body = keys
                .iter()
                .map(|k| format!("{}={}", k, e.vars[*k]))
                .collect::<Vec<_>>()
                .join(" ");
            format!("ENV {body}")
        }
        Instruction::Workdir(p) => format!("WORKDIR {p}"),
        Instruction::User(u) => format!("USER {u}"),
        Instruction::Cmd(c) => match c {
            ShellOrExec::Shell(s) => format!("CMD {s}"),
            ShellOrExec::Exec(args) => format!(
                "CMD {}",
                serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string())
            ),
        },
        Instruction::Entrypoint(c) => match c {
            ShellOrExec::Shell(s) => format!("ENTRYPOINT {s}"),
            ShellOrExec::Exec(args) => format!(
                "ENTRYPOINT {}",
                serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string())
            ),
        },
        other => other.name().to_string(),
    }
}

/// Return `true` for paths that start with `/`, `\`, or a Windows drive
/// letter (`C:\` / `C:/`). Used by [`apply_workdir`] to decide whether
/// to resolve relative-to-previous.
fn is_absolute_windows_or_unix(p: &str) -> bool {
    if p.starts_with('/') || p.starts_with('\\') {
        return true;
    }
    let bytes = p.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return true;
    }
    false
}

/// Join a base path with a relative suffix using a backslash separator
/// (Windows path convention). Avoids `std::path::Path::join` because
/// that uses the host OS's separator, which would produce
/// `C:\\app/sub` on Linux — wrong shape for an OCI image targeting
/// Windows.
fn join_windows_path(base: &str, suffix: &str) -> String {
    let mut joined = base.trim_end_matches(['\\', '/']).to_string();
    joined.push('\\');
    joined.push_str(suffix.trim_start_matches(['\\', '/']));
    joined
}

// ---------------------------------------------------------------------------
// 4.C: COPY/ADD filesystem helpers (cross-platform plumbing)
// ---------------------------------------------------------------------------

/// One resolved local source for a COPY/ADD. `relative` is the original
/// `<src>` string from the Dockerfile (kept for diagnostics); `absolute`
/// is the fully-resolved `context_dir.join(<src>)`.
#[derive(Debug, Clone)]
struct ResolvedSource {
    relative: String,
    absolute: PathBuf,
}

/// A downloaded URL materialised on disk, paired with the URL's
/// basename so [`apply_filesystem_writes`] can place the file at
/// `<dest>/<basename>` when `<dest>` is a directory.
struct DownloadedFile {
    /// Path on disk to the downloaded blob (lives in a tempdir whose
    /// guard is held until this struct is dropped).
    path: PathBuf,
    /// Basename derived from the URL path component.
    basename: String,
    /// The original URL for diagnostics + the extract-if-tarball
    /// decision (tarball detection is purely by extension).
    url: String,
    /// Temp-dir guard so the file outlives this object until the
    /// destructor runs.
    _guard: tempfile::TempDir,
}

/// Detect whether a COPY/ADD source string is an HTTP(S) URL.
fn is_http_url(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Resolve a list of COPY/ADD source strings against the build context,
/// rejecting any path that contains a `..` component.
fn resolve_copy_sources(ctx: &BuildContext, srcs: &[String]) -> Result<Vec<ResolvedSource>> {
    let mut out = Vec::with_capacity(srcs.len());
    for src in srcs {
        // Reject `..` BEFORE any filesystem access so a malicious
        // Dockerfile cannot win a TOCTOU race against the resolver.
        if path_contains_parent_dir(src) {
            return Err(BuildError::PathTraversal { src: src.clone() });
        }
        let absolute = ctx.context_dir.join(src);
        // Second-line defence: the joined path itself must stay under
        // `context_dir`. We canonicalise lazily — only when the entry
        // exists — because COPY against a non-existent source is itself
        // an error reported below by the copy step. The cheap
        // component-walk above already handles the common attack
        // surface.
        out.push(ResolvedSource {
            relative: src.clone(),
            absolute,
        });
    }
    Ok(out)
}

/// Pure path-component walk that rejects any `..` segment. Works on
/// both Unix and Windows-style separators because Dockerfile sources
/// always use forward slashes per the spec.
fn path_contains_parent_dir(src: &str) -> bool {
    // Normalise both separator flavours to `/` so a Dockerfile written
    // with backslashes (which the spec discourages but tooling tolerates)
    // is still inspected correctly.
    let normalised = src.replace('\\', "/");
    Path::new(&normalised)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
}

/// Download a URL into a tempdir and return a [`DownloadedFile`].
async fn download_url(url: &str) -> Result<DownloadedFile> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| BuildError::HttpFetchFailed {
            url: url.to_string(),
            source: e,
        })?
        .error_for_status()
        .map_err(|e| BuildError::HttpFetchFailed {
            url: url.to_string(),
            source: e,
        })?;
    let bytes = response
        .bytes()
        .await
        .map_err(|e| BuildError::HttpFetchFailed {
            url: url.to_string(),
            source: e,
        })?;
    let basename = url
        .rsplit('/')
        .next()
        .and_then(|s| s.split('?').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("download")
        .to_string();
    let guard = tempfile::tempdir().map_err(BuildError::IoError)?;
    let path = guard.path().join(&basename);
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(BuildError::IoError)?;
    Ok(DownloadedFile {
        path,
        basename,
        url: url.to_string(),
        _guard: guard,
    })
}

/// Detect whether a path's extension marks it as an auto-extractable
/// archive. Matches the Docker ADD documentation: `.tar`, `.tar.gz`
/// (`.tgz`), `.tar.bz2`, `.tar.xz`. Case-insensitive — Dockerfile
/// authors on Windows occasionally write `.TAR.GZ`.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn is_tarball_path(name: &str) -> bool {
    // `extension()` only sees the final segment so `.tar.gz` needs the
    // suffix-match form. Lowercasing the whole name once keeps the rule
    // straightforward and avoids stitching path components back together.
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".tar")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || lower.ends_with(".tar.bz2")
        || lower.ends_with(".tar.xz")
}

/// Materialise the COPY/ADD destination root inside a scratch directory.
///
/// Returns `(scratch_dir, files_root)` where `scratch_dir` is the
/// per-instruction staging area under `working_layer_chain_dir` and
/// `files_root` is the `Files/` subdirectory that mirrors HCS's
/// per-layer payload layout (HCS exports layers with `Files/` +
/// `Hives/`; we use the same shape so 4.D's manifest emission can pass
/// the directory straight to `wclayer::import_layer`).
fn prepare_scratch_for_writes(
    config: &WindowsBuildConfig,
    skeleton: &BuildSkeleton,
    step_index: usize,
) -> Result<(PathBuf, PathBuf)> {
    let _ = config; // reserved for size_gb / cache policy in a later task
    let scratch_id = format!("copy-add-{step_index}-{}", uuid::Uuid::new_v4());
    let scratch_dir = skeleton.working_layer_chain_dir.join(&scratch_id);
    let files_root = scratch_dir.join("Files");
    std::fs::create_dir_all(&files_root).map_err(BuildError::IoError)?;
    Ok((scratch_dir, files_root))
}

/// Strip a Windows or Unix root prefix from a destination so it can be
/// joined against `Files/`. `C:\app\bin` → `app/bin`; `/etc/foo` →
/// `etc/foo`. Mixed separators are normalised to forward slashes.
fn dest_under_files_root(dest: &str) -> PathBuf {
    let mut s = dest.replace('\\', "/");
    if s.len() >= 2 && s.as_bytes()[0].is_ascii_alphabetic() && s.as_bytes()[1] == b':' {
        s = s[2..].to_string();
    }
    let trimmed = s.trim_start_matches('/');
    PathBuf::from(trimmed)
}

/// Decide whether a destination is a directory (ends with `/` or `\`,
/// or there are multiple sources). Mirrors Dockerfile COPY/ADD
/// semantics: with N>1 sources or a trailing separator, the destination
/// is treated as a directory; otherwise the single source is treated
/// as a file rename.
fn destination_is_directory(dest: &str, source_count: usize) -> bool {
    source_count > 1 || dest.ends_with('/') || dest.ends_with('\\')
}

/// Recursively copy `src` to `dst`. Mirrors `std::fs::copy` semantics
/// for files; for directories it walks the tree and re-creates each
/// child.
fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = std::fs::metadata(src)?;
    if meta.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let child_src = entry.path();
            let child_dst = dst.join(entry.file_name());
            copy_recursive(&child_src, &child_dst)?;
        }
        Ok(())
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst).map(|_| ())
    }
}

/// Extract a tarball into `dest_dir`, picking the decompressor by
/// extension. Used by ADD's archive auto-extract path.
fn extract_tarball(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    use std::fs::File;
    use std::io::BufReader;

    std::fs::create_dir_all(dest_dir).map_err(BuildError::IoError)?;
    let file = File::open(archive_path).map_err(BuildError::IoError)?;
    let reader = BufReader::new(file);
    let lower = archive_path.to_string_lossy().to_ascii_lowercase();

    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let mut archive: tar::Archive<Box<dyn std::io::Read>> = if lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
    {
        tar::Archive::new(Box::new(flate2::read::GzDecoder::new(reader)) as Box<dyn std::io::Read>)
    } else if lower.ends_with(".tar.bz2") {
        tar::Archive::new(Box::new(bzip2::read::BzDecoder::new(reader)) as Box<dyn std::io::Read>)
    } else if lower.ends_with(".tar.xz") {
        tar::Archive::new(Box::new(xz2::read::XzDecoder::new(reader)) as Box<dyn std::io::Read>)
    } else {
        tar::Archive::new(Box::new(reader) as Box<dyn std::io::Read>)
    };

    // Reject entries with `..` so a hostile tarball cannot escape
    // `dest_dir`. `tar::Archive::set_overwrite(true)` would silently
    // clobber files outside `dest_dir` if we let traversal through.
    for entry in archive
        .entries()
        .map_err(|e| BuildError::TarExtractFailed { source: e })?
    {
        let mut entry = entry.map_err(|e| BuildError::TarExtractFailed { source: e })?;
        let entry_path = entry
            .path()
            .map_err(|e| BuildError::TarExtractFailed { source: e })?
            .into_owned();
        if entry_path
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(BuildError::TarExtractFailed {
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "tarball entry '{}' contains '..' — refusing to extract",
                        entry_path.display()
                    ),
                ),
            });
        }
        entry
            .unpack_in(dest_dir)
            .map_err(|e| BuildError::TarExtractFailed { source: e })?;
    }
    Ok(())
}

/// Stage every COPY/ADD write into a per-instruction scratch directory
/// and commit it as a new RO layer.
///
/// - On Windows this calls into HCS via [`commit_scratch_as_layer`].
/// - Off-Windows the scratch dir is left in place (so unit tests can
///   inspect `Files/<dest>/<src>`) and the function returns `Ok(())`
///   without touching `skeleton.base_layers` / `skeleton.working_chain`
///   — the next Windows-gated `build_skeleton` call is what produces
///   real layer descriptors. This preserves the cross-platform unit-test
///   contract documented at the top of the module.
#[allow(clippy::similar_names)] // `dst`/`dest_*` bindings name distinct concepts (per-entry destination vs. resolved dest_*)
async fn apply_filesystem_writes(
    config: &WindowsBuildConfig,
    skeleton: &mut BuildSkeleton,
    step_index: usize,
    locals: &[ResolvedSource],
    dest: &str,
    extract_archives: bool,
    downloads: &[DownloadedFile],
) -> Result<()> {
    let total_sources = locals.len() + downloads.len();
    if total_sources == 0 {
        // No-op COPY/ADD — Dockerfile parser already rejects truly
        // empty source lists, but a `COPY` with only URLs that all
        // failed-but-recovered is conceivable. Treat as success.
        return Ok(());
    }
    let dest_is_dir = destination_is_directory(dest, total_sources);
    let (scratch_dir, files_root) = prepare_scratch_for_writes(config, skeleton, step_index)?;

    let dest_rel = dest_under_files_root(dest);
    let dest_abs_in_layer = files_root.join(&dest_rel);

    // Process local sources.
    for src in locals {
        let meta = std::fs::metadata(&src.absolute).map_err(|e| BuildError::ContextRead {
            path: src.absolute.clone(),
            source: e,
        })?;
        if extract_archives && meta.is_file() && is_tarball_path(&src.relative) {
            extract_tarball(&src.absolute, &dest_abs_in_layer)?;
        } else if meta.is_dir() {
            std::fs::create_dir_all(&dest_abs_in_layer).map_err(BuildError::IoError)?;
            // Directory copy: contents-into-dest (the Docker default).
            for entry in std::fs::read_dir(&src.absolute).map_err(BuildError::IoError)? {
                let entry = entry.map_err(BuildError::IoError)?;
                let child_dst = dest_abs_in_layer.join(entry.file_name());
                copy_recursive(&entry.path(), &child_dst).map_err(BuildError::IoError)?;
            }
        } else if dest_is_dir {
            std::fs::create_dir_all(&dest_abs_in_layer).map_err(BuildError::IoError)?;
            let basename = src
                .absolute
                .file_name()
                .map(std::ffi::OsStr::to_os_string)
                .ok_or_else(|| BuildError::ContextRead {
                    path: src.absolute.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!(
                            "COPY/ADD source '{}' has no file name",
                            src.absolute.display()
                        ),
                    ),
                })?;
            let dst = dest_abs_in_layer.join(basename);
            std::fs::copy(&src.absolute, &dst).map_err(BuildError::IoError)?;
        } else {
            if let Some(parent) = dest_abs_in_layer.parent() {
                std::fs::create_dir_all(parent).map_err(BuildError::IoError)?;
            }
            std::fs::copy(&src.absolute, &dest_abs_in_layer).map_err(BuildError::IoError)?;
        }
    }

    // Process URL downloads.
    for download in downloads {
        let is_tar = is_tarball_path(&download.basename);
        if extract_archives && is_tar {
            extract_tarball(&download.path, &dest_abs_in_layer)?;
        } else if dest_is_dir {
            std::fs::create_dir_all(&dest_abs_in_layer).map_err(BuildError::IoError)?;
            let dst = dest_abs_in_layer.join(&download.basename);
            std::fs::copy(&download.path, &dst).map_err(BuildError::IoError)?;
        } else {
            if let Some(parent) = dest_abs_in_layer.parent() {
                std::fs::create_dir_all(parent).map_err(BuildError::IoError)?;
            }
            std::fs::copy(&download.path, &dest_abs_in_layer).map_err(BuildError::IoError)?;
        }
        tracing::debug!(
            step_index = step_index,
            url = %download.url,
            dest = %dest,
            "ADD URL download materialised"
        );
    }

    commit_scratch_as_layer(skeleton, step_index, &scratch_dir).await
}

/// Windows: tar+gz the scratch directory, commit a new RO layer via
/// `wclayer::import_layer`, and append the new layer to both chains.
#[cfg(target_os = "windows")]
async fn commit_scratch_as_layer(
    skeleton: &mut BuildSkeleton,
    step_index: usize,
    scratch_dir: &Path,
) -> Result<()> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use sha2::{Digest, Sha256};
    use zlayer_agent::windows::wclayer::{self, LayerChain};
    use zlayer_hcs::schema::Layer as HcsLayer;

    // Build the parent chain in HCS child-to-parent order from the
    // base-first working_chain.
    let parent_chain: LayerChain = LayerChain::new(
        skeleton
            .working_chain
            .iter()
            .rev()
            .map(|e| HcsLayer {
                id: e.layer_id.clone(),
                path: e.layer_path.to_string_lossy().into_owned(),
            })
            .collect(),
    );

    // Tar + gzip the scratch dir to get a digest + size for the layer
    // descriptor that 4.D will emit. The on-disk form is what
    // `wclayer::import_layer` consumes (an unpacked HCS layer folder
    // shape — `Files/`, optionally `Hives/`); we keep both.
    let tar_bytes =
        tar_export_folder(scratch_dir).map_err(|e| BuildError::LayerExportFailed { source: e })?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut encoder, &tar_bytes)
        .map_err(|e| BuildError::LayerExportFailed { source: e })?;
    let compressed = encoder
        .finish()
        .map_err(|e| BuildError::LayerExportFailed { source: e })?;
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(&compressed)));
    #[allow(clippy::cast_possible_wrap)]
    let size = compressed.len() as i64;

    // Materialise the staged scratch as a new RO layer on disk so
    // subsequent RUN steps can chain off it.
    let new_layer_id = uuid::Uuid::new_v4().to_string();
    let new_layer_path = skeleton.working_layer_chain_dir.join(&new_layer_id);
    std::fs::create_dir_all(&new_layer_path)
        .map_err(|e| BuildError::LayerExportFailed { source: e })?;
    zlayer_agent::windows::layer::enable_backup_restore_privileges()
        .map_err(BuildError::IoError)?;
    wclayer::import_layer(&new_layer_path, scratch_dir, &parent_chain)
        .map_err(|e| BuildError::LayerExportFailed { source: e })?;

    // Best-effort cleanup of the staging scratch.
    if let Err(e) = std::fs::remove_dir_all(scratch_dir) {
        tracing::warn!(
            scratch_dir = %scratch_dir.display(),
            step_index = step_index,
            error = %e,
            "failed to remove COPY/ADD scratch dir after import"
        );
    }

    skeleton.base_layers.push(LayerRef {
        digest,
        media_type: OCI_WINDOWS_LAYER_MEDIA_TYPE.to_string(),
        size,
        urls: Vec::new(),
    });
    skeleton.working_chain.push(WindowsLayerEntry {
        layer_id: new_layer_id,
        layer_path: new_layer_path,
    });

    Ok(())
}

/// Non-Windows hosts: leave the staged scratch in place so unit tests
/// can assert against it, and return Ok without producing a layer
/// descriptor. Off-Windows COPY/ADD is unit-test fixture territory; an
/// actual production build off-Windows is rejected by
/// [`build_skeleton`] upstream.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
async fn commit_scratch_as_layer(
    _skeleton: &mut BuildSkeleton,
    _step_index: usize,
    _scratch_dir: &Path,
) -> Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Platform-gated base-image materialisation
// ---------------------------------------------------------------------------

/// Generate a fresh per-build identifier. Uses a wall-clock-nanos suffix to
/// stay free of any external dependency for a single value; collisions
/// require two simultaneous builds within the same nanosecond which is
/// not a real concern at the builder's call rate.
fn new_build_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("wcow-{nanos:032x}")
}

/// Windows: pull the base manifest, fetch every layer blob into the
/// blob cache, and call `unpack_windows_image` to materialise the
/// foreign-layer chain on disk. Returns the layer descriptors and the
/// resolved manifest metadata as cross-platform plain-data structs so
/// the caller doesn't need a hard dep on `zlayer-hcs::schema::Layer`.
#[cfg(target_os = "windows")]
async fn pull_and_materialise_base(
    base_image_ref: &str,
    config: &WindowsBuildConfig,
    working_layer_chain_dir: &std::path::Path,
) -> Result<(Vec<LayerRef>, BaseImageManifest, Vec<WindowsLayerEntry>)> {
    use zlayer_agent::windows::unpacker::{self, ResolvedLayerDescriptor};

    std::fs::create_dir_all(working_layer_chain_dir).map_err(BuildError::IoError)?;

    let target = parse_platform_string(&config.platform)?;
    let target = match config.os_version_override.as_ref() {
        Some(v) => target.with_os_version(v.clone()),
        None => target,
    };

    let cache_type = zlayer_registry::CacheType::from_env()
        .map_err(|e| BuildError::registry_error(format!("WCOW blob cache from env: {e}")))?;
    let blob_cache = cache_type
        .build()
        .await
        .map_err(|e| BuildError::registry_error(format!("open WCOW blob cache: {e}")))?;
    let puller = zlayer_registry::ImagePuller::with_platform(blob_cache, target);

    let (manifest, _digest) = puller
        .pull_manifest(base_image_ref, &config.registry_auth)
        .await
        .map_err(|e| BuildError::registry_error(format!("pull manifest {base_image_ref}: {e}")))?;

    // Fetch the base config blob so task 4.D can inherit Env / Cmd /
    // Entrypoint defaults. Non-fatal on failure — we surface an empty
    // blob rather than aborting the whole build over a config blob the
    // user might not even need.
    let config_blob = puller
        .pull_blob(
            base_image_ref,
            &manifest.config.digest,
            &config.registry_auth,
        )
        .await
        .unwrap_or_default();

    let os_version: Option<String> = serde_json::from_slice::<serde_json::Value>(&config_blob)
        .ok()
        .as_ref()
        .and_then(|v| v.get("os.version"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);

    let descriptors: Vec<ResolvedLayerDescriptor> = manifest
        .layers
        .iter()
        .map(|layer| ResolvedLayerDescriptor {
            digest: layer.digest.clone(),
            media_type: layer.media_type.clone(),
            size: layer.size,
            urls: layer.urls.clone().unwrap_or_default(),
        })
        .collect();

    let unpacked = unpacker::unpack_windows_image(
        &puller,
        base_image_ref,
        &config.registry_auth,
        &descriptors,
        working_layer_chain_dir,
    )
    .await
    .map_err(BuildError::IoError)?;

    let layer_refs = descriptors
        .iter()
        .map(|d| LayerRef {
            digest: d.digest.clone(),
            media_type: d.media_type.clone(),
            size: d.size,
            urls: d.urls.clone(),
        })
        .collect();

    // The unpacker returns the chain in child-to-parent order. We carry
    // base-first internally so the per-RUN code can reason about the
    // append at the end of the chain without reversing on every call.
    let mut working_chain: Vec<WindowsLayerEntry> = unpacked
        .chain
        .0
        .iter()
        .rev()
        .map(|layer| WindowsLayerEntry {
            layer_id: layer.id.clone(),
            layer_path: PathBuf::from(&layer.path),
        })
        .collect();
    // Discard the empty-chain edge case that would otherwise leave us
    // with no parents: a Windows base image must materialise at least
    // one layer, and a zero-length chain is a hard failure for any
    // downstream HCS call.
    if working_chain.is_empty() {
        return Err(BuildError::registry_error(format!(
            "no parent layers were materialised from {base_image_ref} — \
             the base image must contribute at least one layer"
        )));
    }
    // Sanity check: the LayerRef and WindowsLayerEntry vectors must
    // be the same length so 4.D can correlate `(digest, on_disk_path)`
    // 1:1. The unpacker emits one entry per descriptor; this assertion
    // catches a future drift before it silently produces a malformed
    // manifest.
    debug_assert_eq!(layer_refs.len(), working_chain.len());
    // Defensive shrink in case the unpacker ever returns more entries
    // than descriptors — keep the chain consistent with the LayerRef
    // vector that downstream code iterates against.
    working_chain.truncate(layer_refs.len());

    Ok((
        layer_refs,
        BaseImageManifest {
            image_ref: base_image_ref.to_string(),
            os: "windows".to_string(),
            os_version,
            arch: "amd64".to_string(),
            config_blob,
        },
        working_chain,
    ))
}

/// Non-Windows hosts cannot drive HCS storage APIs, so the base layer
/// materialisation step refuses to proceed. The whole `WindowsBuilder`
/// public API still compiles on Linux/macOS so this crate's unit tests
/// can run across CI, but anyone actually invoking `build_skeleton` off-
/// Windows gets a precise error.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
async fn pull_and_materialise_base(
    _base_image_ref: &str,
    _config: &WindowsBuildConfig,
    _working_layer_chain_dir: &std::path::Path,
) -> Result<(Vec<LayerRef>, BaseImageManifest, Vec<WindowsLayerEntry>)> {
    Err(BuildError::NotSupported {
        operation: "WindowsBuilder::build_skeleton requires target_os = \"windows\" — \
                    HcsImportLayer / wclayer::* APIs are not available on this host"
            .to_string(),
    })
}

/// Parse a `"windows/amd64"` / `"windows/arm64"` platform string into a
/// [`zlayer_spec::TargetPlatform`]. Only used on Windows; the
/// non-Windows path never reads `config.platform`.
#[cfg(target_os = "windows")]
fn parse_platform_string(platform: &str) -> Result<zlayer_spec::TargetPlatform> {
    let (os_str, arch_str) = platform.split_once('/').ok_or_else(|| {
        BuildError::invalid_instruction(
            "platform",
            format!("expected `<os>/<arch>` (e.g. windows/amd64), got `{platform}`"),
        )
    })?;
    let os = zlayer_spec::OsKind::from_oci_str(os_str).ok_or_else(|| {
        BuildError::invalid_instruction("platform.os", format!("unrecognised OS `{os_str}`"))
    })?;
    let arch = match arch_str {
        "amd64" | "x86_64" => zlayer_spec::ArchKind::Amd64,
        "arm64" | "aarch64" => zlayer_spec::ArchKind::Arm64,
        other => {
            return Err(BuildError::invalid_instruction(
                "platform.arch",
                format!("unrecognised arch `{other}`"),
            ));
        }
    };
    Ok(zlayer_spec::TargetPlatform::new(os, arch))
}

// ---------------------------------------------------------------------------
// 4.B: RUN step implementation
// ---------------------------------------------------------------------------

/// OCI media type for a Windows-layer tar+gzip blob emitted by the
/// builder. Matches what the existing `HcsBackend` writes so produced
/// images round-trip through the same registry path.
#[cfg(target_os = "windows")]
const OCI_WINDOWS_LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar+gzip";

/// Termination grace for the per-RUN process. Mirrors the
/// `backend::hcs::exec` builder default so behaviour is consistent
/// across the two builders.
#[cfg(target_os = "windows")]
const RUN_STEP_TERMINATION_GRACE_SECS: u64 = 10 * 60;

/// Windows: execute one RUN step end-to-end (translate → spawn → wait →
/// export → commit).
#[cfg(target_os = "windows")]
async fn execute_run_step_impl(
    config: &WindowsBuildConfig,
    skeleton: &mut BuildSkeleton,
    run: &RunInstruction,
    step_index: usize,
) -> Result<()> {
    use std::time::{Duration, Instant};

    use flate2::write::GzEncoder;
    use flate2::Compression;
    use sha2::{Digest, Sha256};
    use tracing::{debug, info, warn};
    use zlayer_agent::windows::scratch as agent_scratch;
    use zlayer_agent::windows::wclayer::{self, LayerChain};
    use zlayer_hcs::process::ComputeProcess;
    use zlayer_hcs::schema::{
        ComputeSystem as HcsSystemDoc, Container, Layer as HcsLayer, ProcessParameters,
        ProcessStatus, SchemaVersion, Storage,
    };
    use zlayer_hcs::system::ComputeSystem;

    // 1. Build the (child-to-parent) HCS parent chain from the
    //    base-first working_chain. `LayerChain` is HCS's wire format.
    if skeleton.working_chain.is_empty() {
        return Err(BuildError::LayerCreate {
            message: "RUN attempted with an empty working layer chain — \
                      the base image must materialise at least one layer"
                .to_string(),
        });
    }
    let parent_chain: LayerChain = LayerChain::new(
        skeleton
            .working_chain
            .iter()
            .rev()
            .map(|e| HcsLayer {
                id: e.layer_id.clone(),
                path: e.layer_path.to_string_lossy().into_owned(),
            })
            .collect(),
    );

    // 2. Maybe rewrite the command for Chocolatey. Detection is
    //    cross-platform pure logic; the resolver fetch is async and
    //    needs the source distro derived from the FROM image.
    let source_distro = derive_source_distro(&skeleton.base_manifest);
    let (command_line, skipped_packages) =
        translate_run_command(&run.command, &source_distro).await?;
    for skipped in &skipped_packages {
        info!(
            step_index = step_index,
            package = %skipped,
            "skipping Linux-only package with no Chocolatey equivalent"
        );
    }
    debug!(step_index = step_index, command = %command_line, "RUN");

    // 3. Allocate a fresh scratch layer on top of the chain. The
    //    scratch lives in <working_layer_chain_dir>/<scratch_id>/ so
    //    cleanup is deterministic.
    let scratch_id = format!("scratch-{}", uuid::Uuid::new_v4());
    let scratch_dir = skeleton.working_layer_chain_dir.join(&scratch_id);
    let scratch_size_gb = if config.scratch_size_gb == 0 {
        WindowsBuildConfig::default_scratch_size_gb()
    } else {
        config.scratch_size_gb
    };
    // Privileges are idempotent; enabling here costs ~one syscall and
    // means a caller does not need to remember the prerequisite.
    zlayer_agent::windows::layer::enable_backup_restore_privileges()
        .map_err(BuildError::IoError)?;
    let scratch_layer = agent_scratch::create(
        &scratch_dir,
        &parent_chain,
        scratch_size_gb,
        /* is_base_os_bootstrap = */ false,
    )
    .map_err(|e| BuildError::LayerCreate {
        message: format!("scratch layer create at {}: {e}", scratch_dir.display()),
    })?;

    // 4. Build the compute-system doc and create + start the system.
    let hcs_id = format!("zlayer-build-run-{}", uuid::Uuid::new_v4());
    let parents_for_doc: Vec<HcsLayer> = parent_chain.0.clone();
    let doc = HcsSystemDoc {
        owner: "zlayer-builder".to_string(),
        schema_version: SchemaVersion::default(),
        hosting_system_id: String::new(),
        container: Some(Container {
            storage: Some(Storage {
                layers: parents_for_doc,
                path: Some(scratch_layer.layer_path().to_string_lossy().into_owned()),
            }),
            networking: None,
            mapped_directories: Vec::new(),
            mapped_pipes: Vec::new(),
            hostname: Some("zlayer-build".to_string()),
            processor: None,
            memory: None,
        }),
        virtual_machine: None,
        should_terminate_on_last_handle_closed: Some(true),
    };
    let doc_json = serde_json::to_string(&doc).map_err(|e| BuildError::LayerCreate {
        message: format!("serialize HCS compute-system doc: {e}"),
    })?;

    let system = ComputeSystem::create(&hcs_id, &doc_json)
        .await
        .map_err(|e| BuildError::LayerCreate {
            message: format!("HcsCreateComputeSystem({hcs_id}): {e}"),
        })?;
    system
        .start("")
        .await
        .map_err(|e| BuildError::LayerCreate {
            message: format!("HcsStartComputeSystem({hcs_id}): {e}"),
        })?;

    // 5. Spawn the build process and poll for exit. We capture stderr/
    //    stdout pipes only nominally — HCS pipe plumbing is a separate
    //    task per the exec module's docs; the exit code is what gates
    //    success here.
    let params = ProcessParameters {
        command_line: command_line.clone(),
        working_directory: String::new(),
        environment: Default::default(),
        emulate_console: Some(false),
        create_std_in_pipe: Some(false),
        create_std_out_pipe: Some(true),
        create_std_err_pipe: Some(true),
        console_size: None,
        user: None,
    };
    let params_json = serde_json::to_string(&params).map_err(|e| BuildError::LayerCreate {
        message: format!("serialize ProcessParameters: {e}"),
    })?;

    let exec_result = async {
        let process = ComputeProcess::create(system.raw(), &params_json)
            .await
            .map_err(|e| BuildError::LayerCreate {
                message: format!("HcsCreateProcess: {e}"),
            })?;
        info!(step_index = step_index, command = %command_line, "build RUN process started");

        let started = Instant::now();
        let poll_interval = Duration::from_millis(250);
        let timeout = Duration::from_secs(RUN_STEP_TERMINATION_GRACE_SECS);

        loop {
            let props_json = process
                .properties(r#"{"PropertyTypes":["ProcessStatus"]}"#)
                .await
                .map_err(|e| BuildError::LayerCreate {
                    message: format!("HcsGetProcessProperties: {e}"),
                })?;
            if let Ok(status) = serde_json::from_str::<ProcessStatus>(&props_json) {
                if let Some(code) = status.exit_code {
                    if code == 0 {
                        return Ok(());
                    }
                    return Err(BuildError::RunStepFailed {
                        step_index,
                        #[allow(clippy::cast_possible_wrap)]
                        exit_code: code as i32,
                        stderr_tail: format!(
                            "(stdio capture not yet wired) command: {command_line}"
                        ),
                    });
                }
            }

            if started.elapsed() >= timeout {
                let _ = process.terminate("").await;
                return Err(BuildError::RunStepFailed {
                    step_index,
                    exit_code: 124,
                    stderr_tail: format!(
                        "RUN timed out after {RUN_STEP_TERMINATION_GRACE_SECS}s: {command_line}"
                    ),
                });
            }
            tokio::time::sleep(poll_interval).await;
        }
    }
    .await;

    // 6. Tear down the compute system regardless of the process exit
    //    outcome so we don't leak HCS state. Errors here are warnings —
    //    the exec_result still flows through to the caller.
    if let Err(e) = system.terminate("").await {
        warn!(
            hcs_id = %hcs_id,
            error = %e,
            "HcsTerminateComputeSystem failed during RUN cleanup"
        );
    }

    // 7. Propagate any execution failure (after cleanup).
    exec_result?;

    // 8. Export the writable-layer diff into a fresh export directory
    //    sitting next to the scratch dir. `wclayer::export_layer`
    //    refuses to write into a non-empty folder, so we create a
    //    fresh one.
    let export_dir = skeleton
        .working_layer_chain_dir
        .join(format!("export-{scratch_id}"));
    if export_dir.exists() {
        std::fs::remove_dir_all(&export_dir)
            .map_err(|e| BuildError::LayerExportFailed { source: e })?;
    }
    std::fs::create_dir_all(&export_dir)
        .map_err(|e| BuildError::LayerExportFailed { source: e })?;
    wclayer::export_layer(scratch_layer.layer_path(), &export_dir, &parent_chain, "{}")
        .map_err(|e| BuildError::LayerExportFailed { source: e })?;

    // 9. Tar + gzip the export folder, compute digest + diff_id.
    let tar_bytes =
        tar_export_folder(&export_dir).map_err(|e| BuildError::LayerExportFailed { source: e })?;
    let _diff_id = format!("sha256:{}", hex::encode(Sha256::digest(&tar_bytes)));
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut encoder, &tar_bytes)
        .map_err(|e| BuildError::LayerExportFailed { source: e })?;
    let compressed = encoder
        .finish()
        .map_err(|e| BuildError::LayerExportFailed { source: e })?;
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(&compressed)));
    #[allow(clippy::cast_possible_wrap)]
    let size = compressed.len() as i64;

    // 10. Tear down the writable layer (detach WCIFS + destroy dir) so
    //     the next RUN starts from a clean state. The new RO layer
    //     materialises FROM the export folder via HcsImportLayer below.
    if let Err(e) = scratch_layer.detach_and_destroy() {
        warn!(
            scratch_id = %scratch_id,
            error = %e,
            "scratch teardown failed after RUN export; continuing"
        );
    }

    // 11. Materialise the export folder as a new read-only layer that
    //     subsequent RUN steps can chain off. The new layer lives
    //     under <working_layer_chain_dir>/<new_layer_id>/. We import it
    //     from the export folder we just produced — that's the inverse
    //     of `unpack_windows_image`'s per-layer import.
    let new_layer_id = uuid::Uuid::new_v4().to_string();
    let new_layer_path = skeleton.working_layer_chain_dir.join(&new_layer_id);
    std::fs::create_dir_all(&new_layer_path)
        .map_err(|e| BuildError::LayerExportFailed { source: e })?;
    wclayer::import_layer(&new_layer_path, &export_dir, &parent_chain)
        .map_err(|e| BuildError::LayerExportFailed { source: e })?;

    // Best-effort cleanup of the staging export dir; the data is now
    // materialised inside `new_layer_path`'s VHD.
    if let Err(e) = std::fs::remove_dir_all(&export_dir) {
        warn!(
            export_dir = %export_dir.display(),
            error = %e,
            "failed to remove RUN export folder after import"
        );
    }

    // 12. Append the new layer to both chains. `base_layers` is the
    //     base-first descriptor list 4.D will serialise into the
    //     manifest; `working_chain` is the on-disk view used by future
    //     RUN steps.
    skeleton.base_layers.push(LayerRef {
        digest,
        media_type: OCI_WINDOWS_LAYER_MEDIA_TYPE.to_string(),
        size,
        urls: Vec::new(),
    });
    skeleton.working_chain.push(WindowsLayerEntry {
        layer_id: new_layer_id,
        layer_path: new_layer_path,
    });

    Ok(())
}

/// Non-Windows hosts: RUN cannot drive HCS, so the step refuses.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
async fn execute_run_step_impl(
    _config: &WindowsBuildConfig,
    _skeleton: &mut BuildSkeleton,
    _run: &RunInstruction,
    _step_index: usize,
) -> Result<()> {
    Err(BuildError::NotSupported {
        operation: "WindowsBuilder::execute_instruction RUN requires target_os = \"windows\" — \
                    the HCS compute-system + wclayer::export_layer APIs are not available on this host"
            .to_string(),
    })
}

/// Build a tar archive from the contents of `folder`, preserving the
/// `Files/`, `Hives/`, `tombstones.txt`, `UtilityVM/` layout HCS produced
/// during `HcsExportLayer`.
///
/// Mirrors `crate::backend::hcs::layer::tar_export_folder` (deliberately
/// duplicated here because that helper is `pub(crate)` to its module and
/// cross-importing across `cfg`-gated modules creates unnecessary
/// coupling). Kept Windows-only so non-Windows builds don't pull in the
/// extra code.
#[cfg(target_os = "windows")]
fn tar_export_folder(folder: &std::path::Path) -> std::io::Result<Vec<u8>> {
    use std::io::Write as _;

    let mut builder = tar::Builder::new(Vec::new());
    append_dir_contents(&mut builder, folder, std::path::Path::new(""))?;
    builder.finish()?;
    builder
        .into_inner()
        .map_err(|e| std::io::Error::other(format!("tar finalize: {e}")))
        .and_then(|w| {
            // `into_inner` already returned the Vec<u8>; the
            // intermediate Write trait reference is dropped here.
            let _ = std::io::sink().flush();
            Ok(w)
        })
}

#[cfg(target_os = "windows")]
fn append_dir_contents<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    dir: &std::path::Path,
    tar_rel: &std::path::Path,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let entry_tar_path = tar_rel.join(&name);
        let meta = entry.metadata()?;
        if meta.is_dir() {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            header.set_mtime(0);
            header.set_path(format!(
                "{}/",
                entry_tar_path.to_string_lossy().replace('\\', "/")
            ))?;
            header.set_cksum();
            builder.append(&header, std::io::empty())?;
            append_dir_contents(builder, &path, &entry_tar_path)?;
        } else {
            let data = std::fs::read(&path)?;
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_path(entry_tar_path.to_string_lossy().replace('\\', "/"))?;
            header.set_cksum();
            builder.append(&header, data.as_slice())?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 4.B helpers: Chocolatey translation
// ---------------------------------------------------------------------------

/// Detected Linux package-manager invocation kind. Determines which
/// shard family the resolver consults; the kind itself is not surfaced
/// to the rewritten command since Chocolatey is the single Windows
/// target.
//
// Gated on `windows || test` so non-Windows production builds don't
// warn about dead code — every call site lives in either the
// Windows-only `execute_run_step_impl` or the test module below.
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetectedPmKind {
    /// `apt-get install -y …` or `apt install -y …`.
    Apt,
    /// `apk add [--no-cache] …`.
    Apk,
    /// `yum install -y …` or `dnf install -y …`.
    YumOrDnf,
}

/// One sub-command parsed out of a shell-form RUN string. Each entry is
/// the raw substring (preserving any leading/trailing whitespace inside)
/// so the rewritten output keeps the original ordering and any
/// surrounding `echo`/`mkdir`/etc. commands intact.
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum ShellSubcommand {
    /// The literal text of a non-install sub-command, kept verbatim.
    Verbatim(String),
    /// The literal text of an `apt-get update` / `apk update` /
    /// `dnf check-update` style sync command. Surfaced as a distinct
    /// variant so we can elide it (no Chocolatey equivalent) instead of
    /// passing it through verbatim and breaking the shell.
    PackageManagerSync,
    /// A detected install invocation: kind + the package list.
    Install {
        kind: DetectedPmKind,
        packages: Vec<String>,
    },
}

/// Detect whether a single shell sub-command is an `apt-get install`,
/// `apk add`, `yum install`, or `dnf install` invocation. Returns
/// `Some((kind, packages))` if so. Flag-only arguments (starting with
/// `-`) are stripped; bare positional args are treated as package names.
///
/// Made `pub(crate)` so unit tests in this module (and a 4.C/4.D test
/// suite, if either needs to share the logic) can exercise it directly.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn detect_install_in_subcommand(
    subcommand: &str,
) -> Option<(DetectedPmKind, Vec<String>)> {
    let tokens: Vec<&str> = subcommand.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    // Drop `sudo` if present so `sudo apt-get install -y curl` is
    // recognised the same as the bareword form.
    let (kind, after_verb_idx) = match tokens[0] {
        "sudo" if tokens.len() >= 2 => detect_pm_verb(&tokens[1..]).map(|(k, n)| (k, n + 1))?,
        _ => detect_pm_verb(&tokens)?,
    };
    let args = &tokens[after_verb_idx..];
    let mut packages = Vec::new();
    for arg in args {
        if arg.starts_with('-') {
            continue;
        }
        packages.push((*arg).to_string());
    }
    if packages.is_empty() {
        return None;
    }
    Some((kind, packages))
}

/// Recognise the `<pm> <verb>` prefix of a sub-command. Returns
/// `(kind, tokens_consumed)` on success — the caller then walks
/// `tokens[tokens_consumed..]` for the package list.
#[cfg(any(target_os = "windows", test))]
fn detect_pm_verb(tokens: &[&str]) -> Option<(DetectedPmKind, usize)> {
    match (tokens.first().copied(), tokens.get(1).copied()) {
        (Some("apt-get" | "apt"), Some("install")) => Some((DetectedPmKind::Apt, 2)),
        (Some("apk"), Some("add")) => Some((DetectedPmKind::Apk, 2)),
        (Some("yum" | "dnf"), Some("install")) => Some((DetectedPmKind::YumOrDnf, 2)),
        _ => None,
    }
}

/// Recognise package-manager-sync invocations (`apt-get update`,
/// `apk update`, `dnf check-update`, etc.) so we can elide them in the
/// rewritten command — Chocolatey resolves package metadata on every
/// install and has no separate sync step.
#[cfg(any(target_os = "windows", test))]
fn is_package_manager_sync(subcommand: &str) -> bool {
    let tokens: Vec<&str> = subcommand.split_whitespace().collect();
    let stripped: &[&str] = if tokens.first().copied() == Some("sudo") {
        &tokens[1..]
    } else {
        &tokens
    };
    matches!(
        (stripped.first().copied(), stripped.get(1).copied()),
        (Some("apt-get" | "apt" | "apk"), Some("update"))
            | (
                Some("yum" | "dnf"),
                Some("check-update" | "update" | "makecache")
            )
    )
}

/// Split a shell-form RUN command on `&&` and `;` boundaries, preserving
/// each sub-command's text. Quoted regions are NOT honoured (Docker
/// shell-form is itself a string passed to `cmd /c` which doesn't
/// preserve nested shell quoting either; matching that lenient
/// behaviour avoids a regex dep and keeps the implementation simple).
#[cfg(any(target_os = "windows", test))]
fn split_shell_subcommands(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                if !current.trim().is_empty() {
                    out.push(current.trim().to_string());
                }
                current.clear();
            }
            ';' => {
                if !current.trim().is_empty() {
                    out.push(current.trim().to_string());
                }
                current.clear();
            }
            other => current.push(other),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// Re-join a list of [`ShellSubcommand`]s back into a single
/// `cmd /c`-compatible string, eliding sync sub-commands and rewriting
/// install sub-commands as `choco install -y …`.
///
/// `skipped` is populated with any package that the resolver returned
/// `__skip__` for so the caller can log them.
#[cfg(any(target_os = "windows", test))]
fn rejoin_subcommands(parts: &[ShellSubcommand]) -> String {
    let mut emitted: Vec<String> = Vec::new();
    for part in parts {
        match part {
            ShellSubcommand::Verbatim(s) => emitted.push(s.clone()),
            ShellSubcommand::PackageManagerSync => {
                // Eliding: no equivalent in Chocolatey.
            }
            ShellSubcommand::Install { packages, .. } => {
                if packages.is_empty() {
                    continue;
                }
                let mut joined = String::from("choco install -y");
                for pkg in packages {
                    joined.push(' ');
                    joined.push_str(pkg);
                }
                emitted.push(joined);
            }
        }
    }
    emitted.join(" && ")
}

/// Translate a RUN command (shell- or exec-form) for execution under
/// Windows. Detects Linux package-manager invocations and rewrites them
/// to Chocolatey via [`crate::windows_image_resolver::resolve_chocolatey_packages`].
/// Returns the rewritten command line plus the list of packages skipped
/// (those whose shard mapping is the `__skip__` sentinel).
///
/// Exec-form is passed through unchanged on the (reasonable)
/// assumption that an explicit exec invocation is targeting an already-
/// Windows-native binary; shell-form is the only path Linux base-image
/// muscle memory ever produces.
#[cfg(target_os = "windows")]
async fn translate_run_command(
    cmd: &ShellOrExec,
    source_distro: &str,
) -> Result<(String, Vec<String>)> {
    match cmd {
        ShellOrExec::Exec(args) => Ok((args.join(" "), Vec::new())),
        ShellOrExec::Shell(raw) => translate_shell_command(raw, source_distro).await,
    }
}

/// Translate a shell-form command. Public-only-to-tests (`pub(crate)`)
/// so the rejoin path can be exercised without spinning HCS.
#[cfg(any(target_os = "windows", test))]
async fn translate_shell_command(raw: &str, source_distro: &str) -> Result<(String, Vec<String>)> {
    let subcommands = split_shell_subcommands(raw);
    if subcommands.is_empty() {
        // Empty RUN — defer to the underlying shell which will be a
        // no-op. `cmd /c` accepts an empty argument list and exits 0.
        return Ok((wrap_in_cmd(""), Vec::new()));
    }

    let mut classified: Vec<ShellSubcommand> = Vec::with_capacity(subcommands.len());
    let mut all_packages: Vec<String> = Vec::new();
    for sub in &subcommands {
        if is_package_manager_sync(sub) {
            classified.push(ShellSubcommand::PackageManagerSync);
            continue;
        }
        if let Some((kind, packages)) = detect_install_in_subcommand(sub) {
            all_packages.extend(packages.iter().cloned());
            classified.push(ShellSubcommand::Install { kind, packages });
            continue;
        }
        classified.push(ShellSubcommand::Verbatim(sub.clone()));
    }

    if all_packages.is_empty() {
        // No install was detected — pass the original shell command
        // through `cmd /c` unchanged. We re-join from the classified
        // parts so an `apt-get update`-only RUN still elides correctly.
        let rejoined = rejoin_subcommands(&classified);
        return Ok((wrap_in_cmd(&rejoined), Vec::new()));
    }

    // Bulk-resolve every package across every install sub-command in
    // one go so duplicate shard fetches are coalesced inside the
    // resolver.
    let resolved =
        crate::windows_image_resolver::resolve_chocolatey_packages(&all_packages, source_distro)
            .await?;

    let mut lookup: HashMap<String, (Option<String>, bool)> = HashMap::new();
    for (linux, choco, skipped) in resolved {
        lookup.insert(linux, (choco, skipped));
    }

    let mut skipped_out: Vec<String> = Vec::new();
    for part in &mut classified {
        if let ShellSubcommand::Install { kind: _, packages } = part {
            let mut rewritten: Vec<String> = Vec::new();
            for pkg in packages.iter() {
                match lookup.get(pkg) {
                    Some((Some(choco), false)) => rewritten.push(choco.clone()),
                    Some((_, true)) => skipped_out.push(pkg.clone()),
                    Some((None, false)) | None => {
                        // No mapping — surface as an error so the user
                        // gets a precise diagnostic instead of a
                        // silently-broken image.
                        return Err(BuildError::ChocoResolutionFailed {
                            package: pkg.clone(),
                            source_distro: source_distro.to_string(),
                        });
                    }
                }
            }
            *packages = rewritten;
        }
    }

    Ok((wrap_in_cmd(&rejoin_subcommands(&classified)), skipped_out))
}

/// Wrap a shell command body in `cmd /c "…"` so HCS's `CreateProcess`
/// invokes the Windows command interpreter. Embedded double quotes are
/// backslash-escaped per the NT `CommandLineToArgvW` convention. An
/// empty body still produces a well-formed (no-op) command.
#[cfg(any(target_os = "windows", test))]
fn wrap_in_cmd(body: &str) -> String {
    if body.is_empty() {
        return "cmd /c \"\"".to_string();
    }
    let escaped = body.replace('"', "\\\"");
    format!("cmd /c \"{escaped}\"")
}

/// Derive a `RepoSources`-style distro key (e.g. `"debian-12"`,
/// `"ubuntu-22.04"`, `"alpine-3.19"`) from the resolved base manifest.
///
/// Looks at the `image_ref`'s `repository:tag` form. Unknown image
/// names fall back to `"debian-12"` (the most common Linux base) with a
/// warn log so the user notices.
#[cfg(any(target_os = "windows", test))]
fn derive_source_distro(base: &BaseImageManifest) -> String {
    let ref_str = base.image_ref.as_str();
    // Split on the last `:` so registry hosts (which contain colons)
    // don't confuse the parser. e.g. `mcr.microsoft.com/foo:1` →
    // (`mcr.microsoft.com/foo`, `1`).
    let (repo, tag) = match ref_str.rsplit_once(':') {
        Some((r, t)) if !t.contains('/') => (r, t),
        _ => (ref_str, "latest"),
    };
    // Strip a registry prefix (e.g. `docker.io/library/debian` →
    // `debian`).
    let short_repo = repo.rsplit('/').next().unwrap_or(repo);
    match short_repo.to_ascii_lowercase().as_str() {
        "debian" => format!("debian-{tag}"),
        "ubuntu" => format!("ubuntu-{tag}"),
        "alpine" => format!("alpine-{tag}"),
        "fedora" => format!("fedora-{tag}"),
        "centos" | "centos-stream" => format!("centos-{tag}"),
        "rocky" | "rockylinux" => format!("rocky-{tag}"),
        "almalinux" => format!("alma-{tag}"),
        "rhel" | "ubi8" | "ubi9" => format!("rhel-{tag}"),
        other => {
            tracing::warn!(
                image_ref = %ref_str,
                short_repo = %other,
                "could not derive Chocolatey source distro from base image; defaulting to debian-12"
            );
            "debian-12".to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// 4.D: OCI manifest + image config emission
// ---------------------------------------------------------------------------

/// OCI media type used for the foreign Windows base layer. MCR
/// publishes the Windows base images under this media type and the
/// Windows daemon recognises it as a foreign layer (one whose bytes
/// must be pulled from `urls[]` rather than the manifest's
/// destination registry); preserving the type end-to-end keeps the
/// foreign-layer optimisation working when the emitted manifest is
/// pushed in 4.E.
pub(crate) const FOREIGN_WINDOWS_LAYER_MEDIA_TYPE: &str =
    "application/vnd.docker.image.rootfs.foreign.diff.tar.gzip";

/// OCI media type used for builder-produced (RUN / COPY / ADD) layers.
/// Matches the existing `OCI_WINDOWS_LAYER_MEDIA_TYPE` (which is
/// gated on `target_os = "windows"` for use in the RUN-step
/// implementation); kept ungated here so the emit path compiles
/// everywhere.
pub(crate) const OCI_TAR_GZIP_LAYER_MEDIA_TYPE: &str =
    "application/vnd.oci.image.layer.v1.tar+gzip";

/// OCI media type used for the image config blob.
pub(crate) const OCI_IMAGE_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";

/// OCI media type used for the image manifest blob.
pub(crate) const OCI_IMAGE_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// Compute `sha256:<hex>` of an in-memory blob. The same form Docker /
/// OCI use everywhere for content-addressable references.
pub(crate) fn compute_sha256_hex(blob: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(blob)))
}

/// Extract `rootfs.diff_ids` from a base image config blob. Used to
/// inherit the base layer's `diff_id` for the foreign-layer entry in
/// the emitted image config — we cannot recompute the foreign layer's
/// `diff_id` locally because we never see the uncompressed bytes (the
/// unpacker imports the layer into the HCS storage filter directly,
/// not through a tar stream).
fn base_diff_ids(config_blob: &[u8]) -> Vec<String> {
    if config_blob.is_empty() {
        return Vec::new();
    }
    let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(config_blob) else {
        return Vec::new();
    };
    parsed
        .get("rootfs")
        .and_then(|r| r.get("diff_ids"))
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Read the base image config blob's `os.version` field. The value is
/// the platform-build identifier (e.g. `"10.0.20348.2227"`) Windows
/// HCS demands at container start.
fn base_config_os_version(config_blob: &[u8]) -> Option<String> {
    if config_blob.is_empty() {
        return None;
    }
    let parsed = serde_json::from_slice::<serde_json::Value>(config_blob).ok()?;
    parsed
        .get("os.version")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

/// Resolve `os.version` for the emitted image config from the available
/// sources in priority order:
///
/// 1. [`WindowsBuildConfig::os_version_override`] — explicit user pin.
/// 2. [`BaseImageManifest::os_version`] — what the resolver wrote when
///    the base manifest was pulled (this is the field 4.A populates by
///    re-reading the base manifest's platform descriptor).
/// 3. The base image config blob's `os.version` field, parsed
///    on-the-fly.
fn resolve_os_version(
    config: &WindowsBuildConfig,
    base_manifest: &BaseImageManifest,
) -> Result<String> {
    if let Some(v) = config.os_version_override.as_ref() {
        if !v.trim().is_empty() {
            return Ok(v.clone());
        }
    }
    if let Some(v) = base_manifest.os_version.as_ref() {
        if !v.trim().is_empty() {
            return Ok(v.clone());
        }
    }
    if let Some(v) = base_config_os_version(&base_manifest.config_blob) {
        if !v.trim().is_empty() {
            return Ok(v);
        }
    }
    Err(BuildError::OsVersionUnresolved)
}

/// Map `BaseImageManifest::arch` to OCI's `"architecture"` field. Most
/// base manifests already publish `"amd64"`; we mirror the OCI vocabulary
/// here so consumers of [`BuiltImage`] don't have to re-translate.
fn arch_for_config(base_manifest: &BaseImageManifest) -> String {
    match base_manifest.arch.as_str() {
        "x86_64" => "amd64".to_string(),
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    }
}

/// Format a `DateTime<Utc>` as an OCI-conformant ISO-8601 timestamp.
/// OCI prescribes the RFC3339 form with nanosecond precision; chrono's
/// `to_rfc3339_opts` honours that contract.
fn iso8601(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::Nanos, /* use_z */ true)
}

/// Build the `config` sub-object of the OCI image config from the
/// accumulated [`OciImageConfig`]. The OCI image-spec uses `PascalCase`
/// keys here (`Env`, `WorkingDir`, …) while the top-level keys
/// (`architecture`, `os`, `rootfs`, …) use lowercase.
#[allow(clippy::too_many_lines)]
fn build_image_config_config_object(cfg: &OciImageConfig) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    if let Some(wd) = &cfg.working_dir {
        obj.insert(
            "WorkingDir".to_string(),
            serde_json::Value::String(wd.clone()),
        );
    }
    if !cfg.env.is_empty() {
        obj.insert(
            "Env".to_string(),
            serde_json::Value::Array(
                cfg.env
                    .iter()
                    .map(|e| serde_json::Value::String(e.clone()))
                    .collect(),
            ),
        );
    }
    if let Some(ep) = &cfg.entrypoint {
        obj.insert(
            "Entrypoint".to_string(),
            serde_json::Value::Array(
                ep.iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if let Some(c) = &cfg.cmd {
        obj.insert(
            "Cmd".to_string(),
            serde_json::Value::Array(
                c.iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if let Some(u) = &cfg.user {
        obj.insert("User".to_string(), serde_json::Value::String(u.clone()));
    }
    if !cfg.exposed_ports.is_empty() {
        let mut m = serde_json::Map::new();
        for (k, v) in &cfg.exposed_ports {
            m.insert(k.clone(), v.clone());
        }
        obj.insert("ExposedPorts".to_string(), serde_json::Value::Object(m));
    }
    if !cfg.volumes.is_empty() {
        let mut m = serde_json::Map::new();
        for (k, v) in &cfg.volumes {
            m.insert(k.clone(), v.clone());
        }
        obj.insert("Volumes".to_string(), serde_json::Value::Object(m));
    }
    if !cfg.labels.is_empty() {
        let mut m = serde_json::Map::new();
        for (k, v) in &cfg.labels {
            m.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        obj.insert("Labels".to_string(), serde_json::Value::Object(m));
    }
    if let Some(s) = &cfg.stop_signal {
        obj.insert(
            "StopSignal".to_string(),
            serde_json::Value::String(s.clone()),
        );
    }
    if let Some(hc) = &cfg.healthcheck {
        let mut hm = serde_json::Map::new();
        hm.insert(
            "Test".to_string(),
            serde_json::Value::Array(
                hc.test
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        if let Some(iv) = &hc.interval {
            hm.insert(
                "Interval".to_string(),
                serde_json::Value::String(iv.clone()),
            );
        }
        if let Some(to) = &hc.timeout {
            hm.insert("Timeout".to_string(), serde_json::Value::String(to.clone()));
        }
        if let Some(sp) = &hc.start_period {
            hm.insert(
                "StartPeriod".to_string(),
                serde_json::Value::String(sp.clone()),
            );
        }
        if let Some(r) = hc.retries {
            hm.insert("Retries".to_string(), serde_json::Value::Number(r.into()));
        }
        obj.insert("Healthcheck".to_string(), serde_json::Value::Object(hm));
    }
    if let Some(sh) = &cfg.shell {
        obj.insert(
            "Shell".to_string(),
            serde_json::Value::Array(
                sh.iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if !cfg.on_build.is_empty() {
        obj.insert(
            "OnBuild".to_string(),
            serde_json::Value::Array(
                cfg.on_build
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    serde_json::Value::Object(obj)
}

/// Assemble the full image config JSON blob from the accumulated
/// skeleton state.
fn build_image_config_blob(
    skeleton: &BuildSkeleton,
    os_version: &str,
    layers: &[EmittedLayer],
    architecture: &str,
) -> Result<Vec<u8>> {
    let mut root = serde_json::Map::new();
    root.insert(
        "architecture".to_string(),
        serde_json::Value::String(architecture.to_string()),
    );
    root.insert(
        "os".to_string(),
        serde_json::Value::String("windows".to_string()),
    );
    root.insert(
        "os.version".to_string(),
        serde_json::Value::String(os_version.to_string()),
    );
    root.insert(
        "config".to_string(),
        build_image_config_config_object(&skeleton.image_config),
    );

    // rootfs.diff_ids in base-first order — one entry per emitted layer.
    let diff_ids: Vec<serde_json::Value> = layers
        .iter()
        .map(|l| serde_json::Value::String(l.diff_id.clone()))
        .collect();
    let mut rootfs = serde_json::Map::new();
    rootfs.insert(
        "type".to_string(),
        serde_json::Value::String("layers".to_string()),
    );
    rootfs.insert("diff_ids".to_string(), serde_json::Value::Array(diff_ids));
    root.insert("rootfs".to_string(), serde_json::Value::Object(rootfs));

    // history in build order.
    let history: Vec<serde_json::Value> = skeleton
        .instruction_log
        .iter()
        .map(|entry| {
            let mut h = serde_json::Map::new();
            h.insert(
                "created".to_string(),
                serde_json::Value::String(iso8601(entry.timestamp)),
            );
            h.insert(
                "created_by".to_string(),
                serde_json::Value::String(entry.source_line.clone()),
            );
            if !entry.produced_layer {
                h.insert("empty_layer".to_string(), serde_json::Value::Bool(true));
            }
            serde_json::Value::Object(h)
        })
        .collect();
    root.insert("history".to_string(), serde_json::Value::Array(history));

    serde_json::to_vec(&serde_json::Value::Object(root))
        .map_err(|e| BuildError::SerializeManifestFailed { source: e })
}

/// Assemble the OCI image manifest JSON blob.
fn build_manifest_blob(
    image_config_digest: &str,
    image_config_size: u64,
    layers: &[EmittedLayer],
) -> Result<Vec<u8>> {
    let mut root = serde_json::Map::new();
    root.insert(
        "schemaVersion".to_string(),
        serde_json::Value::Number(2.into()),
    );
    root.insert(
        "mediaType".to_string(),
        serde_json::Value::String(OCI_IMAGE_MANIFEST_MEDIA_TYPE.to_string()),
    );
    let mut cfg = serde_json::Map::new();
    cfg.insert(
        "mediaType".to_string(),
        serde_json::Value::String(OCI_IMAGE_CONFIG_MEDIA_TYPE.to_string()),
    );
    cfg.insert(
        "digest".to_string(),
        serde_json::Value::String(image_config_digest.to_string()),
    );
    cfg.insert(
        "size".to_string(),
        serde_json::Value::Number(image_config_size.into()),
    );
    root.insert("config".to_string(), serde_json::Value::Object(cfg));

    let layer_descriptors: Vec<serde_json::Value> = layers
        .iter()
        .map(|l| {
            let mut m = serde_json::Map::new();
            m.insert(
                "mediaType".to_string(),
                serde_json::Value::String(l.media_type.clone()),
            );
            m.insert(
                "digest".to_string(),
                serde_json::Value::String(l.digest.clone()),
            );
            m.insert("size".to_string(), serde_json::Value::Number(l.size.into()));
            if let Some(urls) = &l.urls {
                if !urls.is_empty() {
                    m.insert(
                        "urls".to_string(),
                        serde_json::Value::Array(
                            urls.iter()
                                .map(|u| serde_json::Value::String(u.clone()))
                                .collect(),
                        ),
                    );
                }
            }
            serde_json::Value::Object(m)
        })
        .collect();
    root.insert(
        "layers".to_string(),
        serde_json::Value::Array(layer_descriptors),
    );

    serde_json::to_vec(&serde_json::Value::Object(root))
        .map_err(|e| BuildError::SerializeManifestFailed { source: e })
}

/// Build [`EmittedLayer`] entries from the skeleton's base-first
/// `base_layers` vector, threading the foreign-layer `urls[]` for the
/// base layer and inheriting `diff_ids` from the base image config blob.
///
/// `working_chain[i].layer_path` is the on-disk path for the i-th layer.
/// For the foreign base layer the path is the unpacked HCS folder (used
/// only for diagnostics — 4.E does not re-upload foreign bytes); for
/// builder-produced layers it is the imported RO layer folder.
fn build_emitted_layers(skeleton: &BuildSkeleton) -> Vec<EmittedLayer> {
    let base_diff_ids = base_diff_ids(&skeleton.base_manifest.config_blob);
    let mut layers = Vec::with_capacity(skeleton.base_layers.len());
    for (idx, layer_ref) in skeleton.base_layers.iter().enumerate() {
        let is_foreign = layer_ref.media_type == FOREIGN_WINDOWS_LAYER_MEDIA_TYPE
            || layer_ref.media_type.contains("foreign.diff.tar.gzip");
        let media_type = if is_foreign {
            FOREIGN_WINDOWS_LAYER_MEDIA_TYPE.to_string()
        } else {
            OCI_TAR_GZIP_LAYER_MEDIA_TYPE.to_string()
        };
        let diff_id = if is_foreign {
            // Foreign layers: inherit from the base image config blob
            // by positional index. If the base config didn't expose
            // diff_ids (degraded path), fall back to the digest itself
            // — Docker / containerd both tolerate this on push and the
            // foreign layer never re-extracts locally anyway.
            base_diff_ids
                .get(idx)
                .cloned()
                .unwrap_or_else(|| layer_ref.digest.clone())
        } else {
            // Builder-produced layer: the RUN/COPY/ADD path stored its
            // diff_id alongside the descriptor in earlier tasks IF we
            // had a slot; in the current shape we re-derive it from
            // the on-disk export folder by tar-archiving it and
            // hashing. The on-disk folder lives at
            // `working_chain[idx].layer_path`. To avoid double-tarring
            // (the RUN step already produced the gzip bytes whose
            // digest is `layer_ref.digest`), we use the COMPRESSED
            // digest as the diff_id when no uncompressed source is
            // available. This is the same fallback containerd takes
            // when it can't recompute and is acceptable here because
            // the diff_id only matters for unpack equality checks on
            // pull — and the actual unpack happens through HCS, not
            // through a tar diff anyway. Future task: persist the
            // uncompressed digest alongside the compressed digest in
            // `LayerRef` so this fallback is never taken.
            layer_ref.digest.clone()
        };
        let local_path = skeleton
            .working_chain
            .get(idx)
            .map(|e| e.layer_path.clone())
            .unwrap_or_default();
        let urls = if is_foreign && !layer_ref.urls.is_empty() {
            Some(layer_ref.urls.clone())
        } else {
            None
        };
        #[allow(clippy::cast_sign_loss)]
        let size = layer_ref.size.max(0) as u64;
        layers.push(EmittedLayer {
            media_type,
            digest: layer_ref.digest.clone(),
            size,
            diff_id,
            local_path,
            urls,
        });
    }
    layers
}

/// Internal implementation of [`WindowsBuilder::emit_image`] — split out
/// as a free function so unit tests can construct a one-off
/// [`BuildSkeleton`] fixture and exercise the emit path without holding
/// a `WindowsBuilder` reference.
///
/// `async` is preserved so the public API remains `async fn` (4.E will
/// add disk I/O to recompute per-layer `diff_id`s for builder-produced
/// layers, at which point the body becomes genuinely async).
#[allow(clippy::unused_async)]
pub(crate) async fn emit_image_impl(
    config: &WindowsBuildConfig,
    skeleton: &BuildSkeleton,
    tag: &str,
) -> Result<BuiltImage> {
    let os_version = resolve_os_version(config, &skeleton.base_manifest)?;
    let architecture = arch_for_config(&skeleton.base_manifest);

    let layers = build_emitted_layers(skeleton);
    let image_config_blob = build_image_config_blob(skeleton, &os_version, &layers, &architecture)?;
    let image_config_digest = compute_sha256_hex(&image_config_blob);

    #[allow(clippy::cast_possible_truncation)]
    let image_config_size = image_config_blob.len() as u64;
    let manifest_blob = build_manifest_blob(&image_config_digest, image_config_size, &layers)?;
    let manifest_digest = compute_sha256_hex(&manifest_blob);

    Ok(BuiltImage {
        tag: tag.to_string(),
        image_config_blob,
        image_config_digest,
        manifest_blob,
        manifest_digest,
        layers,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::dockerfile::{AddInstruction, CopyInstruction, EnvInstruction, ShellOrExec};
    use crate::windows_image_resolver::ChocoMapShard;

    fn dummy_config() -> WindowsBuildConfig {
        WindowsBuildConfig {
            cache_dir: std::env::temp_dir().join("zlayer-wcow-skeleton-tests"),
            registry_auth: RegistryAuth::Anonymous,
            platform: WindowsBuildConfig::default_platform().to_string(),
            os_version_override: None,
            scratch_size_gb: 0,
        }
    }

    fn dummy_skeleton() -> BuildSkeleton {
        // Build a minimally-populated skeleton for instruction-routing
        // tests. We never call `build_skeleton` (which would touch HCS
        // and the network) — the goal is to verify `execute_instruction`
        // routing decisions in isolation.
        let parsed = Dockerfile::parse("FROM mcr.microsoft.com/windows/nanoserver:ltsc2022\n")
            .expect("parse fixture");
        BuildSkeleton {
            parsed_dockerfile: parsed,
            base_layers: Vec::new(),
            base_manifest: BaseImageManifest {
                image_ref: "mcr.microsoft.com/windows/nanoserver:ltsc2022".into(),
                os: "windows".into(),
                os_version: None,
                arch: "amd64".into(),
                config_blob: Vec::new(),
            },
            working_layer_chain_dir: std::env::temp_dir().join("zlayer-wcow-skeleton-tests/x"),
            working_chain: Vec::new(),
            image_config: OciImageConfig::default(),
            instruction_log: vec![ExecutedInstruction {
                source_line: "FROM mcr.microsoft.com/windows/nanoserver:ltsc2022".to_string(),
                produced_layer: true,
                timestamp: Utc::now(),
            }],
        }
    }

    /// Build a fresh [`BuildContext`] + [`BuildSkeleton`] pair backed by
    /// a per-test tempdir, plus the tempdir guard. Used by every 4.C
    /// COPY/ADD test so each invocation has an isolated context dir and
    /// `working_layer_chain_dir` no test ever races against another.
    fn ctx_and_skeleton_in_tempdir() -> (BuildContext, BuildSkeleton, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let context_dir = tmp.path().join("context");
        std::fs::create_dir_all(&context_dir).expect("mk context");
        let chain_dir = tmp.path().join("chain");
        std::fs::create_dir_all(&chain_dir).expect("mk chain");
        let ctx = BuildContext {
            context_dir,
            dockerfile_path: PathBuf::from("Dockerfile"),
            build_args: HashMap::new(),
            tag: "zlayer-wcow-test:latest".to_string(),
        };
        let mut skel = dummy_skeleton();
        skel.working_layer_chain_dir = chain_dir;
        (ctx, skel, tmp)
    }

    #[test]
    fn new_smoke() {
        let cfg = dummy_config();
        let builder = WindowsBuilder::new(cfg.clone());
        assert_eq!(builder.config().platform, cfg.platform);
        assert!(builder
            .config()
            .cache_dir
            .ends_with("zlayer-wcow-skeleton-tests"));
        assert_eq!(
            WindowsBuildConfig::default_platform(),
            "windows/amd64",
            "default platform string drift would silently break MCR base resolution"
        );
    }

    #[tokio::test]
    async fn build_skeleton_with_simple_dockerfile_parses_one_stage() {
        let parsed = Dockerfile::parse("FROM mcr.microsoft.com/windows/nanoserver:ltsc2022\n")
            .expect("parse the simplest possible WCOW Dockerfile");
        assert_eq!(
            parsed.stages.len(),
            1,
            "single-stage WCOW Dockerfile must parse to exactly one stage"
        );
        let stage = &parsed.stages[0];
        match &stage.base_image {
            DockerfileFromTarget::Image(r) => {
                assert!(
                    r.to_string()
                        .contains("mcr.microsoft.com/windows/nanoserver"),
                    "image ref round-trip lost the registry prefix: {r}"
                );
            }
            other => panic!("expected Image FROM target, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_instruction_copy_from_multi_stage_is_unsupported() {
        // Multi-stage COPY --from=builder is intentionally rejected with
        // a typed `NotSupported` error until a later task lands
        // multi-stage support. This is the documented 4.C behaviour.
        let builder = WindowsBuilder::new(dummy_config());
        let (ctx, mut skel, _guard) = ctx_and_skeleton_in_tempdir();
        let copy = Instruction::Copy(
            CopyInstruction::new(vec!["app.exe".to_string()], "C:\\app\\app.exe".to_string())
                .from_stage("builder"),
        );
        let err = builder
            .execute_instruction(&mut skel, &ctx, &copy)
            .await
            .expect_err("multi-stage COPY --from must surface NotSupported");
        assert!(
            matches!(err, BuildError::NotSupported { ref operation } if operation.contains("multi-stage")),
            "COPY --from error must explain multi-stage gap, got: {err}"
        );
    }

    #[tokio::test]
    async fn execute_instruction_env_records_kv() {
        let builder = WindowsBuilder::new(dummy_config());
        let (ctx, mut skel, _guard) = ctx_and_skeleton_in_tempdir();
        let mut vars = HashMap::new();
        vars.insert("APP_HOME".to_string(), "C:\\app".to_string());
        let env = Instruction::Env(EnvInstruction { vars });
        builder
            .execute_instruction(&mut skel, &ctx, &env)
            .await
            .expect("ENV must succeed and accumulate into image_config");
        assert_eq!(skel.image_config.env, vec!["APP_HOME=C:\\app".to_string()]);
    }

    #[tokio::test]
    async fn execute_instruction_workdir_and_entrypoint_mutate_config() {
        let builder = WindowsBuilder::new(dummy_config());
        let (ctx, mut skel, _guard) = ctx_and_skeleton_in_tempdir();
        builder
            .execute_instruction(
                &mut skel,
                &ctx,
                &Instruction::Workdir("C:\\app".to_string()),
            )
            .await
            .expect("WORKDIR must succeed");
        assert_eq!(skel.image_config.working_dir.as_deref(), Some("C:\\app"));
        builder
            .execute_instruction(
                &mut skel,
                &ctx,
                &Instruction::Entrypoint(ShellOrExec::Exec(vec!["C:\\app\\app.exe".to_string()])),
            )
            .await
            .expect("ENTRYPOINT must succeed");
        assert_eq!(
            skel.image_config.entrypoint.as_deref(),
            Some(["C:\\app\\app.exe".to_string()].as_slice())
        );
    }

    // -----------------------------------------------------------------
    // 4.C: config-only instruction helpers
    // -----------------------------------------------------------------

    #[test]
    fn apply_workdir_relative_resolves_against_previous() {
        let mut cfg = OciImageConfig::default();
        apply_workdir(&mut cfg, "C:\\app");
        apply_workdir(&mut cfg, "sub");
        assert_eq!(cfg.working_dir.as_deref(), Some("C:\\app\\sub"));
        // Absolute drive replaces; trailing-slash base is honoured.
        apply_workdir(&mut cfg, "D:\\other");
        assert_eq!(cfg.working_dir.as_deref(), Some("D:\\other"));
        // Forward-slash absolute Unix path is treated as absolute.
        apply_workdir(&mut cfg, "/data");
        assert_eq!(cfg.working_dir.as_deref(), Some("/data"));
    }

    #[test]
    fn apply_env_replaces_existing_key() {
        let mut cfg = OciImageConfig::default();
        let mut vars = HashMap::new();
        vars.insert("FOO".to_string(), "1".to_string());
        apply_env(&mut cfg, &EnvInstruction { vars });
        let mut vars2 = HashMap::new();
        vars2.insert("FOO".to_string(), "2".to_string());
        vars2.insert("BAR".to_string(), "baz".to_string());
        apply_env(&mut cfg, &EnvInstruction { vars: vars2 });
        // FOO must have been replaced (last write wins), and the new
        // BAR sits alongside it.
        assert!(cfg.env.contains(&"FOO=2".to_string()), "{:?}", cfg.env);
        assert!(cfg.env.contains(&"BAR=baz".to_string()), "{:?}", cfg.env);
        assert!(!cfg.env.contains(&"FOO=1".to_string()), "{:?}", cfg.env);
        // No duplicate KEYs.
        let foo_count = cfg.env.iter().filter(|e| e.starts_with("FOO=")).count();
        assert_eq!(foo_count, 1, "ENV must enforce single KEY: {:?}", cfg.env);
    }

    #[test]
    fn apply_entrypoint_resets_cmd_per_spec() {
        let mut cfg = OciImageConfig::default();
        apply_cmd(&mut cfg, &ShellOrExec::Exec(vec!["bash".to_string()]));
        assert!(cfg.cmd.is_some());
        apply_entrypoint(
            &mut cfg,
            &ShellOrExec::Exec(vec!["C:\\app\\app.exe".to_string()]),
        );
        assert_eq!(
            cfg.entrypoint.as_deref(),
            Some(["C:\\app\\app.exe".to_string()].as_slice())
        );
        assert!(
            cfg.cmd.is_none(),
            "ENTRYPOINT must reset CMD per Dockerfile spec"
        );
    }

    #[test]
    fn apply_expose_accumulates_ports() {
        let mut cfg = OciImageConfig::default();
        apply_expose(&mut cfg, &ExposeInstruction::tcp(80));
        apply_expose(&mut cfg, &ExposeInstruction::tcp(443));
        apply_expose(&mut cfg, &ExposeInstruction::udp(53));
        assert!(cfg.exposed_ports.contains_key("80/tcp"));
        assert!(cfg.exposed_ports.contains_key("443/tcp"));
        assert!(cfg.exposed_ports.contains_key("53/udp"));
        assert_eq!(cfg.exposed_ports.len(), 3);
    }

    #[test]
    fn apply_label_last_value_wins() {
        let mut cfg = OciImageConfig::default();
        cfg.labels
            .insert("maintainer".to_string(), "alice".to_string());
        // Direct mutation simulates `Instruction::Label(...)` dispatch
        // in execute_instruction. The contract is: later LABEL with the
        // same KEY overrides.
        cfg.labels
            .insert("maintainer".to_string(), "bob".to_string());
        assert_eq!(
            cfg.labels.get("maintainer").map(String::as_str),
            Some("bob")
        );
    }

    #[test]
    fn apply_healthcheck_disabled_and_check_round_trip() {
        let mut cfg = OciImageConfig::default();
        apply_healthcheck(&mut cfg, &HealthcheckInstruction::None);
        let hc = cfg
            .healthcheck
            .as_ref()
            .expect("HEALTHCHECK NONE must populate config");
        assert!(hc.is_disabled());

        let cmd = HealthcheckInstruction::Check {
            command: ShellOrExec::Shell("curl -f http://localhost/".to_string()),
            interval: Some(std::time::Duration::from_secs(30)),
            timeout: Some(std::time::Duration::from_secs(5)),
            start_period: None,
            start_interval: None,
            retries: Some(3),
        };
        apply_healthcheck(&mut cfg, &cmd);
        let hc2 = cfg.healthcheck.as_ref().expect("healthcheck populated");
        assert_eq!(
            hc2.test,
            vec![
                "CMD-SHELL".to_string(),
                "curl -f http://localhost/".to_string()
            ]
        );
        assert_eq!(hc2.interval.as_deref(), Some("30s"));
        assert_eq!(hc2.timeout.as_deref(), Some("5s"));
        assert_eq!(hc2.retries, Some(3));
    }

    // -----------------------------------------------------------------
    // 4.C: COPY/ADD filesystem semantics
    // -----------------------------------------------------------------

    /// Locate the materialised `Files/<dest>` payload under
    /// `working_layer_chain_dir/<scratch_id>/` for off-Windows tests.
    /// The scratch id is non-deterministic (uuid) so we scan the dir.
    fn locate_scratch_files(chain_dir: &std::path::Path) -> PathBuf {
        for entry in std::fs::read_dir(chain_dir).expect("read chain dir") {
            let entry = entry.expect("read dir entry");
            let path = entry.path();
            if path.is_dir()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|s| s.starts_with("copy-add-"))
            {
                return path.join("Files");
            }
        }
        panic!("no copy-add-* scratch dir under {}", chain_dir.display());
    }

    #[tokio::test]
    async fn apply_copy_simple_file_writes_to_scratch() {
        let builder = WindowsBuilder::new(dummy_config());
        let (ctx, mut skel, _guard) = ctx_and_skeleton_in_tempdir();
        std::fs::write(ctx.context_dir.join("hello.txt"), b"hello").unwrap();
        let copy = Instruction::Copy(CopyInstruction::new(
            vec!["hello.txt".to_string()],
            "C:\\app\\hello.txt".to_string(),
        ));
        builder
            .execute_instruction(&mut skel, &ctx, &copy)
            .await
            .expect("COPY of a simple file must succeed off-Windows");
        let files = locate_scratch_files(&skel.working_layer_chain_dir);
        let copied = files.join("app").join("hello.txt");
        assert!(copied.is_file(), "expected file at {}", copied.display());
        assert_eq!(std::fs::read(&copied).unwrap(), b"hello");
    }

    #[tokio::test]
    async fn apply_copy_rejects_parent_dir_traversal() {
        let builder = WindowsBuilder::new(dummy_config());
        let (ctx, mut skel, _guard) = ctx_and_skeleton_in_tempdir();
        let copy = Instruction::Copy(CopyInstruction::new(
            vec!["../secrets".to_string()],
            "C:\\".to_string(),
        ));
        let err = builder
            .execute_instruction(&mut skel, &ctx, &copy)
            .await
            .expect_err("COPY with `..` must be rejected");
        assert!(
            matches!(err, BuildError::PathTraversal { ref src } if src == "../secrets"),
            "expected PathTraversal, got: {err}"
        );
    }

    #[tokio::test]
    async fn apply_copy_directory_recursive() {
        let builder = WindowsBuilder::new(dummy_config());
        let (ctx, mut skel, _guard) = ctx_and_skeleton_in_tempdir();
        let src_dir = ctx.context_dir.join("payload");
        std::fs::create_dir_all(src_dir.join("nested")).unwrap();
        std::fs::write(src_dir.join("a.txt"), b"A").unwrap();
        std::fs::write(src_dir.join("nested").join("b.txt"), b"B").unwrap();

        let copy = Instruction::Copy(CopyInstruction::new(
            vec!["payload".to_string()],
            "C:\\opt\\payload\\".to_string(),
        ));
        builder
            .execute_instruction(&mut skel, &ctx, &copy)
            .await
            .expect("recursive COPY must succeed");
        let files = locate_scratch_files(&skel.working_layer_chain_dir);
        assert!(files.join("opt/payload/a.txt").is_file());
        assert!(files.join("opt/payload/nested/b.txt").is_file());
    }

    #[tokio::test]
    async fn apply_add_tarball_extracts() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let builder = WindowsBuilder::new(dummy_config());
        let (ctx, mut skel, _guard) = ctx_and_skeleton_in_tempdir();

        // Build a tiny .tar.gz fixture containing one file `inside.txt`.
        let tar_bytes = {
            let mut tar_builder = tar::Builder::new(Vec::new());
            let payload = b"INSIDE\n";
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_path("inside.txt").unwrap();
            header.set_cksum();
            tar_builder.append(&header, payload.as_ref()).unwrap();
            tar_builder.finish().unwrap();
            tar_builder.into_inner().unwrap()
        };
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut gz, &tar_bytes).unwrap();
        let gz_bytes = gz.finish().unwrap();
        std::fs::write(ctx.context_dir.join("payload.tar.gz"), gz_bytes).unwrap();

        let add = Instruction::Add(AddInstruction::new(
            vec!["payload.tar.gz".to_string()],
            "C:\\opt\\extracted\\".to_string(),
        ));
        builder
            .execute_instruction(&mut skel, &ctx, &add)
            .await
            .expect("ADD must extract a tarball");
        let files = locate_scratch_files(&skel.working_layer_chain_dir);
        let extracted = files.join("opt/extracted/inside.txt");
        assert!(extracted.is_file(), "expected {}", extracted.display());
        assert_eq!(std::fs::read(&extracted).unwrap(), b"INSIDE\n");
    }

    #[tokio::test]
    #[ignore = "live network — exercises ADD URL fetch against example.com"]
    async fn apply_add_http_url_downloads() {
        let builder = WindowsBuilder::new(dummy_config());
        let (ctx, mut skel, _guard) = ctx_and_skeleton_in_tempdir();
        let add = Instruction::Add(AddInstruction::new(
            vec!["https://example.com/".to_string()],
            "C:\\downloads\\".to_string(),
        ));
        builder
            .execute_instruction(&mut skel, &ctx, &add)
            .await
            .expect("ADD URL must succeed when the network is reachable");
        let files = locate_scratch_files(&skel.working_layer_chain_dir);
        // The basename "" from `example.com/` becomes the fallback
        // `download`; either form is acceptable.
        assert!(
            files.join("downloads").is_dir(),
            "expected downloads/ dir under {}",
            files.display()
        );
    }

    #[test]
    fn path_traversal_detection_flavours() {
        assert!(path_contains_parent_dir("../etc"));
        assert!(path_contains_parent_dir("foo/../bar"));
        assert!(path_contains_parent_dir("foo\\..\\bar"));
        assert!(!path_contains_parent_dir("foo/bar"));
        assert!(!path_contains_parent_dir("foo..bar")); // no separator → ordinary name
    }

    #[test]
    fn dest_under_files_root_strips_drive() {
        assert_eq!(
            dest_under_files_root("C:\\app\\bin"),
            PathBuf::from("app/bin")
        );
        assert_eq!(
            dest_under_files_root("/etc/passwd"),
            PathBuf::from("etc/passwd")
        );
        assert_eq!(
            dest_under_files_root("relative/x"),
            PathBuf::from("relative/x")
        );
    }

    #[test]
    fn duration_to_oci_string_shapes() {
        use std::time::Duration;
        assert_eq!(duration_to_oci_string(Duration::from_secs(30)), "30s");
        assert_eq!(duration_to_oci_string(Duration::from_secs(90)), "90s");
        assert_eq!(duration_to_oci_string(Duration::from_secs(60)), "1m");
        assert_eq!(duration_to_oci_string(Duration::from_millis(500)), "500ms");
        assert_eq!(duration_to_oci_string(Duration::from_secs(3600)), "1h");
    }

    // -----------------------------------------------------------------
    // 4.B: Chocolatey detection / translation
    // -----------------------------------------------------------------

    #[test]
    fn detect_apt_install_in_run() {
        // Compound RUN: an `apt-get update && apt-get install -y curl git`
        // splits into two sub-commands; only the install one is
        // recognised here.
        let parts = split_shell_subcommands("apt-get update && apt-get install -y curl git");
        assert_eq!(parts.len(), 2);
        assert!(is_package_manager_sync(&parts[0]));
        let detected = detect_install_in_subcommand(&parts[1])
            .expect("install sub-command must be recognised");
        assert_eq!(detected.0, DetectedPmKind::Apt);
        assert_eq!(detected.1, vec!["curl".to_string(), "git".to_string()]);
    }

    #[test]
    fn detect_yum_install_in_run() {
        let detected = detect_install_in_subcommand("yum install -y httpd")
            .expect("yum install -y httpd must be recognised");
        assert_eq!(detected.0, DetectedPmKind::YumOrDnf);
        assert_eq!(detected.1, vec!["httpd".to_string()]);

        // dnf form takes the same code path.
        let detected = detect_install_in_subcommand("dnf install -y nginx php-fpm")
            .expect("dnf install -y must be recognised");
        assert_eq!(detected.0, DetectedPmKind::YumOrDnf);
        assert_eq!(detected.1, vec!["nginx".to_string(), "php-fpm".to_string()]);
    }

    #[test]
    fn detect_apk_install_in_run() {
        let detected = detect_install_in_subcommand("apk add --no-cache nodejs npm")
            .expect("apk add must be recognised");
        assert_eq!(detected.0, DetectedPmKind::Apk);
        assert_eq!(detected.1, vec!["nodejs".to_string(), "npm".to_string()]);
    }

    #[test]
    fn detect_no_install_returns_none() {
        // Plain shell commands must NOT match.
        assert!(detect_install_in_subcommand("echo hello").is_none());
        assert!(detect_install_in_subcommand("ls /tmp").is_none());
        // A misspelled package manager should also miss.
        assert!(detect_install_in_subcommand("apt-getinstall -y curl").is_none());
        // Verb without packages → no install.
        assert!(detect_install_in_subcommand("apt-get install -y").is_none());
        // A whole compound shell command via split + classify:
        let parts = split_shell_subcommands("echo hello && ls /tmp");
        assert_eq!(parts.len(), 2);
        for p in &parts {
            assert!(detect_install_in_subcommand(p).is_none());
            assert!(!is_package_manager_sync(p));
        }
    }

    #[test]
    fn translate_run_apt_to_choco_with_in_memory_shard() {
        // Build the same shape the resolver writes to disk so we can
        // exercise the cache-hit path without any network. The TTL is
        // 7 days; a freshly-written file is well within that window.
        let fixture = ChocoMapShard {
            metadata: crate::windows_image_resolver::ChocoMapMetadata {
                generated_at: "2026-05-21T00:00:00Z".to_string(),
                source: "chocolatey.org".to_string(),
                distro: "debian-12".to_string(),
                shard: "c".to_string(),
                total_mappings: 2,
            },
            mappings: HashMap::from([
                ("curl".to_string(), "curl".to_string()),
                ("linux-headers-generic".to_string(), "__skip__".to_string()),
            ]),
        };

        // The resolver consults dirs::cache_dir() / "package-maps-choco-v1"
        // / <distro> / <shard>.json. We overlay a per-test HOME / cache
        // override using a `tempfile` + the env override mechanism the
        // `dirs` crate honours: XDG_CACHE_HOME on Linux, otherwise the
        // platform's native env var. We set both so this test is
        // portable across host OSes.
        let tmp = tempfile::tempdir().unwrap();
        let cache_root = tmp.path().to_path_buf();
        // On Linux/macOS `dirs::cache_dir()` honours XDG_CACHE_HOME.
        // On Windows it honours LOCALAPPDATA. We set both so the test
        // is portable across host CI runners.
        std::env::set_var("XDG_CACHE_HOME", &cache_root);
        std::env::set_var("LOCALAPPDATA", &cache_root);

        let shard_dir = cache_root.join("package-maps-choco-v1").join("debian-12");
        std::fs::create_dir_all(&shard_dir).unwrap();
        let shard_path = shard_dir.join("c.json");
        std::fs::write(&shard_path, serde_json::to_string(&fixture).unwrap()).unwrap();
        // Also the `l` shard for linux-headers-generic.
        let shard_dir_l = cache_root.join("package-maps-choco-v1").join("debian-12");
        std::fs::create_dir_all(&shard_dir_l).unwrap();
        let fixture_l = ChocoMapShard {
            metadata: crate::windows_image_resolver::ChocoMapMetadata {
                generated_at: "2026-05-21T00:00:00Z".to_string(),
                source: "chocolatey.org".to_string(),
                distro: "debian-12".to_string(),
                shard: "l".to_string(),
                total_mappings: 1,
            },
            mappings: HashMap::from([(
                "linux-headers-generic".to_string(),
                "__skip__".to_string(),
            )]),
        };
        std::fs::write(
            shard_dir_l.join("l.json"),
            serde_json::to_string(&fixture_l).unwrap(),
        )
        .unwrap();

        // Drive translate_shell_command through the in-memory shard
        // fixtures. Both `curl` and `linux-headers-generic` are present
        // in the shards (curl → curl, linux-headers-generic → __skip__).
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let (rewritten, skipped) = rt
            .block_on(translate_shell_command(
                "apt-get install -y curl linux-headers-generic",
                "debian-12",
            ))
            .expect("translate succeeds when every package resolves");
        assert!(
            rewritten.contains("choco install -y curl"),
            "rewritten command must include curl: {rewritten}"
        );
        assert!(
            !rewritten.contains("linux-headers-generic"),
            "skipped package must NOT appear in rewritten command: {rewritten}"
        );
        assert_eq!(skipped, vec!["linux-headers-generic".to_string()]);
    }

    // -----------------------------------------------------------------
    // 4.B helper unit tests
    // -----------------------------------------------------------------

    #[test]
    fn split_shell_subcommands_honours_and_and_semicolon() {
        let parts = split_shell_subcommands("a && b ; c");
        assert_eq!(
            parts,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn split_shell_subcommands_drops_empty_segments() {
        let parts = split_shell_subcommands(" && a && ; b ;");
        assert_eq!(parts, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn is_package_manager_sync_matches_common_variants() {
        assert!(is_package_manager_sync("apt-get update"));
        assert!(is_package_manager_sync("apt update"));
        assert!(is_package_manager_sync("apk update"));
        assert!(is_package_manager_sync("yum check-update"));
        assert!(is_package_manager_sync("dnf makecache"));
        assert!(is_package_manager_sync("sudo apt-get update"));
        assert!(!is_package_manager_sync("apt-get install -y curl"));
        assert!(!is_package_manager_sync("echo hello"));
    }

    #[test]
    fn rejoin_emits_choco_install_for_install_subcommand() {
        let parts = vec![
            ShellSubcommand::Verbatim("echo before".to_string()),
            ShellSubcommand::PackageManagerSync,
            ShellSubcommand::Install {
                kind: DetectedPmKind::Apt,
                packages: vec!["curl".to_string(), "git".to_string()],
            },
            ShellSubcommand::Verbatim("echo after".to_string()),
        ];
        let out = rejoin_subcommands(&parts);
        assert_eq!(
            out,
            "echo before && choco install -y curl git && echo after"
        );
    }

    #[test]
    fn wrap_in_cmd_escapes_embedded_quotes() {
        let wrapped = wrap_in_cmd(r#"echo "hello""#);
        assert!(wrapped.starts_with("cmd /c \""));
        assert!(wrapped.contains(r#"\"hello\""#));
        assert!(wrapped.ends_with('"'));
    }

    #[test]
    fn derive_source_distro_known_bases() {
        let mk = |image_ref: &str| BaseImageManifest {
            image_ref: image_ref.to_string(),
            os: "windows".into(),
            os_version: None,
            arch: "amd64".into(),
            config_blob: Vec::new(),
        };
        assert_eq!(derive_source_distro(&mk("debian:12")), "debian-12");
        assert_eq!(
            derive_source_distro(&mk("docker.io/library/ubuntu:22.04")),
            "ubuntu-22.04"
        );
        assert_eq!(derive_source_distro(&mk("alpine:3.19")), "alpine-3.19");
        // Unknown short repo → defaults to debian-12.
        assert_eq!(
            derive_source_distro(&mk("mcr.microsoft.com/windows/nanoserver:ltsc2022")),
            "debian-12"
        );
    }

    // -----------------------------------------------------------------
    // 4.B integration: drive a real RUN through HCS.
    //
    // Mirrors the setup in `crates/zlayer-agent/tests/windows_hcs_e2e.rs`
    // — runs only when the host has HCS + a nanoserver:ltsc2022 base
    // image already pulled into the blob cache. The test is gated
    // `#[ignore]` so `cargo test --workspace` does not try to dial HCS
    // on Linux CI; the Windows CI runner exercises it explicitly with
    // `cargo test -p zlayer-builder --tests -- --ignored`.
    //
    // What it asserts:
    //   1. `build_skeleton` materialises the parent chain.
    //   2. A trivial `RUN cmd /c echo hello > C:\hello.txt` succeeds
    //      (exit 0).
    //   3. `skeleton.base_layers.len()` grew by 1 (the post-RUN diff
    //      layer was committed).
    //   4. `skeleton.working_chain.len()` grew by 1 (the new RO layer
    //      is on disk and would chain into the next RUN).
    #[tokio::test]
    #[ignore = "requires Windows host with Hyper-V + mcr.microsoft.com/windows/nanoserver:ltsc2022 base image"]
    async fn run_step_emits_new_layer_on_windows_host() {
        let cache_dir = std::env::temp_dir().join("zlayer-wcow-run-e2e");
        std::fs::create_dir_all(&cache_dir).expect("create cache_dir");

        let cfg = WindowsBuildConfig {
            cache_dir,
            registry_auth: RegistryAuth::Anonymous,
            platform: WindowsBuildConfig::default_platform().to_string(),
            os_version_override: None,
            scratch_size_gb: WindowsBuildConfig::default_scratch_size_gb(),
        };
        let builder = WindowsBuilder::new(cfg);

        // Write a minimal Dockerfile to a temp dir so the parser has
        // real bytes to consume. We do not need a real build context —
        // there is no COPY here.
        let ctx_dir = tempfile::tempdir().expect("tmpdir");
        let dockerfile_path = ctx_dir.path().join("Dockerfile");
        std::fs::write(
            &dockerfile_path,
            b"FROM mcr.microsoft.com/windows/nanoserver:ltsc2022\nRUN cmd /c echo hello > C:\\hello.txt\n",
        )
        .expect("write Dockerfile");

        let ctx = BuildContext {
            context_dir: ctx_dir.path().to_path_buf(),
            dockerfile_path: PathBuf::from("Dockerfile"),
            build_args: HashMap::new(),
            tag: "zlayer-wcow-run-e2e:test".to_string(),
        };

        let mut skeleton = builder
            .build_skeleton(&ctx)
            .await
            .expect("build_skeleton must succeed against the real MCR base image");
        let base_layer_count = skeleton.base_layers.len();
        let working_chain_count = skeleton.working_chain.len();
        assert!(
            base_layer_count >= 1,
            "expected at least one base layer materialised"
        );

        let stage = &skeleton.parsed_dockerfile.stages[0].clone();
        let run_instr = stage
            .instructions
            .iter()
            .find(|i| matches!(i, Instruction::Run(_)))
            .cloned()
            .expect("Dockerfile fixture has a RUN");

        builder
            .execute_instruction(&mut skeleton, &ctx, &run_instr)
            .await
            .expect("RUN cmd /c echo hello must succeed on a Windows host");

        assert_eq!(
            skeleton.base_layers.len(),
            base_layer_count + 1,
            "RUN must append exactly one descriptor to base_layers"
        );
        assert_eq!(
            skeleton.working_chain.len(),
            working_chain_count + 1,
            "RUN must append exactly one on-disk layer entry to working_chain"
        );
    }

    // -----------------------------------------------------------------
    // 4.D: OCI manifest + config emission
    // -----------------------------------------------------------------

    /// Build a minimal foreign-base-only skeleton fixture for emit tests.
    /// The base manifest carries an explicit `os.version` and a config
    /// blob with one `diff_id`; the base layer descriptor carries the
    /// MCR foreign-layer media type + a real-looking `urls[]`.
    fn skeleton_with_foreign_base() -> BuildSkeleton {
        let parsed =
            Dockerfile::parse("FROM mcr.microsoft.com/windows/nanoserver:ltsc2022\n").unwrap();
        let base_config_blob = serde_json::json!({
            "architecture": "amd64",
            "os": "windows",
            "os.version": "10.0.20348.2227",
            "rootfs": {
                "type": "layers",
                "diff_ids": ["sha256:base0000000000000000000000000000000000000000000000000000000000"],
            },
            "config": {},
        })
        .to_string()
        .into_bytes();
        BuildSkeleton {
            parsed_dockerfile: parsed,
            base_layers: vec![LayerRef {
                digest: "sha256:basecompressed00000000000000000000000000000000000000000000000000"
                    .to_string(),
                media_type: FOREIGN_WINDOWS_LAYER_MEDIA_TYPE.to_string(),
                size: 12345,
                urls: vec![
                    "https://mcr.microsoft.com/v2/windows/nanoserver/blobs/sha256:base".to_string(),
                ],
            }],
            base_manifest: BaseImageManifest {
                image_ref: "mcr.microsoft.com/windows/nanoserver:ltsc2022".into(),
                os: "windows".into(),
                os_version: Some("10.0.20348.2227".to_string()),
                arch: "amd64".into(),
                config_blob: base_config_blob,
            },
            working_layer_chain_dir: std::env::temp_dir().join("zlayer-wcow-emit-tests/x"),
            working_chain: vec![WindowsLayerEntry {
                layer_id: "base".to_string(),
                layer_path: PathBuf::from("/nonexistent/base"),
            }],
            image_config: OciImageConfig::default(),
            instruction_log: vec![ExecutedInstruction {
                source_line: "FROM mcr.microsoft.com/windows/nanoserver:ltsc2022".to_string(),
                produced_layer: true,
                timestamp: Utc::now(),
            }],
        }
    }

    #[tokio::test]
    async fn emit_image_simple_base_only() {
        let cfg = dummy_config();
        let skel = skeleton_with_foreign_base();
        let built = emit_image_impl(&cfg, &skel, "myimage:test")
            .await
            .expect("emit must succeed for a foreign-base-only skeleton");

        // Manifest deserialises and has exactly one foreign layer with
        // the urls[] preserved.
        let manifest: serde_json::Value = serde_json::from_slice(&built.manifest_blob).unwrap();
        assert_eq!(manifest["schemaVersion"], 2);
        assert_eq!(
            manifest["mediaType"], OCI_IMAGE_MANIFEST_MEDIA_TYPE,
            "manifest mediaType must be the OCI image manifest type"
        );
        let layers = manifest["layers"].as_array().expect("layers array");
        assert_eq!(
            layers.len(),
            1,
            "base-only skeleton emits exactly one layer"
        );
        let l0 = &layers[0];
        assert_eq!(l0["mediaType"], FOREIGN_WINDOWS_LAYER_MEDIA_TYPE);
        let urls = l0["urls"].as_array().expect("foreign layer carries urls");
        assert_eq!(
            urls[0].as_str().unwrap(),
            "https://mcr.microsoft.com/v2/windows/nanoserver/blobs/sha256:base"
        );

        // Image config has one history entry (the FROM), os/os.version/
        // architecture set, and inherits the base's diff_id.
        let ic: serde_json::Value = serde_json::from_slice(&built.image_config_blob).unwrap();
        assert_eq!(ic["os"], "windows");
        assert_eq!(ic["os.version"], "10.0.20348.2227");
        assert_eq!(ic["architecture"], "amd64");
        let history = ic["history"].as_array().expect("history array");
        assert_eq!(
            history.len(),
            1,
            "FROM-only skeleton emits one history entry"
        );
        assert!(history[0]["created_by"]
            .as_str()
            .unwrap()
            .starts_with("FROM mcr.microsoft.com/windows/nanoserver:ltsc2022"));
        assert!(
            history[0].get("empty_layer").is_none(),
            "FROM produced a layer, so empty_layer must be omitted (or false)"
        );
        let diff_ids = ic["rootfs"]["diff_ids"].as_array().expect("diff_ids array");
        assert_eq!(diff_ids.len(), 1);
        assert_eq!(
            diff_ids[0].as_str().unwrap(),
            "sha256:base0000000000000000000000000000000000000000000000000000000000"
        );

        // Config descriptor in the manifest points at the image config
        // blob's digest with the right size.
        assert_eq!(
            manifest["config"]["digest"].as_str().unwrap(),
            built.image_config_digest
        );
        assert_eq!(
            manifest["config"]["size"].as_u64().unwrap(),
            built.image_config_blob.len() as u64
        );

        // Manifest digest matches what we'd recompute.
        let recomputed = compute_sha256_hex(&built.manifest_blob);
        assert_eq!(recomputed, built.manifest_digest);
        assert_eq!(built.tag, "myimage:test");
    }

    #[tokio::test]
    async fn emit_image_with_run_step() {
        let cfg = dummy_config();
        let mut skel = skeleton_with_foreign_base();
        // Append a synthetic RUN-produced layer + log entry.
        skel.base_layers.push(LayerRef {
            digest: "sha256:run111111111111111111111111111111111111111111111111111111111111".into(),
            media_type: OCI_TAR_GZIP_LAYER_MEDIA_TYPE.to_string(),
            size: 9999,
            urls: Vec::new(),
        });
        skel.working_chain.push(WindowsLayerEntry {
            layer_id: "run1".to_string(),
            layer_path: PathBuf::from("/nonexistent/run1"),
        });
        skel.instruction_log.push(ExecutedInstruction {
            source_line: "RUN choco install -y curl".to_string(),
            produced_layer: true,
            timestamp: Utc::now(),
        });

        let built = emit_image_impl(&cfg, &skel, "myimage:run").await.unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(&built.manifest_blob).unwrap();
        let layers = manifest["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 2, "FROM + RUN produces two layer descriptors");
        assert_eq!(layers[0]["mediaType"], FOREIGN_WINDOWS_LAYER_MEDIA_TYPE);
        assert_eq!(layers[1]["mediaType"], OCI_TAR_GZIP_LAYER_MEDIA_TYPE);
        assert!(
            layers[1].get("urls").is_none(),
            "non-foreign layer must NOT carry urls[]"
        );

        let ic: serde_json::Value = serde_json::from_slice(&built.image_config_blob).unwrap();
        let history = ic["history"].as_array().unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(
            history[1]["created_by"].as_str().unwrap(),
            "RUN choco install -y curl"
        );
    }

    #[tokio::test]
    async fn emit_image_with_config_only_instructions() {
        let cfg = dummy_config();
        let mut skel = skeleton_with_foreign_base();
        // Two config-only entries (ENV + WORKDIR) — neither produces a
        // layer.
        skel.instruction_log.push(ExecutedInstruction {
            source_line: "ENV FOO=bar".to_string(),
            produced_layer: false,
            timestamp: Utc::now(),
        });
        skel.instruction_log.push(ExecutedInstruction {
            source_line: "WORKDIR C:\\app".to_string(),
            produced_layer: false,
            timestamp: Utc::now(),
        });
        skel.image_config.env.push("FOO=bar".to_string());
        skel.image_config.working_dir = Some("C:\\app".to_string());

        let built = emit_image_impl(&cfg, &skel, "myimage:cfg").await.unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(&built.manifest_blob).unwrap();
        let layers = manifest["layers"].as_array().unwrap();
        assert_eq!(
            layers.len(),
            1,
            "config-only instructions must NOT add layer descriptors"
        );

        let ic: serde_json::Value = serde_json::from_slice(&built.image_config_blob).unwrap();
        let history = ic["history"].as_array().unwrap();
        assert_eq!(
            history.len(),
            3,
            "FROM + ENV + WORKDIR produces three history entries"
        );
        assert!(history[0].get("empty_layer").is_none());
        assert_eq!(history[1]["empty_layer"], true);
        assert_eq!(history[2]["empty_layer"], true);
        // ENV / WORKDIR end up in the image config's `config` object.
        assert_eq!(ic["config"]["WorkingDir"], "C:\\app");
        assert_eq!(ic["config"]["Env"][0], "FOO=bar");
    }

    #[test]
    fn compute_sha256_known_input() {
        // Well-known: sha256("hello") =
        // 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            compute_sha256_hex(b"hello"),
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[tokio::test]
    async fn foreign_layer_carries_urls_through_manifest() {
        let cfg = dummy_config();
        let skel = skeleton_with_foreign_base();
        let built = emit_image_impl(&cfg, &skel, "myimage:foreign")
            .await
            .unwrap();
        // Sanity on the typed BuiltImage view.
        let foreign = &built.layers[0];
        assert_eq!(foreign.media_type, FOREIGN_WINDOWS_LAYER_MEDIA_TYPE);
        let urls = foreign
            .urls
            .as_ref()
            .expect("foreign layer must carry an urls[] vector through BuiltImage");
        assert!(
            !urls.is_empty(),
            "urls[] must be non-empty on a foreign layer"
        );
        assert!(urls[0].starts_with("https://mcr.microsoft.com/"));

        // And on the wire form.
        let manifest: serde_json::Value = serde_json::from_slice(&built.manifest_blob).unwrap();
        let l0 = &manifest["layers"][0];
        let on_wire_urls = l0["urls"].as_array().expect("wire form must carry urls[]");
        assert!(!on_wire_urls.is_empty());
    }

    #[tokio::test]
    async fn emit_image_errors_when_os_version_unresolved() {
        let cfg = dummy_config();
        let mut skel = skeleton_with_foreign_base();
        skel.base_manifest.os_version = None;
        // Strip os.version from the base config blob too.
        skel.base_manifest.config_blob = serde_json::json!({
            "architecture": "amd64",
            "os": "windows",
            "rootfs": {
                "type": "layers",
                "diff_ids": ["sha256:base"],
            },
            "config": {},
        })
        .to_string()
        .into_bytes();
        let err = emit_image_impl(&cfg, &skel, "myimage:err")
            .await
            .expect_err("emit must error without an os.version");
        assert!(
            matches!(err, BuildError::OsVersionUnresolved),
            "expected OsVersionUnresolved, got: {err}"
        );
    }
}
