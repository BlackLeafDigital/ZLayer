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
//! - **4.C**: COPY / ADD writes into the working scratch layer.
//!   [`WindowsBuilder::execute_instruction`] for [`Instruction::Copy`] /
//!   [`Instruction::Add`] currently surfaces a clear error pointing here.
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

use std::collections::HashMap;
use std::path::PathBuf;

#[cfg(target_os = "windows")]
use crate::dockerfile::ShellOrExec;
use crate::dockerfile::{Dockerfile, DockerfileFromTarget, Instruction, RunInstruction};
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

        Ok(BuildSkeleton {
            parsed_dockerfile: parsed,
            base_layers,
            base_manifest,
            working_layer_chain_dir,
            working_chain,
        })
    }

    /// Apply a single non-FROM instruction to a [`BuildSkeleton`].
    ///
    /// Routes to the per-instruction handler appropriate for the
    /// instruction kind. Config-only instructions (ENV / WORKDIR /
    /// ENTRYPOINT / CMD / USER / EXPOSE / VOLUME / LABEL / ARG / SHELL /
    /// STOPSIGNAL / HEALTHCHECK / ONBUILD) are accepted as no-ops at the
    /// rootfs level — those mutations land on the OCI image config in
    /// task 4.D, not on the working layer chain. RUN is executed via an
    /// HCS-driven ephemeral compute system (this task). COPY / ADD
    /// return a [`BuildError::NotYetImplemented`] pointing at task 4.C.
    ///
    /// # Errors
    ///
    /// - [`BuildError::NotYetImplemented`] for COPY / ADD (delivered in
    ///   4.C).
    /// - [`BuildError::RunFailed`] when the RUN command exits non-zero.
    /// - [`BuildError::LayerCreate`] / [`BuildError::IoError`] on HCS or
    ///   filesystem failures during RUN execution or layer export.
    /// - [`BuildError::NotSupported`] when invoked on a non-Windows host
    ///   (RUN needs the HCS APIs).
    pub async fn execute_instruction(
        &self,
        skeleton: &mut BuildSkeleton,
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

        match instruction {
            Instruction::Run(run) => self.execute_run_step(skeleton, run, step_index).await,
            Instruction::Copy(_) => Err(BuildError::not_yet_implemented(
                "COPY lands in Phase 4 task 4.C (writes into the scratch layer before the next \
                 RUN commit)",
            )),
            Instruction::Add(_) => Err(BuildError::not_yet_implemented(
                "ADD lands in Phase 4 task 4.C (writes into the scratch layer; URL fetch + \
                 archive auto-extract reuse the existing sandbox_builder helpers)",
            )),
            // Config-only instructions: 4.A treats them as no-ops on the
            // rootfs. The values they carry are applied to the OCI image
            // config in task 4.D.
            Instruction::Env(_)
            | Instruction::Workdir(_)
            | Instruction::Entrypoint(_)
            | Instruction::Cmd(_)
            | Instruction::User(_)
            | Instruction::Expose(_)
            | Instruction::Volume(_)
            | Instruction::Label(_)
            | Instruction::Arg(_)
            | Instruction::Shell(_)
            | Instruction::Stopsignal(_)
            | Instruction::Healthcheck(_)
            | Instruction::Onbuild(_) => Ok(()),
        }
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::dockerfile::{CopyInstruction, EnvInstruction, ShellOrExec};
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
        }
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
    async fn execute_instruction_copy_returns_not_yet_implemented() {
        let builder = WindowsBuilder::new(dummy_config());
        let mut skel = dummy_skeleton();
        let copy = Instruction::Copy(CopyInstruction::new(
            vec!["app.exe".to_string()],
            "C:\\app\\app.exe".to_string(),
        ));
        let err = builder
            .execute_instruction(&mut skel, &copy)
            .await
            .expect_err("COPY must route to a NotYetImplemented error in the 4.A skeleton");
        assert!(
            matches!(err, BuildError::NotYetImplemented(ref msg) if msg.contains("4.C")),
            "COPY error must point at Phase 4 task 4.C, got: {err}"
        );
    }

    #[tokio::test]
    async fn execute_instruction_env_succeeds() {
        let builder = WindowsBuilder::new(dummy_config());
        let mut skel = dummy_skeleton();
        let mut vars = HashMap::new();
        vars.insert("APP_HOME".to_string(), "C:\\app".to_string());
        let env = Instruction::Env(EnvInstruction { vars });
        builder
            .execute_instruction(&mut skel, &env)
            .await
            .expect("ENV is a config-only instruction and must succeed in the 4.A skeleton");
    }

    #[tokio::test]
    async fn execute_instruction_workdir_and_entrypoint_succeed() {
        let builder = WindowsBuilder::new(dummy_config());
        let mut skel = dummy_skeleton();
        builder
            .execute_instruction(&mut skel, &Instruction::Workdir("C:\\app".to_string()))
            .await
            .expect("WORKDIR is config-only and must succeed in the 4.A skeleton");
        builder
            .execute_instruction(
                &mut skel,
                &Instruction::Entrypoint(ShellOrExec::Exec(vec!["C:\\app\\app.exe".to_string()])),
            )
            .await
            .expect("ENTRYPOINT is config-only and must succeed in the 4.A skeleton");
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
            .execute_instruction(&mut skeleton, &run_instr)
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
}
