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

use crate::buildah::BuildahExecutor;
use crate::builder::{BuiltImage, ImageBuilder};
use crate::error::{BuildError, Result};

use super::types::ZPipeline;

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
    pub fn is_success(&self) -> bool {
        self.failed.is_empty()
    }

    /// Returns the total number of images in the pipeline
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
    /// Whether to abort on first failure
    fail_fast: bool,
    /// Whether to push images after building
    push_enabled: bool,
}

impl PipelineExecutor {
    /// Create a new pipeline executor
    ///
    /// # Arguments
    ///
    /// * `pipeline` - The parsed ZPipeline configuration
    /// * `base_dir` - Base directory for resolving relative paths in the pipeline
    /// * `executor` - The buildah executor to use for all builds
    pub fn new(pipeline: ZPipeline, base_dir: PathBuf, executor: BuildahExecutor) -> Self {
        // Determine push behavior from pipeline config
        let push_enabled = pipeline.push.after_all;

        Self {
            pipeline,
            base_dir,
            executor,
            fail_fast: true,
            push_enabled,
        }
    }

    /// Set fail-fast mode (default: true)
    ///
    /// When enabled, the executor will abort immediately when any image
    /// fails to build. When disabled, it will continue building independent
    /// images even after failures.
    pub fn fail_fast(mut self, fail_fast: bool) -> Self {
        self.fail_fast = fail_fast;
        self
    }

    /// Enable or disable pushing (overrides pipeline.push.after_all)
    ///
    /// When enabled and all builds succeed, images will be pushed to their
    /// configured registries.
    pub fn push(mut self, enabled: bool) -> Self {
        self.push_enabled = enabled;
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
                        format!("Image '{}' depends on unknown image '{}'", name, dep),
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
                    if let Err(e) = self.push_image(tag).await {
                        warn!("[{}] Failed to push {}: {}", name, tag, e);
                        // Push failures don't fail the overall pipeline
                        // since the images were built successfully
                    } else {
                        info!("[{}] Pushed: {}", name, tag);
                    }
                }
            }
        }

        Ok(PipelineResult {
            succeeded,
            failed,
            total_time_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Build all images in a wave concurrently
    ///
    /// Returns a vector of (name, result) tuples for each image in the wave.
    async fn build_wave(&self, wave: &[String]) -> Vec<(String, Result<BuiltImage>)> {
        // Create shared data for spawned tasks
        let pipeline = Arc::new(self.pipeline.clone());
        let base_dir = Arc::new(self.base_dir.clone());
        let executor = self.executor.clone();

        let mut set = JoinSet::new();

        for name in wave {
            let name = name.clone();
            let pipeline = Arc::clone(&pipeline);
            let base_dir = Arc::clone(&base_dir);
            let executor = executor.clone();

            set.spawn(async move {
                let result = build_single_image(&name, &pipeline, &base_dir, executor).await;
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
                            format!("Build task panicked: {}", e),
                        )),
                    ));
                }
            }
        }

        results
    }

    /// Push an image to its registry
    async fn push_image(&self, tag: &str) -> Result<()> {
        use crate::buildah::BuildahCommand;

        let cmd = BuildahCommand::push(tag);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }
}

/// Build a single image from the pipeline
///
/// This is extracted as a separate function to make it easier to spawn
/// in a tokio task without borrowing issues.
async fn build_single_image(
    name: &str,
    pipeline: &ZPipeline,
    base_dir: &Path,
    executor: BuildahExecutor,
) -> Result<BuiltImage> {
    let image_config = &pipeline.images[name];
    let context = base_dir.join(&image_config.context);
    let file_path = base_dir.join(&image_config.file);

    let mut builder = ImageBuilder::with_executor(&context, executor)?;

    // Determine if this is a ZImagefile or Dockerfile based on extension/name
    let file_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let extension = file_path
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();

    if extension == "yaml" || extension == "yml" || file_name.starts_with("ZImagefile") {
        builder = builder.zimagefile(&file_path);
    } else {
        builder = builder.dockerfile(&file_path);
    }

    // Apply tags with variable expansion
    for tag in &image_config.tags {
        let expanded = expand_tag_with_vars(tag, &pipeline.vars);
        builder = builder.tag(expanded);
    }

    // Merge build_args: defaults + per-image (per-image overrides defaults)
    let mut args = pipeline.defaults.build_args.clone();
    args.extend(image_config.build_args.clone());
    builder = builder.build_args(args);

    // Apply format (per-image overrides default)
    if let Some(fmt) = image_config
        .format
        .as_ref()
        .or(pipeline.defaults.format.as_ref())
    {
        builder = builder.format(fmt);
    }

    // Apply no_cache (per-image overrides default)
    if image_config.no_cache.unwrap_or(pipeline.defaults.no_cache) {
        builder = builder.no_cache();
    }

    builder.build().await
}

/// Expand variables in a tag string
///
/// Standalone function for use in spawned tasks.
fn expand_tag_with_vars(tag: &str, vars: &HashMap<String, String>) -> String {
    let mut result = tag.to_string();
    for (key, value) in vars {
        result = result.replace(&format!("${{{}}}", key), value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::parse_pipeline;

    #[test]
    fn test_resolve_execution_order_simple() {
        let yaml = r#"
images:
  app:
    file: Dockerfile
"#;
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
        let yaml = r#"
images:
  base:
    file: Dockerfile.base
  app:
    file: Dockerfile.app
    depends_on: [base]
  test:
    file: Dockerfile.test
    depends_on: [app]
"#;
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
        let yaml = r#"
images:
  base:
    file: Dockerfile.base
  app1:
    file: Dockerfile.app1
    depends_on: [base]
  app2:
    file: Dockerfile.app2
    depends_on: [base]
"#;
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
        let yaml = r#"
images:
  app:
    file: Dockerfile
    depends_on: [missing]
"#;
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
        let yaml = r#"
images:
  a:
    file: Dockerfile.a
    depends_on: [b]
  b:
    file: Dockerfile.b
    depends_on: [a]
"#;
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
            e => panic!("Expected CircularDependency error, got: {:?}", e),
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
            },
        );
        result
            .failed
            .insert("app2".to_string(), "error".to_string());

        assert_eq!(result.total_images(), 2);
    }

    #[test]
    fn test_builder_methods() {
        let yaml = r#"
images:
  app:
    file: Dockerfile
push:
  after_all: true
"#;
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
}
