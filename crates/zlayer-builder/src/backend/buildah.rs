//! Buildah-backed build backend.
//!
//! Wraps [`BuildahExecutor`] to implement the [`BuildBackend`] trait.
//! Contains the full buildah build orchestration loop: stage walking,
//! container creation, instruction execution, and commit.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;

use tracing::{debug, info};

use crate::buildah::{BuildahCommand, BuildahExecutor};
use crate::builder::{BuildOptions, BuiltImage, RegistryAuth};
use crate::dockerfile::{Dockerfile, ImageRef, Instruction, RunMount, Stage};
use crate::error::{BuildError, Result};
use crate::tui::BuildEvent;

use super::BuildBackend;

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
        image_ref: &ImageRef,
        stage_images: &HashMap<String, String>,
        options: &BuildOptions,
    ) -> Result<String> {
        match image_ref {
            ImageRef::Stage(name) => {
                return stage_images
                    .get(name)
                    .cloned()
                    .ok_or_else(|| BuildError::stage_not_found(name));
            }
            ImageRef::Scratch => return Ok("scratch".to_string()),
            ImageRef::Registry { .. } => {}
        }

        // Check if name is already fully qualified (has registry hostname).
        let is_qualified = match image_ref {
            ImageRef::Registry { image, .. } => {
                let first = image.split('/').next().unwrap_or("");
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

        // Fall back: qualify to docker.io and build the full string.
        let qualified = image_ref.qualify();
        match &qualified {
            ImageRef::Registry { image, tag, digest } => {
                let mut result = image.clone();
                if let Some(t) = tag {
                    result.push(':');
                    result.push_str(t);
                }
                if let Some(d) = digest {
                    result.push('@');
                    result.push_str(d);
                }
                if tag.is_none() && digest.is_none() {
                    result.push_str(":latest");
                }
                Ok(result)
            }
            _ => unreachable!("qualify() preserves Registry variant"),
        }
    }

    /// Try to resolve an unqualified image from default registry.
    ///
    /// Returns `Some(fully_qualified_name)` if found, `None` to fall back to docker.io.
    #[allow(clippy::unused_async)]
    async fn try_resolve_from_sources(
        &self,
        image_ref: &ImageRef,
        options: &BuildOptions,
    ) -> Option<String> {
        let (name, tag_str) = match image_ref {
            ImageRef::Registry { image, tag, .. } => {
                (image.as_str(), tag.as_deref().unwrap_or("latest"))
            }
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
    async fn create_container(&self, image: &str, platform: Option<&str>) -> Result<String> {
        let cmd = BuildahCommand::new("from")
            .arg_opt("--platform", platform)
            .arg(image);
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

        // Build each stage.
        let mut stage_images: HashMap<String, String> = HashMap::new();
        // Track the final WORKDIR for each committed stage, used to resolve
        // relative source paths in COPY --from instructions.
        let mut stage_workdirs: HashMap<String, String> = HashMap::new();
        let mut final_container: Option<String> = None;
        let mut total_instructions = 0;

        // Initialize the layer cache tracker for this build session.
        let mut cache_tracker = LayerCacheTracker::new();

        for (stage_idx, stage) in stages.iter().enumerate() {
            let is_final_stage = stage_idx == stages.len() - 1;

            Self::send_event(
                event_tx.as_ref(),
                BuildEvent::StageStarted {
                    index: stage_idx,
                    name: stage.name.clone(),
                    base_image: stage.base_image.to_string_ref(),
                },
            );

            // Create container from base image.
            let base = self
                .resolve_base_image(&stage.base_image, &stage_images, options)
                .await?;
            let container_id = self
                .create_container(&base, options.platform.as_deref())
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
                ImageRef::Stage(name) => stage_workdirs
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

                // Resolve COPY --from references to actual committed image names,
                // and resolve relative source paths using the source stage's WORKDIR.
                let resolved_instruction;
                let instruction_ref = if let Instruction::Copy(copy) = instruction {
                    if let Some(ref from) = copy.from {
                        if let Some(image_name) = stage_images.get(from) {
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
                            instruction
                        }
                    } else {
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

                let is_run_instruction = matches!(instruction_ref, Instruction::Run(_));
                let max_attempts = if is_run_instruction {
                    options.retries + 1
                } else {
                    1
                };

                let commands = BuildahCommand::from_instruction(&container_id, instruction_ref);

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
}
