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
use std::str::FromStr;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tracing::{error, info, warn};

use serde::Deserialize;

use crate::backend::{detect_backend, BuildBackend, ImageOs};

/// Minimal struct to read `source_hash` from a cached image's `config.json`.
/// Separate from `SandboxImageConfig` which is macOS-only.
#[derive(Deserialize)]
struct CachedImageConfig {
    #[serde(default)]
    source_hash: Option<String>,
}
use crate::buildah::{BuildahCommand, BuildahExecutor};
use crate::builder::{BuiltImage, ImageBuilder};
use crate::error::{BuildError, Result};
use zlayer_paths::ZLayerDirs;

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
    /// `executor` field directly. This is an *explicit* override: when
    /// present, the executor uses the same backend for every image in the
    /// pipeline regardless of the image's target OS.
    backend: Option<Arc<dyn BuildBackend>>,
    /// Per-target-OS backend cache used when `backend` is `None`.
    ///
    /// Each wave may contain images targeting different OSes (e.g. one Linux
    /// image + one Windows image). Rather than re-run `detect_backend()` for
    /// every image, we memoize the backend selected for each [`ImageOs`] the
    /// first time we see it in the pipeline. Shared via `Arc<Mutex<_>>` so
    /// spawned build tasks can resolve their backend concurrently.
    backend_cache: Arc<Mutex<HashMap<ImageOs, Arc<dyn BuildBackend>>>>,
    /// Whether to abort on first failure
    fail_fast: bool,
    /// Whether to push images after building
    push_enabled: bool,
    /// Whether to force `--net=host` on every `buildah run` for every image
    /// in this pipeline. Forwarded to each [`ImageBuilder`] via
    /// [`ImageBuilder::with_host_network`]. Mirrors the top-level
    /// `zlayer --host-network` CLI flag.
    host_network: bool,
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
            backend_cache: Arc::new(Mutex::new(HashMap::new())),
            fail_fast: true,
            push_enabled,
            host_network: false,
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
            backend_cache: Arc::new(Mutex::new(HashMap::new())),
            fail_fast: true,
            push_enabled,
            host_network: false,
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

    /// Force `--net=host` on every `buildah run` for every image in this
    /// pipeline. Forwarded to each [`ImageBuilder`] via
    /// [`ImageBuilder::with_host_network`]. Mirrors the top-level
    /// `zlayer --host-network` CLI flag.
    #[must_use]
    pub fn with_host_network(mut self, on: bool) -> Self {
        self.host_network = on;
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

            // Ensure secondary tags have on-disk directories (sandbox backend
            // stores the rootfs under the first tag only; additional tags need
            // to be created before push can find them).
            if let Some(ref backend) = self.backend {
                for image in succeeded.values() {
                    if image.tags.len() > 1 {
                        let first = &image.tags[0];
                        for secondary in &image.tags[1..] {
                            if let Err(e) = backend.tag_image(first, secondary).await {
                                warn!("Failed to tag {} as {}: {}", first, secondary, e);
                            }
                        }
                    }
                }
            }

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
    /// # Per-image backend selection
    ///
    /// When the executor was built via [`PipelineExecutor::with_backend`]
    /// (i.e. `self.backend.is_some()`), that explicit backend is used for
    /// every image. Otherwise the target OS is parsed from each image's
    /// `platforms` field and a backend is resolved via [`detect_backend`],
    /// cached in `self.backend_cache` to avoid re-detection. An image that
    /// cannot resolve a backend (e.g. Windows image on a Linux host) fails
    /// only that image — other images in the wave continue.
    ///
    /// Returns a vector of (name, result) tuples for each image in the wave.
    async fn build_wave(&self, wave: &[String]) -> Vec<(String, Result<BuiltImage>)> {
        // Create shared data for spawned tasks
        let pipeline = Arc::new(self.pipeline.clone());
        let base_dir = Arc::new(self.base_dir.clone());
        let executor = self.executor.clone();
        let explicit_backend = self.backend.clone();
        let backend_cache = Arc::clone(&self.backend_cache);
        let host_network = self.host_network;

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
            let explicit_backend = explicit_backend.clone();
            let backend_cache = Arc::clone(&backend_cache);
            let registry_root = registry_root.clone();

            set.spawn(async move {
                let result = build_one_image(
                    &name,
                    &pipeline,
                    &base_dir,
                    executor,
                    explicit_backend,
                    &backend_cache,
                    registry_root.as_deref(),
                    host_network,
                )
                .await;
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

/// Parse the target OS from an image's effective `platforms` list.
///
/// Rules:
/// - `explicit_os` (e.g. `PipelineImage.os:`) takes precedence — when
///   `Some(os)`, we also verify each `platforms` entry parses to the same
///   OS so a misconfigured platform list surfaces loudly rather than being
///   silently overridden.
/// - Empty list with no explicit OS → [`ImageOs::Linux`] (the historical default).
/// - All entries parse to the same OS → that OS.
/// - Any entry fails to parse → an error naming the bad entry.
/// - Entries parse to *different* OSes → an error, because buildah cannot
///   assemble a single manifest list across OSes and the user should split
///   the image into separate `PipelineImage` entries.
fn target_os_for_image(platforms: &[String], explicit_os: Option<ImageOs>) -> Result<ImageOs> {
    // Parse every entry so we still catch malformed strings even when an
    // explicit OS is supplied; the L-7 validation semantics stay intact.
    let mut selected: Option<ImageOs> = None;
    for platform in platforms {
        let os = ImageOs::from_str(platform).map_err(|e| {
            BuildError::invalid_instruction(
                "pipeline",
                format!("unrecognized platform '{platform}': {e}"),
            )
        })?;
        match selected {
            None => selected = Some(os),
            Some(existing) if existing == os => {}
            Some(existing) => {
                return Err(BuildError::invalid_instruction(
                    "pipeline",
                    format!(
                        "multi-platform images cannot mix OSes in a single entry \
                         (found {existing:?} and {os:?} in platforms={platforms:?}); \
                         split into separate PipelineImage entries"
                    ),
                ));
            }
        }
    }

    if let Some(explicit) = explicit_os {
        if let Some(from_platforms) = selected {
            if from_platforms != explicit {
                return Err(BuildError::invalid_instruction(
                    "pipeline",
                    format!(
                        "explicit os={explicit:?} conflicts with OS inferred from \
                         platforms={platforms:?} (got {from_platforms:?}); remove one \
                         or make them agree"
                    ),
                ));
            }
        }
        return Ok(explicit);
    }

    Ok(selected.unwrap_or(ImageOs::Linux))
}

/// Resolve a build backend for `target_os`, using `cache` to memoize prior
/// detections.
///
/// If `explicit` is `Some`, it is returned unconditionally — callers that
/// constructed the executor via [`PipelineExecutor::with_backend`] have opted
/// into a single backend and we do not second-guess them. Otherwise we check
/// the cache and fall back to [`detect_backend`] on a miss, storing the
/// result before returning.
async fn backend_for(
    target_os: ImageOs,
    cache: &Mutex<HashMap<ImageOs, Arc<dyn BuildBackend>>>,
    explicit: Option<Arc<dyn BuildBackend>>,
) -> Result<Arc<dyn BuildBackend>> {
    if let Some(backend) = explicit {
        return Ok(backend);
    }

    // Fast path: already detected.
    {
        let guard = cache.lock().await;
        if let Some(backend) = guard.get(&target_os) {
            return Ok(Arc::clone(backend));
        }
    }

    // Slow path: detect + memoize. Hold the lock across detection so two
    // concurrent images targeting the same OS share one detection call.
    let mut guard = cache.lock().await;
    if let Some(backend) = guard.get(&target_os) {
        return Ok(Arc::clone(backend));
    }
    let backend = detect_backend(target_os).await?;
    guard.insert(target_os, Arc::clone(&backend));
    Ok(backend)
}

/// Build a single pipeline image end-to-end.
///
/// Parses the target OS, resolves a backend (with caching), logs the build
/// intent, then dispatches to the single-platform or multi-platform path
/// based on the effective platform list.
#[allow(clippy::too_many_arguments)]
async fn build_one_image(
    name: &str,
    pipeline: &ZPipeline,
    base_dir: &Path,
    executor: BuildahExecutor,
    explicit_backend: Option<Arc<dyn BuildBackend>>,
    backend_cache: &Mutex<HashMap<ImageOs, Arc<dyn BuildBackend>>>,
    registry_root: Option<&Path>,
    host_network: bool,
) -> Result<BuiltImage> {
    let image_config = &pipeline.images[name];
    let platforms = effective_platforms(image_config, &pipeline.defaults);

    // L-2: `PipelineImage.os:` takes precedence over OS parsed from platforms.
    let target_os = target_os_for_image(&platforms, image_config.os)?;
    info!(
        "Building image '{}' (target_os={:?}, platforms={:?}, explicit_os={:?})",
        name, target_os, platforms, image_config.os
    );

    let backend = backend_for(target_os, backend_cache, explicit_backend).await?;

    match platforms.len() {
        // No platforms specified — native build (existing behavior)
        0 => {
            build_single_image(
                name,
                pipeline,
                base_dir,
                executor,
                Some(backend),
                None,
                registry_root,
                host_network,
            )
            .await
        }
        // Single platform — use build_single_image with platform set
        1 => {
            let platform = platforms[0].clone();
            build_single_image(
                name,
                pipeline,
                base_dir,
                executor,
                Some(backend),
                Some(&platform),
                registry_root,
                host_network,
            )
            .await
        }
        // Multiple platforms — build each, then create manifest list
        _ => {
            build_multiplatform_image(
                name,
                pipeline,
                base_dir,
                executor,
                Some(backend),
                &platforms,
                registry_root,
                host_network,
            )
            .await
        }
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
    let config: CachedImageConfig = serde_json::from_str(&data).ok()?;
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
#[cfg_attr(not(feature = "local-registry"), allow(unused_variables))]
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
async fn build_single_image(
    name: &str,
    pipeline: &ZPipeline,
    base_dir: &Path,
    executor: BuildahExecutor,
    backend: Option<Arc<dyn BuildBackend>>,
    platform: Option<&str>,
    registry_root: Option<&Path>,
    host_network: bool,
) -> Result<BuiltImage> {
    let image_config = &pipeline.images[name];
    let context = base_dir.join(&image_config.context);
    let file_path = base_dir.join(&image_config.file);

    // Content-based cache invalidation: hash the build file and check if the
    // output image was already built from identical source content.
    let file_hash = compute_file_hash(&file_path).await;
    if let Some(ref hash) = file_hash {
        let data_dir = ZLayerDirs::default_data_dir();

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

    // Expand pipeline `${VAR}` references in the ZImagefile body (base:/run:),
    // so a single ZImagefile set parametrizes across variants (e.g. the Windows
    // `--set LTSC=...` lines). No-op for Dockerfiles and for empty vars.
    builder = builder.pipeline_vars(pipeline.vars.clone());

    // Forward the `LTSC` pipeline var to the Windows backend so FROM-image
    // rewrites pick the correct prebuilt (`ltsc2022` vs `ltsc2025`). We only
    // set it when the pipeline actually declares LTSC so non-Windows builds
    // don't carry a meaningless field.
    if let Some(ltsc) = pipeline.vars.get("LTSC") {
        builder = builder.windows_ltsc(ltsc.clone());
    }

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

    // Forward the pipeline-wide `--host-network` flag (from `zlayer
    // --host-network pipeline ...`).
    builder = builder.with_host_network(host_network);

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
#[cfg_attr(not(feature = "local-registry"), allow(unused_variables))]
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
async fn build_multiplatform_image(
    name: &str,
    pipeline: &ZPipeline,
    base_dir: &Path,
    executor: BuildahExecutor,
    backend: Option<Arc<dyn BuildBackend>>,
    platforms: &[String],
    registry_root: Option<&Path>,
    host_network: bool,
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

        // Expand pipeline `${VAR}` refs in the ZImagefile body (see build_single_image).
        builder = builder.pipeline_vars(pipeline.vars.clone());

        // Forward the `LTSC` pipeline var to the Windows backend (see
        // build_single_image for the rationale). Only set when actually
        // declared so non-Windows multi-platform builds aren't polluted.
        if let Some(ltsc) = pipeline.vars.get("LTSC") {
            builder = builder.windows_ltsc(ltsc.clone());
        }

        // Set platform
        builder = builder.platform(platform);

        // Apply platform-specific tags
        for tag in &platform_tags {
            builder = builder.tag(tag);
        }

        // Apply shared config (build_args, format, no_cache, cache_mounts, retries)
        builder = apply_pipeline_config(builder, image_config, &pipeline.defaults);

        // Forward the pipeline-wide `--host-network` flag (from `zlayer
        // --host-network pipeline ...`).
        builder = builder.with_host_network(host_network);

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
            os: None,
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

    // -----------------------------------------------------------------------
    // L-7: per-image backend selection
    // -----------------------------------------------------------------------

    #[test]
    fn test_target_os_for_image_empty_defaults_to_linux() {
        assert_eq!(target_os_for_image(&[], None).unwrap(), ImageOs::Linux);
    }

    #[test]
    fn test_target_os_for_image_single_linux() {
        assert_eq!(
            target_os_for_image(&["linux/amd64".to_string()], None).unwrap(),
            ImageOs::Linux
        );
    }

    #[test]
    fn test_target_os_for_image_single_windows() {
        assert_eq!(
            target_os_for_image(&["windows/amd64".to_string()], None).unwrap(),
            ImageOs::Windows
        );
    }

    #[test]
    fn test_target_os_for_image_multi_same_os() {
        // All-Linux entries should collapse to a single Linux backend.
        let plats = vec!["linux/amd64".to_string(), "linux/arm64".to_string()];
        assert_eq!(target_os_for_image(&plats, None).unwrap(), ImageOs::Linux);
    }

    #[test]
    fn test_target_os_for_image_mixed_os_is_rejected() {
        // Mixed OSes in a single entry must fail — there is no single backend
        // (or buildah manifest list) that can straddle Linux + Windows.
        let plats = vec!["linux/amd64".to_string(), "windows/amd64".to_string()];
        let err = target_os_for_image(&plats, None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("cannot mix OSes"),
            "expected mix-of-OSes error, got: {msg}"
        );
        assert!(
            msg.contains("split into separate PipelineImage entries"),
            "expected remediation hint, got: {msg}"
        );
    }

    #[test]
    fn test_target_os_for_image_unrecognized_platform() {
        let plats = vec!["plan9/amd64".to_string()];
        let err = target_os_for_image(&plats, None).unwrap_err();
        assert!(err.to_string().contains("unrecognized platform"));
    }

    #[test]
    fn test_target_os_for_image_explicit_os_wins_empty_platforms() {
        // L-2: explicit os: on PipelineImage overrides the default Linux
        // inference when no platforms are set.
        assert_eq!(
            target_os_for_image(&[], Some(ImageOs::Windows)).unwrap(),
            ImageOs::Windows
        );
    }

    #[test]
    fn test_target_os_for_image_explicit_os_matches_platforms() {
        // Explicit os: agrees with the OS portion of platforms — totally fine.
        let plats = vec!["windows/amd64".to_string()];
        assert_eq!(
            target_os_for_image(&plats, Some(ImageOs::Windows)).unwrap(),
            ImageOs::Windows
        );
    }

    #[test]
    fn test_target_os_for_image_explicit_os_conflicts_with_platforms() {
        // Explicit os: disagrees with platforms — must fail loudly so we don't
        // silently override user intent.
        let plats = vec!["linux/amd64".to_string()];
        let err = target_os_for_image(&plats, Some(ImageOs::Windows)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("explicit os=")
                && msg.contains("conflicts with OS inferred from platforms"),
            "expected conflict error, got: {msg}"
        );
    }

    /// A fake [`BuildBackend`] that records which tags it was asked to build
    /// and returns synthetic [`BuiltImage`] values. Used to exercise the
    /// per-image routing without invoking the real buildah CLI.
    struct FakeBackend {
        name: &'static str,
    }

    #[async_trait::async_trait]
    impl BuildBackend for FakeBackend {
        async fn build_image(
            &self,
            _context: &Path,
            _dockerfile: &crate::dockerfile::Dockerfile,
            options: &crate::builder::BuildOptions,
            _event_tx: Option<std::sync::mpsc::Sender<crate::tui::BuildEvent>>,
        ) -> Result<BuiltImage> {
            Ok(BuiltImage {
                image_id: format!("{}:fake-id", self.name),
                tags: options.tags.clone(),
                layer_count: 1,
                size: 0,
                build_time_ms: 1,
                is_manifest: false,
            })
        }

        async fn push_image(
            &self,
            _tag: &str,
            _auth: Option<&crate::builder::RegistryAuth>,
        ) -> Result<()> {
            Ok(())
        }

        async fn tag_image(&self, _image: &str, _new_tag: &str) -> Result<()> {
            Ok(())
        }

        async fn manifest_create(&self, _name: &str) -> Result<()> {
            Ok(())
        }

        async fn manifest_add(&self, _manifest: &str, _image: &str) -> Result<()> {
            Ok(())
        }

        async fn manifest_push(&self, _name: &str, _destination: &str) -> Result<()> {
            Ok(())
        }

        async fn is_available(&self) -> bool {
            true
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    #[tokio::test]
    async fn test_backend_for_uses_explicit_override() {
        // When `explicit` is supplied, `backend_for` must return that backend
        // regardless of target_os and must not touch the cache.
        let explicit: Arc<dyn BuildBackend> = Arc::new(FakeBackend { name: "explicit" });
        let cache: Mutex<HashMap<ImageOs, Arc<dyn BuildBackend>>> = Mutex::new(HashMap::new());
        let resolved = backend_for(ImageOs::Linux, &cache, Some(Arc::clone(&explicit)))
            .await
            .unwrap();
        assert_eq!(resolved.name(), "explicit");
        assert!(
            cache.lock().await.is_empty(),
            "explicit override should not populate cache"
        );
    }

    #[tokio::test]
    async fn test_backend_for_cache_hit_returns_cached() {
        // If the cache already has an entry for the target_os, `backend_for`
        // must return it verbatim without calling detect_backend.
        let fake: Arc<dyn BuildBackend> = Arc::new(FakeBackend { name: "cached" });
        let cache: Mutex<HashMap<ImageOs, Arc<dyn BuildBackend>>> = Mutex::new(HashMap::new());
        cache.lock().await.insert(ImageOs::Linux, Arc::clone(&fake));
        let resolved = backend_for(ImageOs::Linux, &cache, None).await.unwrap();
        assert_eq!(resolved.name(), "cached");
    }

    /// Mixed-OS wave: one Linux image (served by a seeded fake backend) and
    /// one Windows image. On a non-Windows host, `detect_backend(Windows)`
    /// returns an error from L-6; the Linux build should still succeed via
    /// the cached fake backend. This verifies per-image backend selection
    /// and per-image error isolation within a single wave.
    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn test_build_one_image_isolates_windows_failure_on_linux_host() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let ctx = tmp.path();
        // A minimal Dockerfile is enough — the fake backend ignores its
        // contents, but the Dockerfile parser needs *something* to read.
        tokio::fs::write(ctx.join("Dockerfile"), "FROM scratch\n")
            .await
            .unwrap();

        let yaml = r#"
images:
  linux-app:
    file: Dockerfile
    platforms: ["linux/amd64"]
    tags: ["example/linux:dev"]
  win-app:
    file: Dockerfile
    platforms: ["windows/amd64"]
    tags: ["example/windows:dev"]
"#;
        let pipeline = parse_pipeline(yaml).unwrap();

        // Pre-seed the cache with a fake Linux backend so the Linux build
        // does not try to invoke real buildah.
        let cache: Arc<Mutex<HashMap<ImageOs, Arc<dyn BuildBackend>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let fake_linux: Arc<dyn BuildBackend> = Arc::new(FakeBackend { name: "fake-linux" });
        cache
            .lock()
            .await
            .insert(ImageOs::Linux, Arc::clone(&fake_linux));

        // Linux image should succeed via the seeded fake backend.
        let linux_res = build_one_image(
            "linux-app",
            &pipeline,
            ctx,
            BuildahExecutor::with_path("/usr/bin/buildah"),
            None, // no explicit override — exercise per-target_os routing
            &cache,
            None,
            false,
        )
        .await;
        assert!(
            linux_res.is_ok(),
            "Linux image should succeed, got: {linux_res:?}"
        );
        assert_eq!(linux_res.unwrap().image_id, "fake-linux:fake-id");

        // Windows image should fail with the L-6 message — a Windows image
        // cannot be built on a non-Windows host.
        let win_res = build_one_image(
            "win-app",
            &pipeline,
            ctx,
            BuildahExecutor::with_path("/usr/bin/buildah"),
            None,
            &cache,
            None,
            false,
        )
        .await;
        let err = win_res.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Windows host") || msg.contains("windows host"),
            "expected Windows-host error from detect_backend, got: {msg}"
        );

        // The Windows detection failure must not pollute the cache for
        // ImageOs::Windows, and the Linux backend must remain cached.
        let guard = cache.lock().await;
        assert!(guard.contains_key(&ImageOs::Linux));
        assert!(!guard.contains_key(&ImageOs::Windows));
    }
}
