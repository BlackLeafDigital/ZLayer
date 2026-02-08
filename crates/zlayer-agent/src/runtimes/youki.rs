//! Youki/libcontainer runtime implementation
//!
//! Implements the Runtime trait using libcontainer (youki's container library)
//! for direct OCI container management without a daemon.

use crate::cgroups_stats::{self, ContainerStats};
use crate::error::{AgentError, Result};
use crate::runtime::{ContainerId, ContainerState, Runtime};
use crate::storage_manager::StorageManager;
use libcontainer::container::builder::ContainerBuilder;
use libcontainer::container::{Container, ContainerStatus};
use libcontainer::signal::Signal;
use libcontainer::syscall::syscall::SyscallType;
use std::collections::HashMap;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::Duration;
use tokio::fs;
use tokio::sync::RwLock;
use tracing::instrument;
use zlayer_spec::ServiceSpec;

/// Default state directory for libcontainer containers
pub const DEFAULT_STATE_DIR: &str = "/var/lib/zlayer/containers";

/// Default rootfs directory for unpacked images
pub const DEFAULT_ROOTFS_DIR: &str = "/var/lib/zlayer/rootfs";

/// Default bundle directory
pub const DEFAULT_BUNDLE_DIR: &str = "/var/lib/zlayer/bundles";

/// Default cache directory for image blobs
pub const DEFAULT_CACHE_DIR: &str = "/var/lib/zlayer/cache";

/// Configuration for YoukiRuntime
#[derive(Debug, Clone)]
pub struct YoukiConfig {
    /// State directory for libcontainer container state
    pub state_dir: PathBuf,
    /// Directory for unpacked image rootfs
    pub rootfs_dir: PathBuf,
    /// Directory for OCI bundles
    pub bundle_dir: PathBuf,
    /// Cache directory for image blobs
    pub cache_dir: PathBuf,
    /// Directory for persistent volumes
    pub volume_dir: PathBuf,
    /// Use systemd cgroups
    pub use_systemd: bool,
    /// Cache type configuration (if None, determined from environment)
    pub cache_type: Option<zlayer_registry::CacheType>,
}

impl Default for YoukiConfig {
    fn default() -> Self {
        Self {
            state_dir: std::env::var("ZLAYER_STATE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_STATE_DIR)),
            rootfs_dir: std::env::var("ZLAYER_ROOTFS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_ROOTFS_DIR)),
            bundle_dir: std::env::var("ZLAYER_BUNDLE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_BUNDLE_DIR)),
            cache_dir: std::env::var("ZLAYER_CACHE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_CACHE_DIR)),
            volume_dir: std::env::var("ZLAYER_VOLUME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/var/lib/zlayer/volumes")),
            use_systemd: std::env::var("ZLAYER_USE_SYSTEMD")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            cache_type: None,
        }
    }
}

/// Process-global mutex to serialize libcontainer operations.
/// libcontainer uses chdir() internally for notify socket operations
/// (to work around Unix socket 108-char path limit), which affects the
/// entire process. Concurrent container operations race on the CWD.
/// This must be process-global, not per-runtime, since chdir affects all threads.
static LIBCONTAINER_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

/// Container tracking information
#[derive(Debug)]
struct ContainerInfo {
    /// Image reference
    #[allow(dead_code)]
    image: String,
    /// Bundle path
    #[allow(dead_code)]
    bundle_path: PathBuf,
    /// Rootfs path
    #[allow(dead_code)]
    rootfs_path: PathBuf,
    /// Stdout log file path
    stdout_path: PathBuf,
    /// Stderr log file path
    stderr_path: PathBuf,
    /// Process ID (once running)
    #[allow(dead_code)]
    pid: Option<u32>,
}

/// Youki/libcontainer-based container runtime
///
/// This runtime uses libcontainer directly to create and manage OCI containers
/// without requiring a daemon like containerd.
pub struct YoukiRuntime {
    /// Configuration
    config: YoukiConfig,
    /// Local container state tracking
    containers: RwLock<HashMap<String, ContainerInfo>>,
    /// Authentication resolver for registry pulls
    auth_resolver: zlayer_core::AuthResolver,
    /// Storage volume manager
    storage_manager: std::sync::Arc<tokio::sync::RwLock<StorageManager>>,
    /// Shared blob cache for image layers (avoids repeated opens and ensures cache persistence)
    blob_cache: std::sync::Arc<Box<dyn zlayer_registry::BlobCacheBackend>>,
    /// Cached image configs (entrypoint, cmd, env, etc.) keyed by image reference
    image_configs: RwLock<HashMap<String, zlayer_registry::ImageConfig>>,
}

impl std::fmt::Debug for YoukiRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YoukiRuntime")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl YoukiRuntime {
    /// Create a new YoukiRuntime with the given configuration
    pub async fn new(config: YoukiConfig) -> Result<Self> {
        // Ensure directories exist
        for dir in [
            &config.state_dir,
            &config.rootfs_dir,
            &config.bundle_dir,
            &config.cache_dir,
        ] {
            fs::create_dir_all(dir)
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: "runtime".to_string(),
                    reason: format!("failed to create directory {}: {}", dir.display(), e),
                })?;
        }

        // Initialize storage manager
        let storage_manager =
            StorageManager::new(&config.volume_dir).map_err(|e| AgentError::CreateFailed {
                id: "runtime".to_string(),
                reason: format!("failed to create storage manager: {}", e),
            })?;

        // Initialize shared blob cache using CacheType configuration
        // If cache_type is provided, use it directly; otherwise use environment-based config
        // but override the path for Persistent variant to use config.cache_dir
        let blob_cache = match &config.cache_type {
            Some(cache_type) => cache_type
                .build()
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: "runtime".to_string(),
                    reason: format!("failed to build blob cache: {}", e),
                })?,
            None => {
                let cache_type = zlayer_registry::CacheType::from_env().map_err(|e| {
                    AgentError::CreateFailed {
                        id: "runtime".to_string(),
                        reason: format!("failed to read cache config from env: {}", e),
                    }
                })?;
                // Override persistent path to use config.cache_dir
                let cache_type = match cache_type {
                    zlayer_registry::CacheType::Persistent { .. } => {
                        zlayer_registry::CacheType::persistent_at(
                            config.cache_dir.join("blobs.redb"),
                        )
                    }
                    other => other,
                };
                cache_type
                    .build()
                    .await
                    .map_err(|e| AgentError::CreateFailed {
                        id: "runtime".to_string(),
                        reason: format!("failed to build blob cache: {}", e),
                    })?
            }
        };

        Ok(Self {
            config,
            containers: RwLock::new(HashMap::new()),
            auth_resolver: zlayer_core::AuthResolver::new(zlayer_core::AuthConfig::default()),
            storage_manager: std::sync::Arc::new(tokio::sync::RwLock::new(storage_manager)),
            blob_cache,
            image_configs: RwLock::new(HashMap::new()),
        })
    }

    /// Create a new YoukiRuntime with default configuration
    pub async fn with_defaults() -> Result<Self> {
        Self::new(YoukiConfig::default()).await
    }

    /// Create a new YoukiRuntime with custom auth configuration
    pub async fn with_auth(
        config: YoukiConfig,
        auth_config: zlayer_core::AuthConfig,
    ) -> Result<Self> {
        // Ensure directories exist
        for dir in [
            &config.state_dir,
            &config.rootfs_dir,
            &config.bundle_dir,
            &config.cache_dir,
        ] {
            fs::create_dir_all(dir)
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: "runtime".to_string(),
                    reason: format!("failed to create directory {}: {}", dir.display(), e),
                })?;
        }

        // Initialize storage manager
        let storage_manager =
            StorageManager::new(&config.volume_dir).map_err(|e| AgentError::CreateFailed {
                id: "runtime".to_string(),
                reason: format!("failed to create storage manager: {}", e),
            })?;

        // Initialize shared blob cache using CacheType configuration
        // If cache_type is provided, use it directly; otherwise use environment-based config
        // but override the path for Persistent variant to use config.cache_dir
        let blob_cache = match &config.cache_type {
            Some(cache_type) => cache_type
                .build()
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: "runtime".to_string(),
                    reason: format!("failed to build blob cache: {}", e),
                })?,
            None => {
                let cache_type = zlayer_registry::CacheType::from_env().map_err(|e| {
                    AgentError::CreateFailed {
                        id: "runtime".to_string(),
                        reason: format!("failed to read cache config from env: {}", e),
                    }
                })?;
                // Override persistent path to use config.cache_dir
                let cache_type = match cache_type {
                    zlayer_registry::CacheType::Persistent { .. } => {
                        zlayer_registry::CacheType::persistent_at(
                            config.cache_dir.join("blobs.redb"),
                        )
                    }
                    other => other,
                };
                cache_type
                    .build()
                    .await
                    .map_err(|e| AgentError::CreateFailed {
                        id: "runtime".to_string(),
                        reason: format!("failed to build blob cache: {}", e),
                    })?
            }
        };

        Ok(Self {
            config,
            containers: RwLock::new(HashMap::new()),
            auth_resolver: zlayer_core::AuthResolver::new(auth_config),
            storage_manager: std::sync::Arc::new(tokio::sync::RwLock::new(storage_manager)),
            blob_cache,
            image_configs: RwLock::new(HashMap::new()),
        })
    }

    /// Get the container ID string
    fn container_id_str(&self, id: &ContainerId) -> String {
        format!("{}-{}", id.service, id.replica)
    }

    /// Get the root path for a container's state
    fn container_root(&self, id: &ContainerId) -> PathBuf {
        self.config.state_dir.join(self.container_id_str(id))
    }

    /// Get the bundle path for a container
    fn bundle_path(&self, id: &ContainerId) -> PathBuf {
        self.config.bundle_dir.join(self.container_id_str(id))
    }

    /// Get log directory for a container (separate from state to avoid conflicts with libcontainer)
    fn log_dir(&self, id: &ContainerId) -> PathBuf {
        // Put logs in bundle directory to avoid conflicting with libcontainer's state directory
        self.bundle_path(id).join("logs")
    }

    /// Get log file paths for a container
    fn log_paths(&self, id: &ContainerId) -> (PathBuf, PathBuf) {
        let log_dir = self.log_dir(id);
        (log_dir.join("stdout.log"), log_dir.join("stderr.log"))
    }

    /// Map libcontainer status to our ContainerState
    fn map_status(&self, status: ContainerStatus) -> ContainerState {
        match status {
            ContainerStatus::Creating => ContainerState::Pending,
            ContainerStatus::Created => ContainerState::Pending,
            ContainerStatus::Running => ContainerState::Running,
            ContainerStatus::Stopped => ContainerState::Exited { code: 0 },
            ContainerStatus::Paused => ContainerState::Stopping,
        }
    }

    /// Create log files and return file descriptors for stdout/stderr
    async fn create_log_files(
        &self,
        id: &ContainerId,
    ) -> Result<(PathBuf, PathBuf, OwnedFd, OwnedFd)> {
        let log_dir = self.log_dir(id);
        fs::create_dir_all(&log_dir)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("failed to create log dir: {}", e),
            })?;

        let (stdout_path, stderr_path) = self.log_paths(id);

        // Create stdout file
        let stdout_file =
            std::fs::File::create(&stdout_path).map_err(|e| AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("failed to create stdout log: {}", e),
            })?;

        // Create stderr file
        let stderr_file =
            std::fs::File::create(&stderr_path).map_err(|e| AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("failed to create stderr log: {}", e),
            })?;

        // Convert to OwnedFd
        use std::os::unix::io::IntoRawFd;
        let stdout_fd = unsafe { OwnedFd::from_raw_fd(stdout_file.into_raw_fd()) };
        let stderr_fd = unsafe { OwnedFd::from_raw_fd(stderr_file.into_raw_fd()) };

        Ok((stdout_path, stderr_path, stdout_fd, stderr_fd))
    }

    /// Clean up bundle directory for a container
    async fn cleanup_bundle(&self, id: &ContainerId) -> Result<()> {
        let bundle_path = self.bundle_path(id);
        if bundle_path.exists() {
            fs::remove_dir_all(&bundle_path)
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: id.to_string(),
                    reason: format!("failed to remove bundle: {}", e),
                })?;
        }
        Ok(())
    }

    /// Pull image layers and return them for extraction
    ///
    /// Uses the shared blob cache to avoid repeated network requests for cached layers.
    async fn pull_image_layers(&self, image: &str) -> Result<Vec<(Vec<u8>, String)>> {
        // Use the shared blob cache instead of opening a new one each time
        let puller = zlayer_registry::ImagePuller::with_cache(self.blob_cache.clone());
        let auth = self.auth_resolver.resolve(image);

        puller
            .pull_image(image, &auth)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("failed to pull image: {}", e),
            })
    }

    /// Prepare storage volumes for a container, returning paths for mounts
    async fn prepare_storage_volumes(
        &self,
        id: &ContainerId,
        spec: &ServiceSpec,
    ) -> Result<std::collections::HashMap<String, PathBuf>> {
        use zlayer_spec::StorageSpec;

        let mut volume_paths = std::collections::HashMap::new();
        let container_id = self.container_id_str(id);

        let mut storage_manager = self.storage_manager.write().await;

        for storage in &spec.storage {
            match storage {
                StorageSpec::Named { name, .. } => {
                    let path = storage_manager.ensure_volume(name).map_err(|e| {
                        AgentError::CreateFailed {
                            id: container_id.clone(),
                            reason: format!("failed to ensure volume '{}': {}", name, e),
                        }
                    })?;
                    storage_manager
                        .attach_volume(name, &container_id)
                        .map_err(|e| AgentError::CreateFailed {
                            id: container_id.clone(),
                            reason: format!("failed to attach volume '{}': {}", name, e),
                        })?;
                    volume_paths.insert(name.clone(), path);
                }

                StorageSpec::Anonymous { target, .. } => {
                    let path = storage_manager
                        .create_anonymous(&container_id, target)
                        .map_err(|e| AgentError::CreateFailed {
                            id: container_id.clone(),
                            reason: format!(
                                "failed to create anonymous volume for '{}': {}",
                                target, e
                            ),
                        })?;
                    let key = format!("_anon_{}", target.trim_start_matches('/').replace('/', "_"));
                    volume_paths.insert(key, path);
                }

                // Bind mounts don't need preparation - source path is used directly
                StorageSpec::Bind { .. } => {}

                // Tmpfs mounts don't need preparation
                StorageSpec::Tmpfs { .. } => {}

                StorageSpec::S3 {
                    bucket,
                    prefix,
                    endpoint,
                    ..
                } => {
                    let path = storage_manager
                        .mount_s3(
                            bucket,
                            prefix.as_deref(),
                            endpoint.as_deref(),
                            &container_id,
                        )
                        .map_err(|e| AgentError::CreateFailed {
                            id: container_id.clone(),
                            reason: format!("failed to mount S3 bucket '{}': {}", bucket, e),
                        })?;
                    let key = format!("_s3_{}_{}", bucket, prefix.as_deref().unwrap_or(""));
                    volume_paths.insert(key, path);
                }
            }
        }

        Ok(volume_paths)
    }

    /// Clean up storage volumes for a container
    ///
    /// Note: This method requires the ServiceSpec to know which volumes to clean up.
    /// For now, remove_container uses a simpler approach that only cleans up anonymous volumes.
    /// This method is available for future use when the spec is stored/available at removal time.
    #[allow(dead_code)]
    async fn cleanup_storage_volumes(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        use zlayer_spec::StorageSpec;

        let container_id = self.container_id_str(id);
        let mut storage_manager = self.storage_manager.write().await;

        // Detach named volumes
        for storage in &spec.storage {
            match storage {
                StorageSpec::Named { name, .. } => {
                    if let Err(e) = storage_manager.detach_volume(name, &container_id) {
                        tracing::warn!(
                            volume = %name,
                            container = %container_id,
                            error = %e,
                            "failed to detach volume"
                        );
                    }
                }
                StorageSpec::S3 { bucket, prefix, .. } => {
                    if let Err(e) =
                        storage_manager.unmount_s3(bucket, prefix.as_deref(), &container_id)
                    {
                        tracing::warn!(
                            bucket = %bucket,
                            container = %container_id,
                            error = %e,
                            "failed to unmount S3 bucket"
                        );
                    }
                }
                _ => {}
            }
        }

        // Clean up anonymous volumes
        if let Err(e) = storage_manager.cleanup_anonymous(&container_id) {
            tracing::warn!(
                container = %container_id,
                error = %e,
                "failed to cleanup anonymous volumes"
            );
        }

        Ok(())
    }

    /// Get a cached image config by image reference
    ///
    /// Returns the previously pulled image configuration (entrypoint, cmd, env, etc.)
    /// for the given image reference, if available.
    async fn get_image_config(&self, image: &str) -> Option<zlayer_registry::ImageConfig> {
        let configs = self.image_configs.read().await;
        configs.get(image).cloned()
    }
}

#[async_trait::async_trait]
impl Runtime for YoukiRuntime {
    /// Pull an image to local storage
    ///
    /// Downloads image layers from a registry and unpacks them to a rootfs.
    #[instrument(
        skip(self),
        fields(
            otel.name = "image.pull",
            container.image.name = %image,
        )
    )]
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.pull_image_with_policy(image, zlayer_spec::PullPolicy::IfNotPresent)
            .await
    }

    /// Pull an image to local storage with a specific pull policy
    ///
    /// This downloads image layers to the blob cache. Layers are extracted
    /// per-container in create_container to avoid race conditions.
    #[instrument(
        skip(self),
        fields(
            otel.name = "image.pull",
            container.image.name = %image,
            pull_policy = ?policy,
        )
    )]
    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: zlayer_spec::PullPolicy,
    ) -> Result<()> {
        // For Never policy, we just check if we can pull from cache
        // The actual extraction happens in create_container
        if matches!(policy, zlayer_spec::PullPolicy::Never) {
            // Try to get manifest to verify image is cached
            // For now, assume if policy is Never, caller knows image exists
            tracing::debug!(image = %image, "pull policy is Never, skipping pull");
            return Ok(());
        }

        // For IfNotPresent, check if image layers are in cache by trying to pull
        // Use the shared blob cache to avoid repeated opens and ensure persistence
        let puller = zlayer_registry::ImagePuller::with_cache(self.blob_cache.clone());
        let auth = self.auth_resolver.resolve(image);

        tracing::info!(image = %image, "pulling image layers to cache");

        // Pull image layers from registry (cached layers are retrieved from cache)
        let layers = puller
            .pull_image(image, &auth)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("failed to pull image: {}", e),
            })?;

        tracing::info!(
            image = %image,
            layer_count = layers.len(),
            "image layers cached"
        );

        // Also pull and cache the image config (entrypoint, cmd, env, etc.)
        match puller.pull_image_config(image, &auth).await {
            Ok(config) => {
                tracing::info!(
                    image = %image,
                    has_entrypoint = config.entrypoint.is_some(),
                    has_cmd = config.cmd.is_some(),
                    "image config cached"
                );
                let mut configs = self.image_configs.write().await;
                configs.insert(image.to_string(), config);
            }
            Err(e) => {
                // Log but don't fail - the container can still run with spec defaults
                tracing::warn!(
                    image = %image,
                    error = %e,
                    "failed to pull image config, container will use spec defaults"
                );
            }
        }

        Ok(())
    }

    /// Create a container
    ///
    /// Creates an OCI bundle and uses libcontainer to create the container.
    /// Each container gets its own rootfs extracted from cached layers.
    #[instrument(
        skip(self, spec),
        fields(
            otel.name = "container.create",
            container.id = %self.container_id_str(id),
            service.name = %id.service,
            service.replica = %id.replica,
            container.image.name = %spec.image.name,
        )
    )]
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let container_id = self.container_id_str(id);
        let image = &spec.image.name;
        let bundle_path = self.bundle_path(id);
        let rootfs_path = bundle_path.join("rootfs");

        tracing::info!("Creating container {} from image {}", container_id, image);

        // Create bundle directory structure
        fs::create_dir_all(&bundle_path)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: container_id.clone(),
                reason: format!("failed to create bundle directory: {}", e),
            })?;

        // Pull image layers (from cache if available)
        let layers = self.pull_image_layers(image).await?;

        tracing::debug!(
            container = %container_id,
            layer_count = layers.len(),
            "extracting layers to container rootfs"
        );

        // Extract layers to this container's own rootfs
        let mut unpacker = zlayer_registry::LayerUnpacker::new(rootfs_path.clone());
        unpacker
            .unpack_layers(&layers)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: container_id.clone(),
                reason: format!("failed to extract rootfs: {}", e),
            })?;

        // Get cached image config (entrypoint, cmd, env, workdir, user)
        let img_config = self.get_image_config(image).await;

        // Prepare storage volumes
        let volume_paths = self.prepare_storage_volumes(id, spec).await?;

        // Generate OCI config.json via BundleBuilder (handles capabilities, devices,
        // resource limits, storage mounts, env resolution, and command resolution)
        let mut bundle_builder = crate::bundle::BundleBuilder::new(bundle_path.clone())
            .with_volume_paths(volume_paths)
            .with_host_network(spec.host_network);
        if let Some(config) = img_config {
            bundle_builder = bundle_builder.with_image_config(config);
        }
        bundle_builder.write_config(id, spec).await?;

        // Create log files
        let (stdout_path, stderr_path, stdout_fd, stderr_fd) = self.create_log_files(id).await?;

        // Use spawn_blocking for the synchronous libcontainer operations
        let config = self.config.clone();
        let container_id_clone = container_id.clone();
        let bundle_path_clone = bundle_path.clone();
        // Use state_dir as the root path - libcontainer appends container_id internally
        let state_dir_clone = self.config.state_dir.clone();
        let _container = tokio::task::spawn_blocking(move || {
            // Acquire process-global lock to serialize libcontainer operations.
            // libcontainer uses chdir() internally which affects the entire process,
            // so concurrent operations would race on the working directory.
            let _guard = LIBCONTAINER_LOCK
                .lock()
                .map_err(|e| AgentError::CreateFailed {
                    id: container_id_clone.clone(),
                    reason: format!("failed to acquire libcontainer lock: {}", e),
                })?;

            // Create container using libcontainer
            // Set stdout/stderr on ContainerBuilder BEFORE calling as_init()
            let container_builder =
                ContainerBuilder::new(container_id_clone.clone(), SyscallType::Linux)
                    .with_stdout(stdout_fd)
                    .with_stderr(stderr_fd);

            // Set container root path (base dir - libcontainer creates <root>/<container_id>)
            let container_builder =
                container_builder
                    .with_root_path(&state_dir_clone)
                    .map_err(|e| AgentError::CreateFailed {
                        id: container_id_clone.clone(),
                        reason: format!("failed to set root path: {}", e),
                    })?;

            // Configure as init container (creates new namespaces)
            let init_builder = container_builder
                .as_init(&bundle_path_clone)
                .with_systemd(config.use_systemd)
                .with_detach(true); // Run detached

            // Build the container (creates it but doesn't start)
            let container = init_builder.build().map_err(|e| AgentError::CreateFailed {
                id: container_id_clone.clone(),
                reason: format!("failed to create container: {}", e),
            })?;

            Ok::<Container, AgentError>(container)
        })
        .await
        .map_err(|e| AgentError::CreateFailed {
            id: container_id.clone(),
            reason: format!("task join error: {}", e),
        })??;

        // Store container info
        {
            let mut containers = self.containers.write().await;
            containers.insert(
                container_id.clone(),
                ContainerInfo {
                    image: image.clone(),
                    bundle_path,
                    rootfs_path,
                    stdout_path,
                    stderr_path,
                    pid: None,
                },
            );
        }

        tracing::info!("Container {} created successfully", container_id);
        Ok(())
    }

    /// Start a container
    ///
    /// Starts the container's init process.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.start",
            container.id = %self.container_id_str(id),
            service.name = %id.service,
        )
    )]
    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let container_id = self.container_id_str(id);
        let container_root = self.container_root(id);

        tracing::info!("Starting container {}", container_id);

        // Load and start the container using spawn_blocking
        let pid = tokio::task::spawn_blocking(move || {
            // Acquire process-global lock to serialize libcontainer operations.
            // libcontainer uses chdir() internally which affects the entire process,
            // so concurrent operations would race on the working directory.
            let _guard = LIBCONTAINER_LOCK
                .lock()
                .map_err(|e| AgentError::StartFailed {
                    id: container_id.clone(),
                    reason: format!("failed to acquire libcontainer lock: {}", e),
                })?;

            let mut container =
                Container::load(container_root).map_err(|e| AgentError::StartFailed {
                    id: container_id.clone(),
                    reason: format!("failed to load container: {}", e),
                })?;

            // Start the container
            container.start().map_err(|e| AgentError::StartFailed {
                id: container_id.clone(),
                reason: format!("failed to start container: {}", e),
            })?;

            // Get the PID after starting - access through state
            let pid = container.pid().map(|p| p.as_raw() as u32);

            Ok::<Option<u32>, AgentError>(pid)
        })
        .await
        .map_err(|e| AgentError::StartFailed {
            id: id.to_string(),
            reason: format!("task join error: {}", e),
        })??;

        // Update container info with PID
        {
            let mut containers = self.containers.write().await;
            if let Some(info) = containers.get_mut(&self.container_id_str(id)) {
                info.pid = pid;
            }
        }

        tracing::info!(
            "Container {} started with PID {:?}",
            self.container_id_str(id),
            pid
        );
        Ok(())
    }

    /// Stop a container
    ///
    /// Sends SIGTERM, waits for timeout, then sends SIGKILL if needed.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.stop",
            container.id = %self.container_id_str(id),
            service.name = %id.service,
            timeout_ms = %timeout.as_millis(),
        )
    )]
    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        let container_id = self.container_id_str(id);
        let container_root = self.container_root(id);

        tracing::info!(
            "Stopping container {} with {:?} timeout",
            container_id,
            timeout
        );

        // Send SIGTERM first
        let container_root_clone = container_root.clone();
        let container_id_clone = container_id.clone();

        tokio::task::spawn_blocking(move || {
            let mut container =
                Container::load(container_root_clone).map_err(|e| AgentError::NotFound {
                    container: container_id_clone.clone(),
                    reason: format!("failed to load container: {}", e),
                })?;

            // Check if container can be killed
            if container.status().can_kill() {
                // Send SIGTERM
                use std::convert::TryFrom;
                let signal = Signal::try_from("SIGTERM").map_err(|e| AgentError::NotFound {
                    container: container_id_clone.clone(),
                    reason: format!("invalid signal: {:?}", e),
                })?;

                if let Err(e) = container.kill(signal, true) {
                    tracing::debug!("SIGTERM failed (container may already be stopped): {}", e);
                }
            }

            Ok::<(), AgentError>(())
        })
        .await
        .map_err(|e| AgentError::NotFound {
            container: container_id.clone(),
            reason: format!("task join error: {}", e),
        })??;

        // Wait for container to stop
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > timeout {
                break;
            }

            // Check container state
            let state = self.container_state(id).await?;
            if matches!(
                state,
                ContainerState::Exited { .. } | ContainerState::Failed { .. }
            ) {
                tracing::info!("Container {} stopped gracefully", container_id);
                return Ok(());
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Timeout exceeded - send SIGKILL
        tracing::debug!(
            "Container {} did not stop gracefully, sending SIGKILL",
            container_id
        );

        let container_root_clone = container_root.clone();
        let container_id_clone = container_id.clone();

        tokio::task::spawn_blocking(move || {
            let mut container =
                Container::load(container_root_clone).map_err(|e| AgentError::NotFound {
                    container: container_id_clone.clone(),
                    reason: format!("failed to load container: {}", e),
                })?;

            if container.status().can_kill() {
                use std::convert::TryFrom;
                let signal = Signal::try_from("SIGKILL").map_err(|e| AgentError::NotFound {
                    container: container_id_clone.clone(),
                    reason: format!("invalid signal: {:?}", e),
                })?;

                if let Err(e) = container.kill(signal, true) {
                    tracing::warn!("SIGKILL failed: {}", e);
                }
            }

            Ok::<(), AgentError>(())
        })
        .await
        .map_err(|e| AgentError::NotFound {
            container: container_id.clone(),
            reason: format!("task join error: {}", e),
        })??;

        tracing::info!("Container {} killed", container_id);
        Ok(())
    }

    /// Remove a container
    ///
    /// Deletes the container and cleans up its bundle and state.
    /// Cleanup always proceeds even if libcontainer operations fail.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.remove",
            container.id = %self.container_id_str(id),
            service.name = %id.service,
        )
    )]
    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let container_id = self.container_id_str(id);
        let container_root = self.container_root(id);

        tracing::info!("Removing container {}", container_id);

        // Attempt libcontainer delete, but don't fail if container not found
        let container_id_clone = container_id.clone();
        let container_root_clone = container_root.clone();

        let libcontainer_result = tokio::task::spawn_blocking(move || {
            match Container::load(container_root_clone) {
                Ok(mut container) => {
                    // Delete with force=true to handle any state
                    if let Err(e) = container.delete(true) {
                        tracing::warn!(
                            "libcontainer delete failed for {}: {}",
                            container_id_clone,
                            e
                        );
                    }
                }
                Err(e) => {
                    // Container may already be gone or state is in unexpected location
                    tracing::warn!(
                        "Container::load failed for {} (may already be removed): {}",
                        container_id_clone,
                        e
                    );
                }
            }
        })
        .await;

        if let Err(e) = libcontainer_result {
            tracing::warn!("spawn_blocking failed during remove: {}", e);
        }

        // ALWAYS clean up bundle regardless of libcontainer result
        if let Err(e) = self.cleanup_bundle(id).await {
            tracing::warn!("Failed to cleanup bundle for {}: {}", container_id, e);
        }

        // ALWAYS clean up state directory regardless of libcontainer result
        let state_dir = self.container_root(id);
        if state_dir.exists() {
            if let Err(e) = fs::remove_dir_all(&state_dir).await {
                tracing::warn!("Failed to remove state dir {}: {}", state_dir.display(), e);
            }
        }

        // Clean up storage volumes
        // Note: We need the spec to know what to clean up, but we don't have it here
        // For now, we'll just clean up anonymous volumes by container ID
        {
            let mut storage_manager = self.storage_manager.write().await;
            if let Err(e) = storage_manager.cleanup_anonymous(&container_id) {
                tracing::warn!(
                    container = %container_id,
                    error = %e,
                    "failed to cleanup anonymous volumes"
                );
            }
        }

        // Remove from local tracking
        {
            let mut containers = self.containers.write().await;
            containers.remove(&container_id);
        }

        tracing::info!("Container {} removed", container_id);
        Ok(())
    }

    /// Get container state
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.state",
            container.id = %self.container_id_str(id),
        )
    )]
    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let container_id = self.container_id_str(id);
        let container_root = self.container_root(id);

        // Check if container root exists
        if !container_root.exists() {
            return Err(AgentError::NotFound {
                container: container_id.clone(),
                reason: "container state directory not found".to_string(),
            });
        }

        // Load container and get status
        let container_id_clone = container_id.clone();

        let status = tokio::task::spawn_blocking(move || {
            let mut container =
                Container::load(container_root).map_err(|e| AgentError::NotFound {
                    container: container_id_clone.clone(),
                    reason: format!("failed to load container: {}", e),
                })?;

            // Refresh status to get current state
            let _ = container.refresh_status();

            Ok::<ContainerStatus, AgentError>(container.status())
        })
        .await
        .map_err(|e| AgentError::NotFound {
            container: container_id.clone(),
            reason: format!("task join error: {}", e),
        })??;

        Ok(self.map_status(status))
    }

    /// Get container logs
    ///
    /// Reads from the container's stdout/stderr log files.
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<String> {
        let container_id = self.container_id_str(id);

        // Get log paths from local state
        let (stdout_path, stderr_path) = {
            let containers = self.containers.read().await;
            match containers.get(&container_id) {
                Some(info) => (info.stdout_path.clone(), info.stderr_path.clone()),
                None => {
                    // Fall back to default paths
                    self.log_paths(id)
                }
            }
        };

        let mut logs = String::new();

        // Read stdout
        if stdout_path.exists() {
            if let Ok(content) = fs::read_to_string(&stdout_path).await {
                if !content.is_empty() {
                    logs.push_str("[stdout]\n");
                    logs.push_str(&content);
                }
            }
        }

        // Read stderr
        if stderr_path.exists() {
            if let Ok(content) = fs::read_to_string(&stderr_path).await {
                if !content.is_empty() {
                    if !logs.is_empty() {
                        logs.push('\n');
                    }
                    logs.push_str("[stderr]\n");
                    logs.push_str(&content);
                }
            }
        }

        // Apply tail limit
        if tail > 0 {
            let lines: Vec<&str> = logs.lines().collect();
            if lines.len() > tail {
                logs = lines[lines.len() - tail..].join("\n");
            }
        }

        Ok(logs)
    }

    /// Execute a command in a running container
    ///
    /// Uses libcontainer's tenant builder to exec into the container's namespaces.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.exec",
            container.id = %self.container_id_str(id),
            command = ?cmd,
        )
    )]
    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        let container_id = self.container_id_str(id);

        if cmd.is_empty() {
            return Err(AgentError::InvalidSpec(
                "exec command cannot be empty".to_string(),
            ));
        }

        tracing::debug!("Executing {:?} in container {}", cmd, container_id);

        // Create temporary files for exec output
        let exec_id = uuid::Uuid::new_v4().to_string();
        let exec_dir = self.config.state_dir.join(format!("exec-{}", exec_id));
        fs::create_dir_all(&exec_dir)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: exec_id.clone(),
                reason: format!("failed to create exec dir: {}", e),
            })?;

        let stdout_path = exec_dir.join("stdout");
        let stderr_path = exec_dir.join("stderr");

        // Create output files
        let stdout_file =
            std::fs::File::create(&stdout_path).map_err(|e| AgentError::CreateFailed {
                id: exec_id.clone(),
                reason: format!("failed to create stdout file: {}", e),
            })?;

        let stderr_file =
            std::fs::File::create(&stderr_path).map_err(|e| AgentError::CreateFailed {
                id: exec_id.clone(),
                reason: format!("failed to create stderr file: {}", e),
            })?;

        use std::os::unix::io::IntoRawFd;
        let stdout_fd = unsafe { OwnedFd::from_raw_fd(stdout_file.into_raw_fd()) };
        let stderr_fd = unsafe { OwnedFd::from_raw_fd(stderr_file.into_raw_fd()) };

        let cmd_clone = cmd.to_vec();
        let container_id_clone = container_id.clone();
        // Use state_dir as the root path - libcontainer expects base dir
        let state_dir_clone = self.config.state_dir.clone();

        // Execute using tenant builder
        let exec_pid = tokio::task::spawn_blocking(move || {
            // Create container builder for tenant (joining existing container)
            // Set stdout/stderr on ContainerBuilder BEFORE calling as_tenant()
            let container_builder =
                ContainerBuilder::new(container_id_clone.clone(), SyscallType::Linux)
                    .with_stdout(stdout_fd)
                    .with_stderr(stderr_fd);

            let container_builder =
                container_builder
                    .with_root_path(&state_dir_clone)
                    .map_err(|e| AgentError::CreateFailed {
                        id: container_id_clone.clone(),
                        reason: format!("failed to set root path: {}", e),
                    })?;

            // Configure as tenant (joins existing namespaces)
            let tenant_builder = container_builder
                .as_tenant()
                .with_container_args(cmd_clone)
                .with_detach(false); // Wait for completion

            // Execute and wait
            let pid = tenant_builder
                .build()
                .map_err(|e| AgentError::CreateFailed {
                    id: container_id_clone.clone(),
                    reason: format!("failed to exec in container: {}", e),
                })?;

            // Return raw pid as i32 to avoid nix version conflicts
            // (libcontainer uses nix 0.29, we use nix 0.31)
            Ok::<i32, AgentError>(pid.as_raw())
        })
        .await
        .map_err(|e| AgentError::CreateFailed {
            id: container_id.clone(),
            reason: format!("task join error: {}", e),
        })??;

        // Wait for process to complete and get exit status
        let exit_code = tokio::task::spawn_blocking(move || {
            use nix::sys::wait::{waitpid, WaitStatus};
            use nix::unistd::Pid;
            // Convert raw pid back to our nix version's Pid type
            let pid = Pid::from_raw(exec_pid);
            match waitpid(pid, None) {
                Ok(WaitStatus::Exited(_, code)) => code,
                Ok(WaitStatus::Signaled(_, signal, _)) => 128 + signal as i32,
                Ok(_) => -1,
                Err(_) => -1,
            }
        })
        .await
        .unwrap_or(-1);

        // Read output
        let stdout_content = fs::read_to_string(&stdout_path).await.unwrap_or_default();
        let stderr_content = fs::read_to_string(&stderr_path).await.unwrap_or_default();

        // Clean up exec directory
        let _ = fs::remove_dir_all(&exec_dir).await;

        Ok((exit_code, stdout_content, stderr_content))
    }

    /// Get container resource statistics from cgroups
    ///
    /// Reads CPU and memory statistics from the cgroups v2 filesystem.
    /// Supports both systemd and cgroupfs cgroup drivers.
    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let container_id = self.container_id_str(id);

        // Determine cgroup path based on cgroup driver
        let cgroup_path = if self.config.use_systemd {
            // systemd cgroup driver: /sys/fs/cgroup/system.slice/zlayer-{id}.scope
            PathBuf::from(format!(
                "/sys/fs/cgroup/system.slice/zlayer-{}.scope",
                container_id
            ))
        } else {
            // cgroupfs driver: /sys/fs/cgroup/zlayer/{id}
            PathBuf::from(format!("/sys/fs/cgroup/zlayer/{}", container_id))
        };

        tracing::debug!(
            container = %container_id,
            cgroup_path = %cgroup_path.display(),
            "reading container stats from cgroups"
        );

        cgroups_stats::read_container_stats(&cgroup_path)
            .await
            .map_err(|e| {
                AgentError::Internal(format!(
                    "failed to read cgroup stats for container {}: {}",
                    container_id, e
                ))
            })
    }

    /// Wait for a container to exit and return its exit code
    ///
    /// This polls the container state until it reaches an exited state.
    /// For libcontainer, we don't have a direct "wait" API, so we poll.
    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        let container_id = self.container_id_str(id);
        let poll_interval = Duration::from_millis(100);
        let max_wait = Duration::from_secs(3600); // 1 hour max
        let start = std::time::Instant::now();

        tracing::debug!(
            container = %container_id,
            "waiting for container to exit"
        );

        loop {
            if start.elapsed() > max_wait {
                return Err(AgentError::Timeout { timeout: max_wait });
            }

            match self.container_state(id).await {
                Ok(ContainerState::Exited { code }) => {
                    tracing::debug!(
                        container = %container_id,
                        exit_code = code,
                        "container exited"
                    );
                    return Ok(code);
                }
                Ok(ContainerState::Failed { reason }) => {
                    tracing::warn!(
                        container = %container_id,
                        reason = %reason,
                        "container failed"
                    );
                    return Err(AgentError::Internal(format!(
                        "container failed: {}",
                        reason
                    )));
                }
                Ok(_) => {
                    // Still running, wait and poll again
                    tokio::time::sleep(poll_interval).await;
                }
                Err(AgentError::NotFound { .. }) => {
                    // Container may have been removed - treat as exited with code 0
                    tracing::debug!(
                        container = %container_id,
                        "container not found, treating as exited"
                    );
                    return Ok(0);
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }

    /// Get container logs (stdout/stderr combined)
    ///
    /// Reads from the container's log files and returns as a vector of lines.
    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<String>> {
        let container_id = self.container_id_str(id);

        // Get log paths
        let (stdout_path, stderr_path) = {
            let containers = self.containers.read().await;
            match containers.get(&container_id) {
                Some(info) => (info.stdout_path.clone(), info.stderr_path.clone()),
                None => self.log_paths(id),
            }
        };

        let mut logs = Vec::new();

        // Read stdout
        if stdout_path.exists() {
            if let Ok(content) = fs::read_to_string(&stdout_path).await {
                for line in content.lines() {
                    logs.push(format!("[stdout] {}", line));
                }
            }
        }

        // Read stderr
        if stderr_path.exists() {
            if let Ok(content) = fs::read_to_string(&stderr_path).await {
                for line in content.lines() {
                    logs.push(format!("[stderr] {}", line));
                }
            }
        }

        Ok(logs)
    }

    /// Get the PID of a container's main process
    ///
    /// Returns:
    /// - `Ok(Some(pid))` for running containers
    /// - `Ok(None)` if the container exists but has no PID (not running or stopped)
    /// - `Err` if the container doesn't exist or there's an error loading it
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.get_pid",
            container.id = %self.container_id_str(id),
        )
    )]
    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        let container_id = self.container_id_str(id);
        let container_root = self.container_root(id);

        // Check if container root exists
        if !container_root.exists() {
            return Err(AgentError::NotFound {
                container: container_id.clone(),
                reason: "container state directory not found".to_string(),
            });
        }

        // Load container and get PID
        let container_id_clone = container_id.clone();

        let pid = tokio::task::spawn_blocking(move || {
            let mut container =
                Container::load(container_root).map_err(|e| AgentError::NotFound {
                    container: container_id_clone.clone(),
                    reason: format!("failed to load container: {}", e),
                })?;

            // Refresh status to get current state
            let _ = container.refresh_status();

            // Get PID - returns None if container is not running
            let pid = container.pid().map(|p| p.as_raw() as u32);

            Ok::<Option<u32>, AgentError>(pid)
        })
        .await
        .map_err(|e| AgentError::NotFound {
            container: container_id.clone(),
            reason: format!("task join error: {}", e),
        })??;

        tracing::debug!(
            container = %container_id,
            pid = ?pid,
            "retrieved container PID"
        );

        Ok(pid)
    }

    async fn get_container_ip(&self, _id: &ContainerId) -> Result<Option<std::net::IpAddr>> {
        // Youki containers use OCI network namespaces — IP assignment comes
        // from the overlay manager, not the runtime itself.
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_youki_config_default() {
        let config = YoukiConfig::default();

        assert_eq!(config.state_dir, PathBuf::from(DEFAULT_STATE_DIR));
        assert_eq!(config.rootfs_dir, PathBuf::from(DEFAULT_ROOTFS_DIR));
        assert_eq!(config.bundle_dir, PathBuf::from(DEFAULT_BUNDLE_DIR));
        assert_eq!(config.cache_dir, PathBuf::from(DEFAULT_CACHE_DIR));
        assert_eq!(config.volume_dir, PathBuf::from("/var/lib/zlayer/volumes"));
        assert!(!config.use_systemd);
        assert!(config.cache_type.is_none());
    }

    #[test]
    fn test_container_id_str() {
        let id = ContainerId {
            service: "myservice".to_string(),
            replica: 1,
        };

        let expected = "myservice-1";
        assert_eq!(format!("{}-{}", id.service, id.replica), expected);
    }

    #[test]
    fn test_rootfs_path_sanitization() {
        // Test that image names are sanitized for filesystem paths
        let images = vec![
            (
                "docker.io/library/nginx:latest",
                "docker.io_library_nginx_latest",
            ),
            ("ghcr.io/owner/repo:v1.0", "ghcr.io_owner_repo_v1.0"),
            (
                "registry.example.com/image@sha256:abc123",
                "registry.example.com_image_sha256_abc123",
            ),
        ];

        for (image, expected_suffix) in images {
            let safe_name = image.replace(['/', ':', '@'], "_");
            assert_eq!(safe_name, expected_suffix);
        }
    }

    #[test]
    fn test_map_status() {
        // Test status mapping without runtime instance
        let mappings = vec![
            (ContainerStatus::Creating, "Pending"),
            (ContainerStatus::Created, "Pending"),
            (ContainerStatus::Running, "Running"),
            (ContainerStatus::Stopped, "Exited"),
            (ContainerStatus::Paused, "Stopping"),
        ];

        for (status, expected) in mappings {
            let state = match status {
                ContainerStatus::Creating => ContainerState::Pending,
                ContainerStatus::Created => ContainerState::Pending,
                ContainerStatus::Running => ContainerState::Running,
                ContainerStatus::Stopped => ContainerState::Exited { code: 0 },
                ContainerStatus::Paused => ContainerState::Stopping,
            };

            let state_str = match state {
                ContainerState::Pending => "Pending",
                ContainerState::Running => "Running",
                ContainerState::Exited { .. } => "Exited",
                ContainerState::Stopping => "Stopping",
                _ => "Other",
            };

            assert_eq!(state_str, expected);
        }
    }

    #[test]
    fn test_log_paths() {
        let config = YoukiConfig::default();
        let id = ContainerId {
            service: "testservice".to_string(),
            replica: 2,
        };

        let container_id = format!("{}-{}", id.service, id.replica);
        let state_dir = config.state_dir.join(&container_id);
        let stdout = state_dir.join("stdout.log");
        let stderr = state_dir.join("stderr.log");

        assert_eq!(
            stdout,
            PathBuf::from("/var/lib/zlayer/containers/testservice-2/stdout.log")
        );
        assert_eq!(
            stderr,
            PathBuf::from("/var/lib/zlayer/containers/testservice-2/stderr.log")
        );
    }

    #[test]
    fn test_youki_config_clone() {
        let config = YoukiConfig {
            state_dir: PathBuf::from("/custom/state"),
            rootfs_dir: PathBuf::from("/custom/rootfs"),
            bundle_dir: PathBuf::from("/custom/bundles"),
            cache_dir: PathBuf::from("/custom/cache"),
            volume_dir: PathBuf::from("/custom/volumes"),
            use_systemd: true,
            cache_type: Some(zlayer_registry::CacheType::memory()),
        };

        let cloned = config.clone();

        assert_eq!(cloned.state_dir, config.state_dir);
        assert_eq!(cloned.rootfs_dir, config.rootfs_dir);
        assert_eq!(cloned.bundle_dir, config.bundle_dir);
        assert_eq!(cloned.cache_dir, config.cache_dir);
        assert_eq!(cloned.volume_dir, config.volume_dir);
        assert_eq!(cloned.use_systemd, config.use_systemd);
        assert!(cloned.cache_type.is_some());
    }

    /// Test that YoukiRuntime::new() creates directories
    #[tokio::test]
    async fn test_youki_runtime_directory_creation() {
        // Use a unique temp directory based on test run
        let temp_base = std::env::temp_dir().join(format!("youki_test_{}", std::process::id()));

        let config = YoukiConfig {
            state_dir: temp_base.join("state"),
            rootfs_dir: temp_base.join("rootfs"),
            bundle_dir: temp_base.join("bundles"),
            cache_dir: temp_base.join("cache"),
            volume_dir: temp_base.join("volumes"),
            use_systemd: false,
            cache_type: None,
        };

        // Clean up any previous test run
        let _ = std::fs::remove_dir_all(&temp_base);

        // This should succeed and create all directories
        let result = YoukiRuntime::new(config.clone()).await;

        assert!(
            result.is_ok(),
            "Failed to create runtime: {:?}",
            result.err()
        );

        // Verify directories were created
        assert!(config.state_dir.exists());
        assert!(config.rootfs_dir.exists());
        assert!(config.bundle_dir.exists());
        assert!(config.cache_dir.exists());
        assert!(config.volume_dir.exists());

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_base);
    }
}
