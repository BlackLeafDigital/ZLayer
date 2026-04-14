//! `ImageBuilder` - High-level API for building container images
//!
//! This module provides the [`ImageBuilder`] type which orchestrates the full
//! container image build process, from Dockerfile parsing through buildah
//! execution to final image creation.
//!
//! # Example
//!
//! ```no_run
//! use zlayer_builder_zql::{ImageBuilder, Runtime};
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
//! use zlayer_builder_zql::{ImageBuilder, Runtime};
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
//! use zlayer_builder_zql::ImageBuilder;
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
//! use zlayer_builder_zql::{ImageBuilder, BuildEvent};
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
//! use zlayer_builder_zql::ImageBuilder;
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
use std::sync::Arc;

use tokio::fs;
use tracing::{debug, info, instrument, warn};

use crate::backend::BuildBackend;
use crate::buildah::{BuildahCommand, BuildahExecutor};
use crate::dockerfile::{Dockerfile, RunMount};
use crate::error::{BuildError, Result};
use crate::templates::{get_template, Runtime};
use crate::tui::BuildEvent;

#[cfg(feature = "cache")]
use zlayer_registry::cache::BlobCacheBackend;

#[cfg(feature = "local-registry")]
use zlayer_registry::LocalRegistry;

#[cfg(feature = "local-registry")]
use zlayer_registry::import_image;

/// Output from parsing a `ZImagefile` - either a Dockerfile for container builds
/// or a WASM build result for WebAssembly builds.
///
/// Most `ZImagefile` modes (runtime, single-stage, multi-stage) produce a
/// [`Dockerfile`] IR that is then built with buildah. WASM mode produces
/// a compiled artifact directly, bypassing the container build pipeline.
#[derive(Debug)]
pub enum BuildOutput {
    /// Standard container build - produces a Dockerfile to be built with buildah.
    Dockerfile(Dockerfile),
    /// WASM component build - already built, produces artifact path.
    WasmArtifact {
        /// Path to the compiled WASM binary.
        wasm_path: PathBuf,
        /// Path to the OCI artifact directory (if exported).
        oci_path: Option<PathBuf>,
        /// Source language used.
        language: String,
        /// Whether optimization was applied.
        optimized: bool,
        /// Size of the output file in bytes.
        size: u64,
    },
}

/// Configuration for the layer cache backend.
///
/// This enum specifies which cache backend to use for storing and retrieving
/// cached layers during builds. The cache feature must be enabled for this
/// to be available.
///
/// # Example
///
/// ```no_run,ignore
/// use zlayer_builder_zql::{ImageBuilder, CacheBackendConfig};
///
/// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
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
        /// Custom endpoint URL (for S3-compatible services like R2, B2, `MinIO`)
        endpoint: Option<String>,
        /// Key prefix for cached blobs (default: "zlayer/layers/")
        prefix: Option<String>,
    },
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
    /// Whether this image is a manifest list (multi-arch).
    pub is_manifest: bool,
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

/// Strategy for pulling the base image before building.
///
/// Controls the `--pull` flag passed to `buildah from`. The default is
/// [`PullBaseMode::Newer`], matching the behaviour users expect from
/// modern build tools: fast when nothing has changed, correct when the
/// upstream base image has been republished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PullBaseMode {
    /// Pull only if the registry has a newer version (`--pull=newer`).
    /// Default behaviour.
    #[default]
    Newer,
    /// Always pull, even if a local copy exists (`--pull=always`).
    Always,
    /// Never pull — use whatever is in local storage (no `--pull` flag passed).
    Never,
}

/// Build options for customizing the image build process
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct BuildOptions {
    /// Dockerfile path (default: Dockerfile in context)
    pub dockerfile: Option<PathBuf>,
    /// `ZImagefile` path (alternative to Dockerfile)
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
    /// Note: `ZLayer` uses manual container creation (`buildah from`, `buildah run`,
    /// `buildah commit`) rather than `buildah build`, so this flag is reserved
    /// for future use when/if we switch to `buildah build` (bud) command.
    pub layers: bool,
    /// Registry to pull cache from (--cache-from for `buildah build`).
    ///
    /// Note: This would be used with `buildah build --cache-from=<registry>`.
    /// Currently `ZLayer` uses manual container creation, so this is reserved
    /// for future implementation or for switching to `buildah build`.
    ///
    /// TODO: Implement remote cache support. This would require either:
    /// 1. Switching to `buildah build` command which supports --cache-from natively
    /// 2. Implementing custom layer caching with registry push/pull for intermediate layers
    pub cache_from: Option<String>,
    /// Registry to push cache to (--cache-to for `buildah build`).
    ///
    /// Note: This would be used with `buildah build --cache-to=<registry>`.
    /// Currently `ZLayer` uses manual container creation, so this is reserved
    /// for future implementation or for switching to `buildah build`.
    ///
    /// TODO: Implement remote cache support. This would require either:
    /// 1. Switching to `buildah build` command which supports --cache-to natively
    /// 2. Implementing custom layer caching with registry push/pull for intermediate layers
    pub cache_to: Option<String>,
    /// Maximum cache age (--cache-ttl for `buildah build`).
    ///
    /// Note: This would be used with `buildah build --cache-ttl=<duration>`.
    /// Currently `ZLayer` uses manual container creation, so this is reserved
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
    /// buildah's native caching and operates at the `ZLayer` level.
    ///
    /// # Integration Points
    ///
    /// The cache backend is used at several points during the build:
    ///
    /// 1. **Before instruction execution**: Check if a cached layer exists
    ///    for the (`instruction_hash`, `base_layer`) tuple
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
    /// `"git.example.com:5000"` and the `ZImagefile` uses `base: "myapp:latest"`,
    /// the builder will check `git.example.com:5000/myapp:latest` first.
    pub default_registry: Option<String>,
    /// Default cache mounts injected into all RUN instructions.
    /// These are merged with any step-level cache mounts (deduped by target path).
    pub default_cache_mounts: Vec<RunMount>,
    /// Number of retries for failed RUN steps (0 = no retries, default)
    pub retries: u32,
    /// Target platform for the build (e.g., "linux/amd64", "linux/arm64").
    /// When set, `buildah from` pulls the platform-specific image variant.
    pub platform: Option<String>,
    /// Pre-computed source hash for content-based cache invalidation.
    ///
    /// When set, the sandbox builder uses this hash directly instead of
    /// computing one from the parsed Dockerfile.
    pub source_hash: Option<String>,
    /// How to handle base-image pulling during `buildah from`.
    ///
    /// Default: [`PullBaseMode::Newer`] — only pull if the registry has a
    /// newer version. Set to [`PullBaseMode::Always`] for CI builds that
    /// must always refresh, or [`PullBaseMode::Never`] for offline builds.
    pub pull: PullBaseMode,
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
            default_cache_mounts: Vec::new(),
            retries: 0,
            platform: None,
            source_hash: None,
            pull: PullBaseMode::default(),
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
/// use zlayer_builder_zql::ImageBuilder;
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
    /// Buildah executor (kept for backwards compatibility)
    #[allow(dead_code)]
    executor: BuildahExecutor,
    /// Event sender for TUI updates
    event_tx: Option<mpsc::Sender<BuildEvent>>,
    /// Pluggable build backend (buildah, sandbox, etc.).
    ///
    /// When set, the `build()` method delegates to this backend instead of
    /// using the inline buildah logic. Set automatically by `new()` via
    /// `detect_backend()`, or explicitly via `with_backend()`.
    backend: Option<Arc<dyn BuildBackend>>,
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
    /// Create a new `ImageBuilder` with the given context directory
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
    /// use zlayer_builder_zql::ImageBuilder;
    ///
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
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

        // Detect the best available build backend for this platform.
        let backend = crate::backend::detect_backend().await.ok();

        // Initialize buildah executor.
        // On macOS, if buildah is not found we fall back to a default executor
        // (the backend will handle the actual build dispatch).
        let executor = match BuildahExecutor::new_async().await {
            Ok(exec) => exec,
            #[cfg(target_os = "macos")]
            Err(_) => {
                info!("Buildah not found on macOS; backend will handle build dispatch");
                BuildahExecutor::default()
            }
            #[cfg(not(target_os = "macos"))]
            Err(e) => return Err(e),
        };

        debug!("Created ImageBuilder for context: {}", context.display());

        Ok(Self {
            context,
            options: BuildOptions::default(),
            executor,
            event_tx: None,
            backend,
            #[cfg(feature = "cache")]
            cache_backend: None,
            #[cfg(feature = "local-registry")]
            local_registry: None,
        })
    }

    /// Create an `ImageBuilder` with a custom buildah executor
    ///
    /// This is useful for testing or when you need to configure
    /// the executor with specific storage options. The executor is
    /// wrapped in a [`BuildahBackend`] so the build dispatches through
    /// the [`BuildBackend`] trait.
    ///
    /// # Errors
    ///
    /// Returns an error if the context directory does not exist.
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

        let backend: Arc<dyn BuildBackend> = Arc::new(
            crate::backend::BuildahBackend::with_executor(executor.clone()),
        );

        Ok(Self {
            context,
            options: BuildOptions::default(),
            executor,
            event_tx: None,
            backend: Some(backend),
            #[cfg(feature = "cache")]
            cache_backend: None,
            #[cfg(feature = "local-registry")]
            local_registry: None,
        })
    }

    /// Create an `ImageBuilder` with an explicit [`BuildBackend`].
    ///
    /// The backend is used for all build, push, tag, and manifest
    /// operations. The internal `BuildahExecutor` is set to the default
    /// (it is only used if no backend is set).
    ///
    /// # Errors
    ///
    /// Returns an error if the context directory does not exist.
    pub fn with_backend(context: impl AsRef<Path>, backend: Arc<dyn BuildBackend>) -> Result<Self> {
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
            executor: BuildahExecutor::default(),
            event_tx: None,
            backend: Some(backend),
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
    /// # use zlayer_builder_zql::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .dockerfile("./my-project/Dockerfile.prod");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn dockerfile(mut self, path: impl AsRef<Path>) -> Self {
        self.options.dockerfile = Some(path.as_ref().to_path_buf());
        self
    }

    /// Set a custom `ZImagefile` path
    ///
    /// `ZImagefiles` are a YAML-based alternative to Dockerfiles. When set,
    /// the builder will parse the `ZImagefile` and convert it to the internal
    /// Dockerfile IR for execution.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use zlayer_builder_zql::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .zimagefile("./my-project/ZImagefile");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
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
    /// use zlayer_builder_zql::{ImageBuilder, Runtime};
    ///
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let builder = ImageBuilder::new("./my-node-app").await?
    ///     .runtime(Runtime::Node20);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
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
    /// # use zlayer_builder_zql::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .build_arg("VERSION", "1.0.0")
    ///     .build_arg("DEBUG", "false");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn build_arg(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.build_args.insert(key.into(), value.into());
        self
    }

    /// Set multiple build arguments at once
    #[must_use]
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
    /// # use zlayer_builder_zql::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
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
    #[must_use]
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
    /// # use zlayer_builder_zql::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .tag("myapp:latest")
    ///     .tag("myapp:v1.0.0")
    ///     .tag("registry.example.com/myapp:v1.0.0");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
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
    /// build process. `ZLayer` uses manual container creation (`buildah from`,
    /// `buildah run`, `buildah commit`) which doesn't have built-in caching
    /// like `buildah build` does. Future work could implement layer-level
    /// caching by checking instruction hashes against previously built layers.
    #[must_use]
    pub fn no_cache(mut self) -> Self {
        self.options.no_cache = true;
        self
    }

    /// Set the base-image pull strategy for the build.
    ///
    /// By default, `buildah from` is invoked with `--pull=newer`, so an
    /// up-to-date local base image is reused but a newer one on the
    /// registry will be fetched. Pass [`PullBaseMode::Always`] to force a
    /// fresh pull on every build, or [`PullBaseMode::Never`] to stay fully
    /// offline.
    #[must_use]
    pub fn pull(mut self, mode: PullBaseMode) -> Self {
        self.options.pull = mode;
        self
    }

    /// Enable or disable layer caching
    ///
    /// This controls the `--layers` flag for buildah. When enabled (default),
    /// buildah can cache and reuse intermediate layers.
    ///
    /// Note: `ZLayer` currently uses manual container creation (`buildah from`,
    /// `buildah run`, `buildah commit`) rather than `buildah build`, so this
    /// flag is reserved for future use when/if we switch to `buildah build`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use zlayer_builder_zql::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .layers(false)  // Disable layer caching
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn layers(mut self, enable: bool) -> Self {
        self.options.layers = enable;
        self
    }

    /// Set registry to pull cache from
    ///
    /// This corresponds to buildah's `--cache-from` flag, which allows
    /// pulling cached layers from a remote registry to speed up builds.
    ///
    /// Note: `ZLayer` currently uses manual container creation (`buildah from`,
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
    /// # use zlayer_builder_zql::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .cache_from("registry.example.com/myapp:cache")
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn cache_from(mut self, registry: impl Into<String>) -> Self {
        self.options.cache_from = Some(registry.into());
        self
    }

    /// Set registry to push cache to
    ///
    /// This corresponds to buildah's `--cache-to` flag, which allows
    /// pushing cached layers to a remote registry for future builds to use.
    ///
    /// Note: `ZLayer` currently uses manual container creation (`buildah from`,
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
    /// # use zlayer_builder_zql::ImageBuilder;
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .cache_to("registry.example.com/myapp:cache")
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn cache_to(mut self, registry: impl Into<String>) -> Self {
        self.options.cache_to = Some(registry.into());
        self
    }

    /// Set maximum cache age
    ///
    /// This corresponds to buildah's `--cache-ttl` flag, which sets the
    /// maximum age for cached layers before they are considered stale.
    ///
    /// Note: `ZLayer` currently uses manual container creation (`buildah from`,
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
    /// # use zlayer_builder_zql::ImageBuilder;
    /// # use std::time::Duration;
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .cache_ttl(Duration::from_secs(3600 * 24))  // 24 hours
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
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
    /// use zlayer_builder_zql::{ImageBuilder, RegistryAuth};
    ///
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .tag("registry.example.com/myapp:v1.0.0")
    ///     .push(RegistryAuth::new("user", "password"));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn push(mut self, auth: RegistryAuth) -> Self {
        self.options.push = true;
        self.options.registry_auth = Some(auth);
        self
    }

    /// Enable pushing without authentication
    ///
    /// Use this for registries that don't require authentication
    /// (e.g., local registries, insecure registries).
    #[must_use]
    pub fn push_without_auth(mut self) -> Self {
        self.options.push = true;
        self.options.registry_auth = None;
        self
    }

    /// Set a default OCI/WASM-compatible registry to check for images.
    ///
    /// When set, the builder will probe this registry for short image names
    /// before qualifying them to `docker.io`. For example, if set to
    /// `"git.example.com:5000"` and the `ZImagefile` uses `base: "myapp:latest"`,
    /// the builder will check `git.example.com:5000/myapp:latest` first.
    #[must_use]
    pub fn default_registry(mut self, registry: impl Into<String>) -> Self {
        self.options.default_registry = Some(registry.into());
        self
    }

    /// Set a local OCI registry for image resolution.
    ///
    /// When set, the builder checks the local registry for cached images
    /// before pulling from remote registries.
    #[cfg(feature = "local-registry")]
    #[must_use]
    pub fn with_local_registry(mut self, registry: LocalRegistry) -> Self {
        self.local_registry = Some(registry);
        self
    }

    /// Squash all layers into a single layer
    ///
    /// This reduces image size but loses layer caching benefits.
    #[must_use]
    pub fn squash(mut self) -> Self {
        self.options.squash = true;
        self
    }

    /// Set the image format
    ///
    /// Valid values are "oci" (default) or "docker".
    #[must_use]
    pub fn format(mut self, format: impl Into<String>) -> Self {
        self.options.format = Some(format.into());
        self
    }

    /// Set default cache mounts to inject into all RUN instructions
    #[must_use]
    pub fn default_cache_mounts(mut self, mounts: Vec<RunMount>) -> Self {
        self.options.default_cache_mounts = mounts;
        self
    }

    /// Set the number of retries for failed RUN steps
    #[must_use]
    pub fn retries(mut self, retries: u32) -> Self {
        self.options.retries = retries;
        self
    }

    /// Set the target platform for cross-architecture builds.
    #[must_use]
    pub fn platform(mut self, platform: impl Into<String>) -> Self {
        self.options.platform = Some(platform.into());
        self
    }

    /// Set a pre-computed source hash for content-based cache invalidation.
    ///
    /// When set, the sandbox builder can skip a full rebuild if the cached
    /// image was produced from identical source content.
    #[must_use]
    pub fn source_hash(mut self, hash: impl Into<String>) -> Self {
        self.options.source_hash = Some(hash.into());
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
    /// use zlayer_builder_zql::{ImageBuilder, BuildEvent};
    /// use std::sync::mpsc;
    ///
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let (tx, rx) = mpsc::channel::<BuildEvent>();
    ///
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .tag("myapp:latest")
    ///     .with_events(tx);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
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
    /// use zlayer_builder_zql::ImageBuilder;
    ///
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
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
    #[must_use]
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
    /// use zlayer_builder_zql::ImageBuilder;
    ///
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
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
    #[must_use]
    pub fn with_memory_cache(mut self) -> Self {
        self.options.cache_backend_config = Some(CacheBackendConfig::Memory);
        debug!("Configured in-memory cache");
        self
    }

    /// Configure an S3-compatible storage backend for layer caching.
    ///
    /// This is useful for distributed build systems where multiple build
    /// machines need to share a layer cache. Supports AWS S3, Cloudflare R2,
    /// Backblaze B2, `MinIO`, and other S3-compatible services.
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
    /// use zlayer_builder_zql::ImageBuilder;
    ///
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
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
    #[must_use]
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
    /// endpoint URL (e.g., Cloudflare R2, `MinIO`, local development).
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
    /// use zlayer_builder_zql::ImageBuilder;
    ///
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
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
    #[must_use]
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
    /// use zlayer_builder_zql::{ImageBuilder, CacheBackendConfig};
    ///
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .with_cache_config(CacheBackendConfig::Memory)
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "cache")]
    #[must_use]
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
    /// use zlayer_builder_zql::ImageBuilder;
    /// use zlayer_registry::cache::BlobCache;
    /// use std::sync::Arc;
    ///
    /// # async fn example() -> Result<(), zlayer_builder_zql::BuildError> {
    /// let cache = Arc::new(Box::new(BlobCache::new()?) as Box<dyn zlayer_registry::cache::BlobCacheBackend>);
    ///
    /// let builder = ImageBuilder::new("./my-project").await?
    ///     .with_cache_backend(cache)
    ///     .tag("myapp:latest");
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "cache")]
    #[must_use]
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
    ///
    /// # Panics
    ///
    /// Panics if an instruction output is missing after all retry attempts (internal invariant).
    #[instrument(skip(self), fields(context = %self.context.display()))]
    #[allow(clippy::too_many_lines)]
    pub async fn build(self) -> Result<BuiltImage> {
        let start_time = std::time::Instant::now();

        info!("Starting build in context: {}", self.context.display());

        // 1. Get build output (Dockerfile IR or WASM artifact)
        let build_output = self.get_build_output().await?;

        // If this is a WASM build, return early with the artifact info.
        if let BuildOutput::WasmArtifact {
            wasm_path,
            oci_path: _,
            language,
            optimized,
            size,
        } = build_output
        {
            #[allow(clippy::cast_possible_truncation)]
            let build_time_ms = start_time.elapsed().as_millis() as u64;

            self.send_event(BuildEvent::BuildComplete {
                image_id: wasm_path.display().to_string(),
            });

            info!(
                "WASM build completed in {}ms: {} ({}, {} bytes, optimized={})",
                build_time_ms,
                wasm_path.display(),
                language,
                size,
                optimized
            );

            return Ok(BuiltImage {
                image_id: format!("wasm:{}", wasm_path.display()),
                tags: self.options.tags.clone(),
                layer_count: 1,
                size,
                build_time_ms,
                is_manifest: false,
            });
        }

        // Extract the Dockerfile from the BuildOutput.
        let BuildOutput::Dockerfile(dockerfile) = build_output else {
            unreachable!("WasmArtifact case handled above");
        };
        debug!("Parsed Dockerfile with {} stages", dockerfile.stages.len());

        // Delegate the build to the backend.
        let backend = self
            .backend
            .as_ref()
            .ok_or_else(|| BuildError::BuildahNotFound {
                message: "No build backend configured".into(),
            })?;

        info!("Delegating build to {} backend", backend.name());
        let built = backend
            .build_image(
                &self.context,
                &dockerfile,
                &self.options,
                self.event_tx.clone(),
            )
            .await?;

        // Import the built image into ZLayer's local registry and blob cache
        // so the runtime can find it without pulling from a remote registry.
        //
        // A user who wired up a local registry clearly wants built images to
        // live there — if the import fails (almost always EACCES on the
        // registry dir for an unprivileged user), bail with the registry path
        // in the message instead of silently producing a build that the
        // daemon can't find.
        #[cfg(feature = "local-registry")]
        if let Some(ref registry) = self.local_registry {
            if !built.tags.is_empty() {
                let tmp_path = std::env::temp_dir().join(format!(
                    "zlayer-build-{}-{}.tar",
                    std::process::id(),
                    start_time.elapsed().as_nanos()
                ));

                // Export the image from buildah's store to an OCI archive.
                let export_tag = &built.tags[0];
                let dest = format!("oci-archive:{}", tmp_path.display());
                let push_cmd = BuildahCommand::push_to(export_tag, &dest);

                self.executor
                    .execute_checked(&push_cmd)
                    .await
                    .map_err(|e| BuildError::RegistryError {
                        message: format!(
                            "failed to export image to OCI archive for local registry \
                             import at {}: {e}",
                            registry.root().display()
                        ),
                    })?;

                // Resolve the blob cache backend (if available).
                let blob_cache: Option<&dyn zlayer_registry::cache::BlobCacheBackend> =
                    self.cache_backend.as_ref().map(|arc| arc.as_ref().as_ref());

                let import_result = async {
                    for tag in &built.tags {
                        let info =
                            import_image(registry, &tmp_path, Some(tag.as_str()), blob_cache)
                                .await
                                .map_err(|e| BuildError::RegistryError {
                                    message: format!(
                                        "failed to import '{tag}' into local registry at {}: {e}",
                                        registry.root().display()
                                    ),
                                })?;
                        info!(
                            tag = %tag,
                            digest = %info.digest,
                            "Imported into local registry"
                        );
                    }
                    Ok::<(), BuildError>(())
                }
                .await;

                // Clean up the temporary archive regardless of whether the
                // import succeeded (best-effort; warn on failure).
                if let Err(e) = fs::remove_file(&tmp_path).await {
                    warn!(path = %tmp_path.display(), error = %e, "Failed to remove temp OCI archive");
                }

                import_result?;
            }
        }

        Ok(built)
    }

    /// Detection order:
    /// 1. If `runtime` is set -> use template string -> parse as Dockerfile
    /// 2. If `zimagefile` is explicitly set -> read & parse `ZImagefile` -> convert
    /// 3. If a file called `ZImagefile` exists in the context dir -> same as (2)
    /// 4. Fall back to reading a Dockerfile (from `dockerfile` option or default)
    ///
    /// Returns [`BuildOutput::Dockerfile`] for container builds or
    /// [`BuildOutput::WasmArtifact`] for WASM builds.
    async fn get_build_output(&self) -> Result<BuildOutput> {
        // (a) Runtime template takes highest priority.
        if let Some(runtime) = &self.options.runtime {
            debug!("Using runtime template: {}", runtime);
            let content = get_template(*runtime);
            return Ok(BuildOutput::Dockerfile(Dockerfile::parse(content)?));
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

        Ok(BuildOutput::Dockerfile(Dockerfile::parse(&content)?))
    }

    /// Convert a parsed [`ZImage`] into a [`BuildOutput`].
    ///
    /// Handles all four `ZImage` modes:
    /// - **Runtime** mode: delegates to the template system -> [`BuildOutput::Dockerfile`]
    /// - **Single-stage / Multi-stage**: converts via [`zimage_to_dockerfile`] -> [`BuildOutput::Dockerfile`]
    /// - **WASM** mode: builds a WASM component -> [`BuildOutput::WasmArtifact`]
    ///
    /// Any `build:` directives are resolved first by spawning nested builds.
    async fn handle_zimage(&self, zimage: &crate::zimage::ZImage) -> Result<BuildOutput> {
        // Runtime mode: delegate to template system.
        if let Some(ref runtime_name) = zimage.runtime {
            let rt = Runtime::from_name(runtime_name).ok_or_else(|| {
                BuildError::zimagefile_validation(format!(
                    "unknown runtime '{runtime_name}' in ZImagefile"
                ))
            })?;
            let content = get_template(rt);
            return Ok(BuildOutput::Dockerfile(Dockerfile::parse(content)?));
        }

        // WASM mode: build a WASM component.
        if let Some(ref wasm_config) = zimage.wasm {
            return self.handle_wasm_build(wasm_config).await;
        }

        // Resolve any `build:` directives to concrete base image tags.
        let resolved = self.resolve_build_directives(zimage).await?;

        // Single-stage or multi-stage: convert to Dockerfile IR directly.
        Ok(BuildOutput::Dockerfile(
            crate::zimage::zimage_to_dockerfile(&resolved)?,
        ))
    }

    /// Build a WASM component from the `ZImagefile` wasm configuration.
    ///
    /// Converts [`ZWasmConfig`](crate::zimage::ZWasmConfig) into a
    /// [`WasmBuildConfig`](crate::wasm_builder::WasmBuildConfig) and invokes
    /// the WASM builder pipeline.
    async fn handle_wasm_build(
        &self,
        wasm_config: &crate::zimage::ZWasmConfig,
    ) -> Result<BuildOutput> {
        use crate::wasm_builder::{build_wasm, WasiTarget, WasmBuildConfig, WasmLanguage};

        info!("ZImagefile specifies WASM mode, running WASM build");

        // Convert target string to WasiTarget enum.
        let target = match wasm_config.target.as_str() {
            "preview1" => WasiTarget::Preview1,
            _ => WasiTarget::Preview2,
        };

        // Resolve language: parse from string or leave as None for auto-detection.
        let language = wasm_config
            .language
            .as_deref()
            .and_then(WasmLanguage::from_name);

        if let Some(ref lang_str) = wasm_config.language {
            if language.is_none() {
                return Err(BuildError::zimagefile_validation(format!(
                    "unknown WASM language '{lang_str}'. Supported: rust, go, python, \
                     typescript, assemblyscript, c, zig"
                )));
            }
        }

        // Build the WasmBuildConfig.
        let mut config = WasmBuildConfig {
            language,
            target,
            optimize: wasm_config.optimize,
            opt_level: wasm_config
                .opt_level
                .clone()
                .unwrap_or_else(|| "Oz".to_string()),
            wit_path: wasm_config.wit.as_ref().map(PathBuf::from),
            output_path: wasm_config.output.as_ref().map(PathBuf::from),
            world: wasm_config.world.clone(),
            features: wasm_config.features.clone(),
            build_args: wasm_config.build_args.clone(),
            pre_build: Vec::new(),
            post_build: Vec::new(),
            adapter: wasm_config.adapter.as_ref().map(PathBuf::from),
        };

        // Convert ZCommand pre/post build steps to Vec<Vec<String>>.
        for cmd in &wasm_config.pre_build {
            config.pre_build.push(zcommand_to_args(cmd));
        }
        for cmd in &wasm_config.post_build {
            config.post_build.push(zcommand_to_args(cmd));
        }

        // Build the WASM component.
        let result = build_wasm(&self.context, config).await?;

        let language_name = result.language.name().to_string();
        let wasm_path = result.wasm_path;
        let size = result.size;

        info!(
            "WASM build complete: {} ({} bytes, optimized={})",
            wasm_path.display(),
            size,
            wasm_config.optimize
        );

        Ok(BuildOutput::WasmArtifact {
            wasm_path,
            oci_path: None,
            language: language_name,
            optimized: wasm_config.optimize,
            size,
        })
    }

    /// Resolve `build:` directives in a `ZImage` by running nested builds.
    ///
    /// For each `build:` directive (top-level or per-stage), this method:
    /// 1. Determines the build context directory
    /// 2. Auto-detects the build file (`ZImagefile` > Dockerfile) unless specified
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
            if std::path::Path::new(file).extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml")
            }) || file.starts_with("ZImagefile")
            {
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

/// Convert a [`ZCommand`](crate::zimage::ZCommand) into a vector of string arguments
/// suitable for passing to [`WasmBuildConfig`](crate::wasm_builder::WasmBuildConfig)
/// pre/post build command lists.
fn zcommand_to_args(cmd: &crate::zimage::ZCommand) -> Vec<String> {
    match cmd {
        crate::zimage::ZCommand::Shell(s) => {
            vec!["/bin/sh".to_string(), "-c".to_string(), s.clone()]
        }
        crate::zimage::ZCommand::Exec(args) => args.clone(),
    }
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

    fn create_test_builder() -> ImageBuilder {
        // Create a minimal builder for testing (without async initialization)
        ImageBuilder {
            context: PathBuf::from("/tmp/test"),
            options: BuildOptions::default(),
            executor: BuildahExecutor::with_path("/usr/bin/buildah"),
            event_tx: None,
            backend: None,
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
        assert!(parsed > 1_700_000_000);
    }
}
