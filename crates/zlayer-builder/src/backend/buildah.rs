//! Buildah-backed build backend.
//!
//! Wraps [`BuildahExecutor`] to implement the [`BuildBackend`] trait.
//! Contains the full buildah build orchestration loop: stage walking,
//! container creation, instruction execution, and commit.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;

use tracing::{debug, info};

use crate::buildah::{BuildahCommand, BuildahExecutor, DockerfileTranslator};
use crate::builder::{BuildOptions, BuiltImage, PullBaseMode, RegistryAuth};
use crate::dockerfile::{
    expand_variables, Dockerfile, DockerfileFromTarget, EnvInstruction, Instruction, RunMount,
    ShellOrExec, Stage,
};
use crate::error::{BuildError, Result};
use crate::tui::BuildEvent;

use super::{BuildBackend, ImageOs};

// ---------------------------------------------------------------------------
// LayerCacheTracker (moved from builder.rs)
// ---------------------------------------------------------------------------

/// Tracks layer cache state during builds.
///
/// Maintains a mapping of instruction cache keys combined with base layer
/// identifiers to determine if a layer was previously built and can be
/// served from cache.
#[derive(Debug, Default)]
struct LayerCacheTracker {
    /// Maps (`instruction_cache_key`, `base_layer_id`) -> `was_cached`
    known_layers: HashMap<(String, String), bool>,
}

impl LayerCacheTracker {
    fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    fn is_cached(&self, instruction_key: &str, base_layer: &str) -> bool {
        self.known_layers
            .get(&(instruction_key.to_string(), base_layer.to_string()))
            .copied()
            .unwrap_or(false)
    }

    fn record(&mut self, instruction_key: String, base_layer: String, cached: bool) {
        self.known_layers
            .insert((instruction_key, base_layer), cached);
    }

    #[allow(dead_code, clippy::unused_self)]
    fn detect_cache_hit(
        &self,
        _instruction: &Instruction,
        _execution_time_ms: u64,
        _output: &str,
    ) -> bool {
        // TODO: Implement cache hit detection heuristics
        false
    }
}

// ---------------------------------------------------------------------------
// BuildahBackend
// ---------------------------------------------------------------------------

/// Build backend that delegates to the `buildah` CLI.
pub struct BuildahBackend {
    executor: BuildahExecutor,
}

impl BuildahBackend {
    /// Try to create a new `BuildahBackend`.
    ///
    /// Returns `Ok` if buildah is found and functional, `Err` otherwise.
    ///
    /// # Errors
    ///
    /// Returns an error if buildah is not installed or is not responding.
    pub async fn try_new() -> Result<Self> {
        let executor = BuildahExecutor::new_async().await?;
        if !executor.is_available().await {
            return Err(crate::error::BuildError::BuildahNotFound {
                message: "buildah is installed but not responding".into(),
            });
        }
        Ok(Self { executor })
    }

    /// Create a new `BuildahBackend`, returning an error if buildah is not available.
    ///
    /// # Errors
    ///
    /// Returns an error if buildah is not installed or cannot be initialized.
    pub async fn new() -> Result<Self> {
        let executor = BuildahExecutor::new_async().await?;
        Ok(Self { executor })
    }

    /// Create a `BuildahBackend` from an existing executor.
    #[must_use]
    pub fn with_executor(executor: BuildahExecutor) -> Self {
        Self { executor }
    }

    /// Borrow the inner executor (useful for low-level operations).
    #[must_use]
    pub fn executor(&self) -> &BuildahExecutor {
        &self.executor
    }

    // -----------------------------------------------------------------------
    // Build orchestration helpers
    // -----------------------------------------------------------------------

    /// Resolve which stages need to be built.
    #[allow(clippy::unused_self)]
    fn resolve_stages<'a>(
        &self,
        dockerfile: &'a Dockerfile,
        target: Option<&str>,
    ) -> Result<Vec<&'a Stage>> {
        if let Some(target) = target {
            Self::resolve_target_stages(dockerfile, target)
        } else {
            Ok(dockerfile.stages.iter().collect())
        }
    }

    /// Resolve stages needed for a specific target.
    fn resolve_target_stages<'a>(
        dockerfile: &'a Dockerfile,
        target: &str,
    ) -> Result<Vec<&'a Stage>> {
        let target_stage = dockerfile
            .get_stage(target)
            .ok_or_else(|| BuildError::stage_not_found(target))?;

        let mut stages: Vec<&Stage> = Vec::new();
        for stage in &dockerfile.stages {
            stages.push(stage);
            if stage.index == target_stage.index {
                break;
            }
        }
        Ok(stages)
    }

    /// Resolve a base image reference to an actual image name.
    ///
    /// Resolution chain for short (unqualified) image names:
    /// 1. Check `default_registry` for the image (if configured)
    /// 2. Fall back to Docker Hub qualification (`docker.io/library/...`)
    async fn resolve_base_image(
        &self,
        image_ref: &DockerfileFromTarget,
        stage_images: &HashMap<String, String>,
        options: &BuildOptions,
    ) -> Result<String> {
        match image_ref {
            DockerfileFromTarget::Stage(name) => {
                return stage_images
                    .get(name)
                    .cloned()
                    .ok_or_else(|| BuildError::stage_not_found(name));
            }
            DockerfileFromTarget::Scratch => return Ok("scratch".to_string()),
            DockerfileFromTarget::Image(_) => {}
        }

        // Check if name is already fully qualified (has registry hostname).
        let is_qualified = match image_ref {
            DockerfileFromTarget::Image(r) => {
                let repo = r.repository();
                let first = repo.split('/').next().unwrap_or("");
                first.contains('.') || first.contains(':') || first == "localhost"
            }
            _ => false,
        };

        // For unqualified names, try default registry first.
        if !is_qualified {
            if let Some(resolved) = self.try_resolve_from_sources(image_ref, options).await {
                return Ok(resolved);
            }
        }

        // Fall back: rely on oci-spec normalization performed during parse.
        // Reconstruct the fully-qualified reference string from the parsed
        // ImageReference (registry/repository[:tag][@digest]).
        match image_ref {
            DockerfileFromTarget::Image(r) => {
                let mut result = format!("{}/{}", r.registry(), r.repository());
                if let Some(t) = r.tag() {
                    result.push(':');
                    result.push_str(t);
                }
                if let Some(d) = r.digest() {
                    result.push('@');
                    result.push_str(d);
                }
                if r.tag().is_none() && r.digest().is_none() {
                    result.push_str(":latest");
                }
                Ok(result)
            }
            _ => unreachable!("Stage and Scratch handled above"),
        }
    }

    /// Try to resolve an unqualified image from default registry.
    ///
    /// Returns `Some(fully_qualified_name)` if found, `None` to fall back to docker.io.
    #[allow(clippy::unused_async)]
    async fn try_resolve_from_sources(
        &self,
        image_ref: &DockerfileFromTarget,
        options: &BuildOptions,
    ) -> Option<String> {
        let (name, tag_str) = match image_ref {
            DockerfileFromTarget::Image(r) => (
                r.repository().to_string(),
                r.tag().unwrap_or("latest").to_string(),
            ),
            _ => return None,
        };

        // Check configured default registry.
        if let Some(ref registry) = options.default_registry {
            let qualified = format!("{registry}/{name}:{tag_str}");
            debug!("Checking default registry for image: {}", qualified);
            return Some(qualified);
        }

        None
    }

    /// Create a working container from an image.
    async fn create_container(
        &self,
        image: &str,
        platform: Option<&str>,
        pull: PullBaseMode,
    ) -> Result<String> {
        let mut cmd = BuildahCommand::new("from").arg_opt("--platform", platform);

        match pull {
            PullBaseMode::Newer => cmd = cmd.arg("--pull=newer"),
            PullBaseMode::Always => cmd = cmd.arg("--pull=always"),
            PullBaseMode::Never => { /* no flag — let buildah use whatever is local */ }
        }

        cmd = cmd.arg(image);

        let output = self.executor.execute_checked(&cmd).await?;
        Ok(output.stdout.trim().to_string())
    }

    /// Commit a container to create an image.
    async fn commit_container(
        &self,
        container: &str,
        image_name: &str,
        format: Option<&str>,
        squash: bool,
    ) -> Result<String> {
        let cmd = BuildahCommand::commit_with_opts(container, image_name, format, squash);
        let output = self.executor.execute_checked(&cmd).await?;
        Ok(output.stdout.trim().to_string())
    }

    /// Tag an image with an additional tag.
    async fn tag_image_internal(&self, image: &str, tag: &str) -> Result<()> {
        let cmd = BuildahCommand::tag(image, tag);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    /// Push an image to a registry.
    async fn push_image_internal(&self, tag: &str, auth: Option<&RegistryAuth>) -> Result<()> {
        let mut cmd = BuildahCommand::push(tag);
        if let Some(auth) = auth {
            cmd = cmd
                .arg("--creds")
                .arg(format!("{}:{}", auth.username, auth.password));
        }
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    /// Pull an external image referenced by `COPY --from=<image-ref>` so that
    /// buildah's local image store contains it before the copy runs.
    ///
    /// Buildah's `copy --from=<image>` does not auto-pull from a remote
    /// registry; it resolves the reference against the configured local
    /// storage only. We therefore have to issue a `buildah pull` ourselves.
    /// The pull lands in the same `--root` / `--runroot` configured on the
    /// executor, so subsequent `copy --from=<image-ref>` invocations will
    /// resolve it correctly.
    ///
    /// `pull_mode` is the same `--pull` policy we use for the stage base
    /// image, so external `--from` images obey the user-selected freshness
    /// policy (`Newer` / `Always` / `Never`).
    async fn pull_external_image(&self, image: &str, pull_mode: PullBaseMode) -> Result<()> {
        let policy = match pull_mode {
            PullBaseMode::Newer => Some("newer"),
            PullBaseMode::Always => Some("always"),
            // `Never` means "use whatever is in local storage". Skip the pull
            // entirely; if the image isn't already cached the downstream
            // `buildah copy --from=...` will fail with a clear error.
            PullBaseMode::Never => return Ok(()),
        };

        let cmd = BuildahCommand::pull(image, policy);
        debug!("Pulling external COPY --from image: {}", image);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    /// Send an event to the TUI (if configured).
    fn send_event(event_tx: Option<&mpsc::Sender<BuildEvent>>, event: BuildEvent) {
        if let Some(tx) = event_tx {
            let _ = tx.send(event);
        }
    }
}

#[async_trait::async_trait]
impl BuildBackend for BuildahBackend {
    #[allow(clippy::too_many_lines)]
    async fn build_image(
        &self,
        _context: &Path,
        dockerfile: &Dockerfile,
        options: &BuildOptions,
        event_tx: Option<mpsc::Sender<BuildEvent>>,
    ) -> Result<BuiltImage> {
        let start_time = std::time::Instant::now();
        let build_id = generate_build_id();

        debug!(
            "BuildahBackend: starting build (build_id: {}, {} stages)",
            build_id,
            dockerfile.stages.len()
        );

        // Determine stages to build.
        let stages = self.resolve_stages(dockerfile, options.target.as_deref())?;
        debug!("Building {} stages", stages.len());

        // Emit the total stage / instruction count up-front so the TUI
        // progress bar has a stable denominator (otherwise it would
        // grow in lockstep with the numerator as events arrive).
        let total_instructions_planned: usize = stages.iter().map(|s| s.instructions.len()).sum();
        Self::send_event(
            event_tx.as_ref(),
            BuildEvent::BuildStarted {
                total_stages: stages.len(),
                total_instructions: total_instructions_planned,
            },
        );

        // Build each stage.
        let mut stage_images: HashMap<String, String> = HashMap::new();
        // Track the final WORKDIR for each committed stage, used to resolve
        // relative source paths in COPY --from instructions.
        let mut stage_workdirs: HashMap<String, String> = HashMap::new();
        // Track external images we have already pulled this build, so a
        // multi-line `COPY --from=ghcr.io/...:tag` sequence doesn't re-pull
        // the same image once per instruction.
        let mut pulled_external_images: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut final_container: Option<String> = None;
        let mut total_instructions = 0;

        // Initialize the layer cache tracker for this build session.
        let mut cache_tracker = LayerCacheTracker::new();

        // An empty host directory used as the source for `buildah copy` when
        // translating WORKDIR instructions. `buildah copy <ctr> <empty>/.
        // <dir>` materializes `<dir>` in the rootfs without running a process
        // inside the container, so WORKDIR works on `gcr.io/distroless/*`,
        // `scratch`, and other shell-less bases that the legacy
        // `buildah run -- mkdir -p` path can't handle. Held for the entire
        // build; dropped (auto-removed) when this function returns. The
        // directory MUST stay empty — never write into it.
        let workdir_empty_src = tempfile::Builder::new()
            .prefix("zlayer-workdir-mkdir-")
            .tempdir()
            .map_err(BuildError::from)?;

        // Build a translator configured for this build. The BuildahBackend is
        // always Linux-targeted (Windows builds dispatch through HcsBackend),
        // so we pin `ImageOs::Linux` here. `host_network` is forwarded from
        // `BuildOptions` so the `--host-network` CLI flag actually reaches
        // every translated `RUN` as `--net=host` on the buildah invocation.
        // `with_empty_src_dir` routes WORKDIR through `buildah copy` instead
        // of `buildah run -- mkdir -p` so the build works on shell-less
        // bases (distroless, scratch).
        let translator = DockerfileTranslator::new(ImageOs::Linux)
            .with_host_network(options.host_network)
            .with_empty_src_dir(workdir_empty_src.path().to_path_buf());

        for (stage_idx, stage) in stages.iter().enumerate() {
            let is_final_stage = stage_idx == stages.len() - 1;

            // Build-time variable bindings for this stage, mirroring the macOS
            // sandbox builder so the buildah CLI path actually expands `${ARG}`
            // / build args (e.g. `${FORGEJO_TOKEN}`) in
            // ENV/RUN/COPY/ADD/WORKDIR/USER/LABEL instead of handing them to
            // buildah literally. ARG scope is per-stage (Docker semantics): we
            // reset to the passed build args + the global (pre-FROM) ARG
            // defaults at each stage, then accumulate stage-local ARG/ENV
            // bindings as we walk the instructions.
            let mut arg_values: HashMap<String, String> = options.build_args.clone();
            for global_arg in &dockerfile.global_args {
                if !arg_values.contains_key(&global_arg.name) {
                    if let Some(default) = &global_arg.default {
                        arg_values.insert(global_arg.name.clone(), default.clone());
                    }
                }
            }
            let mut env_values: HashMap<String, String> = HashMap::new();

            Self::send_event(
                event_tx.as_ref(),
                BuildEvent::StageStarted {
                    index: stage_idx,
                    name: stage.name.clone(),
                    base_image: stage.base_image.to_string(),
                },
            );

            // Create container from base image.
            let base = self
                .resolve_base_image(&stage.base_image, &stage_images, options)
                .await?;
            let container_id = self
                .create_container(&base, options.platform.as_deref(), options.pull)
                .await?;

            debug!(
                "Created container {} for stage {} (base: {})",
                container_id,
                stage.identifier(),
                base
            );

            // Track the current base layer for cache key computation.
            let mut current_base_layer = container_id.clone();

            // Track the current WORKDIR for this stage.
            let mut current_workdir = match &stage.base_image {
                DockerfileFromTarget::Stage(name) => stage_workdirs
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| String::from("/")),
                _ => String::from("/"),
            };

            // Execute instructions.
            for (inst_idx, instruction) in stage.instructions.iter().enumerate() {
                Self::send_event(
                    event_tx.as_ref(),
                    BuildEvent::InstructionStarted {
                        stage: stage_idx,
                        index: inst_idx,
                        instruction: format!("{instruction:?}"),
                    },
                );

                let instruction_cache_key = instruction.cache_key();
                let instruction_start = std::time::Instant::now();

                // Resolve COPY --from references.
                //
                // There are three cases:
                //
                // 1. `from` matches a previously-committed stage in this build
                //    (by name or numeric index). Replace the reference with the
                //    committed intermediate image name, and rewrite relative
                //    source paths using the source stage's WORKDIR.
                //
                // 2. `from` is an external image reference (e.g.
                //    `ghcr.io/astral-sh/uv:0.5.0` or `docker.io/library/alpine`).
                //    Buildah's `copy --from=<image>` does not auto-pull, so we
                //    explicitly `buildah pull` the image into local storage
                //    once per build, then leave the `--from` value untouched
                //    so buildah resolves it against the now-populated store.
                //
                // 3. No `from` at all — pass the instruction through verbatim.
                let resolved_instruction;
                let instruction_ref = if let Instruction::Copy(copy) = instruction {
                    if let Some(ref from) = copy.from {
                        if let Some(image_name) = stage_images.get(from) {
                            // Case 1: known stage.
                            let mut resolved_copy = copy.clone();
                            resolved_copy.from = Some(image_name.clone());

                            // Resolve relative source paths using the source stage's WORKDIR.
                            if let Some(source_workdir) = stage_workdirs.get(from) {
                                resolved_copy.sources = resolved_copy
                                    .sources
                                    .iter()
                                    .map(|src| {
                                        if src.starts_with('/') {
                                            src.clone()
                                        } else if source_workdir == "/" {
                                            format!("/{src}")
                                        } else {
                                            format!("{source_workdir}/{src}")
                                        }
                                    })
                                    .collect();
                            }

                            resolved_instruction = Instruction::Copy(resolved_copy);
                            &resolved_instruction
                        } else {
                            // Case 2: external image reference. Pull it into
                            // local storage so buildah's downstream `copy
                            // --from=<image>` can resolve it.
                            if !pulled_external_images.contains(from) {
                                self.pull_external_image(from, options.pull).await?;
                                pulled_external_images.insert(from.clone());
                            }
                            instruction
                        }
                    } else {
                        // Case 3.
                        instruction
                    }
                } else {
                    instruction
                };

                // Inject default cache mounts into RUN instructions.
                let instruction_with_defaults;
                let instruction_ref = if options.default_cache_mounts.is_empty() {
                    instruction_ref
                } else if let Instruction::Run(run) = instruction_ref {
                    let mut merged = run.clone();
                    for default_mount in &options.default_cache_mounts {
                        let RunMount::Cache { target, .. } = default_mount else {
                            continue;
                        };
                        let already_has = merged
                            .mounts
                            .iter()
                            .any(|m| matches!(m, RunMount::Cache { target: t, .. } if t == target));
                        if !already_has {
                            merged.mounts.push(default_mount.clone());
                        }
                    }
                    instruction_with_defaults = Instruction::Run(merged);
                    &instruction_with_defaults
                } else {
                    instruction_ref
                };

                // Expand build-time variables (`${ARG}` / `$ENV`) using the
                // bindings accumulated for this stage so far, and fold ARG/ENV
                // instructions into those bindings for subsequent instructions.
                // This is what makes build args reach the buildah build: without
                // it, `RUN ... ${FORGEJO_TOKEN}`, `ENV TOKEN=${FORGEJO_TOKEN}`,
                // `COPY ${SRC} ...` and friends were handed to buildah
                // unexpanded.
                let expanded_instruction =
                    expand_instruction(instruction_ref, &mut arg_values, &mut env_values);
                let instruction_ref = &expanded_instruction;

                let is_run_instruction = matches!(instruction_ref, Instruction::Run(_));
                let max_attempts = if is_run_instruction {
                    options.retries + 1
                } else {
                    1
                };

                // Construct a fresh translator per instruction so we
                // preserve the historical byte-for-byte behavior of the
                // pre-host-network `BuildahCommand::from_instruction`
                // wrapper (which also constructed a fresh translator each
                // call and therefore did not persist SHELL state across
                // instructions). The host_network flag is sticky for the
                // whole build, so we read it from `options`.
                let mut translator = translator.clone();
                let commands = translator.translate(&container_id, instruction_ref);

                let mut combined_output = String::new();
                for cmd in commands {
                    let mut last_output = None;

                    for attempt in 1..=max_attempts {
                        if attempt > 1 {
                            tracing::warn!(
                                "Retrying step (attempt {}/{})...",
                                attempt,
                                max_attempts
                            );
                            Self::send_event(
                                event_tx.as_ref(),
                                BuildEvent::Output {
                                    line: format!(
                                        "⟳ Retrying step (attempt {attempt}/{max_attempts})..."
                                    ),
                                    is_stderr: false,
                                },
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        }

                        let event_tx_clone = event_tx.clone();
                        let output = self
                            .executor
                            .execute_streaming(&cmd, |is_stdout, line| {
                                Self::send_event(
                                    event_tx_clone.as_ref(),
                                    BuildEvent::Output {
                                        line: line.to_string(),
                                        is_stderr: !is_stdout,
                                    },
                                );
                            })
                            .await?;

                        combined_output.push_str(&output.stdout);
                        combined_output.push_str(&output.stderr);

                        if output.success() {
                            last_output = Some(output);
                            break;
                        }

                        last_output = Some(output);
                    }

                    let output = last_output.unwrap();
                    if !output.success() {
                        Self::send_event(
                            event_tx.as_ref(),
                            BuildEvent::BuildFailed {
                                error: output.stderr.clone(),
                            },
                        );

                        // Cleanup container.
                        let _ = self
                            .executor
                            .execute(&BuildahCommand::rm(&container_id))
                            .await;

                        return Err(BuildError::buildah_execution(
                            cmd.to_command_string(),
                            output.exit_code,
                            output.stderr,
                        ));
                    }
                }

                #[allow(clippy::cast_possible_truncation)]
                let instruction_elapsed_ms = instruction_start.elapsed().as_millis() as u64;

                // Track WORKDIR changes for later COPY --from resolution.
                if let Instruction::Workdir(dir) = instruction {
                    current_workdir.clone_from(dir);
                }

                // Attempt to detect if this was a cache hit.
                let cached = cache_tracker.detect_cache_hit(
                    instruction,
                    instruction_elapsed_ms,
                    &combined_output,
                );

                cache_tracker.record(
                    instruction_cache_key.clone(),
                    current_base_layer.clone(),
                    cached,
                );

                current_base_layer = format!("{current_base_layer}:{instruction_cache_key}");

                Self::send_event(
                    event_tx.as_ref(),
                    BuildEvent::InstructionComplete {
                        stage: stage_idx,
                        index: inst_idx,
                        cached,
                    },
                );

                total_instructions += 1;
            }

            // Handle stage completion.
            if let Some(name) = &stage.name {
                let image_name = format!("zlayer-build-{build_id}-stage-{name}");
                self.commit_container(&container_id, &image_name, options.format.as_deref(), false)
                    .await?;
                stage_images.insert(name.clone(), image_name.clone());
                stage_workdirs.insert(name.clone(), current_workdir.clone());

                // Also add by index.
                stage_images.insert(stage.index.to_string(), image_name.clone());
                stage_workdirs.insert(stage.index.to_string(), current_workdir.clone());

                if is_final_stage {
                    final_container = Some(container_id);
                } else {
                    let _ = self
                        .executor
                        .execute(&BuildahCommand::rm(&container_id))
                        .await;
                }
            } else if is_final_stage {
                final_container = Some(container_id);
            } else {
                let image_name = format!("zlayer-build-{}-stage-{}", build_id, stage.index);
                self.commit_container(&container_id, &image_name, options.format.as_deref(), false)
                    .await?;
                stage_images.insert(stage.index.to_string(), image_name);
                stage_workdirs.insert(stage.index.to_string(), current_workdir.clone());
                let _ = self
                    .executor
                    .execute(&BuildahCommand::rm(&container_id))
                    .await;
            }

            Self::send_event(
                event_tx.as_ref(),
                BuildEvent::StageComplete { index: stage_idx },
            );
        }

        // Commit final image.
        let final_container = final_container.ok_or_else(|| BuildError::InvalidInstruction {
            instruction: "build".to_string(),
            reason: "No stages to build".to_string(),
        })?;

        let image_name = options
            .tags
            .first()
            .cloned()
            .unwrap_or_else(|| format!("zlayer-build:{}", chrono_lite_timestamp()));

        let image_id = self
            .commit_container(
                &final_container,
                &image_name,
                options.format.as_deref(),
                options.squash,
            )
            .await?;

        info!("Committed final image: {} ({})", image_name, image_id);

        // Apply additional tags.
        for tag in options.tags.iter().skip(1) {
            self.tag_image_internal(&image_id, tag).await?;
            debug!("Applied tag: {}", tag);
        }

        // Cleanup.
        let _ = self
            .executor
            .execute(&BuildahCommand::rm(&final_container))
            .await;

        // Cleanup intermediate stage images.
        for (_, img) in stage_images {
            let _ = self.executor.execute(&BuildahCommand::rmi(&img)).await;
        }

        // Push if requested.
        if options.push {
            for tag in &options.tags {
                self.push_image_internal(tag, options.registry_auth.as_ref())
                    .await?;
                info!("Pushed image: {}", tag);
            }
        }

        #[allow(clippy::cast_possible_truncation)]
        let build_time_ms = start_time.elapsed().as_millis() as u64;

        Self::send_event(
            event_tx.as_ref(),
            BuildEvent::BuildComplete {
                image_id: image_id.clone(),
            },
        );

        info!(
            "Build completed in {}ms: {} with {} tags",
            build_time_ms,
            image_id,
            options.tags.len()
        );

        Ok(BuiltImage {
            image_id,
            tags: options.tags.clone(),
            layer_count: total_instructions,
            size: 0, // TODO: get actual size via buildah inspect
            build_time_ms,
            is_manifest: false,
        })
    }

    async fn push_image(&self, tag: &str, auth: Option<&RegistryAuth>) -> Result<()> {
        self.push_image_internal(tag, auth).await
    }

    async fn tag_image(&self, image: &str, new_tag: &str) -> Result<()> {
        self.tag_image_internal(image, new_tag).await
    }

    async fn manifest_create(&self, name: &str) -> Result<()> {
        let cmd = BuildahCommand::manifest_create(name);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    async fn manifest_add(&self, manifest: &str, image: &str) -> Result<()> {
        let cmd = BuildahCommand::manifest_add(manifest, image);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    async fn manifest_push(&self, name: &str, destination: &str) -> Result<()> {
        let cmd = BuildahCommand::manifest_push(name, destination);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    async fn is_available(&self) -> bool {
        self.executor.is_available().await
    }

    fn name(&self) -> &'static str {
        "buildah"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Expand `${VAR}` / `$VAR` references in an instruction's build-time-expandable
/// fields using the current ARG + ENV bindings, and fold `ARG` / `ENV`
/// instructions into those bindings so later instructions observe them.
///
/// Mirrors the macOS sandbox builder (`sandbox_builder::execute_instruction`):
/// Docker performs build-time variable substitution on ENV/COPY/ADD/WORKDIR/
/// USER/LABEL, and we additionally expand the RUN command so build args reach
/// the executed shell. Without this the buildah CLI backend handed
/// `${FORGEJO_TOKEN}` (and any other build arg) straight to buildah unexpanded —
/// the bug this fixes. The expansion engine ([`expand_variables`]) is shared
/// with the sandbox path so both backends resolve a Dockerfile identically.
fn expand_instruction(
    instruction: &Instruction,
    arg_values: &mut HashMap<String, String>,
    env_values: &mut HashMap<String, String>,
) -> Instruction {
    match instruction {
        Instruction::Run(run) => {
            let mut run = run.clone();
            run.command = match &run.command {
                ShellOrExec::Shell(s) => {
                    ShellOrExec::Shell(expand_variables(s, arg_values, env_values))
                }
                ShellOrExec::Exec(args) => ShellOrExec::Exec(
                    args.iter()
                        .map(|a| expand_variables(a, arg_values, env_values))
                        .collect(),
                ),
            };
            Instruction::Run(run)
        }
        Instruction::Env(env) => {
            let mut vars = HashMap::with_capacity(env.vars.len());
            for (key, value) in &env.vars {
                let expanded = expand_variables(value, arg_values, env_values);
                env_values.insert(key.clone(), expanded.clone());
                vars.insert(key.clone(), expanded);
            }
            Instruction::Env(EnvInstruction { vars })
        }
        Instruction::Copy(copy) => {
            let mut copy = copy.clone();
            copy.sources = copy
                .sources
                .iter()
                .map(|s| expand_variables(s, arg_values, env_values))
                .collect();
            copy.destination = expand_variables(&copy.destination, arg_values, env_values);
            Instruction::Copy(copy)
        }
        Instruction::Add(add) => {
            let mut add = add.clone();
            add.sources = add
                .sources
                .iter()
                .map(|s| expand_variables(s, arg_values, env_values))
                .collect();
            add.destination = expand_variables(&add.destination, arg_values, env_values);
            Instruction::Add(add)
        }
        Instruction::Workdir(dir) => {
            Instruction::Workdir(expand_variables(dir, arg_values, env_values))
        }
        Instruction::User(user) => {
            Instruction::User(expand_variables(user, arg_values, env_values))
        }
        Instruction::Label(labels) => {
            let expanded = labels
                .iter()
                .map(|(k, v)| (k.clone(), expand_variables(v, arg_values, env_values)))
                .collect();
            Instruction::Label(expanded)
        }
        Instruction::Arg(arg) => {
            // `ARG NAME=default` contributes its (expanded) default only when
            // the build did not already pass a value for it; a bare `ARG NAME`
            // leaves the variable unset (preserved as-is by `expand_variables`).
            if !arg_values.contains_key(&arg.name) {
                if let Some(default) = &arg.default {
                    let expanded = expand_variables(default, arg_values, env_values);
                    arg_values.insert(arg.name.clone(), expanded);
                }
            }
            instruction.clone()
        }
        other => other.clone(),
    }
}

fn chrono_lite_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}

/// Generate a short unique build ID for namespacing intermediate stage images.
fn generate_build_id() -> String {
    use sha2::{Digest, Sha256};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(pid.to_le_bytes());
    hasher.update(count.to_le_bytes());
    let hash = hasher.finalize();
    hex::encode(&hash[..6])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layer_cache_tracker_new() {
        let tracker = LayerCacheTracker::new();
        assert!(tracker.known_layers.is_empty());
    }

    #[test]
    fn test_layer_cache_tracker_record_and_lookup() {
        let mut tracker = LayerCacheTracker::new();

        tracker.record("abc123".to_string(), "container-1".to_string(), false);
        assert!(!tracker.is_cached("abc123", "container-1"));

        tracker.record("def456".to_string(), "container-2".to_string(), true);
        assert!(tracker.is_cached("def456", "container-2"));
    }

    #[test]
    fn test_layer_cache_tracker_unknown_returns_false() {
        let tracker = LayerCacheTracker::new();
        assert!(!tracker.is_cached("unknown", "unknown"));
    }

    #[test]
    fn test_layer_cache_tracker_different_base_layers() {
        let mut tracker = LayerCacheTracker::new();

        tracker.record("inst-1".to_string(), "base-a".to_string(), true);
        tracker.record("inst-1".to_string(), "base-b".to_string(), false);

        assert!(tracker.is_cached("inst-1", "base-a"));
        assert!(!tracker.is_cached("inst-1", "base-b"));
    }

    #[test]
    fn test_layer_cache_tracker_detect_cache_hit() {
        use crate::dockerfile::RunInstruction;

        let tracker = LayerCacheTracker::new();
        let instruction = Instruction::Run(RunInstruction::shell("echo hello"));

        assert!(!tracker.detect_cache_hit(&instruction, 50, ""));
        assert!(!tracker.detect_cache_hit(&instruction, 1000, ""));
        assert!(!tracker.detect_cache_hit(&instruction, 50, "Using cache"));
    }

    #[test]
    fn test_layer_cache_tracker_overwrite() {
        let mut tracker = LayerCacheTracker::new();

        tracker.record("key".to_string(), "base".to_string(), false);
        assert!(!tracker.is_cached("key", "base"));

        tracker.record("key".to_string(), "base".to_string(), true);
        assert!(tracker.is_cached("key", "base"));
    }

    /// Regression: the buildah CLI backend used to hand `${ARG}` / build args to
    /// buildah unexpanded (only the macOS sandbox path substituted them), so
    /// `${FORGEJO_TOKEN}` never resolved on the Linux/CI build. `expand_instruction`
    /// must resolve build args in RUN/ENV/COPY and track ARG/ENV bindings for
    /// later instructions — matching the sandbox builder.
    #[test]
    fn expand_instruction_resolves_build_args_in_run_env_copy() {
        use crate::dockerfile::{ArgInstruction, CopyInstruction, EnvInstruction, RunInstruction};

        let mut args: HashMap<String, String> = HashMap::new();
        args.insert("FORGEJO_TOKEN".to_string(), "s3cr3t".to_string());
        let mut env: HashMap<String, String> = HashMap::new();

        // RUN ${FORGEJO_TOKEN} — the exact bug: previously passed through literally.
        let run = Instruction::Run(RunInstruction::shell(
            "echo //forge/:_authToken=${FORGEJO_TOKEN} > .npmrc",
        ));
        let Instruction::Run(expanded) = expand_instruction(&run, &mut args, &mut env) else {
            panic!("expected Run");
        };
        let ShellOrExec::Shell(cmd) = expanded.command else {
            panic!("expected shell form");
        };
        assert!(
            cmd.contains("s3cr3t"),
            "RUN must expand the build arg: {cmd}"
        );
        assert!(
            !cmd.contains("FORGEJO_TOKEN"),
            "literal var must be gone: {cmd}"
        );

        // ENV TOKEN=${FORGEJO_TOKEN} — expands AND records into env_values.
        let env_inst = Instruction::Env(EnvInstruction::new("TOKEN", "${FORGEJO_TOKEN}"));
        let Instruction::Env(e) = expand_instruction(&env_inst, &mut args, &mut env) else {
            panic!("expected Env");
        };
        assert_eq!(e.vars.get("TOKEN").map(String::as_str), Some("s3cr3t"));
        assert_eq!(env.get("TOKEN").map(String::as_str), Some("s3cr3t"));

        // COPY app /opt/${TOKEN} — uses the ENV binding just set above.
        let copy = Instruction::Copy(CopyInstruction::new(
            vec!["app".to_string()],
            "/opt/${TOKEN}".to_string(),
        ));
        let Instruction::Copy(c) = expand_instruction(&copy, &mut args, &mut env) else {
            panic!("expected Copy");
        };
        assert_eq!(c.destination, "/opt/s3cr3t");

        // ARG EXTRA=fallback — default only fills an unset arg.
        let arg = Instruction::Arg(ArgInstruction::with_default("EXTRA", "fallback"));
        let _ = expand_instruction(&arg, &mut args, &mut env);
        assert_eq!(args.get("EXTRA").map(String::as_str), Some("fallback"));
    }
}
