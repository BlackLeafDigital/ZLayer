//! Pipeline executor - Coordinates building multiple images with wave-based orchestration
//!
//! This module provides the [`PipelineExecutor`] which processes [`ZPipeline`] manifests,
//! resolving dependencies and building images in parallel waves.
//!
//! # Execution Model
//!
//! Images are grouped into "waves" based on their dependency depth:
//! - **Wave 0**: Images with no dependencies - can all run in parallel
//! - **Wave 1**: Images that depend only on Wave 0 images
//! - **Wave N**: Images that depend only on images from earlier waves
//!
//! Within each wave, all builds run concurrently, sharing the same
//! [`BuildahExecutor`] (and thus cache storage).
//!
//! # Example
//!
//! ```no_run
//! use zlayer_builder::pipeline::{PipelineExecutor, parse_pipeline};
//! use zlayer_builder::BuildahExecutor;
//! use std::path::PathBuf;
//!
//! # async fn example() -> Result<(), zlayer_builder::BuildError> {
//! let yaml = std::fs::read_to_string("ZPipeline.yaml")?;
//! let pipeline = parse_pipeline(&yaml)?;
//!
//! let executor = BuildahExecutor::new_async().await?;
//! let result = PipelineExecutor::new(pipeline, PathBuf::from("."), executor)
//!     .fail_fast(true)
//!     .run()
//!     .await?;
//!
//! println!("Built {} images in {}ms", result.succeeded.len(), result.total_time_ms);
//! # Ok(())
//! # }
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::task::JoinSet;
use tracing::{error, info, warn};

use crate::backend::BuildBackend;
use crate::buildah::{BuildahCommand, BuildahExecutor};
use crate::builder::{BuiltImage, ImageBuilder};
use crate::error::{BuildError, Result};

use super::types::{PipelineDefaults, PipelineImage, ZPipeline};

#[cfg(feature = "local-registry")]
use zlayer_registry::LocalRegistry;

/// Result of a pipeline execution
#[derive(Debug)]
pub struct PipelineResult {
    /// Images that were successfully built
    pub succeeded: HashMap<String, BuiltImage>,
    /// Images that failed to build (name -> error message)
    pub failed: HashMap<String, String>,
    /// Total execution time in milliseconds
    pub total_time_ms: u64,
}

impl PipelineResult {
    /// Returns true if all images were built successfully
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.failed.is_empty()
    }

    /// Returns the total number of images in the pipeline
    #[must_use]
    pub fn total_images(&self) -> usize {
        self.succeeded.len() + self.failed.len()
    }
}

/// Pipeline executor configuration and runtime
///
/// The executor processes a [`ZPipeline`] manifest, resolving dependencies
/// and building images in parallel waves.
pub struct PipelineExecutor {
    /// The pipeline configuration
    pipeline: ZPipeline,
    /// Base directory for resolving relative paths
    base_dir: PathBuf,
    /// Buildah executor (shared across all builds)
    executor: BuildahExecutor,
    /// Pluggable build backend (buildah, sandbox, etc.).
    ///
    /// When set, builds delegate to this backend instead of using the
    /// `executor` field directly.
    backend: Option<Arc<dyn BuildBackend>>,
    /// Whether to abort on first failure
    fail_fast: bool,
    /// Whether to push images after building
    push_enabled: bool,
    /// Optional local registry for sharing built images between pipeline stages
    #[cfg(feature = "local-registry")]
    local_registry: Option<Arc<LocalRegistry>>,
}

impl PipelineExecutor {
    /// Create a new pipeline executor
    ///
    /// # Arguments
    ///
    /// * `pipeline` - The parsed `ZPipeline` configuration
    /// * `base_dir` - Base directory for resolving relative paths in the pipeline
    /// * `executor` - The buildah executor to use for all builds
    #[must_use]
    pub fn new(pipeline: ZPipeline, base_dir: PathBuf, executor: BuildahExecutor) -> Self {
        // Determine push behavior from pipeline config
        let push_enabled = pipeline.push.after_all;

        Self {
            pipeline,
            base_dir,
            executor,
            backend: None,
            fail_fast: true,
            push_enabled,
            #[cfg(feature = "local-registry")]
            local_registry: None,
        }
    }

    /// Create a new pipeline executor with an explicit [`BuildBackend`].
    ///
    /// The backend is used for all build, push, and manifest operations.
    /// A default `BuildahExecutor` is kept for backwards compatibility but
    /// is not used when a backend is set.
    ///
    /// # Arguments
    ///
    /// * `pipeline` - The parsed `ZPipeline` configuration
    /// * `base_dir` - Base directory for resolving relative paths in the pipeline
    /// * `backend`  - The build backend to use for all operations
    #[must_use]
    pub fn with_backend(
        pipeline: ZPipeline,
        base_dir: PathBuf,
        backend: Arc<dyn BuildBackend>,
    ) -> Self {
        let push_enabled = pipeline.push.after_all;

        Self {
            pipeline,
            base_dir,
            executor: BuildahExecutor::default(),
            backend: Some(backend),
            fail_fast: true,
            push_enabled,
            #[cfg(feature = "local-registry")]
            local_registry: None,
        }
    }

    /// Set fail-fast mode (default: true)
    ///
    /// When enabled, the executor will abort immediately when any image
    /// fails to build. When disabled, it will continue building independent
    /// images even after failures.
    #[must_use]
    pub fn fail_fast(mut self, fail_fast: bool) -> Self {
        self.fail_fast = fail_fast;
        self
    }

    /// Enable or disable pushing (overrides `pipeline.push.after_all`)
    ///
    /// When enabled and all builds succeed, images will be pushed to their
    /// configured registries.
    #[must_use]
    pub fn push(mut self, enabled: bool) -> Self {
        self.push_enabled = enabled;
        self
    }

    /// Set a local registry for sharing built images between pipeline stages.
    ///
    /// When set, each image build receives a fresh [`LocalRegistry`] handle
    /// pointing at the same on-disk root, so downstream images can resolve
    /// base images that were built by earlier waves.
    #[cfg(feature = "local-registry")]
    #[must_use]
    pub fn with_local_registry(mut self, registry: Arc<LocalRegistry>) -> Self {
        self.local_registry = Some(registry);
        self
    }

    /// Resolve execution order into waves
    ///
    /// Returns a vector of waves, where each wave contains image names
    /// that can be built in parallel. Images in wave N depend only on
    /// images from waves 0..N-1.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - An image depends on an unknown image
    /// - A circular dependency is detected
    fn resolve_execution_order(&self) -> Result<Vec<Vec<String>>> {
        let mut waves: Vec<Vec<String>> = Vec::new();
        let mut assigned: HashSet<String> = HashSet::new();
        let mut remaining: HashSet<String> = self.pipeline.images.keys().cloned().collect();

        // Validate: check for missing dependencies
        for (name, image) in &self.pipeline.images {
            for dep in &image.depends_on {
                if !self.pipeline.images.contains_key(dep) {
                    return Err(BuildError::invalid_instruction(
                        "pipeline",
                        format!("Image '{name}' depends on unknown image '{dep}'"),
                    ));
                }
            }
        }

        // Build waves iteratively
        while !remaining.is_empty() {
            let mut wave: Vec<String> = Vec::new();

            for name in &remaining {
                let image = &self.pipeline.images[name];
                // Can build if all dependencies are already assigned to previous waves
                let deps_satisfied = image.depends_on.iter().all(|d| assigned.contains(d));
                if deps_satisfied {
                    wave.push(name.clone());
                }
            }

            if wave.is_empty() {
                // No images could be added to this wave - circular dependency
                return Err(BuildError::CircularDependency {
                    stages: remaining.into_iter().collect(),
                });
            }

            // Move wave images from remaining to assigned
            for name in &wave {
                remaining.remove(name);
                assigned.insert(name.clone());
            }

            waves.push(wave);
        }

        Ok(waves)
    }

    /// Execute the pipeline
    ///
    /// Builds all images in dependency order, with images in the same wave
    /// running in parallel.
    ///
    /// # Returns
    ///
    /// A [`PipelineResult`] containing information about successful and failed builds.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The dependency graph is invalid (missing deps, cycles)
    /// - Any build fails and `fail_fast` is enabled
    pub async fn run(&self) -> Result<PipelineResult> {
        let start = std::time::Instant::now();
        let waves = self.resolve_execution_order()?;

        let mut succeeded: HashMap<String, BuiltImage> = HashMap::new();
        let mut failed: HashMap<String, String> = HashMap::new();

        info!(
            "Building {} images in {} waves",
            self.pipeline.images.len(),
            waves.len()
        );

        for (wave_idx, wave) in waves.iter().enumerate() {
            info!("Wave {}: {:?}", wave_idx, wave);

            // Check if we should abort due to previous failures
            if self.fail_fast && !failed.is_empty() {
                warn!("Aborting pipeline due to previous failures (fail_fast enabled)");
                break;
            }

            // Build all images in this wave concurrently
            let wave_results = self.build_wave(wave).await;

            // Process results
            for (name, result) in wave_results {
                match result {
                    Ok(image) => {
                        info!("[{}] Build succeeded: {}", name, image.image_id);
                        succeeded.insert(name, image);
                    }
                    Err(e) => {
                        error!("[{}] Build failed: {}", name, e);
                        failed.insert(name.clone(), e.to_string());

                        if self.fail_fast {
                            // Return early with the first error
                            return Err(e);
                        }
                    }
                }
            }
        }

        // Push phase (only if all succeeded and push enabled)
        if self.push_enabled && failed.is_empty() {
            info!("Pushing {} images", succeeded.len());

            for (name, image) in &succeeded {
                for tag in &image.tags {
                    let push_result = if image.is_manifest {
                        self.push_manifest(tag).await
                    } else {
                        self.push_image(tag).await
                    };

                    if let Err(e) = push_result {
                        warn!("[{}] Failed to push {}: {}", name, tag, e);
                        // Push failures don't fail the overall pipeline
                        // since the images were built successfully
                    } else {
                        info!("[{}] Pushed: {}", name, tag);
                    }
                }
            }
        }

        #[allow(clippy::cast_possible_truncation)]
        let total_time_ms = start.elapsed().as_millis() as u64;

        Ok(PipelineResult {
            succeeded,
            failed,
            total_time_ms,
        })
    }

    /// Build all images in a wave concurrently
    ///
    /// Each image is checked for multi-platform configuration. Images with
    /// 2+ platforms use `build_multiplatform_image` (manifest list), images
    /// with exactly 1 platform use `build_single_image` with that platform
    /// set, and images with no platforms use the native platform (existing
    /// behavior).
    ///
    /// Returns a vector of (name, result) tuples for each image in the wave.
    async fn build_wave(&self, wave: &[String]) -> Vec<(String, Result<BuiltImage>)> {
        // Create shared data for spawned tasks
        let pipeline = Arc::new(self.pipeline.clone());
        let base_dir = Arc::new(self.base_dir.clone());
        let executor = self.executor.clone();
        let backend = self.backend.clone();

        // Extract local registry root path (if configured) so spawned tasks
        // can create their own LocalRegistry handles pointing at the same store.
        #[cfg(feature = "local-registry")]
        let registry_root: Option<PathBuf> =
            self.local_registry.as_ref().map(|r| r.root().to_path_buf());
        #[cfg(not(feature = "local-registry"))]
        let registry_root: Option<PathBuf> = None;

        let mut set = JoinSet::new();

        for name in wave {
            let name = name.clone();
            let pipeline = Arc::clone(&pipeline);
            let base_dir = Arc::clone(&base_dir);
            let executor = executor.clone();
            let backend = backend.clone();
            let registry_root = registry_root.clone();

            set.spawn(async move {
                let platforms = {
                    let image_config = &pipeline.images[&name];
                    effective_platforms(image_config, &pipeline.defaults)
                };

                let result = match platforms.len() {
                    // No platforms specified — native build (existing behavior)
                    0 => {
                        build_single_image(
                            &name,
                            &pipeline,
                            &base_dir,
                            executor,
                            backend.as_ref().map(Arc::clone),
                            None,
                            registry_root.as_deref(),
                        )
                        .await
                    }
                    // Single platform — use build_single_image with platform set
                    1 => {
                        let platform = platforms[0].clone();
                        build_single_image(
                            &name,
                            &pipeline,
                            &base_dir,
                            executor,
                            backend.as_ref().map(Arc::clone),
                            Some(&platform),
                            registry_root.as_deref(),
                        )
                        .await
                    }
                    // Multiple platforms — build each, then create manifest list
                    _ => {
                        build_multiplatform_image(
                            &name,
                            &pipeline,
                            &base_dir,
                            executor,
                            backend.as_ref().map(Arc::clone),
                            &platforms,
                            registry_root.as_deref(),
                        )
                        .await
                    }
                };

                (name, result)
            });
        }

        // Collect all results
        let mut results = Vec::new();
        while let Some(join_result) = set.join_next().await {
            match join_result {
                Ok((name, result)) => {
                    results.push((name, result));
                }
                Err(e) => {
                    // Task panicked
                    error!("Build task panicked: {}", e);
                    results.push((
                        "unknown".to_string(),
                        Err(BuildError::invalid_instruction(
                            "pipeline",
                            format!("Build task panicked: {e}"),
                        )),
                    ));
                }
            }
        }

        results
    }

    /// Push a regular image to its registry
    async fn push_image(&self, tag: &str) -> Result<()> {
        if let Some(ref backend) = self.backend {
            return backend.push_image(tag, None).await;
        }
        let cmd = BuildahCommand::push(tag);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    /// Push a manifest list (and all referenced images) to its registry
    async fn push_manifest(&self, tag: &str) -> Result<()> {
        if let Some(ref backend) = self.backend {
            let destination = format!("docker://{tag}");
            return backend.manifest_push(tag, &destination).await;
        }
        let destination = format!("docker://{tag}");
        let cmd = BuildahCommand::manifest_push(tag, &destination);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }
}

/// Get the effective platforms for an image, considering defaults.
///
/// If the image specifies its own platforms, those take precedence.
/// Otherwise, the pipeline-level defaults are used. An empty result
/// means "native platform only" (no multi-arch).
fn effective_platforms(image: &PipelineImage, defaults: &PipelineDefaults) -> Vec<String> {
    if image.platforms.is_empty() {
        defaults.platforms.clone()
    } else {
        image.platforms.clone()
    }
}

/// Extract architecture suffix from a platform string.
///
/// # Examples
///
/// - `"linux/amd64"` -> `"amd64"`
/// - `"linux/arm64"` -> `"arm64"`
/// - `"linux/arm64/v8"` -> `"arm64-v8"`
/// - `"linux"` -> `"linux"`
fn platform_to_suffix(platform: &str) -> String {
    let parts: Vec<&str> = platform.split('/').collect();
    match parts.len() {
        0 | 1 => platform.replace('/', "-"),
        2 => parts[1].to_string(),
        _ => format!("{}-{}", parts[1], parts[2]),
    }
}

/// Apply pipeline configuration (`build_args`, format, `cache_mounts`, retries, `no_cache`)
/// to an [`ImageBuilder`].
///
/// This merges default-level and per-image settings, with per-image values taking
/// precedence for scalar settings and being additive for collections.
fn apply_pipeline_config(
    mut builder: ImageBuilder,
    image_config: &PipelineImage,
    defaults: &PipelineDefaults,
) -> ImageBuilder {
    // Merge build_args: defaults + per-image (per-image overrides defaults)
    let mut args = defaults.build_args.clone();
    args.extend(image_config.build_args.clone());
    builder = builder.build_args(args);

    // Format (per-image overrides default)
    if let Some(fmt) = image_config.format.as_ref().or(defaults.format.as_ref()) {
        builder = builder.format(fmt);
    }

    // No cache (per-image overrides default)
    if image_config.no_cache.unwrap_or(defaults.no_cache) {
        builder = builder.no_cache();
    }

    // Cache mounts: defaults + per-image (per-image are additive)
    let mut cache_mounts = defaults.cache_mounts.clone();
    cache_mounts.extend(image_config.cache_mounts.clone());
    if !cache_mounts.is_empty() {
        let run_mounts: Vec<_> = cache_mounts
            .iter()
            .map(crate::zimage::convert_cache_mount)
            .collect();
        builder = builder.default_cache_mounts(run_mounts);
    }

    // Retries (per-image overrides default)
    let retries = image_config.retries.or(defaults.retries).unwrap_or(0);
    if retries > 0 {
        builder = builder.retries(retries);
    }

    builder
}

/// Detect whether a build file is a ZImagefile/YAML or a Dockerfile and
/// configure the builder accordingly.
fn apply_build_file(builder: ImageBuilder, file_path: &Path) -> ImageBuilder {
    let file_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let extension = file_path
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();

    if extension == "yaml" || extension == "yml" || file_name.starts_with("ZImagefile") {
        builder.zimagefile(file_path)
    } else {
        builder.dockerfile(file_path)
    }
}

/// Compute a SHA-256 hash of a file's contents for content-based cache invalidation.
///
/// Returns `None` if the file cannot be read.
async fn compute_file_hash(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};

    let content = tokio::fs::read(path).await.ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    Some(format!("{:x}", hasher.finalize()))
}

/// Sanitize an image reference into a filesystem-safe directory name.
///
/// Mirrors the logic in `sandbox_builder::sanitize_image_name`.
fn sanitize_image_name_for_cache(image: &str) -> String {
    image.replace(['/', ':', '@'], "_")
}

/// Check if a cached sandbox image at `data_dir/images/{sanitized}/config.json`
/// has a `source_hash` matching `expected_hash`.
///
/// Returns the sanitized image name if a match is found.
async fn check_cached_image_hash(
    data_dir: &Path,
    tag: &str,
    expected_hash: &str,
) -> Option<String> {
    let sanitized = sanitize_image_name_for_cache(tag);
    let config_path = data_dir.join("images").join(&sanitized).join("config.json");
    let data = tokio::fs::read_to_string(&config_path).await.ok()?;
    let config: crate::sandbox_builder::SandboxImageConfig = serde_json::from_str(&data).ok()?;
    if config.source_hash.as_deref() == Some(expected_hash) {
        Some(sanitized)
    } else {
        None
    }
}

/// Build a single image from the pipeline
///
/// This is extracted as a separate function to make it easier to spawn
/// in a tokio task without borrowing issues.
///
/// When `platform` is `Some`, the builder is configured for that specific
/// platform (e.g. `"linux/arm64"`), enabling cross-architecture builds.
async fn build_single_image(
    name: &str,
    pipeline: &ZPipeline,
    base_dir: &Path,
    executor: BuildahExecutor,
    backend: Option<Arc<dyn BuildBackend>>,
    platform: Option<&str>,
    registry_root: Option<&Path>,
) -> Result<BuiltImage> {
    let image_config = &pipeline.images[name];
    let context = base_dir.join(&image_config.context);
    let file_path = base_dir.join(&image_config.file);

    // Content-based cache invalidation: hash the build file and check if the
    // output image was already built from identical source content.
    let file_hash = compute_file_hash(&file_path).await;
    if let Some(ref hash) = file_hash {
        let data_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".zlayer");

        let expanded_tags: Vec<String> = image_config
            .tags
            .iter()
            .map(|t| expand_tag_with_vars(t, &pipeline.vars))
            .collect();

        // Check the first tag — if it has a cached image with matching hash, skip the build
        if let Some(first_tag) = expanded_tags.first() {
            if let Some(cached_id) = check_cached_image_hash(&data_dir, first_tag, hash).await {
                info!(
                    "[{}] Skipping build — cached image hash matches ({})",
                    name, cached_id
                );
                return Ok(BuiltImage {
                    image_id: cached_id,
                    tags: expanded_tags,
                    layer_count: 1,
                    size: 0,
                    build_time_ms: 0,
                    is_manifest: false,
                });
            }
        }
    }

    let effective_backend: Arc<dyn BuildBackend> = backend
        .unwrap_or_else(|| Arc::new(crate::backend::BuildahBackend::with_executor(executor)));
    let mut builder = ImageBuilder::with_backend(&context, effective_backend)?;

    // Determine if this is a ZImagefile or Dockerfile based on extension/name
    builder = apply_build_file(builder, &file_path);

    // Pass the source hash so the sandbox builder stores it for future cache checks
    if let Some(hash) = file_hash {
        builder = builder.source_hash(hash);
    }

    // Set platform if specified
    if let Some(plat) = platform {
        builder = builder.platform(plat);
    }

    // Apply tags with variable expansion
    for tag in &image_config.tags {
        let expanded = expand_tag_with_vars(tag, &pipeline.vars);
        builder = builder.tag(expanded);
    }

    // Apply shared pipeline config (build_args, format, no_cache, cache_mounts, retries)
    builder = apply_pipeline_config(builder, image_config, &pipeline.defaults);

    // Wire up local registry so this build can resolve images from earlier waves
    #[cfg(feature = "local-registry")]
    if let Some(root) = registry_root {
        let shared_registry = LocalRegistry::new(root.to_path_buf()).await.map_err(|e| {
            BuildError::invalid_instruction(
                "pipeline",
                format!("failed to open local registry: {e}"),
            )
        })?;
        builder = builder.with_local_registry(shared_registry);
    }

    builder.build().await
}

/// Build an image for multiple platforms and create a manifest list.
///
/// Each platform is built sequentially (QEMU can be flaky with parallel
/// cross-arch builds), then a buildah manifest list is created that
/// references all per-platform images.
async fn build_multiplatform_image(
    name: &str,
    pipeline: &ZPipeline,
    base_dir: &Path,
    executor: BuildahExecutor,
    backend: Option<Arc<dyn BuildBackend>>,
    platforms: &[String],
    registry_root: Option<&Path>,
) -> Result<BuiltImage> {
    let image_config = &pipeline.images[name];
    let start_time = std::time::Instant::now();

    // Expand tags with variables
    let expanded_tags: Vec<String> = image_config
        .tags
        .iter()
        .map(|t| expand_tag_with_vars(t, &pipeline.vars))
        .collect();

    let manifest_name = expanded_tags
        .first()
        .cloned()
        .unwrap_or_else(|| format!("zlayer-manifest-{name}"));

    // Build for each platform sequentially (QEMU can be flaky with parallel cross-arch)
    let mut arch_tags: Vec<String> = Vec::new();
    let mut total_layers = 0usize;
    let mut total_size = 0u64;

    for platform in platforms {
        let suffix = platform_to_suffix(platform);
        let platform_tags: Vec<String> = expanded_tags
            .iter()
            .map(|t| format!("{t}-{suffix}"))
            .collect();

        info!("[{name}] Building for platform {platform}");

        // Build with platform-specific tags
        let context = base_dir.join(&image_config.context);
        let file_path = base_dir.join(&image_config.file);

        let effective_backend: Arc<dyn BuildBackend> = match backend {
            Some(ref b) => Arc::clone(b),
            None => Arc::new(crate::backend::BuildahBackend::with_executor(
                executor.clone(),
            )),
        };
        let mut builder = ImageBuilder::with_backend(&context, effective_backend)?;

        // Determine file type (same detection as build_single_image)
        builder = apply_build_file(builder, &file_path);

        // Set platform
        builder = builder.platform(platform);

        // Apply platform-specific tags
        for tag in &platform_tags {
            builder = builder.tag(tag);
        }

        // Apply shared config (build_args, format, no_cache, cache_mounts, retries)
        builder = apply_pipeline_config(builder, image_config, &pipeline.defaults);

        // Wire up local registry so this build can resolve images from earlier waves
        #[cfg(feature = "local-registry")]
        if let Some(root) = registry_root {
            let shared_registry = LocalRegistry::new(root.to_path_buf()).await.map_err(|e| {
                BuildError::invalid_instruction(
                    "pipeline",
                    format!("failed to open local registry: {e}"),
                )
            })?;
            builder = builder.with_local_registry(shared_registry);
        }

        let built = builder.build().await?;
        total_layers += built.layer_count;
        total_size += built.size;

        if let Some(first_tag) = platform_tags.first() {
            arch_tags.push(first_tag.clone());
        }
    }

    // Assemble the manifest list from per-platform images
    assemble_manifest(
        name,
        &manifest_name,
        &arch_tags,
        &expanded_tags,
        backend.as_ref(),
        &executor,
    )
    .await?;

    #[allow(clippy::cast_possible_truncation)]
    let build_time_ms = start_time.elapsed().as_millis() as u64;

    Ok(BuiltImage {
        image_id: manifest_name,
        tags: expanded_tags,
        layer_count: total_layers,
        size: total_size,
        build_time_ms,
        is_manifest: true,
    })
}

/// Create a manifest list, add per-platform images, and apply additional tags.
///
/// This is a helper extracted from [`build_multiplatform_image`] to keep that
/// function under the line-count limit.
async fn assemble_manifest(
    name: &str,
    manifest_name: &str,
    arch_tags: &[String],
    expanded_tags: &[String],
    backend: Option<&Arc<dyn BuildBackend>>,
    executor: &BuildahExecutor,
) -> Result<()> {
    // Create manifest list — delegate to backend if available
    info!("[{name}] Creating manifest: {manifest_name}");
    if let Some(backend) = backend {
        backend
            .manifest_create(manifest_name)
            .await
            .map_err(|e| BuildError::pipeline_error(format!("manifest create failed: {e}")))?;
    } else {
        executor
            .execute_checked(&BuildahCommand::manifest_create(manifest_name))
            .await
            .map_err(|e| BuildError::pipeline_error(format!("manifest create failed: {e}")))?;
    }

    // Add each arch image to the manifest
    for arch_tag in arch_tags {
        info!("[{name}] Adding to manifest: {arch_tag}");
        if let Some(backend) = backend {
            backend
                .manifest_add(manifest_name, arch_tag)
                .await
                .map_err(|e| BuildError::pipeline_error(format!("manifest add failed: {e}")))?;
        } else {
            executor
                .execute_checked(&BuildahCommand::manifest_add(manifest_name, arch_tag))
                .await
                .map_err(|e| BuildError::pipeline_error(format!("manifest add failed: {e}")))?;
        }
    }

    // Tag the manifest with additional tags
    for tag in expanded_tags.iter().skip(1) {
        if let Some(backend) = backend {
            backend
                .tag_image(manifest_name, tag)
                .await
                .map_err(|e| BuildError::pipeline_error(format!("manifest tag failed: {e}")))?;
        } else {
            executor
                .execute_checked(&BuildahCommand::tag(manifest_name, tag))
                .await
                .map_err(|e| BuildError::pipeline_error(format!("manifest tag failed: {e}")))?;
        }
    }

    Ok(())
}

/// Expand variables in a tag string
///
/// Standalone function for use in spawned tasks.
fn expand_tag_with_vars(tag: &str, vars: &HashMap<String, String>) -> String {
    let mut result = tag.to_string();
    for (key, value) in vars {
        result = result.replace(&format!("${{{key}}}"), value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::parse_pipeline;

    #[test]
    fn test_resolve_execution_order_simple() {
        let yaml = r"
images:
  app:
    file: Dockerfile
";
        let pipeline = parse_pipeline(yaml).unwrap();
        let executor = PipelineExecutor::new(
            pipeline,
            PathBuf::from("/tmp"),
            BuildahExecutor::with_path("/usr/bin/buildah"),
        );

        let waves = executor.resolve_execution_order().unwrap();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0], vec!["app"]);
    }

    #[test]
    fn test_resolve_execution_order_with_deps() {
        let yaml = r"
images:
  base:
    file: Dockerfile.base
  app:
    file: Dockerfile.app
    depends_on: [base]
  test:
    file: Dockerfile.test
    depends_on: [app]
";
        let pipeline = parse_pipeline(yaml).unwrap();
        let executor = PipelineExecutor::new(
            pipeline,
            PathBuf::from("/tmp"),
            BuildahExecutor::with_path("/usr/bin/buildah"),
        );

        let waves = executor.resolve_execution_order().unwrap();
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0], vec!["base"]);
        assert_eq!(waves[1], vec!["app"]);
        assert_eq!(waves[2], vec!["test"]);
    }

    #[test]
    fn test_resolve_execution_order_parallel() {
        let yaml = r"
images:
  base:
    file: Dockerfile.base
  app1:
    file: Dockerfile.app1
    depends_on: [base]
  app2:
    file: Dockerfile.app2
    depends_on: [base]
";
        let pipeline = parse_pipeline(yaml).unwrap();
        let executor = PipelineExecutor::new(
            pipeline,
            PathBuf::from("/tmp"),
            BuildahExecutor::with_path("/usr/bin/buildah"),
        );

        let waves = executor.resolve_execution_order().unwrap();
        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0], vec!["base"]);
        // app1 and app2 should be in the same wave (order may vary)
        assert_eq!(waves[1].len(), 2);
        assert!(waves[1].contains(&"app1".to_string()));
        assert!(waves[1].contains(&"app2".to_string()));
    }

    #[test]
    fn test_resolve_execution_order_missing_dep() {
        let yaml = r"
images:
  app:
    file: Dockerfile
    depends_on: [missing]
";
        let pipeline = parse_pipeline(yaml).unwrap();
        let executor = PipelineExecutor::new(
            pipeline,
            PathBuf::from("/tmp"),
            BuildahExecutor::with_path("/usr/bin/buildah"),
        );

        let result = executor.resolve_execution_order();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing"));
    }

    #[test]
    fn test_resolve_execution_order_circular() {
        let yaml = r"
images:
  a:
    file: Dockerfile.a
    depends_on: [b]
  b:
    file: Dockerfile.b
    depends_on: [a]
";
        let pipeline = parse_pipeline(yaml).unwrap();
        let executor = PipelineExecutor::new(
            pipeline,
            PathBuf::from("/tmp"),
            BuildahExecutor::with_path("/usr/bin/buildah"),
        );

        let result = executor.resolve_execution_order();
        assert!(result.is_err());
        match result.unwrap_err() {
            BuildError::CircularDependency { stages } => {
                assert!(stages.contains(&"a".to_string()));
                assert!(stages.contains(&"b".to_string()));
            }
            e => panic!("Expected CircularDependency error, got: {e:?}"),
        }
    }

    #[test]
    fn test_expand_tag() {
        let mut vars = HashMap::new();
        vars.insert("VERSION".to_string(), "1.0.0".to_string());
        vars.insert("REGISTRY".to_string(), "ghcr.io/myorg".to_string());

        let tag = "${REGISTRY}/app:${VERSION}";
        let expanded = expand_tag_with_vars(tag, &vars);
        assert_eq!(expanded, "ghcr.io/myorg/app:1.0.0");
    }

    #[test]
    fn test_expand_tag_partial() {
        let mut vars = HashMap::new();
        vars.insert("VERSION".to_string(), "1.0.0".to_string());

        // Unknown vars are left as-is
        let tag = "myapp:${VERSION}-${UNKNOWN}";
        let expanded = expand_tag_with_vars(tag, &vars);
        assert_eq!(expanded, "myapp:1.0.0-${UNKNOWN}");
    }

    #[test]
    fn test_pipeline_result_is_success() {
        let mut result = PipelineResult {
            succeeded: HashMap::new(),
            failed: HashMap::new(),
            total_time_ms: 100,
        };

        assert!(result.is_success());

        result.failed.insert("app".to_string(), "error".to_string());
        assert!(!result.is_success());
    }

    #[test]
    fn test_pipeline_result_total_images() {
        let mut result = PipelineResult {
            succeeded: HashMap::new(),
            failed: HashMap::new(),
            total_time_ms: 100,
        };

        result.succeeded.insert(
            "app1".to_string(),
            BuiltImage {
                image_id: "sha256:abc".to_string(),
                tags: vec!["app1:latest".to_string()],
                layer_count: 5,
                size: 0,
                build_time_ms: 50,
                is_manifest: false,
            },
        );
        result
            .failed
            .insert("app2".to_string(), "error".to_string());

        assert_eq!(result.total_images(), 2);
    }

    #[test]
    fn test_builder_methods() {
        let yaml = r"
images:
  app:
    file: Dockerfile
push:
  after_all: true
";
        let pipeline = parse_pipeline(yaml).unwrap();
        let executor = PipelineExecutor::new(
            pipeline,
            PathBuf::from("/tmp"),
            BuildahExecutor::with_path("/usr/bin/buildah"),
        )
        .fail_fast(false)
        .push(false);

        assert!(!executor.fail_fast);
        assert!(!executor.push_enabled);
    }

    /// Helper to create a minimal `PipelineImage` for tests.
    fn test_pipeline_image() -> PipelineImage {
        PipelineImage {
            file: PathBuf::from("Dockerfile"),
            context: PathBuf::from("."),
            tags: vec![],
            build_args: HashMap::new(),
            depends_on: vec![],
            no_cache: None,
            format: None,
            cache_mounts: vec![],
            retries: None,
            platforms: vec![],
        }
    }

    #[test]
    fn test_platform_to_suffix() {
        assert_eq!(platform_to_suffix("linux/amd64"), "amd64");
        assert_eq!(platform_to_suffix("linux/arm64"), "arm64");
        assert_eq!(platform_to_suffix("linux/arm64/v8"), "arm64-v8");
        assert_eq!(platform_to_suffix("linux"), "linux");
    }

    #[test]
    fn test_effective_platforms_image_overrides() {
        let defaults = PipelineDefaults {
            platforms: vec!["linux/amd64".into()],
            ..Default::default()
        };
        let image = PipelineImage {
            platforms: vec!["linux/arm64".into()],
            ..test_pipeline_image()
        };
        assert_eq!(effective_platforms(&image, &defaults), vec!["linux/arm64"]);
    }

    #[test]
    fn test_effective_platforms_inherits_defaults() {
        let defaults = PipelineDefaults {
            platforms: vec!["linux/amd64".into()],
            ..Default::default()
        };
        let image = test_pipeline_image();
        assert_eq!(effective_platforms(&image, &defaults), vec!["linux/amd64"]);
    }

    #[test]
    fn test_effective_platforms_empty() {
        let defaults = PipelineDefaults::default();
        let image = test_pipeline_image();
        assert!(effective_platforms(&image, &defaults).is_empty());
    }

    #[test]
    fn test_platform_to_suffix_edge_cases() {
        // Empty string
        assert_eq!(platform_to_suffix(""), "");
        // Single component
        assert_eq!(platform_to_suffix("linux"), "linux");
        // Four components (unusual but handle gracefully)
        assert_eq!(platform_to_suffix("linux/arm/v7/extra"), "arm-v7");
    }

    #[test]
    fn test_effective_platforms_multiple_defaults() {
        let defaults = PipelineDefaults {
            platforms: vec!["linux/amd64".into(), "linux/arm64".into()],
            ..Default::default()
        };
        let image = test_pipeline_image();
        assert_eq!(
            effective_platforms(&image, &defaults),
            vec!["linux/amd64", "linux/arm64"]
        );
    }

    #[test]
    fn test_effective_platforms_image_overrides_multiple() {
        let defaults = PipelineDefaults {
            platforms: vec!["linux/amd64".into(), "linux/arm64".into()],
            ..Default::default()
        };
        let image = PipelineImage {
            platforms: vec!["linux/s390x".into()],
            ..test_pipeline_image()
        };
        // Image platforms completely replace defaults, not merge
        assert_eq!(effective_platforms(&image, &defaults), vec!["linux/s390x"]);
    }
}
