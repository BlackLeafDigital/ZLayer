//! Native Windows builder backend using HCS (Host Compute Service).
//!
//! This backend builds Windows container images on a Windows host without
//! requiring Docker Desktop or buildah. It reuses the NTFS layer primitives
//! already shipped by `zlayer-agent` for running containers:
//!
//! - [`zlayer_agent::windows::scratch`] — writable (sandbox) layer creation via
//!   `HcsInitializeWritableLayer` / `HcsAttachLayerStorageFilter`.
//! - [`zlayer_agent::windows::layer`] — `BackupRead`/`BackupWrite` streaming
//!   for NTFS metadata-preserving layer capture.
//! - [`zlayer_agent::windows::wclayer`] — direct HCS layer storage wrappers
//!   (`HcsImportLayer`, `HcsExportLayer`, `HcsDestroyLayer`, …).
//!
//! # Build sequence
//!
//! 1. [`scratch`] pulls the parent chain (via
//!    [`zlayer_registry::client::ImagePuller::with_platform`] with a
//!    `windows/amd64` target) and materialises a writable layer on top.
//! 2. [`exec`] executes each shell-form / exec-form `RUN` instruction inside
//!    an ephemeral HCS compute system rooted at the scratch layer.
//! 3. File-producing instructions (`COPY` / `ADD`) copy content directly into
//!    the scratch layer's filesystem view — WCIFS is attached, so writes land
//!    in `sandbox.vhdx`.
//! 4. Metadata-only instructions (`ENV`, `LABEL`, `CMD`, `ENTRYPOINT`,
//!    `EXPOSE`, `VOLUME`, `USER`, `WORKDIR`, `STOPSIGNAL`, `SHELL`) accumulate
//!    into an in-memory image-config builder.
//! 5. [`layer`] diffs the scratch layer into a `BackupRead`-framed tar.gz blob
//!    (`application/vnd.oci.image.layer.v1.tar+gzip` media type, standard OCI
//!    but with the `os: windows` flag carried on the enclosing image config
//!    per the OCI image-spec §6).
//! 6. [`commit`] writes the final OCI image manifest + config JSON into the
//!    output registry. Layer blobs are stored content-addressed.
//!
//! # Deferred
//!
//! The following are intentionally scoped out of the first iteration. Each
//! has a `TODO(L-4-followup)` comment at the relevant call site:
//!
//! - **Multi-stage builds (`FROM ... AS builder`)** — single-stage only for
//!   now. The dispatch in [`HcsBackend::build_image`] rejects a dockerfile
//!   with more than one stage with a specific error message.
//! - **CPU / memory limits on build containers** — ephemeral build compute
//!   systems run unconstrained; adding resource caps is future work once the
//!   HCS backend has a resource configuration surface.
//! - **Inter-build cache reuse** — every build produces fresh layers. Layer
//!   digest hashing + local-registry lookups are planned follow-ups.

// `#[cfg(target_os = "windows")]` is applied by the parent `backend/mod.rs`
// module declaration, so it is not repeated here. This is the authoritative
// HCS-builder FFI boundary for `zlayer-builder`; `unsafe` is allowed here for
// the same reasons it is allowed in `zlayer-hcs`.
//
// `too_many_lines` and `items_after_test_module` are allowed because the
// backend functions intentionally keep their full HCS call sequences in
// a single body for readability, and some test modules predate later
// helper functions that ended up in the same file.
#![allow(
    unsafe_code,
    clippy::borrow_as_ptr,
    clippy::too_many_lines,
    clippy::items_after_test_module
)]

mod commit;
mod exec;
mod layer;
mod scratch;

pub use commit::{
    build_image_config_bytes, build_manifest_bytes, BuildCommitArtifacts, ImageConfigBuilder,
    OCI_IMAGE_CONFIG_MEDIA_TYPE, OCI_IMAGE_MANIFEST_MEDIA_TYPE, OCI_WINDOWS_LAYER_MEDIA_TYPE,
};

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::builder::{BuildOptions, BuiltImage, RegistryAuth};
use crate::dockerfile::{Dockerfile, DockerfileFromTarget, Instruction};
use crate::error::{BuildError, Result};
use crate::tui::BuildEvent;

use super::{BuildBackend, ImageOs};

/// Default scratch layer size in GiB used when a build does not specify one.
/// Matches the agent's production default so hosts that already tuned the
/// size don't see a different value at build time.
const DEFAULT_SCRATCH_SIZE_GB: u64 = 20;

/// Root directory under the user's local app data where the HCS builder
/// stages scratch layers, pulled base-image chains, and written OCI blobs.
///
/// Resolved via [`dirs::data_local_dir`] with a last-resort fallback to
/// `C:\ProgramData\zlayer\builder-hcs` when the per-user dir is unavailable
/// (for example when running inside a Windows Service session that has no
/// profile folder).
fn default_storage_root() -> PathBuf {
    if let Some(dir) = dirs::data_local_dir() {
        dir.join("zlayer").join("builder-hcs")
    } else {
        PathBuf::from(r"C:\ProgramData\zlayer\builder-hcs")
    }
}

/// Native Windows build backend.
///
/// Holds the on-disk scratch root (for per-build scratch layers, pulled base
/// chains, and written OCI artifacts) plus the registry client used to pull
/// Windows base images. Stateless otherwise — every `build_image` call
/// operates in its own isolated tree under `storage_root/<build-id>/`.
pub struct HcsBackend {
    /// Root directory for per-build state.
    storage_root: PathBuf,
    /// Registry client used to pull the parent chain. Configured with a
    /// Windows / amd64 target platform so multi-platform base images resolve
    /// to the correct manifest variant.
    registry: std::sync::Arc<zlayer_registry::ImagePuller>,
}

impl std::fmt::Debug for HcsBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HcsBackend")
            .field("storage_root", &self.storage_root)
            .finish_non_exhaustive()
    }
}

impl HcsBackend {
    /// Construct a new HCS backend with default storage root and a registry
    /// client targeting `windows/amd64`.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage root cannot be created or the blob
    /// cache cannot be initialised from the environment.
    pub async fn new() -> Result<Self> {
        let root = default_storage_root();
        Self::with_storage_root(root).await
    }

    /// Construct a new HCS backend with an explicit storage root.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage root cannot be created or the blob
    /// cache cannot be initialised from the environment.
    pub async fn with_storage_root(storage_root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&storage_root).map_err(|e| BuildError::ContextRead {
            path: storage_root.clone(),
            source: e,
        })?;

        // Configure the puller with an explicit Windows / amd64 target. That
        // routes multi-platform base images (e.g. mcr.microsoft.com's Server
        // Core / Nanoserver) to the correct manifest variant and avoids the
        // runtime-platform auto-detection that would otherwise hand us a
        // Linux manifest when we ask from a non-Windows host.
        let cache_type = zlayer_registry::CacheType::from_env()
            .map_err(|e| BuildError::registry_error(format!("HCS blob cache from env: {e}")))?;
        let blob_cache = cache_type
            .build()
            .await
            .map_err(|e| BuildError::registry_error(format!("open HCS blob cache: {e}")))?;
        let target = zlayer_spec::TargetPlatform::new(
            zlayer_spec::OsKind::Windows,
            zlayer_spec::ArchKind::Amd64,
        );
        let registry = std::sync::Arc::new(zlayer_registry::ImagePuller::with_platform(
            blob_cache, target,
        ));

        Ok(Self {
            storage_root,
            registry,
        })
    }

    /// Where per-build artifacts for a specific build id live on disk.
    fn build_dir(&self, build_id: &str) -> PathBuf {
        self.storage_root.join("builds").join(build_id)
    }

    /// Emit a `BuildEvent` to the TUI when a sender is wired up.
    fn send_event(event_tx: Option<&mpsc::Sender<BuildEvent>>, event: BuildEvent) {
        if let Some(tx) = event_tx {
            let _ = tx.send(event);
        }
    }
}

#[async_trait]
impl BuildBackend for HcsBackend {
    async fn build_image(
        &self,
        context: &Path,
        dockerfile: &Dockerfile,
        options: &BuildOptions,
        event_tx: Option<mpsc::Sender<BuildEvent>>,
    ) -> Result<BuiltImage> {
        let started = std::time::Instant::now();

        // Multi-stage builds are deferred to L-4-followup. Fail loudly rather
        // than silently ignoring extra stages.
        if dockerfile.stages.len() != 1 {
            return Err(BuildError::NotSupported {
                operation: format!(
                    "multi-stage Windows builds ({} stages) — HCS backend supports a single stage \
                     in the first iteration; track the follow-up at TODO(L-4-followup)",
                    dockerfile.stages.len()
                ),
            });
        }
        let stage = &dockerfile.stages[0];

        // Resolve the base image string. Stage references (COPY --from) and
        // scratch are rejected here — the single-stage restriction above
        // already rules out the former, and scratch is not a valid Windows
        // base in practice (no OS to run HCS processes against).
        let base_ref = match &stage.base_image {
            DockerfileFromTarget::Image(r) => r.to_string(),
            DockerfileFromTarget::Stage(name) => {
                return Err(BuildError::stage_not_found(name));
            }
            DockerfileFromTarget::Scratch => {
                return Err(BuildError::InvalidInstruction {
                    instruction: "FROM scratch".to_string(),
                    reason: "HCS builder requires a Windows base image — `scratch` cannot run HCS \
                             processes (no OS kernel, no cmd.exe). Use `mcr.microsoft.com/windows/\
                             nanoserver:...` or `.../servercore:...`."
                        .to_string(),
                });
            }
        };

        let build_id = new_build_id();
        let build_dir = self.build_dir(&build_id);
        std::fs::create_dir_all(&build_dir).map_err(|e| BuildError::ContextRead {
            path: build_dir.clone(),
            source: e,
        })?;

        let total_instructions_planned = stage.instructions.len();
        Self::send_event(
            event_tx.as_ref(),
            BuildEvent::BuildStarted {
                total_stages: 1,
                total_instructions: total_instructions_planned,
            },
        );
        Self::send_event(
            event_tx.as_ref(),
            BuildEvent::StageStarted {
                index: 0,
                name: stage.name.clone(),
                base_image: base_ref.clone(),
            },
        );

        info!(
            build_id = %build_id,
            base = %base_ref,
            instructions = total_instructions_planned,
            "HCS build starting"
        );

        // 1. Pull + unpack the Windows base image chain under build_dir/base.
        //    Also record the parent chain + base OS version for the final
        //    image config so `os.version` lines up with the base we used.
        let base_root = build_dir.join("base");
        let base_artifacts = scratch::prepare_base_chain(&self.registry, &base_ref, &base_root)
            .await
            .map_err(|e| {
                BuildError::registry_error(format!("pull windows base {base_ref}: {e}"))
            })?;

        // 2. Create the writable scratch layer on top of the parent chain.
        let scratch_path = build_dir.join("scratch");
        let scratch_layer = scratch::create_writable_layer(
            &scratch_path,
            &base_artifacts.parent_chain,
            options
                .platform
                .as_deref()
                .and_then(parse_scratch_size_gb)
                .unwrap_or(DEFAULT_SCRATCH_SIZE_GB),
        )
        .map_err(|e| BuildError::LayerCreate {
            message: format!("create scratch layer at {}: {e}", scratch_path.display()),
        })?;

        debug!(
            scratch = %scratch_layer.layer_path().display(),
            vhd_mount = %scratch_layer.vhd_mount_path(),
            "scratch layer ready"
        );

        // 3. Walk the instructions, dispatching to the right handler. We
        //    accumulate metadata into an ImageConfigBuilder and side-effects
        //    into the scratch layer directly.
        let mut config = ImageConfigBuilder::new();
        // Apply any config the base image carried forward (Env/Entrypoint/...)
        // — the `os` field is already pinned to "windows" by the builder.
        if let Some(ref base_cfg) = base_artifacts.base_config {
            config.inherit_from_base(base_cfg);
        }
        config.set_os_version(base_artifacts.os_version.clone());

        // Stateful translator: tracks the SHELL override across RUN/CMD/ENTRYPOINT.
        let mut translator = crate::buildah::DockerfileTranslator::new(ImageOs::Windows);

        for (inst_idx, instruction) in stage.instructions.iter().enumerate() {
            Self::send_event(
                event_tx.as_ref(),
                BuildEvent::InstructionStarted {
                    stage: 0,
                    index: inst_idx,
                    instruction: format!("{instruction:?}"),
                },
            );

            let inst_started = std::time::Instant::now();
            let dispatch_result = dispatch_instruction(
                instruction,
                context,
                scratch_layer.layer_path(),
                &base_artifacts,
                &mut translator,
                &mut config,
                event_tx.as_ref(),
            )
            .await;

            match dispatch_result {
                Ok(()) => {
                    #[allow(clippy::cast_possible_truncation)]
                    let elapsed = inst_started.elapsed().as_millis() as u64;
                    debug!(
                        stage = 0,
                        index = inst_idx,
                        elapsed_ms = elapsed,
                        "instruction complete"
                    );
                    Self::send_event(
                        event_tx.as_ref(),
                        BuildEvent::InstructionComplete {
                            stage: 0,
                            index: inst_idx,
                            cached: false,
                        },
                    );
                }
                Err(e) => {
                    // Best-effort teardown of the scratch layer before bailing
                    // so we don't leak WCIFS state.
                    if let Err(teardown_err) = scratch_layer.detach_and_destroy() {
                        warn!(
                            error = %teardown_err,
                            "failed to tear down scratch layer after instruction failure"
                        );
                    }
                    Self::send_event(
                        event_tx.as_ref(),
                        BuildEvent::BuildFailed {
                            error: e.to_string(),
                        },
                    );
                    return Err(e);
                }
            }
        }

        Self::send_event(event_tx.as_ref(), BuildEvent::StageComplete { index: 0 });

        // 4. Capture the NTFS diff between the scratch layer and the parent
        //    chain as a BackupRead-framed tar.gz blob.
        let export_dir = build_dir.join("export");
        let diff_blob = layer::capture_diff_blob(
            scratch_layer.layer_path(),
            &base_artifacts.parent_chain,
            &export_dir,
        )
        .map_err(|e| BuildError::LayerCreate {
            message: format!("capture NTFS diff at {}: {e}", export_dir.display()),
        })?;

        // Done with the scratch layer now — tear it down so WCIFS state is
        // released and the on-disk directory is removed. The layer blob has
        // already been fully read into memory.
        scratch_layer
            .detach_and_destroy()
            .map_err(|e| BuildError::LayerCreate {
                message: format!("tear down scratch layer: {e}"),
            })?;

        // 5. Write manifest + config + layer blobs to build_dir/oci.
        let oci_out = build_dir.join("oci");
        let artifacts =
            commit::write_oci_artifacts(&oci_out, &config, &base_artifacts.layer_blobs, &diff_blob)
                .map_err(|e| BuildError::LayerCreate {
                    message: format!("write OCI artifacts: {e}"),
                })?;

        #[allow(clippy::cast_possible_truncation)]
        let elapsed_ms = started.elapsed().as_millis() as u64;

        let image_id = artifacts.manifest_digest.clone();
        let mut tags = options.tags.clone();
        if tags.is_empty() {
            tags.push(format!("zlayer-windows-build:{build_id}"));
        }

        Self::send_event(
            event_tx.as_ref(),
            BuildEvent::BuildComplete {
                image_id: image_id.clone(),
            },
        );

        info!(
            build_id = %build_id,
            image_id = %image_id,
            elapsed_ms = elapsed_ms,
            layers = artifacts.layer_count,
            "HCS build finished"
        );

        Ok(BuiltImage {
            image_id,
            tags,
            layer_count: artifacts.layer_count,
            size: artifacts.total_size,
            build_time_ms: elapsed_ms,
            is_manifest: false,
        })
    }

    async fn push_image(&self, _tag: &str, _auth: Option<&RegistryAuth>) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "HCS backend push — push is routed through the registry client directly; \
                        wire this path when the HCS builder gains a push integration \
                        (TODO(L-4-followup))"
                .to_string(),
        })
    }

    async fn tag_image(&self, _image: &str, _new_tag: &str) -> Result<()> {
        // The OCI artifacts written by `write_oci_artifacts` record the tag
        // embedded in the index annotation; a retag post-build is a pure
        // manifest-index rewrite. Exposing a knob for it lands with the push
        // path above.
        Err(BuildError::NotSupported {
            operation: "HCS backend retag — tags are embedded in the OCI index annotations at \
                        build time; a standalone retag lands with the push path \
                        (TODO(L-4-followup))"
                .to_string(),
        })
    }

    async fn manifest_create(&self, _name: &str) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "HCS backend manifest create — manifest lists are buildah-specific; the \
                        HCS builder produces a single platform-specific image. Use a Linux peer \
                        with buildah for multi-platform manifest composition."
                .to_string(),
        })
    }

    async fn manifest_add(&self, _manifest: &str, _image: &str) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "HCS backend manifest add — see manifest_create for rationale".to_string(),
        })
    }

    async fn manifest_push(&self, _name: &str, _destination: &str) -> Result<()> {
        Err(BuildError::NotSupported {
            operation: "HCS backend manifest push — see manifest_create for rationale".to_string(),
        })
    }

    async fn is_available(&self) -> bool {
        // HCS is present on any supported Windows Server / desktop build; we
        // rely on `cfg(target_os = "windows")` at compile time. A deeper probe
        // would open a dummy operation handle, but that carries measurable
        // cost at every detect_backend() call so we keep this cheap.
        true
    }

    fn name(&self) -> &'static str {
        "hcs"
    }
}

/// Dispatch a single instruction to the right handler. Split out of the
/// main build loop for readability and to keep the handler surface small.
async fn dispatch_instruction(
    instruction: &Instruction,
    context: &Path,
    scratch_root: &Path,
    base: &scratch::BaseChainArtifacts,
    translator: &mut crate::buildah::DockerfileTranslator,
    config: &mut ImageConfigBuilder,
    event_tx: Option<&mpsc::Sender<BuildEvent>>,
) -> Result<()> {
    match instruction {
        Instruction::Run(run) => {
            // Shell-form respects the translator's current shell override;
            // exec-form runs verbatim.
            exec::run_in_compute_system(run, scratch_root, base, translator, config, event_tx).await
        }
        Instruction::Copy(copy) => {
            if copy.from.is_some() {
                return Err(BuildError::NotSupported {
                    operation: "COPY --from (multi-stage) — HCS backend supports single-stage \
                                builds in the first iteration (TODO(L-4-followup))"
                        .to_string(),
                });
            }
            exec::copy_into_scratch(context, scratch_root, &copy.sources, &copy.destination)
        }
        Instruction::Add(add) => {
            // ADD with URLs or auto-extraction is out of scope for the first
            // cut; mirror the buildah backend's shape and treat it as a
            // plain copy when sources are local filesystem paths.
            if add.sources.iter().any(|s| s.starts_with("http")) {
                return Err(BuildError::NotSupported {
                    operation: "ADD <url> — HCS backend does not yet fetch URLs; use RUN with \
                                curl/Invoke-WebRequest or a multi-stage Linux builder \
                                (TODO(L-4-followup))"
                        .to_string(),
                });
            }
            exec::copy_into_scratch(context, scratch_root, &add.sources, &add.destination)
        }
        Instruction::Env(env) => {
            for (k, v) in &env.vars {
                config.push_env(k, v);
            }
            Ok(())
        }
        Instruction::Workdir(dir) => {
            config.set_working_dir(dir);
            // Mirror Docker's WORKDIR semantics: create the dir so a later
            // process can chdir into it. We create it through the scratch
            // layer (WCIFS) so the result persists into the final blob.
            exec::ensure_workdir(scratch_root, dir)
        }
        Instruction::Expose(expose) => {
            config.add_exposed_port(
                expose.port,
                matches!(expose.protocol, crate::dockerfile::ExposeProtocol::Tcp),
            );
            Ok(())
        }
        Instruction::Label(labels) => {
            for (k, v) in labels {
                config.add_label(k, v);
            }
            Ok(())
        }
        Instruction::User(user) => {
            config.set_user(user);
            Ok(())
        }
        Instruction::Entrypoint(cmd) => {
            config.set_entrypoint(translator, cmd);
            Ok(())
        }
        Instruction::Cmd(cmd) => {
            config.set_cmd(translator, cmd);
            Ok(())
        }
        Instruction::Volume(paths) => {
            for p in paths {
                config.add_volume(p);
            }
            Ok(())
        }
        Instruction::Shell(shell) => {
            translator.set_shell_override(shell.clone());
            config.set_shell(shell.clone());
            Ok(())
        }
        Instruction::Arg(_) => {
            // ARG is handled during variable expansion upstream of the
            // backend; no image-level side effect here.
            Ok(())
        }
        Instruction::Stopsignal(signal) => {
            config.set_stop_signal(signal);
            Ok(())
        }
        Instruction::Healthcheck(hc) => {
            config.set_healthcheck(hc.clone());
            Ok(())
        }
        Instruction::Onbuild(_) => {
            warn!("ONBUILD instruction ignored in HCS builder (not supported)");
            Ok(())
        }
    }
}

/// Parse a `ZLAYER_HCS_SCRATCH_SIZE_GB=<n>` style hint out of the user's
/// `--platform` flag.
///
/// `BuildOptions` does not currently carry an explicit scratch-size knob and
/// plumbing one through would churn every caller. Instead we accept a
/// `scratch:<n>g` suffix on the platform string — cheap to thread through,
/// trivially removable when a real field lands.
fn parse_scratch_size_gb(platform: &str) -> Option<u64> {
    let s = platform
        .rsplit(',')
        .find_map(|chunk| chunk.trim().strip_prefix("scratch:"))?;
    let digits = s.strip_suffix('g').or_else(|| s.strip_suffix('G'))?;
    digits.parse::<u64>().ok()
}

/// Generate a fresh build identifier. Matches the shape the buildah backend
/// uses (6-hex chars from a time + pid + counter hash) so log lines line up
/// across backends.
fn new_build_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = u128::from(std::process::id());
    let count = u128::from(COUNTER.fetch_add(1, Ordering::Relaxed));
    let mixed = nanos ^ (pid.rotate_left(17)) ^ count.rotate_left(33);
    format!("{:012x}", mixed & 0xFFFF_FFFF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_id_is_unique_and_hex_shaped() {
        let a = new_build_id();
        let b = new_build_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 12);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn parse_scratch_size_extracts_gb_suffix() {
        assert_eq!(parse_scratch_size_gb("windows/amd64,scratch:40g"), Some(40));
        assert_eq!(parse_scratch_size_gb("scratch:2G"), Some(2));
        assert_eq!(parse_scratch_size_gb("windows/amd64"), None);
        assert_eq!(parse_scratch_size_gb(""), None);
    }

    #[test]
    fn default_storage_root_ends_in_zlayer_builder_hcs() {
        let root = default_storage_root();
        let s = root.to_string_lossy().to_ascii_lowercase();
        assert!(s.contains("zlayer"));
        assert!(s.contains("builder-hcs"));
    }
}
