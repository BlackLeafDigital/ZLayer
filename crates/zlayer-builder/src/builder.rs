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
//!
//! # With Cache Backend (requires `cache` feature)
//!
//! ```no_run,ignore
//! use zlayer_builder::ImageBuilder;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let image = ImageBuilder::new("./my-app").await?
//!         .with_cache_dir("/var/cache/zlayer")  // Use persistent disk cache
//!         .tag("myapp:latest")
//!         .build()
//!         .await?;
//!
//!     println!("Built image: {}", image.image_id);
//!     Ok(())
//! }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use tokio::fs;
use tracing::{debug, info, instrument};

use crate::buildah::{BuildahCommand, BuildahExecutor};
use crate::dockerfile::{Dockerfile, ImageRef, Instruction, Stage};
use crate::error::{BuildError, Result};
use crate::templates::{get_template, Runtime};
use crate::tui::BuildEvent;

// Cache backend integration (optional, requires `cache` feature)
#[cfg(feature = "cache")]
use std::sync::Arc;

#[cfg(feature = "cache")]
use zlayer_registry::cache::BlobCacheBackend;

#[cfg(feature = "local-registry")]
use zlayer_registry::LocalRegistry;

/// Configuration for the layer cache backend.
///
/// This enum specifies which cache backend to use for storing and retrieving
/// cached layers during builds. The cache feature must be enabled for this
/// to be available.
///
/// # Example
///
/// ```no_run,ignore
/// use zlayer_builder::{ImageBuilder, CacheBackendConfig};
///
/// # async fn example() -> Result<(), zlayer_builder::BuildError> {
/// // Use persistent disk cache
/// let builder = ImageBuilder::new("./my-app").await?
///     .with_cache_config(CacheBackendConfig::Persistent {
///         path: "/var/cache/zlayer".into(),
///     })
///     .tag("myapp:latest");
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "cache")]
#[derive(Debug, Clone, Default)]
pub enum CacheBackendConfig {
    /// In-memory cache (cleared when process exits).
    ///
    /// Useful for CI/CD environments where persistence isn't needed
    /// but you want to avoid re-downloading base image layers within
    /// a single build session.
    #[default]
    Memory,

    /// Persistent disk-based cache using redb.
    ///
    /// Requires the `cache-persistent` feature. Layers are stored on disk
    /// and persist across builds, significantly speeding up repeated builds.
    #[cfg(feature = "cache-persistent")]
    Persistent {
        /// Path to the cache directory or database file.
        /// If a directory, `blob_cache.redb` will be created inside it.
        path: PathBuf,
    },

    /// S3-compatible object storage backend.
    ///
    /// Requires the `cache-s3` feature. Useful for distributed build systems
    /// where multiple build machines need to share a cache.
    #[cfg(feature = "cache-s3")]
    S3 {
        /// S3 bucket name
        bucket: String,
        /// AWS region (optional, uses SDK default if not set)
        region: Option<String>,
        /// Custom endpoint URL (for S3-compatible services like R2, B2, MinIO)
        endpoint: Option<String>,
        /// Key prefix for cached blobs (default: "zlayer/layers/")
        prefix: Option<String>,
    },
}

/// Tracks layer cache state during builds.
///
/// This struct maintains a mapping of instruction cache keys combined with
/// base layer identifiers to determine if a layer was previously built and
/// can be served from cache.
///
/// # Cache Key Format
///
/// The cache key is a tuple of:
/// - `instruction_key`: A hash of the instruction type and its parameters
///   (generated by [`Instruction::cache_key()`])
/// - `base_layer`: The container/image ID that the instruction was executed on
///
/// Together, these uniquely identify a layer's content.
///
/// # Future Enhancements
///
/// Currently, cache hit detection is limited because buildah's manual container
/// creation workflow (`buildah from`, `buildah run`, `buildah commit`) doesn't
/// directly expose layer reuse information. To implement true cache detection,
/// we would need to:
///
/// 1. **Parse buildah output**: Look for indicators in command output that suggest
///    layer reuse (e.g., fast execution time, specific log messages)
///
/// 2. **Implement layer digest comparison**: Before executing an instruction,
///    compute what the expected layer digest would be and check if it already
///    exists in local storage
///
/// 3. **Switch to `buildah build`**: The `buildah build` command has native
///    caching support with `--layers` flag that automatically handles cache hits
///
/// 4. **Use external cache registry**: Implement `--cache-from`/`--cache-to`
///    semantics by pulling/pushing layer digests from a remote registry
#[derive(Debug, Default)]
struct LayerCacheTracker {
    /// Maps (instruction_cache_key, base_layer_id) -> was_cached
    known_layers: HashMap<(String, String), bool>,
}

impl LayerCacheTracker {
    /// Create a new empty cache tracker.
    fn new() -> Self {
        Self::default()
    }

    /// Check if we have a cached result for this instruction on the given base layer.
    ///
    /// # Arguments
    ///
    /// * `instruction_key` - The cache key from [`Instruction::cache_key()`]
    /// * `base_layer` - The container or image ID the instruction runs on
    ///
    /// # Returns
    ///
    /// `true` if we've previously recorded this instruction as cached,
    /// `false` otherwise (including if we've never seen this combination).
    #[allow(dead_code)]
    fn is_cached(&self, instruction_key: &str, base_layer: &str) -> bool {
        self.known_layers
            .get(&(instruction_key.to_string(), base_layer.to_string()))
            .copied()
            .unwrap_or(false)
    }

    /// Record the cache status for an instruction execution.
    ///
    /// # Arguments
    ///
    /// * `instruction_key` - The cache key from [`Instruction::cache_key()`]
    /// * `base_layer` - The container or image ID the instruction ran on
    /// * `cached` - Whether this execution was a cache hit
    fn record(&mut self, instruction_key: String, base_layer: String, cached: bool) {
        self.known_layers
            .insert((instruction_key, base_layer), cached);
    }

    /// Attempt to detect if an instruction execution was a cache hit.
    ///
    /// This is a heuristic-based approach since buildah doesn't directly report
    /// cache status for manual container operations.
    ///
    /// # Current Implementation
    ///
    /// Always returns `false` - true cache detection would require:
    /// - Timing analysis (cached operations are typically < 100ms)
    /// - Output parsing for cache-related messages
    /// - Pre-computation of expected layer digests
    ///
    /// # Arguments
    ///
    /// * `_instruction` - The instruction that was executed
    /// * `_execution_time_ms` - How long the execution took in milliseconds
    /// * `_output` - The command's stdout/stderr output
    ///
    /// # Returns
    ///
    /// `true` if the execution appears to be a cache hit, `false` otherwise.
    ///
    /// TODO: Implement heuristic cache detection based on:
    /// - Execution time (cached layers typically commit in < 100ms)
    /// - Output analysis (look for "Using cache" or similar messages)
    /// - Layer digest comparison with existing images
    #[allow(dead_code)]
    fn detect_cache_hit(
        &self,
        _instruction: &Instruction,
        _execution_time_ms: u64,
        _output: &str,
    ) -> bool {
        // TODO: Implement cache hit detection heuristics
        //
        // Possible approaches:
        // 1. Time-based: If execution took < 100ms, likely cached
        //    if execution_time_ms < 100 { return true; }
        //
        // 2. Output-based: Look for cache indicators in buildah output
        //    if output.contains("Using cache") { return true; }
        //
        // 3. Digest-based: Pre-compute expected digest and check storage
        //    let expected = compute_layer_digest(instruction, base_layer);
        //    if layer_exists_in_storage(expected) { return true; }
        //
        // For now, always return false until we have reliable detection
        false
    }
}

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
#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Dockerfile path (default: Dockerfile in context)
    pub dockerfile: Option<PathBuf>,
    /// ZImagefile path (alternative to Dockerfile)
    pub zimagefile: Option<PathBuf>,
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
    /// Enable buildah layer caching (--layers flag for `buildah build`).
    /// Default: true
    ///
    /// Note: ZLayer uses manual container creation (`buildah from`, `buildah run`,
    /// `buildah commit`) rather than `buildah build`, so this flag is reserved
    /// for future use when/if we switch to `buildah build` (bud) command.
    pub layers: bool,
    /// Registry to pull cache from (--cache-from for `buildah build`).
    ///
    /// Note: This would be used with `buildah build --cache-from=<registry>`.
    /// Currently ZLayer uses manual container creation, so this is reserved
    /// for future implementation or for switching to `buildah build`.
    ///
    /// TODO: Implement remote cache support. This would require either:
    /// 1. Switching to `buildah build` command which supports --cache-from natively
    /// 2. Implementing custom layer caching with registry push/pull for intermediate layers
    pub cache_from: Option<String>,
    /// Registry to push cache to (--cache-to for `buildah build`).
    ///
    /// Note: This would be used with `buildah build --cache-to=<registry>`.
    /// Currently ZLayer uses manual container creation, so this is reserved
    /// for future implementation or for switching to `buildah build`.
    ///
    /// TODO: Implement remote cache support. This would require either:
    /// 1. Switching to `buildah build` command which supports --cache-to natively
    /// 2. Implementing custom layer caching with registry push/pull for intermediate layers
    pub cache_to: Option<String>,
    /// Maximum cache age (--cache-ttl for `buildah build`).
    ///
    /// Note: This would be used with `buildah build --cache-ttl=<duration>`.
    /// Currently ZLayer uses manual container creation, so this is reserved
    /// for future implementation or for switching to `buildah build`.
    ///
    /// TODO: Implement cache TTL support. This would require either:
    /// 1. Switching to `buildah build` command which supports --cache-ttl natively
    /// 2. Implementing custom cache expiration logic for our layer caching system
    pub cache_ttl: Option<std::time::Duration>,
    /// Cache backend configuration (requires `cache` feature).
    ///
    /// When configured, the builder will store layer data in the specified
    /// cache backend for faster subsequent builds. This is separate from
    /// buildah's native caching and operates at the ZLayer level.
    ///
    /// # Integration Points
    ///
    /// The cache backend is used at several points during the build:
    ///
    /// 1. **Before instruction execution**: Check if a cached layer exists
    ///    for the (instruction_hash, base_layer) tuple
    /// 2. **After instruction execution**: Store the resulting layer data
    ///    in the cache for future builds
    /// 3. **Base image layers**: Cache pulled base image layers to avoid
    ///    re-downloading from registries
    ///
    /// TODO: Wire up cache lookups in the build loop once layer digests
    /// are properly computed and tracked.
    #[cfg(feature = "cache")]
    pub cache_backend_config: Option<CacheBackendConfig>,
    /// Default OCI/WASM-compatible registry to check for images before falling
    /// back to Docker Hub qualification.
    ///
    /// When set, the builder will probe this registry for short image names
    /// before qualifying them to `docker.io`. For example, if set to
    /// `"git.example.com:5000"` and the ZImagefile uses `base: "myapp:latest"`,
    /// the builder will check `git.example.com:5000/myapp:latest` first.
    pub default_registry: Option<String>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            dockerfile: None,
            zimagefile: None,
            runtime: None,
            build_args: HashMap::new(),
            target: None,
            tags: Vec::new(),
            no_cache: false,
            push: false,
            registry_auth: None,
            squash: false,
            format: None,
            layers: true,
            cache_from: None,
            cache_to: None,
            cache_ttl: None,
            #[cfg(feature = "cache")]
            cache_backend_config: None,
            default_registry: None,
        }
    }
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
///
/// # Cache Backend Integration (requires `cache` feature)
///
/// When a cache backend is configured, the builder can store and retrieve
/// cached layer data to speed up subsequent builds:
///
/// ```no_run,ignore
/// use zlayer_builder::ImageBuilder;
///
/// let builder = ImageBuilder::new("./my-app").await?
///     .with_cache_dir("/var/cache/zlayer")
///     .tag("myapp:latest");
/// ```
pub struct ImageBuilder {
    /// Build context directory
    context: PathBuf,
    /// Build options
    options: BuildOptions,
    /// Buildah executor
    executor: BuildahExecutor,
    /// Event sender for TUI updates
    event_tx: Option<mpsc::Sender<BuildEvent>>,
    /// Cache backend for layer caching (requires `cache` feature).
    ///
    /// When set, the builder will attempt to retrieve cached layers before
    /// executing instructions, and store results in the cache after execution.
    ///
    /// TODO: Implement cache lookups in the build loop. Currently the backend
    /// is stored but not actively used during builds. Integration points:
    /// - Check cache before executing RUN instructions
    /// - Store layer data after successful instruction execution
    /// - Cache base image layers pulled from registries
    #[cfg(feature = "cache")]
    cache_backend: Option<Arc<Box<dyn BlobCacheBackend>>>,
    /// Local OCI registry for checking cached images before remote pulls.
    #[cfg(feature = "local-registry")]
    local_registry: Option<LocalRegistry>,
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
            #[cfg(feature = "cache")]
            cache_backend: None,
            #[cfg(feature = "local-registry")]
            local_registry: None,
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
            #[cfg(feature = "cache")]
            cache_backend: None,
            #[cfg(feature = "local-registry")]
            local_registry: None,
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

    /// Set a custom ZImagefile path
    ///
    /// ZImagefiles are a YAML-based alternative to Dockerfiles. When set,
    /// the builder will parse the ZImagefile and convert it to the internal
    /// Dockerfile IR for execution.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use zlayer_builder::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .zimagefile("./my-project/ZImagefile");
    /// # Ok(())
    /// # }
    /// ```
    pub fn zimagefile(mut self, path: impl AsRef<Path>) -> Self {
        self.options.zimagefile = Some(path.as_ref().to_path_buf());
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
    ///
    /// Note: Currently this flag is tracked but not fully implemented in the
    /// build process. ZLayer uses manual container creation (`buildah from`,
    /// `buildah run`, `buildah commit`) which doesn't have built-in caching
    /// like `buildah build` does. Future work could implement layer-level
    /// caching by checking instruction hashes against previously built layers.
    pub fn no_cache(mut self) -> Self {
        self.options.no_cache = true;
        self
    }

    /// Enable or disable layer caching
    ///
    /// This controls the `--layers` flag for buildah. When enabled (default),
    /// buildah can cache and reuse intermediate layers.
    ///
    /// Note: ZLayer currently uses manual container creation (`buildah from`,
    /// `buildah run`, `buildah commit`) rather than `buildah build`, so this
    /// flag is reserved for future use when/if we switch to `buildah build`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use zlayer_builder::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .layers(false)  // Disable layer caching
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    pub fn layers(mut self, enable: bool) -> Self {
        self.options.layers = enable;
        self
    }

    /// Set registry to pull cache from
    ///
    /// This corresponds to buildah's `--cache-from` flag, which allows
    /// pulling cached layers from a remote registry to speed up builds.
    ///
    /// Note: ZLayer currently uses manual container creation (`buildah from`,
    /// `buildah run`, `buildah commit`) rather than `buildah build`, so this
    /// option is reserved for future implementation.
    ///
    /// TODO: Implement remote cache support. This would require either:
    /// 1. Switching to `buildah build` command which supports --cache-from natively
    /// 2. Implementing custom layer caching with registry pull for intermediate layers
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use zlayer_builder::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .cache_from("registry.example.com/myapp:cache")
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    pub fn cache_from(mut self, registry: impl Into<String>) -> Self {
        self.options.cache_from = Some(registry.into());
        self
    }

    /// Set registry to push cache to
    ///
    /// This corresponds to buildah's `--cache-to` flag, which allows
    /// pushing cached layers to a remote registry for future builds to use.
    ///
    /// Note: ZLayer currently uses manual container creation (`buildah from`,
    /// `buildah run`, `buildah commit`) rather than `buildah build`, so this
    /// option is reserved for future implementation.
    ///
    /// TODO: Implement remote cache support. This would require either:
    /// 1. Switching to `buildah build` command which supports --cache-to natively
    /// 2. Implementing custom layer caching with registry push for intermediate layers
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use zlayer_builder::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .cache_to("registry.example.com/myapp:cache")
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    pub fn cache_to(mut self, registry: impl Into<String>) -> Self {
        self.options.cache_to = Some(registry.into());
        self
    }

    /// Set maximum cache age
    ///
    /// This corresponds to buildah's `--cache-ttl` flag, which sets the
    /// maximum age for cached layers before they are considered stale.
    ///
    /// Note: ZLayer currently uses manual container creation (`buildah from`,
    /// `buildah run`, `buildah commit`) rather than `buildah build`, so this
    /// option is reserved for future implementation.
    ///
    /// TODO: Implement cache TTL support. This would require either:
    /// 1. Switching to `buildah build` command which supports --cache-ttl natively
    /// 2. Implementing custom cache expiration logic for our layer caching system
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use zlayer_builder::ImageBuilder;
    /// # use std::time::Duration;
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .cache_ttl(Duration::from_secs(3600 * 24))  // 24 hours
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    pub fn cache_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.options.cache_ttl = Some(ttl);
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

    /// Set a default OCI/WASM-compatible registry to check for images.
    ///
    /// When set, the builder will probe this registry for short image names
    /// before qualifying them to `docker.io`. For example, if set to
    /// `"git.example.com:5000"` and the ZImagefile uses `base: "myapp:latest"`,
    /// the builder will check `git.example.com:5000/myapp:latest` first.
    pub fn default_registry(mut self, registry: impl Into<String>) -> Self {
        self.options.default_registry = Some(registry.into());
        self
    }

    /// Set a local OCI registry for image resolution.
    ///
    /// When set, the builder checks the local registry for cached images
    /// before pulling from remote registries.
    #[cfg(feature = "local-registry")]
    pub fn with_local_registry(mut self, registry: LocalRegistry) -> Self {
        self.local_registry = Some(registry);
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

    /// Configure a persistent disk cache backend for layer caching.
    ///
    /// When configured, the builder will store layer data on disk at the
    /// specified path. This cache persists across builds and significantly
    /// speeds up repeated builds of similar images.
    ///
    /// Requires the `cache-persistent` feature to be enabled.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the cache directory. If a directory, creates
    ///   `blob_cache.redb` inside it. If a file path, uses it directly.
    ///
    /// # Example
    ///
    /// ```no_run,ignore
    /// use zlayer_builder::ImageBuilder;
    ///
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .with_cache_dir("/var/cache/zlayer")
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Integration Status
    ///
    /// TODO: The cache backend is currently stored but not actively used
    /// during builds. Future work will wire up:
    /// - Cache lookups before executing RUN instructions
    /// - Storing layer data after successful execution
    /// - Caching base image layers from registry pulls
    #[cfg(feature = "cache-persistent")]
    pub fn with_cache_dir(mut self, path: impl AsRef<Path>) -> Self {
        self.options.cache_backend_config = Some(CacheBackendConfig::Persistent {
            path: path.as_ref().to_path_buf(),
        });
        debug!(
            "Configured persistent cache at: {}",
            path.as_ref().display()
        );
        self
    }

    /// Configure an in-memory cache backend for layer caching.
    ///
    /// The in-memory cache is cleared when the process exits, but can
    /// speed up builds within a single session by caching intermediate
    /// layers and avoiding redundant operations.
    ///
    /// Requires the `cache` feature to be enabled.
    ///
    /// # Example
    ///
    /// ```no_run,ignore
    /// use zlayer_builder::ImageBuilder;
    ///
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .with_memory_cache()
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Integration Status
    ///
    /// TODO: The cache backend is currently stored but not actively used
    /// during builds. See `with_cache_dir` for integration status details.
    #[cfg(feature = "cache")]
    pub fn with_memory_cache(mut self) -> Self {
        self.options.cache_backend_config = Some(CacheBackendConfig::Memory);
        debug!("Configured in-memory cache");
        self
    }

    /// Configure an S3-compatible storage backend for layer caching.
    ///
    /// This is useful for distributed build systems where multiple build
    /// machines need to share a layer cache. Supports AWS S3, Cloudflare R2,
    /// Backblaze B2, MinIO, and other S3-compatible services.
    ///
    /// Requires the `cache-s3` feature to be enabled.
    ///
    /// # Arguments
    ///
    /// * `bucket` - S3 bucket name
    /// * `region` - AWS region (optional, uses SDK default if not set)
    ///
    /// # Example
    ///
    /// ```no_run,ignore
    /// use zlayer_builder::ImageBuilder;
    ///
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .with_s3_cache("my-build-cache", Some("us-west-2"))
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Integration Status
    ///
    /// TODO: The cache backend is currently stored but not actively used
    /// during builds. See `with_cache_dir` for integration status details.
    #[cfg(feature = "cache-s3")]
    pub fn with_s3_cache(mut self, bucket: impl Into<String>, region: Option<String>) -> Self {
        self.options.cache_backend_config = Some(CacheBackendConfig::S3 {
            bucket: bucket.into(),
            region,
            endpoint: None,
            prefix: None,
        });
        debug!("Configured S3 cache");
        self
    }

    /// Configure an S3-compatible storage backend with custom endpoint.
    ///
    /// Use this method for S3-compatible services that require a custom
    /// endpoint URL (e.g., Cloudflare R2, MinIO, local development).
    ///
    /// Requires the `cache-s3` feature to be enabled.
    ///
    /// # Arguments
    ///
    /// * `bucket` - S3 bucket name
    /// * `endpoint` - Custom endpoint URL
    /// * `region` - Region (required for some S3-compatible services)
    ///
    /// # Example
    ///
    /// ```no_run,ignore
    /// use zlayer_builder::ImageBuilder;
    ///
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// // Cloudflare R2
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .with_s3_cache_endpoint(
    ///         "my-bucket",
    ///         "https://accountid.r2.cloudflarestorage.com",
    ///         Some("auto".to_string()),
    ///     )
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "cache-s3")]
    pub fn with_s3_cache_endpoint(
        mut self,
        bucket: impl Into<String>,
        endpoint: impl Into<String>,
        region: Option<String>,
    ) -> Self {
        self.options.cache_backend_config = Some(CacheBackendConfig::S3 {
            bucket: bucket.into(),
            region,
            endpoint: Some(endpoint.into()),
            prefix: None,
        });
        debug!("Configured S3 cache with custom endpoint");
        self
    }

    /// Configure a custom cache backend configuration.
    ///
    /// This is the most flexible way to configure the cache backend,
    /// allowing full control over all cache settings.
    ///
    /// Requires the `cache` feature to be enabled.
    ///
    /// # Example
    ///
    /// ```no_run,ignore
    /// use zlayer_builder::{ImageBuilder, CacheBackendConfig};
    ///
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .with_cache_config(CacheBackendConfig::Memory)
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "cache")]
    pub fn with_cache_config(mut self, config: CacheBackendConfig) -> Self {
        self.options.cache_backend_config = Some(config);
        debug!("Configured custom cache backend");
        self
    }

    /// Set an already-initialized cache backend directly.
    ///
    /// This is useful when you have a pre-configured cache backend instance
    /// that you want to share across multiple builders or when you need
    /// fine-grained control over cache initialization.
    ///
    /// Requires the `cache` feature to be enabled.
    ///
    /// # Example
    ///
    /// ```no_run,ignore
    /// use zlayer_builder::ImageBuilder;
    /// use zlayer_registry::cache::BlobCache;
    /// use std::sync::Arc;
    ///
    /// # async fn example() -> Result<(), zlayer_builder::BuildError> {
    /// let cache = Arc::new(Box::new(BlobCache::new()?) as Box<dyn zlayer_registry::cache::BlobCacheBackend>);
    ///
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .with_cache_backend(cache)
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "cache")]
    pub fn with_cache_backend(mut self, backend: Arc<Box<dyn BlobCacheBackend>>) -> Self {
        self.cache_backend = Some(backend);
        debug!("Configured pre-initialized cache backend");
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
        let build_id = generate_build_id();

        info!(
            "Starting build in context: {} (build_id: {})",
            self.context.display(),
            build_id
        );

        // 1. Get parsed Dockerfile (from template, ZImagefile, or Dockerfile)
        let dockerfile = self.get_dockerfile().await?;
        debug!("Parsed Dockerfile with {} stages", dockerfile.stages.len());

        // 2. Determine stages to build
        let stages = self.resolve_stages(&dockerfile)?;
        debug!("Building {} stages", stages.len());

        // 4. Build each stage
        let mut stage_images: HashMap<String, String> = HashMap::new();
        // Track the final WORKDIR for each committed stage, used to resolve
        // relative source paths in COPY --from instructions.
        let mut stage_workdirs: HashMap<String, String> = HashMap::new();
        let mut final_container: Option<String> = None;
        let mut total_instructions = 0;

        // Initialize the layer cache tracker for this build session.
        // This tracks which instruction+base_layer combinations we've seen
        // and whether they were cache hits.
        let mut cache_tracker = LayerCacheTracker::new();

        for (stage_idx, stage) in stages.iter().enumerate() {
            let is_final_stage = stage_idx == stages.len() - 1;

            self.send_event(BuildEvent::StageStarted {
                index: stage_idx,
                name: stage.name.clone(),
                base_image: stage.base_image.to_string_ref(),
            });

            // Create container from base image
            let base = self
                .resolve_base_image(&stage.base_image, &stage_images)
                .await?;
            let container_id = self.create_container(&base).await?;

            debug!(
                "Created container {} for stage {} (base: {})",
                container_id,
                stage.identifier(),
                base
            );

            // Track the current base layer for cache key computation.
            // Each instruction modifies the container, so we update this after each instruction.
            let mut current_base_layer = container_id.clone();

            // Track the current WORKDIR for this stage. Used to resolve relative paths
            // when this stage is used as a source for COPY --from in a later stage.
            let mut current_workdir = String::from("/");

            // Execute instructions
            for (inst_idx, instruction) in stage.instructions.iter().enumerate() {
                self.send_event(BuildEvent::InstructionStarted {
                    stage: stage_idx,
                    index: inst_idx,
                    instruction: format!("{:?}", instruction),
                });

                // Generate the cache key for this instruction
                let instruction_cache_key = instruction.cache_key();

                // Track instruction start time for potential cache hit heuristics
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
                            // If the source stage had `workdir: "/build"` and the copy source
                            // is `"app"`, we need to resolve it to `"/build/app"`.
                            if let Some(source_workdir) = stage_workdirs.get(from) {
                                resolved_copy.sources = resolved_copy
                                    .sources
                                    .iter()
                                    .map(|src| {
                                        if src.starts_with('/') {
                                            // Absolute path - use as-is
                                            src.clone()
                                        } else {
                                            // Relative path - prepend source stage's workdir
                                            if source_workdir == "/" {
                                                format!("/{}", src)
                                            } else {
                                                format!("{}/{}", source_workdir, src)
                                            }
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

                let commands = BuildahCommand::from_instruction(&container_id, instruction_ref);

                let mut combined_output = String::new();
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

                    combined_output.push_str(&output.stdout);
                    combined_output.push_str(&output.stderr);

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

                let instruction_elapsed_ms = instruction_start.elapsed().as_millis() as u64;

                // Track WORKDIR changes for later COPY --from resolution.
                // We need to know the final WORKDIR of each stage so we can resolve
                // relative paths when copying from that stage.
                if let Instruction::Workdir(dir) = instruction {
                    current_workdir = dir.clone();
                }

                // Attempt to detect if this was a cache hit.
                // TODO: Implement proper cache detection. Currently always returns false.
                // Possible approaches:
                // - Time-based: Cached layers typically execute in < 100ms
                // - Output-based: Look for cache indicators in buildah output
                // - Digest-based: Pre-compute expected digest and check storage
                let cached = cache_tracker.detect_cache_hit(
                    instruction,
                    instruction_elapsed_ms,
                    &combined_output,
                );

                // Record this instruction execution for future reference
                cache_tracker.record(
                    instruction_cache_key.clone(),
                    current_base_layer.clone(),
                    cached,
                );

                // Update the base layer identifier for the next instruction.
                // In a proper implementation, this would be the new layer digest
                // after the instruction was committed. For now, we use a composite
                // of the previous base and the instruction key.
                current_base_layer = format!("{}:{}", current_base_layer, instruction_cache_key);

                self.send_event(BuildEvent::InstructionComplete {
                    stage: stage_idx,
                    index: inst_idx,
                    cached,
                });

                total_instructions += 1;
            }

            // Handle stage completion
            if let Some(name) = &stage.name {
                // Named stage - commit and save for COPY --from
                // Include the build_id to prevent collisions when parallel
                // builds share stage names (e.g., two Dockerfiles both having
                // a stage named "builder").
                let image_name = format!("zlayer-build-{}-stage-{}", build_id, name);
                self.commit_container(&container_id, &image_name, false)
                    .await?;
                stage_images.insert(name.clone(), image_name.clone());

                // Store the final WORKDIR for this stage so COPY --from can resolve
                // relative paths correctly.
                stage_workdirs.insert(name.clone(), current_workdir.clone());

                // Also add by index
                stage_images.insert(stage.index.to_string(), image_name.clone());
                stage_workdirs.insert(stage.index.to_string(), current_workdir.clone());

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
                let image_name = format!("zlayer-build-{}-stage-{}", build_id, stage.index);
                self.commit_container(&container_id, &image_name, false)
                    .await?;
                stage_images.insert(stage.index.to_string(), image_name);
                // Store the final WORKDIR for this stage
                stage_workdirs.insert(stage.index.to_string(), current_workdir.clone());
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

    /// Get a parsed [`Dockerfile`] from the configured source.
    ///
    /// Detection order:
    /// 1. If `runtime` is set → use template string → parse as Dockerfile
    /// 2. If `zimagefile` is explicitly set → read & parse ZImagefile → convert
    /// 3. If a file called `ZImagefile` exists in the context dir → same as (2)
    /// 4. Fall back to reading a Dockerfile (from `dockerfile` option or default)
    async fn get_dockerfile(&self) -> Result<Dockerfile> {
        // (a) Runtime template takes highest priority.
        if let Some(runtime) = &self.options.runtime {
            debug!("Using runtime template: {}", runtime);
            let content = get_template(*runtime);
            return Dockerfile::parse(content);
        }

        // (b) Explicit ZImagefile path.
        if let Some(ref zimage_path) = self.options.zimagefile {
            debug!("Reading ZImagefile: {}", zimage_path.display());
            let content =
                fs::read_to_string(zimage_path)
                    .await
                    .map_err(|e| BuildError::ContextRead {
                        path: zimage_path.clone(),
                        source: e,
                    })?;
            let zimage = crate::zimage::parse_zimagefile(&content)?;
            return self.handle_zimage(&zimage).await;
        }

        // (c) Auto-detect ZImagefile in context directory.
        let auto_zimage_path = self.context.join("ZImagefile");
        if auto_zimage_path.exists() {
            debug!(
                "Found ZImagefile in context: {}",
                auto_zimage_path.display()
            );
            let content = fs::read_to_string(&auto_zimage_path).await.map_err(|e| {
                BuildError::ContextRead {
                    path: auto_zimage_path,
                    source: e,
                }
            })?;
            let zimage = crate::zimage::parse_zimagefile(&content)?;
            return self.handle_zimage(&zimage).await;
        }

        // (d) Fall back to Dockerfile.
        let dockerfile_path = self
            .options
            .dockerfile
            .clone()
            .unwrap_or_else(|| self.context.join("Dockerfile"));

        debug!("Reading Dockerfile: {}", dockerfile_path.display());

        let content =
            fs::read_to_string(&dockerfile_path)
                .await
                .map_err(|e| BuildError::ContextRead {
                    path: dockerfile_path,
                    source: e,
                })?;

        Dockerfile::parse(&content)
    }

    /// Convert a parsed [`ZImage`] into the internal [`Dockerfile`] IR.
    ///
    /// Handles the three ZImage modes that can produce a Dockerfile:
    /// - **Runtime** mode: delegates to the template system
    /// - **Single-stage / Multi-stage**: converts via [`zimage_to_dockerfile`]
    /// - **WASM** mode: errors out (WASM uses `zlayer wasm build`, not `zlayer build`)
    ///
    /// Any `build:` directives are resolved first by spawning nested builds.
    async fn handle_zimage(&self, zimage: &crate::zimage::ZImage) -> Result<Dockerfile> {
        // Runtime mode: delegate to template system.
        if let Some(ref runtime_name) = zimage.runtime {
            let rt = Runtime::from_name(runtime_name).ok_or_else(|| {
                BuildError::zimagefile_validation(format!(
                    "unknown runtime '{runtime_name}' in ZImagefile"
                ))
            })?;
            let content = get_template(rt);
            return Dockerfile::parse(content);
        }

        // WASM mode: not supported through `zlayer build`.
        if zimage.wasm.is_some() {
            return Err(BuildError::invalid_instruction(
                "ZImagefile",
                "WASM builds use `zlayer wasm build`, not `zlayer build`",
            ));
        }

        // Resolve any `build:` directives to concrete base image tags.
        let resolved = self.resolve_build_directives(zimage).await?;

        // Single-stage or multi-stage: convert to Dockerfile IR directly.
        crate::zimage::zimage_to_dockerfile(&resolved)
    }

    /// Resolve `build:` directives in a ZImage by running nested builds.
    ///
    /// For each `build:` directive (top-level or per-stage), this method:
    /// 1. Determines the build context directory
    /// 2. Auto-detects the build file (ZImagefile > Dockerfile) unless specified
    /// 3. Spawns a nested `ImageBuilder` to build the context
    /// 4. Tags the result and replaces `build` with `base`
    async fn resolve_build_directives(
        &self,
        zimage: &crate::zimage::ZImage,
    ) -> Result<crate::zimage::ZImage> {
        let mut resolved = zimage.clone();

        // Resolve top-level `build:` directive.
        if let Some(ref build_ctx) = resolved.build {
            let tag = self.run_nested_build(build_ctx, "toplevel").await?;
            resolved.base = Some(tag);
            resolved.build = None;
        }

        // Resolve per-stage `build:` directives.
        if let Some(ref mut stages) = resolved.stages {
            for (name, stage) in stages.iter_mut() {
                if let Some(ref build_ctx) = stage.build {
                    let tag = self.run_nested_build(build_ctx, name).await?;
                    stage.base = Some(tag);
                    stage.build = None;
                }
            }
        }

        Ok(resolved)
    }

    /// Run a nested build from a `build:` directive and return the resulting image tag.
    fn run_nested_build<'a>(
        &'a self,
        build_ctx: &'a crate::zimage::types::ZBuildContext,
        stage_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(self.run_nested_build_inner(build_ctx, stage_name))
    }

    async fn run_nested_build_inner(
        &self,
        build_ctx: &crate::zimage::types::ZBuildContext,
        stage_name: &str,
    ) -> Result<String> {
        let context_dir = build_ctx.context_dir(&self.context);

        if !context_dir.exists() {
            return Err(BuildError::ContextRead {
                path: context_dir,
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "build context directory not found for build directive in '{stage_name}'"
                    ),
                ),
            });
        }

        info!(
            "Building nested image for '{}' from context: {}",
            stage_name,
            context_dir.display()
        );

        // Create a tag for the nested build result.
        let tag = format!(
            "zlayer-build-dep-{}:{}",
            stage_name,
            chrono_lite_timestamp()
        );

        // Create nested builder.
        let mut nested = ImageBuilder::new(&context_dir).await?;
        nested = nested.tag(&tag);

        // Apply explicit build file if specified.
        if let Some(file) = build_ctx.file() {
            let file_path = context_dir.join(file);
            if file.ends_with(".yml") || file.ends_with(".yaml") || file.starts_with("ZImagefile") {
                nested = nested.zimagefile(file_path);
            } else {
                nested = nested.dockerfile(file_path);
            }
        }

        // Apply build args.
        for (key, value) in build_ctx.args() {
            nested = nested.build_arg(&key, &value);
        }

        // Propagate default registry if set.
        if let Some(ref reg) = self.options.default_registry {
            nested = nested.default_registry(reg.clone());
        }

        // Run the nested build.
        let result = nested.build().await?;
        info!(
            "Nested build for '{}' completed: {}",
            stage_name, result.image_id
        );

        Ok(tag)
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

    /// Resolve a base image reference to an actual image name.
    ///
    /// Resolution chain for short (unqualified) image names:
    /// 1. Check `LocalRegistry` for a cached copy (if configured)
    /// 2. Check `default_registry` for the image (if configured)
    /// 3. Fall back to Docker Hub qualification (`docker.io/library/...`)
    ///
    /// Already-qualified names (containing a registry hostname) skip this chain.
    async fn resolve_base_image(
        &self,
        image_ref: &ImageRef,
        stage_images: &HashMap<String, String>,
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

        // For unqualified names, try local registry and default registry first.
        if !is_qualified {
            if let Some(resolved) = self.try_resolve_from_sources(image_ref).await {
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

    /// Try to resolve an unqualified image from local registry or default registry.
    ///
    /// Returns `Some(fully_qualified_name)` if found, `None` to fall back to docker.io.
    async fn try_resolve_from_sources(&self, image_ref: &ImageRef) -> Option<String> {
        let (name, tag_str) = match image_ref {
            ImageRef::Registry { image, tag, .. } => {
                (image.as_str(), tag.as_deref().unwrap_or("latest"))
            }
            _ => return None,
        };

        // 1. Check local OCI registry
        #[cfg(feature = "local-registry")]
        if let Some(ref local_reg) = self.local_registry {
            if local_reg.has_manifest(name, tag_str).await {
                info!(
                    "Found {}:{} in local registry, using local copy",
                    name, tag_str
                );
                // Build an OCI reference pointing to the local registry path.
                // buildah can pull from an OCI layout directory.
                let oci_path = format!("oci:{}:{}", local_reg.root().display(), tag_str);
                return Some(oci_path);
            }
        }

        // 2. Check configured default registry
        if let Some(ref registry) = self.options.default_registry {
            let qualified = format!("{}/{}:{}", registry, name, tag_str);
            debug!("Checking default registry for image: {}", qualified);
            // Return the qualified name for the configured registry.
            // buildah will attempt to pull from this registry; if it fails,
            // the build will error (the user explicitly configured this registry).
            return Some(qualified);
        }

        None
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

/// Generate a short unique build ID for namespacing intermediate stage images.
///
/// This prevents parallel builds from clobbering each other's intermediate
/// stage images when they share stage names (e.g., two Dockerfiles both have
/// a stage named "builder").
///
/// The ID combines nanosecond-precision timestamp with the process ID, then
/// takes 12 hex characters from a SHA-256 hash for a compact, collision-resistant
/// identifier.
fn generate_build_id() -> String {
    use sha2::{Digest, Sha256};
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    // Use a monotonic counter to guarantee uniqueness even within the same
    // nanosecond on the same process (e.g. tests or very fast sequential calls).
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(pid.to_le_bytes());
    hasher.update(count.to_le_bytes());
    let hash = hasher.finalize();
    // 12 hex chars = 6 bytes = 48 bits of entropy, ample for build parallelism
    hex::encode(&hash[..6])
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
        assert!(opts.zimagefile.is_none());
        assert!(opts.runtime.is_none());
        assert!(opts.build_args.is_empty());
        assert!(opts.target.is_none());
        assert!(opts.tags.is_empty());
        assert!(!opts.no_cache);
        assert!(!opts.push);
        assert!(!opts.squash);
        // New cache-related fields
        assert!(opts.layers); // Default is true
        assert!(opts.cache_from.is_none());
        assert!(opts.cache_to.is_none());
        assert!(opts.cache_ttl.is_none());
        // Cache backend config (only with cache feature)
        #[cfg(feature = "cache")]
        assert!(opts.cache_backend_config.is_none());
    }

    #[tokio::test]
    async fn test_resolve_base_image_registry() {
        let builder = create_test_builder();
        let stage_images = HashMap::new();

        // Simple image (qualified to docker.io)
        let image_ref = ImageRef::Registry {
            image: "alpine".to_string(),
            tag: Some("3.18".to_string()),
            digest: None,
        };
        let result = builder.resolve_base_image(&image_ref, &stage_images).await;
        assert_eq!(result.unwrap(), "docker.io/library/alpine:3.18");

        // Image with digest (qualified to docker.io)
        let image_ref = ImageRef::Registry {
            image: "alpine".to_string(),
            tag: None,
            digest: Some("sha256:abc123".to_string()),
        };
        let result = builder.resolve_base_image(&image_ref, &stage_images).await;
        assert_eq!(result.unwrap(), "docker.io/library/alpine@sha256:abc123");

        // Image with no tag or digest (qualified to docker.io + :latest)
        let image_ref = ImageRef::Registry {
            image: "alpine".to_string(),
            tag: None,
            digest: None,
        };
        let result = builder.resolve_base_image(&image_ref, &stage_images).await;
        assert_eq!(result.unwrap(), "docker.io/library/alpine:latest");

        // Already-qualified image (unchanged)
        let image_ref = ImageRef::Registry {
            image: "ghcr.io/org/myimage".to_string(),
            tag: Some("v1".to_string()),
            digest: None,
        };
        let result = builder.resolve_base_image(&image_ref, &stage_images).await;
        assert_eq!(result.unwrap(), "ghcr.io/org/myimage:v1");
    }

    #[tokio::test]
    async fn test_resolve_base_image_stage() {
        let builder = create_test_builder();
        let mut stage_images = HashMap::new();
        stage_images.insert(
            "builder".to_string(),
            "zlayer-build-stage-builder".to_string(),
        );

        let image_ref = ImageRef::Stage("builder".to_string());
        let result = builder.resolve_base_image(&image_ref, &stage_images).await;
        assert_eq!(result.unwrap(), "zlayer-build-stage-builder");

        // Missing stage
        let image_ref = ImageRef::Stage("missing".to_string());
        let result = builder.resolve_base_image(&image_ref, &stage_images).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resolve_base_image_scratch() {
        let builder = create_test_builder();
        let stage_images = HashMap::new();

        let image_ref = ImageRef::Scratch;
        let result = builder.resolve_base_image(&image_ref, &stage_images).await;
        assert_eq!(result.unwrap(), "scratch");
    }

    #[tokio::test]
    async fn test_resolve_base_image_with_default_registry() {
        let mut builder = create_test_builder();
        builder.options.default_registry = Some("git.example.com:5000".to_string());
        let stage_images = HashMap::new();

        // Unqualified image should resolve to default registry
        let image_ref = ImageRef::Registry {
            image: "myapp".to_string(),
            tag: Some("v1".to_string()),
            digest: None,
        };
        let result = builder.resolve_base_image(&image_ref, &stage_images).await;
        assert_eq!(result.unwrap(), "git.example.com:5000/myapp:v1");

        // Already-qualified image should NOT use default registry
        let image_ref = ImageRef::Registry {
            image: "ghcr.io/org/image".to_string(),
            tag: Some("latest".to_string()),
            digest: None,
        };
        let result = builder.resolve_base_image(&image_ref, &stage_images).await;
        assert_eq!(result.unwrap(), "ghcr.io/org/image:latest");
    }

    fn create_test_builder() -> ImageBuilder {
        // Create a minimal builder for testing (without async initialization)
        ImageBuilder {
            context: PathBuf::from("/tmp/test"),
            options: BuildOptions::default(),
            executor: BuildahExecutor::with_path("/usr/bin/buildah"),
            event_tx: None,
            #[cfg(feature = "cache")]
            cache_backend: None,
            #[cfg(feature = "local-registry")]
            local_registry: None,
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
    fn test_builder_layers() {
        let mut builder = create_test_builder();
        // Default is true
        assert!(builder.options.layers);

        // Disable layers
        builder = builder.layers(false);
        assert!(!builder.options.layers);

        // Re-enable layers
        builder = builder.layers(true);
        assert!(builder.options.layers);
    }

    #[test]
    fn test_builder_cache_from() {
        let mut builder = create_test_builder();
        assert!(builder.options.cache_from.is_none());

        builder = builder.cache_from("registry.example.com/myapp:cache");
        assert_eq!(
            builder.options.cache_from,
            Some("registry.example.com/myapp:cache".to_string())
        );
    }

    #[test]
    fn test_builder_cache_to() {
        let mut builder = create_test_builder();
        assert!(builder.options.cache_to.is_none());

        builder = builder.cache_to("registry.example.com/myapp:cache");
        assert_eq!(
            builder.options.cache_to,
            Some("registry.example.com/myapp:cache".to_string())
        );
    }

    #[test]
    fn test_builder_cache_ttl() {
        use std::time::Duration;

        let mut builder = create_test_builder();
        assert!(builder.options.cache_ttl.is_none());

        builder = builder.cache_ttl(Duration::from_secs(3600));
        assert_eq!(builder.options.cache_ttl, Some(Duration::from_secs(3600)));
    }

    #[test]
    fn test_builder_cache_options_chaining() {
        use std::time::Duration;

        let builder = create_test_builder()
            .layers(true)
            .cache_from("registry.example.com/cache:input")
            .cache_to("registry.example.com/cache:output")
            .cache_ttl(Duration::from_secs(7200))
            .no_cache();

        assert!(builder.options.layers);
        assert_eq!(
            builder.options.cache_from,
            Some("registry.example.com/cache:input".to_string())
        );
        assert_eq!(
            builder.options.cache_to,
            Some("registry.example.com/cache:output".to_string())
        );
        assert_eq!(builder.options.cache_ttl, Some(Duration::from_secs(7200)));
        assert!(builder.options.no_cache);
    }

    #[test]
    fn test_chrono_lite_timestamp() {
        let ts = chrono_lite_timestamp();
        // Should be a valid number
        let parsed: u64 = ts.parse().expect("Should be a valid u64");
        // Should be reasonably recent (after 2024)
        assert!(parsed > 1700000000);
    }

    // LayerCacheTracker tests
    #[test]
    fn test_layer_cache_tracker_new() {
        let tracker = LayerCacheTracker::new();
        assert!(tracker.known_layers.is_empty());
    }

    #[test]
    fn test_layer_cache_tracker_record_and_lookup() {
        let mut tracker = LayerCacheTracker::new();

        // Record a cache miss
        tracker.record("abc123".to_string(), "container-1".to_string(), false);

        // Check that we can look it up
        assert!(!tracker.is_cached("abc123", "container-1"));

        // Record a cache hit
        tracker.record("def456".to_string(), "container-2".to_string(), true);

        assert!(tracker.is_cached("def456", "container-2"));
    }

    #[test]
    fn test_layer_cache_tracker_unknown_returns_false() {
        let tracker = LayerCacheTracker::new();

        // Unknown entries should return false
        assert!(!tracker.is_cached("unknown", "unknown"));
    }

    #[test]
    fn test_layer_cache_tracker_different_base_layers() {
        let mut tracker = LayerCacheTracker::new();

        // Same instruction key but different base layers
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

        // Currently always returns false - this test documents the expected behavior
        // and will need to be updated when cache detection is implemented
        assert!(!tracker.detect_cache_hit(&instruction, 50, ""));
        assert!(!tracker.detect_cache_hit(&instruction, 1000, ""));
        assert!(!tracker.detect_cache_hit(&instruction, 50, "Using cache"));
    }

    #[test]
    fn test_layer_cache_tracker_overwrite() {
        let mut tracker = LayerCacheTracker::new();

        // Record as cache miss first
        tracker.record("key".to_string(), "base".to_string(), false);
        assert!(!tracker.is_cached("key", "base"));

        // Overwrite with cache hit
        tracker.record("key".to_string(), "base".to_string(), true);
        assert!(tracker.is_cached("key", "base"));
    }
}
