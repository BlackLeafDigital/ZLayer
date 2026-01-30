//! ImageBuilder - High-level API for building container images
//!
//! This module provides the [`ImageBuilder`] type which orchestrates the full
//! container image build process, from Dockerfile parsing through buildah
//! execution to final image creation.
//!
//! # Example
//!
//! ```no_run
//! use zlayer_builder::{ImageBuilder, Runtime};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Build from a Dockerfile
//!     let image = ImageBuilder::new("./my-app").await?
//!         .tag("myapp:latest")
//!         .tag("myapp:v1.0.0")
//!         .build()
//!         .await?;
//!
//!     println!("Built image: {}", image.image_id);
//!     Ok(())
//! }
//! ```
//!
//! # Using Runtime Templates
//!
//! ```no_run
//! use zlayer_builder::{ImageBuilder, Runtime};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Build using a runtime template (no Dockerfile needed)
//!     let image = ImageBuilder::new("./my-node-app").await?
//!         .runtime(Runtime::Node20)
//!         .tag("myapp:latest")
//!         .build()
//!         .await?;
//!
//!     println!("Built image: {}", image.image_id);
//!     Ok(())
//! }
//! ```
//!
//! # Multi-stage Builds with Target
//!
//! ```no_run
//! use zlayer_builder::ImageBuilder;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Build only up to a specific stage
//!     let image = ImageBuilder::new("./my-app").await?
//!         .target("builder")
//!         .tag("myapp:builder")
//!         .build()
//!         .await?;
//!
//!     println!("Built intermediate image: {}", image.image_id);
//!     Ok(())
//! }
//! ```
//!
//! # With TUI Progress Updates
//!
//! ```no_run
//! use zlayer_builder::{ImageBuilder, BuildEvent};
//! use std::sync::mpsc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let (tx, rx) = mpsc::channel::<BuildEvent>();
//!
//!     // Start TUI in another thread
//!     std::thread::spawn(move || {
//!         // Process events from rx...
//!         while let Ok(event) = rx.recv() {
//!             println!("Event: {:?}", event);
//!         }
//!     });
//!
//!     let image = ImageBuilder::new("./my-app").await?
//!         .tag("myapp:latest")
//!         .with_events(tx)
//!         .build()
//!         .await?;
//!
//!     Ok(())
//! }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use tokio::fs;
use tracing::{debug, info, instrument};

use crate::buildah::{BuildahCommand, BuildahExecutor};
use crate::dockerfile::{Dockerfile, ImageRef, Stage};
use crate::error::{BuildError, Result};
use crate::templates::{get_template, Runtime};
use crate::tui::BuildEvent;

/// Built image information returned after a successful build
#[derive(Debug, Clone)]
pub struct BuiltImage {
    /// Image ID (sha256:...)
    pub image_id: String,
    /// Applied tags
    pub tags: Vec<String>,
    /// Number of layers in the final image
    pub layer_count: usize,
    /// Total size in bytes (0 if not computed)
    pub size: u64,
    /// Build duration in milliseconds
    pub build_time_ms: u64,
}

/// Registry authentication credentials
#[derive(Debug, Clone)]
pub struct RegistryAuth {
    /// Registry username
    pub username: String,
    /// Registry password or token
    pub password: String,
}

impl RegistryAuth {
    /// Create new registry authentication
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }
}

/// Build options for customizing the image build process
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Dockerfile path (default: Dockerfile in context)
    pub dockerfile: Option<PathBuf>,
    /// Use runtime template instead of Dockerfile
    pub runtime: Option<Runtime>,
    /// Build arguments (ARG values)
    pub build_args: HashMap<String, String>,
    /// Target stage for multi-stage builds
    pub target: Option<String>,
    /// Image tags to apply
    pub tags: Vec<String>,
    /// Disable layer caching
    pub no_cache: bool,
    /// Push to registry after build
    pub push: bool,
    /// Registry auth (if pushing)
    pub registry_auth: Option<RegistryAuth>,
    /// Squash all layers into one
    pub squash: bool,
    /// Image format (oci or docker)
    pub format: Option<String>,
}

/// Image builder - orchestrates the full build process
///
/// `ImageBuilder` provides a fluent API for configuring and executing
/// container image builds using buildah as the backend.
///
/// # Build Process
///
/// 1. Parse Dockerfile (or use runtime template)
/// 2. Resolve target stages if specified
/// 3. Build each stage sequentially:
///    - Create working container from base image
///    - Execute each instruction
///    - Commit intermediate stages for COPY --from
/// 4. Commit final image with tags
/// 5. Push to registry if configured
/// 6. Clean up intermediate containers
pub struct ImageBuilder {
    /// Build context directory
    context: PathBuf,
    /// Build options
    options: BuildOptions,
    /// Buildah executor
    executor: BuildahExecutor,
    /// Event sender for TUI updates
    event_tx: Option<mpsc::Sender<BuildEvent>>,
}

impl ImageBuilder {
    /// Create a new ImageBuilder with the given context directory
    ///
    /// The context directory should contain the Dockerfile (unless using
    /// a runtime template) and any files that will be copied into the image.
    ///
    /// # Arguments
    ///
    /// * `context` - Path to the build context directory
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The context directory does not exist
    /// - Buildah is not installed or not accessible
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zlayer_builder::ImageBuilder;
    ///
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?;
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip_all, fields(context = %context.as_ref().display()))]
    pub async fn new(context: impl AsRef<Path>) -> Result<Self> {
        let context = context.as_ref().to_path_buf();

        // Verify context exists
        if !context.exists() {
            return Err(BuildError::ContextRead {
                path: context,
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Build context directory not found",
                ),
            });
        }

        // Initialize buildah executor
        let executor = BuildahExecutor::new_async().await?;

        debug!("Created ImageBuilder for context: {}", context.display());

        Ok(Self {
            context,
            options: BuildOptions::default(),
            executor,
            event_tx: None,
        })
    }

    /// Create an ImageBuilder with a custom buildah executor
    ///
    /// This is useful for testing or when you need to configure
    /// the executor with specific storage options.
    pub fn with_executor(context: impl AsRef<Path>, executor: BuildahExecutor) -> Result<Self> {
        let context = context.as_ref().to_path_buf();

        if !context.exists() {
            return Err(BuildError::ContextRead {
                path: context,
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Build context directory not found",
                ),
            });
        }

        Ok(Self {
            context,
            options: BuildOptions::default(),
            executor,
            event_tx: None,
        })
    }

    /// Set a custom Dockerfile path
    ///
    /// By default, the builder looks for a file named `Dockerfile` in the
    /// context directory. Use this method to specify a different path.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use zlayer_builder::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .dockerfile("./my-project/Dockerfile.prod");
    /// # Ok(())
    /// # }
    /// ```
    pub fn dockerfile(mut self, path: impl AsRef<Path>) -> Self {
        self.options.dockerfile = Some(path.as_ref().to_path_buf());
        self
    }

    /// Use a runtime template instead of a Dockerfile
    ///
    /// Runtime templates provide pre-built Dockerfiles for common
    /// development environments. When set, the Dockerfile option is ignored.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zlayer_builder::{ImageBuilder, Runtime};
    ///
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-node-app").await?
    ///     .runtime(Runtime::Node20);
    /// # Ok(())
    /// # }
    /// ```
    pub fn runtime(mut self, runtime: Runtime) -> Self {
        self.options.runtime = Some(runtime);
        self
    }

    /// Add a build argument
    ///
    /// Build arguments are passed to the Dockerfile and can be referenced
    /// using the `ARG` instruction.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use zlayer_builder::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .build_arg("VERSION", "1.0.0")
    ///     .build_arg("DEBUG", "false");
    /// # Ok(())
    /// # }
    /// ```
    pub fn build_arg(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.build_args.insert(key.into(), value.into());
        self
    }

    /// Set multiple build arguments at once
    pub fn build_args(mut self, args: HashMap<String, String>) -> Self {
        self.options.build_args.extend(args);
        self
    }

    /// Set the target stage for multi-stage builds
    ///
    /// When building a multi-stage Dockerfile, you can stop at a specific
    /// stage instead of building all stages.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use zlayer_builder::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// // Dockerfile:
    /// // FROM node:20 AS builder
    /// // ...
    /// // FROM node:20-slim AS runtime
    /// // ...
    ///
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .target("builder")
    ///     .tag("myapp:builder");
    /// # Ok(())
    /// # }
    /// ```
    pub fn target(mut self, stage: impl Into<String>) -> Self {
        self.options.target = Some(stage.into());
        self
    }

    /// Add an image tag
    ///
    /// Tags are applied to the final image. You can add multiple tags.
    /// The first tag is used as the primary image name during commit.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use zlayer_builder::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .tag("myapp:latest")
    ///     .tag("myapp:v1.0.0")
    ///     .tag("registry.example.com/myapp:v1.0.0");
    /// # Ok(())
    /// # }
    /// ```
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.options.tags.push(tag.into());
        self
    }

    /// Disable layer caching
    ///
    /// When enabled, all layers are rebuilt from scratch even if
    /// they could be served from cache.
    pub fn no_cache(mut self) -> Self {
        self.options.no_cache = true;
        self
    }

    /// Push the image to a registry after building
    ///
    /// # Arguments
    ///
    /// * `auth` - Registry authentication credentials
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zlayer_builder::{ImageBuilder, RegistryAuth};
    ///
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .tag("registry.example.com/myapp:v1.0.0")
    ///     .push(RegistryAuth::new("user", "password"));
    /// # Ok(())
    /// # }
    /// ```
    pub fn push(mut self, auth: RegistryAuth) -> Self {
        self.options.push = true;
        self.options.registry_auth = Some(auth);
        self
    }

    /// Enable pushing without authentication
    ///
    /// Use this for registries that don't require authentication
    /// (e.g., local registries, insecure registries).
    pub fn push_without_auth(mut self) -> Self {
        self.options.push = true;
        self.options.registry_auth = None;
        self
    }

    /// Squash all layers into a single layer
    ///
    /// This reduces image size but loses layer caching benefits.
    pub fn squash(mut self) -> Self {
        self.options.squash = true;
        self
    }

    /// Set the image format
    ///
    /// Valid values are "oci" (default) or "docker".
    pub fn format(mut self, format: impl Into<String>) -> Self {
        self.options.format = Some(format.into());
        self
    }

    /// Set an event sender for TUI progress updates
    ///
    /// Events will be sent as the build progresses, allowing you to
    /// display a progress UI or log build status.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zlayer_builder::{ImageBuilder, BuildEvent};
    /// use std::sync::mpsc;
    ///
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let (tx, rx) = mpsc::channel::<BuildEvent>();
    ///
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .tag("myapp:latest")
    ///     .with_events(tx);
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_events(mut self, tx: mpsc::Sender<BuildEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Run the build
    ///
    /// This executes the complete build process:
    /// 1. Parse Dockerfile or load runtime template
    /// 2. Build all required stages
    /// 3. Commit and tag the final image
    /// 4. Push to registry if configured
    /// 5. Clean up intermediate containers
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Dockerfile parsing fails
    /// - A buildah command fails
    /// - Target stage is not found
    /// - Registry push fails
    #[instrument(skip(self), fields(context = %self.context.display()))]
    pub async fn build(self) -> Result<BuiltImage> {
        let start_time = std::time::Instant::now();

        info!("Starting build in context: {}", self.context.display());

        // 1. Get Dockerfile content
        let dockerfile_content = self.get_dockerfile_content().await?;

        // 2. Parse Dockerfile
        let dockerfile = Dockerfile::parse(&dockerfile_content)?;
        debug!("Parsed Dockerfile with {} stages", dockerfile.stages.len());

        // 3. Determine stages to build
        let stages = self.resolve_stages(&dockerfile)?;
        debug!("Building {} stages", stages.len());

        // 4. Build each stage
        let mut stage_images: HashMap<String, String> = HashMap::new();
        let mut final_container: Option<String> = None;
        let mut total_instructions = 0;

        for (stage_idx, stage) in stages.iter().enumerate() {
            let is_final_stage = stage_idx == stages.len() - 1;

            self.send_event(BuildEvent::StageStarted {
                index: stage_idx,
                name: stage.name.clone(),
                base_image: stage.base_image.to_string_ref(),
            });

            // Create container from base image
            let base = self.resolve_base_image(&stage.base_image, &stage_images)?;
            let container_id = self.create_container(&base).await?;

            debug!(
                "Created container {} for stage {} (base: {})",
                container_id,
                stage.identifier(),
                base
            );

            // Execute instructions
            for (inst_idx, instruction) in stage.instructions.iter().enumerate() {
                self.send_event(BuildEvent::InstructionStarted {
                    stage: stage_idx,
                    index: inst_idx,
                    instruction: format!("{:?}", instruction),
                });

                let commands = BuildahCommand::from_instruction(&container_id, instruction);

                for cmd in commands {
                    let output = self
                        .executor
                        .execute_streaming(&cmd, |is_stdout, line| {
                            self.send_event(BuildEvent::Output {
                                line: line.to_string(),
                                is_stderr: !is_stdout,
                            });
                        })
                        .await?;

                    if !output.success() {
                        self.send_event(BuildEvent::BuildFailed {
                            error: output.stderr.clone(),
                        });

                        // Cleanup container
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

                self.send_event(BuildEvent::InstructionComplete {
                    stage: stage_idx,
                    index: inst_idx,
                    cached: false, // TODO: implement caching
                });

                total_instructions += 1;
            }

            // Handle stage completion
            if let Some(name) = &stage.name {
                // Named stage - commit and save for COPY --from
                let image_name = format!("zlayer-build-stage-{}", name);
                self.commit_container(&container_id, &image_name, false)
                    .await?;
                stage_images.insert(name.clone(), image_name.clone());

                // Also add by index
                stage_images.insert(stage.index.to_string(), image_name);

                // If this is also the final stage (named target), keep reference
                if is_final_stage {
                    final_container = Some(container_id);
                } else {
                    // Cleanup intermediate container
                    let _ = self
                        .executor
                        .execute(&BuildahCommand::rm(&container_id))
                        .await;
                }
            } else if is_final_stage {
                // Unnamed final stage - keep container for final commit
                final_container = Some(container_id);
            } else {
                // Unnamed intermediate stage - commit by index for COPY --from
                let image_name = format!("zlayer-build-stage-{}", stage.index);
                self.commit_container(&container_id, &image_name, false)
                    .await?;
                stage_images.insert(stage.index.to_string(), image_name);
                let _ = self
                    .executor
                    .execute(&BuildahCommand::rm(&container_id))
                    .await;
            }

            self.send_event(BuildEvent::StageComplete { index: stage_idx });
        }

        // 5. Commit final image
        let final_container = final_container.ok_or_else(|| BuildError::InvalidInstruction {
            instruction: "build".to_string(),
            reason: "No stages to build".to_string(),
        })?;

        let image_name = self
            .options
            .tags
            .first()
            .cloned()
            .unwrap_or_else(|| format!("zlayer-build:{}", chrono_lite_timestamp()));

        let image_id = self
            .commit_container(&final_container, &image_name, self.options.squash)
            .await?;

        info!("Committed final image: {} ({})", image_name, image_id);

        // 6. Apply additional tags
        for tag in self.options.tags.iter().skip(1) {
            self.tag_image(&image_id, tag).await?;
            debug!("Applied tag: {}", tag);
        }

        // 7. Cleanup
        let _ = self
            .executor
            .execute(&BuildahCommand::rm(&final_container))
            .await;

        // Cleanup intermediate stage images
        for (_, img) in stage_images {
            let _ = self.executor.execute(&BuildahCommand::rmi(&img)).await;
        }

        // 8. Push if requested
        if self.options.push {
            for tag in &self.options.tags {
                self.push_image(tag).await?;
                info!("Pushed image: {}", tag);
            }
        }

        let build_time_ms = start_time.elapsed().as_millis() as u64;

        self.send_event(BuildEvent::BuildComplete {
            image_id: image_id.clone(),
        });

        info!(
            "Build completed in {}ms: {} with {} tags",
            build_time_ms,
            image_id,
            self.options.tags.len()
        );

        Ok(BuiltImage {
            image_id,
            tags: self.options.tags.clone(),
            layer_count: total_instructions,
            size: 0, // TODO: get actual size via buildah inspect
            build_time_ms,
        })
    }

    /// Get Dockerfile content from template or file
    async fn get_dockerfile_content(&self) -> Result<String> {
        if let Some(runtime) = &self.options.runtime {
            debug!("Using runtime template: {}", runtime);
            return Ok(get_template(*runtime).to_string());
        }

        let dockerfile_path = self
            .options
            .dockerfile
            .clone()
            .unwrap_or_else(|| self.context.join("Dockerfile"));

        debug!("Reading Dockerfile: {}", dockerfile_path.display());

        fs::read_to_string(&dockerfile_path)
            .await
            .map_err(|e| BuildError::ContextRead {
                path: dockerfile_path,
                source: e,
            })
    }

    /// Resolve which stages need to be built
    fn resolve_stages<'a>(&self, dockerfile: &'a Dockerfile) -> Result<Vec<&'a Stage>> {
        if let Some(target) = &self.options.target {
            // Find target stage and all its dependencies
            self.resolve_target_stages(dockerfile, target)
        } else {
            // Build all stages
            Ok(dockerfile.stages.iter().collect())
        }
    }

    /// Resolve stages needed for a specific target
    fn resolve_target_stages<'a>(
        &self,
        dockerfile: &'a Dockerfile,
        target: &str,
    ) -> Result<Vec<&'a Stage>> {
        // Find the target stage
        let target_stage = dockerfile
            .get_stage(target)
            .ok_or_else(|| BuildError::stage_not_found(target))?;

        // Collect all stages up to and including the target
        // This is a simplified approach - a full implementation would
        // analyze COPY --from dependencies
        let mut stages: Vec<&Stage> = Vec::new();

        for stage in &dockerfile.stages {
            stages.push(stage);
            if stage.index == target_stage.index {
                break;
            }
        }

        Ok(stages)
    }

    /// Resolve a base image reference to an actual image name
    fn resolve_base_image(
        &self,
        image_ref: &ImageRef,
        stage_images: &HashMap<String, String>,
    ) -> Result<String> {
        match image_ref {
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
            ImageRef::Stage(name) => stage_images
                .get(name)
                .cloned()
                .ok_or_else(|| BuildError::stage_not_found(name)),
            ImageRef::Scratch => Ok("scratch".to_string()),
        }
    }

    /// Create a working container from an image
    async fn create_container(&self, image: &str) -> Result<String> {
        let cmd = BuildahCommand::from_image(image);
        let output = self.executor.execute_checked(&cmd).await?;
        Ok(output.stdout.trim().to_string())
    }

    /// Commit a container to create an image
    async fn commit_container(
        &self,
        container: &str,
        image_name: &str,
        squash: bool,
    ) -> Result<String> {
        let cmd = BuildahCommand::commit_with_opts(
            container,
            image_name,
            self.options.format.as_deref(),
            squash,
        );
        let output = self.executor.execute_checked(&cmd).await?;
        Ok(output.stdout.trim().to_string())
    }

    /// Tag an image with an additional tag
    async fn tag_image(&self, image: &str, tag: &str) -> Result<()> {
        let cmd = BuildahCommand::tag(image, tag);
        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    /// Push an image to a registry
    async fn push_image(&self, tag: &str) -> Result<()> {
        let mut cmd = BuildahCommand::push(tag);

        // Add auth if provided
        if let Some(auth) = &self.options.registry_auth {
            cmd = cmd
                .arg("--creds")
                .arg(format!("{}:{}", auth.username, auth.password));
        }

        self.executor.execute_checked(&cmd).await?;
        Ok(())
    }

    /// Send an event to the TUI (if configured)
    fn send_event(&self, event: BuildEvent) {
        if let Some(tx) = &self.event_tx {
            // Ignore send errors - the receiver may have been dropped
            let _ = tx.send(event);
        }
    }
}

// Helper function to generate a timestamp-based name
fn chrono_lite_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_auth_new() {
        let auth = RegistryAuth::new("user", "pass");
        assert_eq!(auth.username, "user");
        assert_eq!(auth.password, "pass");
    }

    #[test]
    fn test_build_options_default() {
        let opts = BuildOptions::default();
        assert!(opts.dockerfile.is_none());
        assert!(opts.runtime.is_none());
        assert!(opts.build_args.is_empty());
        assert!(opts.target.is_none());
        assert!(opts.tags.is_empty());
        assert!(!opts.no_cache);
        assert!(!opts.push);
        assert!(!opts.squash);
    }

    #[test]
    fn test_resolve_base_image_registry() {
        let builder = create_test_builder();
        let stage_images = HashMap::new();

        // Simple image
        let image_ref = ImageRef::Registry {
            image: "alpine".to_string(),
            tag: Some("3.18".to_string()),
            digest: None,
        };
        let result = builder.resolve_base_image(&image_ref, &stage_images);
        assert_eq!(result.unwrap(), "alpine:3.18");

        // Image with digest
        let image_ref = ImageRef::Registry {
            image: "alpine".to_string(),
            tag: None,
            digest: Some("sha256:abc123".to_string()),
        };
        let result = builder.resolve_base_image(&image_ref, &stage_images);
        assert_eq!(result.unwrap(), "alpine@sha256:abc123");

        // Image with no tag or digest
        let image_ref = ImageRef::Registry {
            image: "alpine".to_string(),
            tag: None,
            digest: None,
        };
        let result = builder.resolve_base_image(&image_ref, &stage_images);
        assert_eq!(result.unwrap(), "alpine:latest");
    }

    #[test]
    fn test_resolve_base_image_stage() {
        let builder = create_test_builder();
        let mut stage_images = HashMap::new();
        stage_images.insert(
            "builder".to_string(),
            "zlayer-build-stage-builder".to_string(),
        );

        let image_ref = ImageRef::Stage("builder".to_string());
        let result = builder.resolve_base_image(&image_ref, &stage_images);
        assert_eq!(result.unwrap(), "zlayer-build-stage-builder");

        // Missing stage
        let image_ref = ImageRef::Stage("missing".to_string());
        let result = builder.resolve_base_image(&image_ref, &stage_images);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_base_image_scratch() {
        let builder = create_test_builder();
        let stage_images = HashMap::new();

        let image_ref = ImageRef::Scratch;
        let result = builder.resolve_base_image(&image_ref, &stage_images);
        assert_eq!(result.unwrap(), "scratch");
    }

    fn create_test_builder() -> ImageBuilder {
        // Create a minimal builder for testing (without async initialization)
        ImageBuilder {
            context: PathBuf::from("/tmp/test"),
            options: BuildOptions::default(),
            executor: BuildahExecutor::with_path("/usr/bin/buildah"),
            event_tx: None,
        }
    }

    // Builder method chaining tests
    #[test]
    fn test_builder_chaining() {
        let mut builder = create_test_builder();

        builder = builder
            .dockerfile("./Dockerfile.test")
            .runtime(Runtime::Node20)
            .build_arg("VERSION", "1.0")
            .target("builder")
            .tag("myapp:latest")
            .tag("myapp:v1")
            .no_cache()
            .squash()
            .format("oci");

        assert_eq!(
            builder.options.dockerfile,
            Some(PathBuf::from("./Dockerfile.test"))
        );
        assert_eq!(builder.options.runtime, Some(Runtime::Node20));
        assert_eq!(
            builder.options.build_args.get("VERSION"),
            Some(&"1.0".to_string())
        );
        assert_eq!(builder.options.target, Some("builder".to_string()));
        assert_eq!(builder.options.tags.len(), 2);
        assert!(builder.options.no_cache);
        assert!(builder.options.squash);
        assert_eq!(builder.options.format, Some("oci".to_string()));
    }

    #[test]
    fn test_builder_push_with_auth() {
        let mut builder = create_test_builder();
        builder = builder.push(RegistryAuth::new("user", "pass"));

        assert!(builder.options.push);
        assert!(builder.options.registry_auth.is_some());
        let auth = builder.options.registry_auth.unwrap();
        assert_eq!(auth.username, "user");
        assert_eq!(auth.password, "pass");
    }

    #[test]
    fn test_builder_push_without_auth() {
        let mut builder = create_test_builder();
        builder = builder.push_without_auth();

        assert!(builder.options.push);
        assert!(builder.options.registry_auth.is_none());
    }

    #[test]
    fn test_chrono_lite_timestamp() {
        let ts = chrono_lite_timestamp();
        // Should be a valid number
        let parsed: u64 = ts.parse().expect("Should be a valid u64");
        // Should be reasonably recent (after 2024)
        assert!(parsed > 1700000000);
    }
}
