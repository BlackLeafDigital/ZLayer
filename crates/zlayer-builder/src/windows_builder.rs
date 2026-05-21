//! Native WCOW (Windows Container On Windows) image builder.
//!
//! Parses a Dockerfile/ZImagefile, pulls the Windows base image via the
//! registry client, materialises the foreign-layer base via the Windows
//! unpacker, and prepares a layer chain that subsequent Phase 4 tasks
//! extend:
//!
//! - **4.A** (this task): Dockerfile parse + base image pull + foreign-layer
//!   materialisation. Non-FROM instructions are routed through
//!   [`WindowsBuilder::execute_instruction`], which returns a typed
//!   [`BuildError::NotYetImplemented`] for instructions whose execution lands
//!   in a later task.
//! - **4.B**: RUN execution via a transient HCS compute system attached to
//!   the working layer chain. [`WindowsBuilder::execute_instruction`] for
//!   [`Instruction::Run`] currently surfaces a clear error pointing here.
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
//! Phase 4 follow-up tasks (4.B–4.E) can extend it incrementally without
//! disturbing the working `HcsBackend`. Once Phase 4 lands, `HcsBackend`
//! can be retargeted onto `WindowsBuilder` if desired; for now they
//! co-exist.
//!
//! ## Cross-platform compilation
//!
//! The data types ([`WindowsBuilder`], [`WindowsBuildConfig`],
//! [`BuildContext`], [`BuildSkeleton`], [`LayerRef`],
//! [`BaseImageManifest`]) compile on every host so unit tests run on the
//! CI Linux runners. The actual base-layer materialisation in
//! [`WindowsBuilder::build_skeleton`] is gated on `target_os = "windows"`;
//! on other hosts it returns
//! `BuildError::NotSupported { operation: "WindowsBuilder requires
//! target_os = \"windows\"" }`. Phase 4 follow-up tasks preserve this
//! gating discipline.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::dockerfile::{Dockerfile, DockerfileFromTarget, Instruction};
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
/// Windows base image. Subsequent Phase 4 tasks (4.B RUN, 4.C COPY/ADD,
/// 4.D manifest) thread additional knobs through this struct; for the
/// 4.A skeleton only the fields necessary to pull and unpack the base
/// image are present.
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
}

impl WindowsBuildConfig {
    /// The default target platform string: `"windows/amd64"`. The vast
    /// majority of Windows container base images are published for this
    /// platform; `windows/arm64` exists but is not in widespread use yet.
    #[must_use]
    pub const fn default_platform() -> &'static str {
        "windows/amd64"
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
/// - 4.B (RUN) iterates `parsed_dockerfile.stages[0].instructions`, for
///   each [`Instruction::Run`] spawning an HCS compute system attached to
///   [`working_layer_chain_dir`](Self::working_layer_chain_dir) and
///   committing the writable layer delta as a new RO layer appended to
///   [`base_layers`](Self::base_layers).
/// - 4.C (COPY/ADD) writes context files into the working scratch layer
///   before the next 4.B commit.
/// - 4.D (manifest) emits the final OCI image with `os: "windows"`,
///   `os.version` from [`base_manifest`](Self::base_manifest), and the
///   accumulated `base_layers + post-4.B layers` chain.
/// - 4.E (push) pipes the 4.D manifest into the registry client.
#[derive(Debug)]
pub struct BuildSkeleton {
    /// Parsed Dockerfile — `parsed_dockerfile.stages[0]` is the single
    /// stage supported by the 4.A skeleton.
    pub parsed_dockerfile: Dockerfile,
    /// Base image layers in **base-first** order (matching OCI manifest
    /// order). Subsequent task 4.B appends post-RUN layers to the end of
    /// this vector.
    pub base_layers: Vec<LayerRef>,
    /// Manifest metadata from the resolved base image.
    pub base_manifest: BaseImageManifest,
    /// On-disk path to the working layer chain. On Windows this is the
    /// directory passed to
    /// [`zlayer_agent::windows::unpacker::unpack_windows_image`]; on other
    /// hosts this field is the `<cache_dir>/<build_id>/unpacked/` path
    /// the skeleton would have used had the build proceeded.
    pub working_layer_chain_dir: PathBuf,
}

/// Native WCOW image builder.
///
/// Stateless apart from the static [`WindowsBuildConfig`] passed at
/// construction. Per-build state lives entirely on the stack of
/// [`WindowsBuilder::build_skeleton`] and (in later tasks) inside the
/// returned [`BuildSkeleton`].
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
    /// extend. Does **not** execute RUN, COPY, or ADD — those route
    /// through [`Self::execute_instruction`] and surface
    /// [`BuildError::NotYetImplemented`] until the relevant Phase 4 task
    /// lands.
    ///
    /// # Errors
    ///
    /// - [`BuildError::ContextRead`] when the Dockerfile cannot be read.
    /// - [`BuildError::DockerfileParse`] on a malformed Dockerfile.
    /// - [`BuildError::NotSupported`] when the Dockerfile has more than
    ///   one stage (multi-stage WCOW builds are intentionally out of
    ///   scope for the 4.A skeleton — Phase 4 follow-up task).
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
        let (base_layers, base_manifest) =
            pull_and_materialise_base(&base_image_ref, &self.config, &working_layer_chain_dir)
                .await?;

        Ok(BuildSkeleton {
            parsed_dockerfile: parsed,
            base_layers,
            base_manifest,
            working_layer_chain_dir,
        })
    }

    /// Apply a single non-FROM instruction to a [`BuildSkeleton`].
    ///
    /// Routes to the per-instruction handler appropriate for the
    /// instruction kind. The 4.A skeleton handles only config-only
    /// instructions (ENV / WORKDIR / ENTRYPOINT / CMD / USER / EXPOSE /
    /// VOLUME / LABEL / ARG / SHELL / STOPSIGNAL / HEALTHCHECK /
    /// ONBUILD) by accepting them as no-ops at the rootfs level — those
    /// mutations land on the OCI image config in task 4.D, not on the
    /// working layer chain. Filesystem-mutating instructions return a
    /// [`BuildError::NotYetImplemented`] pointing at the Phase 4 task
    /// that delivers them.
    ///
    /// # Errors
    ///
    /// - [`BuildError::NotYetImplemented`] for RUN (delivered in 4.B)
    ///   and COPY / ADD (delivered in 4.C).
    //
    // `async fn` is intentional even though the 4.A skeleton body never
    // awaits: 4.B's RUN implementation spawns + awaits an HCS compute
    // system, and 4.C's COPY/ADD awaits the registry client for `ADD
    // <url>`. Keeping the signature `async` from day one avoids a
    // breaking change at every call site when those tasks land.
    #[allow(clippy::unused_async)]
    pub async fn execute_instruction(
        &self,
        _skeleton: &mut BuildSkeleton,
        instruction: &Instruction,
    ) -> Result<()> {
        match instruction {
            Instruction::Run(_) => Err(BuildError::not_yet_implemented(
                "RUN execution lands in Phase 4 task 4.B (HCS-backed transient compute system + \
                 wclayer::export_layer commit)",
            )),
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
) -> Result<(Vec<LayerRef>, BaseImageManifest)> {
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

    unpacker::unpack_windows_image(
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

    Ok((
        layer_refs,
        BaseImageManifest {
            image_ref: base_image_ref.to_string(),
            os: "windows".to_string(),
            os_version,
            arch: "amd64".to_string(),
            config_blob,
        },
    ))
}

/// Non-Windows hosts cannot drive HCS storage APIs, so the base layer
/// materialisation step refuses to proceed. The whole `WindowsBuilder`
/// public API still compiles on Linux/macOS so this crate's unit tests
/// can run across CI, but anyone actually invoking `build_skeleton` off-
/// Windows gets a precise error.
// Matches the Windows signature (async + RegistryAuth-typed config) so
// `build_skeleton` can call into it without per-platform plumbing. The
// `clippy::unused_async` allow is intentional — keeping the signature
// stable across `cfg`s avoids a duplicate call-site shape.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
async fn pull_and_materialise_base(
    _base_image_ref: &str,
    _config: &WindowsBuildConfig,
    _working_layer_chain_dir: &std::path::Path,
) -> Result<(Vec<LayerRef>, BaseImageManifest)> {
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::dockerfile::{CopyInstruction, EnvInstruction, RunInstruction, ShellOrExec};

    fn dummy_config() -> WindowsBuildConfig {
        WindowsBuildConfig {
            cache_dir: std::env::temp_dir().join("zlayer-wcow-skeleton-tests"),
            registry_auth: RegistryAuth::Anonymous,
            platform: WindowsBuildConfig::default_platform().to_string(),
            os_version_override: None,
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
        // We use the parser directly here because the live network
        // pull would require an `--ignored` integration test against
        // MCR + a Windows host. The shape we assert is the same shape
        // `build_skeleton` would produce after step 2 of its body
        // (Dockerfile::parse) before it hands off to the Windows-only
        // unpacker.
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
    async fn execute_instruction_run_returns_not_yet_implemented() {
        let builder = WindowsBuilder::new(dummy_config());
        let mut skel = dummy_skeleton();
        let run = Instruction::Run(RunInstruction::shell("cmd /c echo hello"));
        let err = builder
            .execute_instruction(&mut skel, &run)
            .await
            .expect_err("RUN must route to a NotYetImplemented error in the 4.A skeleton");
        assert!(
            matches!(err, BuildError::NotYetImplemented(ref msg) if msg.contains("4.B")),
            "RUN error must point at Phase 4 task 4.B, got: {err}"
        );
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
}
