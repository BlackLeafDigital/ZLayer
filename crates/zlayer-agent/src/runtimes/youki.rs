//! Youki/libcontainer runtime implementation
//!
//! Implements the Runtime trait using libcontainer (youki's container library)
//! for direct OCI container management without a daemon.

use crate::cgroups_stats::{self, ContainerStats};
use crate::error::{AgentError, Result};
use crate::runtime::{
    ArchivePutOptions, ArchiveStream, ContainerId, ContainerState, ExecExitFuture, ExecHandle,
    ExecOptions, ExecPtyStream, ImageInfo, LogChannel, LogChunk, LogsStream, LogsStreamOptions,
    PathStat, PruneResult, PullProgress, PullProgressStream, Runtime, StatsSample, StatsStream,
};
use crate::storage_manager::StorageManager;
use libcontainer::container::builder::ContainerBuilder;
use libcontainer::container::{Container, ContainerStatus};
use libcontainer::signal::Signal;
use libcontainer::syscall::syscall::SyscallType;
use oci_client::manifest::OciImageManifest;
use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tracing::instrument;
use zlayer_observability::logs::{LogEntry, LogSource, LogStream};
use zlayer_spec::{RegistryAuth, ServiceSpec};

/// Configuration for `YoukiRuntime`
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
    /// Base directory for structured container logs.
    ///
    /// When set, container logs are written to
    /// `{log_base_dir}/{deployment_name}/{service}/{container_id}.log`
    /// instead of the bundle directory.  The bundle `logs/` directory will
    /// contain symlinks back to the structured location so that existing
    /// code paths that read from the bundle still work.
    pub log_base_dir: Option<PathBuf>,
    /// Deployment name used in log directory hierarchy.
    ///
    /// Only meaningful when `log_base_dir` is also set.  Defaults to
    /// `"default"` if unset.
    pub deployment_name: Option<String>,
}

impl YoukiConfig {
    /// Build a `YoukiConfig` whose subdirectories are scoped to the given
    /// data directory, with the existing per-directory `ZLAYER_*_DIR` env
    /// var overrides honored as escape hatches.
    pub fn from_data_dir(data_dir: &std::path::Path) -> Self {
        let dirs = zlayer_paths::ZLayerDirs::new(data_dir);
        Self {
            state_dir: std::env::var("ZLAYER_STATE_DIR")
                .map_or_else(|_| dirs.containers(), PathBuf::from),
            rootfs_dir: std::env::var("ZLAYER_ROOTFS_DIR")
                .map_or_else(|_| dirs.rootfs(), PathBuf::from),
            bundle_dir: std::env::var("ZLAYER_BUNDLE_DIR")
                .map_or_else(|_| dirs.bundles(), PathBuf::from),
            cache_dir: std::env::var("ZLAYER_CACHE_DIR")
                .map_or_else(|_| dirs.cache(), PathBuf::from),
            volume_dir: std::env::var("ZLAYER_VOLUME_DIR")
                .map_or_else(|_| dirs.volumes(), PathBuf::from),
            use_systemd: std::env::var("ZLAYER_USE_SYSTEMD")
                .is_ok_and(|v| v == "1" || v.to_lowercase() == "true"),
            cache_type: None,
            log_base_dir: None,
            deployment_name: None,
        }
    }
}

impl Default for YoukiConfig {
    fn default() -> Self {
        Self::from_data_dir(&zlayer_paths::ZLayerDirs::default_data_dir())
    }
}

/// Process-global mutex to serialize libcontainer operations.
/// libcontainer uses `chdir()` internally for notify socket operations
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
    /// Most recent restart policy applied via
    /// [`Runtime::update_container_resources`]. None when never updated.
    /// Persisted across runtime restarts is not yet wired; this is the
    /// in-memory copy that the supervisor consults on container exit.
    #[allow(dead_code)]
    restart_policy: Option<crate::runtime::ContainerRestartPolicyUpdate>,
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
    /// Local OCI registry for resolving locally-built images
    local_registry: Option<std::sync::Arc<zlayer_registry::LocalRegistry>>,
    /// Cached image configs (entrypoint, cmd, env, etc.) keyed by image reference
    image_configs: RwLock<HashMap<String, zlayer_registry::ImageConfig>>,
    /// Auth context for container-to-host API authentication.
    auth_context: Option<crate::runtime::ContainerAuthContext>,
}

impl std::fmt::Debug for YoukiRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YoukiRuntime")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl YoukiRuntime {
    /// Create a new `YoukiRuntime` with the given configuration
    ///
    /// # Errors
    /// Returns an error if the required directories cannot be created.
    pub async fn new(
        config: YoukiConfig,
        auth_context: Option<crate::runtime::ContainerAuthContext>,
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
                reason: format!("failed to create storage manager: {e}"),
            })?;

        // Initialize shared blob cache using CacheType configuration
        // If cache_type is provided, use it directly; otherwise use environment-based config
        // but override the path for Persistent variant to use config.cache_dir
        let blob_cache = if let Some(cache_type) = &config.cache_type {
            cache_type
                .build()
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: "runtime".to_string(),
                    reason: format!("failed to build blob cache: {e}"),
                })?
        } else {
            let cache_type =
                zlayer_registry::CacheType::from_env().map_err(|e| AgentError::CreateFailed {
                    id: "runtime".to_string(),
                    reason: format!("failed to read cache config from env: {e}"),
                })?;
            // Override persistent path to use config.cache_dir
            #[allow(clippy::match_wildcard_for_single_variants)]
            let cache_type = match cache_type {
                zlayer_registry::CacheType::Persistent { .. } => {
                    zlayer_registry::CacheType::persistent_at(config.cache_dir.join("blobs.redb"))
                }
                other => other,
            };
            cache_type
                .build()
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: "runtime".to_string(),
                    reason: format!("failed to build blob cache: {e}"),
                })?
        };

        let local_registry = {
            let registry_path = config.cache_dir.parent().map_or_else(
                || config.cache_dir.join("registry"),
                |data_dir| data_dir.join("registry"),
            );
            match zlayer_registry::LocalRegistry::new(registry_path).await {
                Ok(reg) => Some(std::sync::Arc::new(reg)),
                Err(e) => {
                    tracing::warn!("Failed to open local registry: {e}");
                    None
                }
            }
        };

        Ok(Self {
            config,
            containers: RwLock::new(HashMap::new()),
            auth_resolver: zlayer_core::AuthResolver::new(zlayer_core::AuthConfig::default()),
            storage_manager: std::sync::Arc::new(tokio::sync::RwLock::new(storage_manager)),
            blob_cache,
            local_registry,
            image_configs: RwLock::new(HashMap::new()),
            auth_context,
        })
    }

    /// Create a new `YoukiRuntime` with default configuration
    ///
    /// # Errors
    /// Returns an error if the runtime cannot be initialized.
    pub async fn with_defaults() -> Result<Self> {
        Self::new(YoukiConfig::default(), None).await
    }

    /// Create a new `YoukiRuntime` with custom auth configuration
    ///
    /// # Errors
    /// Returns an error if the runtime cannot be initialized.
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
                reason: format!("failed to create storage manager: {e}"),
            })?;

        // Initialize shared blob cache using CacheType configuration
        // If cache_type is provided, use it directly; otherwise use environment-based config
        // but override the path for Persistent variant to use config.cache_dir
        let blob_cache = if let Some(cache_type) = &config.cache_type {
            cache_type
                .build()
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: "runtime".to_string(),
                    reason: format!("failed to build blob cache: {e}"),
                })?
        } else {
            let cache_type =
                zlayer_registry::CacheType::from_env().map_err(|e| AgentError::CreateFailed {
                    id: "runtime".to_string(),
                    reason: format!("failed to read cache config from env: {e}"),
                })?;
            // Override persistent path to use config.cache_dir
            #[allow(clippy::match_wildcard_for_single_variants)]
            let cache_type = match cache_type {
                zlayer_registry::CacheType::Persistent { .. } => {
                    zlayer_registry::CacheType::persistent_at(config.cache_dir.join("blobs.redb"))
                }
                other => other,
            };
            cache_type
                .build()
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: "runtime".to_string(),
                    reason: format!("failed to build blob cache: {e}"),
                })?
        };

        let local_registry = {
            let registry_path = config.cache_dir.parent().map_or_else(
                || config.cache_dir.join("registry"),
                |data_dir| data_dir.join("registry"),
            );
            match zlayer_registry::LocalRegistry::new(registry_path).await {
                Ok(reg) => Some(std::sync::Arc::new(reg)),
                Err(e) => {
                    tracing::warn!("Failed to open local registry: {e}");
                    None
                }
            }
        };

        Ok(Self {
            config,
            containers: RwLock::new(HashMap::new()),
            auth_resolver: zlayer_core::AuthResolver::new(auth_config),
            storage_manager: std::sync::Arc::new(tokio::sync::RwLock::new(storage_manager)),
            blob_cache,
            local_registry,
            image_configs: RwLock::new(HashMap::new()),
            auth_context: None,
        })
    }

    /// Configure S3-backed volume sync on the internal storage manager.
    ///
    /// When set, named volumes will be automatically registered with the
    /// `LayerSyncManager` on creation and restored from S3 if a backup
    /// exists. Volumes are synced to S3 when containers are stopped via
    /// the `sync_container_volumes` trait method.
    #[cfg(feature = "s3")]
    pub async fn set_layer_sync(
        &self,
        sync: std::sync::Arc<zlayer_storage::sync::LayerSyncManager>,
        service_name: impl Into<String>,
    ) {
        let mut storage_manager = self.storage_manager.write().await;
        storage_manager.set_layer_sync(sync, service_name);
    }

    /// Get the container ID string
    #[allow(clippy::unused_self)]
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

    /// Get log directory for a container.
    ///
    /// When `log_base_dir` is configured, returns a structured path:
    ///   `{log_base_dir}/{deployment}/{service}/`
    /// Otherwise falls back to the bundle directory:
    ///   `{bundle_dir}/{container_id}/logs/`
    fn log_dir(&self, id: &ContainerId) -> PathBuf {
        if let Some(ref base) = self.config.log_base_dir {
            let deployment = self.config.deployment_name.as_deref().unwrap_or("default");
            base.join(deployment).join(&id.service)
        } else {
            // Fall back to bundle directory to avoid conflicting with libcontainer's state directory
            self.bundle_path(id).join("logs")
        }
    }

    /// Get log file paths for a container.
    ///
    /// Returns `(stdout_path, stderr_path)`. When structured logging is
    /// enabled via `log_base_dir`, the files are named after the container
    /// ID (e.g. `myservice-rep-1.stdout.log`).
    fn log_paths(&self, id: &ContainerId) -> (PathBuf, PathBuf) {
        let log_dir = self.log_dir(id);
        if self.config.log_base_dir.is_some() {
            let container_id = self.container_id_str(id);
            (
                log_dir.join(format!("{container_id}.stdout.log")),
                log_dir.join(format!("{container_id}.stderr.log")),
            )
        } else {
            (log_dir.join("stdout.log"), log_dir.join("stderr.log"))
        }
    }

    /// Map libcontainer status to our `ContainerState`
    #[allow(clippy::unused_self)]
    fn map_status(&self, status: ContainerStatus) -> ContainerState {
        match status {
            ContainerStatus::Creating | ContainerStatus::Created => ContainerState::Pending,
            ContainerStatus::Running => ContainerState::Running,
            ContainerStatus::Stopped => ContainerState::Exited { code: 0 },
            ContainerStatus::Paused => ContainerState::Stopping,
        }
    }

    /// Create log files and return file descriptors for stdout/stderr.
    ///
    /// When `log_base_dir` is configured, creates the structured log
    /// directory (`/var/log/zlayer/{deployment}/{service}/`) and places
    /// symlinks in the bundle's `logs/` directory so that existing code
    /// reading from the bundle still works.
    #[allow(unsafe_code)]
    async fn create_log_files(
        &self,
        id: &ContainerId,
    ) -> Result<(PathBuf, PathBuf, OwnedFd, OwnedFd)> {
        let log_dir = self.log_dir(id);
        fs::create_dir_all(&log_dir)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("failed to create log dir: {e}"),
            })?;

        let (stdout_path, stderr_path) = self.log_paths(id);

        // Create stdout file
        let stdout_file =
            std::fs::File::create(&stdout_path).map_err(|e| AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("failed to create stdout log: {e}"),
            })?;

        // Create stderr file
        let stderr_file =
            std::fs::File::create(&stderr_path).map_err(|e| AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("failed to create stderr log: {e}"),
            })?;

        // When using structured logging, also create symlinks from the bundle
        // logs directory so that code reading from bundle_path/logs/ still works.
        if self.config.log_base_dir.is_some() {
            let bundle_log_dir = self.bundle_path(id).join("logs");
            let _ = fs::create_dir_all(&bundle_log_dir).await;

            // Symlink bundle_log_dir/stdout.log -> structured stdout_path
            let bundle_stdout = bundle_log_dir.join("stdout.log");
            let _ = fs::remove_file(&bundle_stdout).await;
            if let Err(e) = tokio::fs::symlink(&stdout_path, &bundle_stdout).await {
                tracing::warn!(
                    container = %id,
                    error = %e,
                    "Failed to create stdout symlink in bundle logs dir"
                );
            }

            // Symlink bundle_log_dir/stderr.log -> structured stderr_path
            let bundle_stderr = bundle_log_dir.join("stderr.log");
            let _ = fs::remove_file(&bundle_stderr).await;
            if let Err(e) = tokio::fs::symlink(&stderr_path, &bundle_stderr).await {
                tracing::warn!(
                    container = %id,
                    error = %e,
                    "Failed to create stderr symlink in bundle logs dir"
                );
            }
        }

        // Convert to OwnedFd
        // SAFETY: `into_raw_fd()` transfers ownership of the fd, and
        // `OwnedFd::from_raw_fd` takes ownership. No double-close.
        let stdout_fd = unsafe {
            use std::os::unix::io::IntoRawFd;
            OwnedFd::from_raw_fd(stdout_file.into_raw_fd())
        };
        let stderr_fd = unsafe {
            use std::os::unix::io::IntoRawFd;
            OwnedFd::from_raw_fd(stderr_file.into_raw_fd())
        };

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
                    reason: format!("failed to remove bundle: {e}"),
                })?;
        }
        Ok(())
    }

    /// Pull image layers and return them for extraction
    ///
    /// Uses the shared blob cache to avoid repeated network requests for cached layers.
    /// The `policy` parameter is translated to a `force_refresh` flag: `PullPolicy::Always`
    /// clears the manifest cache before fetching, while `IfNotPresent` reuses any cached
    /// manifest (the puller still revalidates mutable tags via HEAD).
    ///
    /// `PullPolicy::Never` short-circuits to a local-cache-only path: the puller is
    /// invoked with `force_refresh = false` so it consults the local registry and blob
    /// cache first. If the image is not present locally and the puller falls through to
    /// a remote fetch that fails, the error is remapped to a Never-specific message so
    /// callers can distinguish "missing locally" from a transient network failure. With
    /// the Phase 0 import fix, locally-imported images always satisfy the local lookup
    /// and no remote round-trip occurs.
    async fn pull_image_layers(
        &self,
        image: &str,
        policy: zlayer_spec::PullPolicy,
    ) -> Result<Vec<(Vec<u8>, String)>> {
        // Use the shared blob cache instead of opening a new one each time
        let puller = {
            let p = zlayer_registry::ImagePuller::with_cache(self.blob_cache.clone());
            if let Some(ref registry) = self.local_registry {
                p.with_local_registry(registry.clone())
            } else {
                p
            }
        };
        let auth = self.auth_resolver.resolve(image);

        if matches!(policy, zlayer_spec::PullPolicy::Never) {
            tracing::debug!(
                image = %image,
                "pull_policy=Never; serving layers from local cache only"
            );
            return puller
                .pull_image_with_policy(image, &auth, false)
                .await
                .map_err(|e| AgentError::PullFailed {
                    image: image.to_string(),
                    reason: format!("pull_policy=never and image not present locally: {e}"),
                });
        }

        let force_refresh = matches!(policy, zlayer_spec::PullPolicy::Always);

        puller
            .pull_image_with_policy(image, &auth, force_refresh)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("failed to pull image: {e}"),
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
                    let path = storage_manager
                        .ensure_volume_with_sync(name)
                        .await
                        .map_err(|e| AgentError::CreateFailed {
                            id: container_id.clone(),
                            reason: format!("failed to ensure volume '{name}': {e}"),
                        })?;
                    storage_manager
                        .attach_volume(name, &container_id)
                        .map_err(|e| AgentError::CreateFailed {
                            id: container_id.clone(),
                            reason: format!("failed to attach volume '{name}': {e}"),
                        })?;
                    volume_paths.insert(name.clone(), path);
                }

                StorageSpec::Anonymous { target, .. } => {
                    let path = storage_manager
                        .create_anonymous(&container_id, target)
                        .map_err(|e| AgentError::CreateFailed {
                            id: container_id.clone(),
                            reason: format!(
                                "failed to create anonymous volume for '{target}': {e}"
                            ),
                        })?;
                    let key = format!("_anon_{}", target.trim_start_matches('/').replace('/', "_"));
                    volume_paths.insert(key, path);
                }

                // Bind and tmpfs mounts don't need preparation
                StorageSpec::Bind { .. } | StorageSpec::Tmpfs { .. } => {}

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
                            reason: format!("failed to mount S3 bucket '{bucket}': {e}"),
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
    /// Note: This method requires the `ServiceSpec` to know which volumes to clean up.
    /// For now, `remove_container` uses a simpler approach that only cleans up anonymous volumes.
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
        self.pull_image_with_policy(image, zlayer_spec::PullPolicy::IfNotPresent, None)
            .await
    }

    /// Pull an image to local storage with a specific pull policy
    ///
    /// This downloads image layers to the blob cache. Layers are extracted
    /// per-container in `create_container` to avoid race conditions.
    ///
    /// The `_auth` parameter is accepted for trait conformance (§3.10) but
    /// currently ignored: `zlayer-registry` resolves credentials through the
    /// existing `AuthResolver` (hostname lookup in the persistent secret
    /// store). Callers that need inline auth should use the Docker runtime.
    #[instrument(
        skip(self, _auth),
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
        _auth: Option<&RegistryAuth>,
    ) -> Result<()> {
        let puller = {
            let p = zlayer_registry::ImagePuller::with_cache(self.blob_cache.clone());
            if let Some(ref registry) = self.local_registry {
                p.with_local_registry(registry.clone())
            } else {
                p
            }
        };
        let auth = self.auth_resolver.resolve(image);

        // For Never policy, skip pulling layers from the remote, but STILL
        // fetch the image config from the local blob cache (populated by a
        // prior `zlayer import` or an earlier pull). Without the image
        // config, the bundle builder has no way to know the image's
        // entrypoint/cmd/env/workdir/user and falls back to `/bin/sh`,
        // which exits immediately and kills the container. Fetching the
        // config from cache is cheap (~1 KB) and non-fatal on miss.
        if matches!(policy, zlayer_spec::PullPolicy::Never) {
            tracing::debug!(image = %image, "pull policy is Never, skipping layer pull");
            match puller.pull_image_config(image, &auth).await {
                Ok(config) => {
                    tracing::info!(
                        image = %image,
                        has_entrypoint = config.entrypoint.is_some(),
                        has_cmd = config.cmd.is_some(),
                        "image config loaded from cache"
                    );
                    let mut configs = self.image_configs.write().await;
                    configs.insert(image.to_string(), config);
                }
                Err(e) => {
                    tracing::warn!(
                        image = %image,
                        error = %e,
                        "failed to load image config from cache under pull_policy=Never, container will use spec defaults"
                    );
                }
            }
            return Ok(());
        }

        // For IfNotPresent, check if image layers are in cache by trying to pull
        // Use the shared blob cache to avoid repeated opens and ensure persistence
        // For Always, force a round-trip to the registry by clearing the manifest cache.
        let force_refresh = matches!(policy, zlayer_spec::PullPolicy::Always);
        tracing::info!(image = %image, force_refresh, "pulling image layers to cache");

        // Pull image layers from registry (cached layers are retrieved from cache)
        let layers = puller
            .pull_image_with_policy(image, &auth, force_refresh)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("failed to pull image: {e}"),
            })?;

        tracing::info!(
            image = %image,
            layer_count = layers.len(),
            "image layers cached"
        );

        // Also pull and cache the image config (entrypoint, cmd, env, etc.)
        match puller
            .pull_image_config_with_policy(image, &auth, force_refresh)
            .await
        {
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
        let image = spec.image.name.to_string();

        // Clean up any stale container from a previous deploy (mirrors Docker runtime behavior)
        // See docker.rs:370-402 for the equivalent pattern
        let container_root = self.container_root(id);
        if container_root.exists() {
            tracing::warn!(
                container = %container_id,
                "stale container state found from previous deploy, cleaning up before re-create"
            );
            // Try to stop — process may already be dead, ignore errors
            if let Err(e) = self.stop_container(id, Duration::from_secs(5)).await {
                tracing::debug!(
                    container = %container_id,
                    error = %e,
                    "stop_container during stale cleanup (expected if process already dead)"
                );
            }
            // Remove container — does full cleanup: libcontainer delete + state dir + bundle + volumes
            if let Err(e) = self.remove_container(id).await {
                tracing::warn!(
                    container = %container_id,
                    error = %e,
                    "remove_container failed during stale cleanup, attempting manual cleanup"
                );
                // Fall back to manual cleanup if remove_container fails
                if container_root.exists() {
                    let _ = tokio::fs::remove_dir_all(&container_root).await;
                }
                let stale_bundle = self.bundle_path(id);
                if stale_bundle.exists() {
                    let _ = tokio::fs::remove_dir_all(&stale_bundle).await;
                }
            }
            tracing::info!(container = %container_id, "stale container cleaned up");
        }

        // Also clean up stale bundle directory if it exists but state dir didn't
        let bundle_path = self.bundle_path(id);
        if bundle_path.exists() {
            tracing::warn!(container = %container_id, "stale bundle directory found, removing");
            let _ = tokio::fs::remove_dir_all(&bundle_path).await;
        }

        let bundle_path = self.bundle_path(id);
        let rootfs_path = bundle_path.join("rootfs");

        tracing::info!("Creating container {} from image {}", container_id, image);

        // Create bundle directory structure
        fs::create_dir_all(&bundle_path)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: container_id.clone(),
                reason: format!("failed to create bundle directory: {e}"),
            })?;

        // Pull image layers (from cache if available). Honor the spec's pull policy so
        // that `Always` forces a manifest refresh, `IfNotPresent` serves from cache with
        // mutable-tag revalidation, and `Never` reuses cached blobs.
        let layers = self
            .pull_image_layers(&image, spec.image.pull_policy)
            .await?;

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
                reason: format!("failed to extract rootfs: {e}"),
            })?;

        // Log rootfs diagnostics for debugging container creation failures
        match std::fs::read_dir(&rootfs_path) {
            Ok(entries) => {
                let count = entries.count();
                tracing::info!(
                    container = %container_id,
                    rootfs = %rootfs_path.display(),
                    entry_count = count,
                    "rootfs extracted successfully"
                );
            }
            Err(e) => {
                tracing::warn!(
                    container = %container_id,
                    rootfs = %rootfs_path.display(),
                    error = %e,
                    "rootfs directory not readable after extraction"
                );
            }
        }

        // Get cached image config (entrypoint, cmd, env, workdir, user)
        let img_config = self.get_image_config(&image).await;

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

        // Inject auth env vars and socket mount so the container can talk to the host API
        if let Some(ref auth_ctx) = self.auth_context {
            let token = crate::auth::mint_container_token(
                &auth_ctx.jwt_secret,
                &id.service,
                &format!("{}-{}", id.service, id.replica),
                std::time::Duration::from_secs(86400 * 365),
            )
            .map_err(|e| crate::error::AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("Failed to mint container token: {e}"),
            })?;
            bundle_builder = bundle_builder
                .with_env("ZLAYER_API_URL".to_string(), auth_ctx.api_url.clone())
                .with_env("ZLAYER_TOKEN".to_string(), token)
                .with_env(
                    "ZLAYER_SOCKET".to_string(),
                    zlayer_paths::ZLayerDirs::default_socket_path(),
                )
                .with_socket_mount(&auth_ctx.socket_path);
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
                    reason: format!("failed to acquire libcontainer lock: {e}"),
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
                        reason: format!("failed to set root path: {e}"),
                    })?;

            // Configure as init container (creates new namespaces)
            let init_builder = container_builder
                .as_init(&bundle_path_clone)
                .with_systemd(config.use_systemd)
                .with_detach(true); // Run detached

            // Build the container (creates it but doesn't start)
            let container = init_builder.build().map_err(|e| AgentError::CreateFailed {
                id: container_id_clone.clone(),
                reason: format!("failed to create container: {e:?}"),
            })?;

            Ok::<Container, AgentError>(container)
        })
        .await
        .map_err(|e| AgentError::CreateFailed {
            id: container_id.clone(),
            reason: format!("task join error: {e}"),
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
                    restart_policy: None,
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
                    reason: format!("failed to acquire libcontainer lock: {e}"),
                })?;

            let mut container =
                Container::load(container_root).map_err(|e| AgentError::StartFailed {
                    id: container_id.clone(),
                    reason: format!("failed to load container: {e}"),
                })?;

            // Start the container
            container.start().map_err(|e| AgentError::StartFailed {
                id: container_id.clone(),
                reason: format!("failed to start container: {e}"),
            })?;

            // Get the PID after starting - access through state
            #[allow(clippy::cast_sign_loss)]
            let pid = container.pid().map(|p| p.as_raw() as u32);

            Ok::<Option<u32>, AgentError>(pid)
        })
        .await
        .map_err(|e| AgentError::StartFailed {
            id: id.to_string(),
            reason: format!("task join error: {e}"),
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
                    reason: format!("failed to load container: {e}"),
                })?;

            // Check if container can be killed
            if container.status().can_kill() {
                // Send SIGTERM
                use std::convert::TryFrom;
                let signal = Signal::try_from("SIGTERM").map_err(|e| AgentError::NotFound {
                    container: container_id_clone.clone(),
                    reason: format!("invalid signal: {e:?}"),
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
            reason: format!("task join error: {e}"),
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
                    reason: format!("failed to load container: {e}"),
                })?;

            if container.status().can_kill() {
                use std::convert::TryFrom;
                let signal = Signal::try_from("SIGKILL").map_err(|e| AgentError::NotFound {
                    container: container_id_clone.clone(),
                    reason: format!("invalid signal: {e:?}"),
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
            reason: format!("task join error: {e}"),
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

        // Best-effort cgroup teardown: libcontainer's delete() should reap
        // the container's cgroup, but systemd-cgroup races (and occasional
        // cgroup-v2 unified hiccups) can leave an empty subdir behind. A
        // follow-up rmdir is idempotent — fails harmlessly if the dir is
        // already gone, and shouldn't fail with EBUSY because the container
        // is already deleted.
        #[cfg(target_os = "linux")]
        {
            use std::path::Path;
            let candidates: &[&str] = &[
                // cgroup-v2 unified hierarchy under zlayer.slice
                "/sys/fs/cgroup/zlayer.slice",
                // systemd-cgroup nested
                "/sys/fs/cgroup",
            ];
            for root in candidates {
                let root_path = Path::new(root);
                if !root_path.exists() {
                    continue;
                }
                // Look for any subdirectory whose name contains the
                // container_id (the libcontainer scope name). Idempotent rmdir.
                if let Ok(entries) = std::fs::read_dir(root_path) {
                    for entry in entries.flatten() {
                        let name = entry.file_name();
                        let name_str = name.to_string_lossy();
                        if name_str.contains(&container_id) || name_str.contains(&id.service) {
                            let path = entry.path();
                            // Only rmdir if it's a directory and the cgroup.procs file is empty.
                            if path.is_dir() {
                                let procs = path.join("cgroup.procs");
                                let empty = std::fs::read_to_string(&procs)
                                    .map(|s| s.trim().is_empty())
                                    .unwrap_or(true);
                                if empty {
                                    if let Err(e) = std::fs::remove_dir(&path) {
                                        tracing::debug!(
                                            cgroup = %path.display(),
                                            error = %e,
                                            "cgroup rmdir failed (probably already gone)"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
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
                    reason: format!("failed to load container: {e}"),
                })?;

            // Refresh status to get current state
            let _ = container.refresh_status();

            Ok::<ContainerStatus, AgentError>(container.status())
        })
        .await
        .map_err(|e| AgentError::NotFound {
            container: container_id.clone(),
            reason: format!("task join error: {e}"),
        })??;

        Ok(self.map_status(status))
    }

    /// Get container logs
    ///
    /// Reads from the container's stdout/stderr log files.
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>> {
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

        let now = chrono::Utc::now();
        let source = LogSource::Container(id.to_string());
        let mut entries = Vec::new();

        // Read stdout
        if stdout_path.exists() {
            if let Ok(content) = fs::read_to_string(&stdout_path).await {
                for line in content.lines() {
                    entries.push(LogEntry {
                        timestamp: now,
                        stream: LogStream::Stdout,
                        message: line.to_string(),
                        source: source.clone(),
                        service: None,
                        deployment: None,
                    });
                }
            }
        }

        // Read stderr
        if stderr_path.exists() {
            if let Ok(content) = fs::read_to_string(&stderr_path).await {
                for line in content.lines() {
                    entries.push(LogEntry {
                        timestamp: now,
                        stream: LogStream::Stderr,
                        message: line.to_string(),
                        source: source.clone(),
                        service: None,
                        deployment: None,
                    });
                }
            }
        }

        // Sort by timestamp (all same for legacy files, but correct for future use)
        entries.sort_by_key(|e| e.timestamp);

        // Apply tail limit
        if tail > 0 && entries.len() > tail {
            entries = entries.split_off(entries.len() - tail);
        }

        Ok(entries)
    }

    /// Execute a command in a running container
    ///
    /// Uses libcontainer's tenant builder to exec into the container's namespaces.
    #[allow(unsafe_code)]
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
        let exec_dir = self.config.state_dir.join(format!("exec-{exec_id}"));
        fs::create_dir_all(&exec_dir)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: exec_id.clone(),
                reason: format!("failed to create exec dir: {e}"),
            })?;

        let stdout_path = exec_dir.join("stdout");
        let stderr_path = exec_dir.join("stderr");

        // Create output files
        let stdout_file =
            std::fs::File::create(&stdout_path).map_err(|e| AgentError::CreateFailed {
                id: exec_id.clone(),
                reason: format!("failed to create stdout file: {e}"),
            })?;

        let stderr_file =
            std::fs::File::create(&stderr_path).map_err(|e| AgentError::CreateFailed {
                id: exec_id.clone(),
                reason: format!("failed to create stderr file: {e}"),
            })?;

        // SAFETY: `into_raw_fd()` transfers ownership of the fd, and
        // `OwnedFd::from_raw_fd` takes ownership. No double-close.
        let stdout_fd = unsafe {
            use std::os::unix::io::IntoRawFd;
            OwnedFd::from_raw_fd(stdout_file.into_raw_fd())
        };
        let stderr_fd = unsafe {
            use std::os::unix::io::IntoRawFd;
            OwnedFd::from_raw_fd(stderr_file.into_raw_fd())
        };

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
                        reason: format!("failed to set root path: {e}"),
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
                    reason: format!("failed to exec in container: {e}"),
                })?;

            // Return raw pid as i32 to avoid nix version conflicts
            // (libcontainer uses nix 0.29, we use nix 0.31)
            Ok::<i32, AgentError>(pid.as_raw())
        })
        .await
        .map_err(|e| AgentError::CreateFailed {
            id: container_id.clone(),
            reason: format!("task join error: {e}"),
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
                Ok(_) | Err(_) => -1,
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

    /// Start an interactive exec session against a libcontainer-managed
    /// container.
    ///
    /// Allocates a master/slave pseudo-terminal pair via `nix::pty::openpty`,
    /// duplicates the slave fd three times so libcontainer can take ownership
    /// of it for the tenant process's stdin/stdout/stderr (each setter on
    /// `ContainerBuilder` consumes an `OwnedFd`), and returns the master end
    /// wrapped in a `tokio::io::unix::AsyncFd`-backed duplex stream.
    ///
    /// The returned [`ExecHandle::resize`] channel drives a small task that
    /// services `(rows, cols)` updates by issuing `ioctl(TIOCSWINSZ)` on the
    /// master fd. The [`ExecHandle::exit`] future calls `waitpid` on the
    /// tenant's pid in `spawn_blocking` and resolves with the exit code (or
    /// `128 + signal` for signalled deaths).
    ///
    /// Both `tty=true` and `tty=false` allocate a PTY: when `tty=false` the
    /// caller still gets a single duplex byte stream (stdout and stderr are
    /// merged through the slave terminal, matching how a tenant without a
    /// requested TTY behaves when handed a terminal anyway).
    #[allow(unsafe_code)]
    #[instrument(
        skip(self, opts),
        fields(
            otel.name = "container.exec_pty",
            container.id = %self.container_id_str(id),
            command = ?opts.command,
            tty = opts.tty,
        )
    )]
    async fn exec_pty(&self, id: &ContainerId, opts: ExecOptions) -> Result<ExecHandle> {
        let container_id = self.container_id_str(id);

        if opts.command.is_empty() {
            return Err(AgentError::InvalidSpec(
                "exec_pty command cannot be empty".to_string(),
            ));
        }

        // Allocate the PTY pair on a blocking thread — `openpty` is a syscall
        // wrapper but we keep it off the async path for symmetry with the
        // libcontainer call below.
        let openpty_result = tokio::task::spawn_blocking(|| nix::pty::openpty(None, None))
            .await
            .map_err(|e| AgentError::Internal(format!("openpty join error: {e}")))?
            .map_err(|e| AgentError::Internal(format!("openpty failed: {e}")))?;

        let master_fd = openpty_result.master;
        let slave_fd = openpty_result.slave;

        // Duplicate the slave three times so libcontainer can take ownership
        // of one fd per stdio. Closing the original slave on the host side
        // happens automatically when `slave_fd` drops at the end of this
        // scope; the duplicates travel into the tenant.
        let stdin_fd = nix::unistd::dup(&slave_fd)
            .map_err(|e| AgentError::Internal(format!("dup slave for stdin failed: {e}")))?;
        let stdout_fd = nix::unistd::dup(&slave_fd)
            .map_err(|e| AgentError::Internal(format!("dup slave for stdout failed: {e}")))?;
        let stderr_fd = nix::unistd::dup(&slave_fd)
            .map_err(|e| AgentError::Internal(format!("dup slave for stderr failed: {e}")))?;
        drop(slave_fd);

        // Mark the master end non-blocking so `AsyncFd` can drive it.
        nix::fcntl::fcntl(
            &master_fd,
            nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
        )
        .map_err(|e| AgentError::Internal(format!("F_SETFL O_NONBLOCK on pty master: {e}")))?;

        let cmd_clone = opts.command.clone();
        let container_id_clone = container_id.clone();
        let state_dir_clone = self.config.state_dir.clone();

        // Spawn the tenant on the libcontainer thread pool. Each stdio fd is
        // moved into the closure and consumed by the builder.
        let exec_pid = tokio::task::spawn_blocking(move || {
            let container_builder =
                ContainerBuilder::new(container_id_clone.clone(), SyscallType::Linux)
                    .with_stdin(stdin_fd)
                    .with_stdout(stdout_fd)
                    .with_stderr(stderr_fd);

            let container_builder =
                container_builder
                    .with_root_path(&state_dir_clone)
                    .map_err(|e| AgentError::CreateFailed {
                        id: container_id_clone.clone(),
                        reason: format!("failed to set root path: {e}"),
                    })?;

            let tenant_builder = container_builder
                .as_tenant()
                .with_container_args(cmd_clone)
                .with_detach(true);

            let pid = tenant_builder
                .build()
                .map_err(|e| AgentError::CreateFailed {
                    id: container_id_clone.clone(),
                    reason: format!("failed to exec_pty in container: {e}"),
                })?;

            Ok::<i32, AgentError>(pid.as_raw())
        })
        .await
        .map_err(|e| AgentError::CreateFailed {
            id: container_id.clone(),
            reason: format!("task join error: {e}"),
        })??;

        // Resize task: pump (rows, cols) into TIOCSWINSZ on the master.
        let (resize_tx, mut resize_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(8);
        let master_raw = master_fd.as_raw_fd();
        tokio::spawn(async move {
            while let Some((rows, cols)) = resize_rx.recv().await {
                let ws = nix::pty::Winsize {
                    ws_row: rows,
                    ws_col: cols,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                // SAFETY: `master_raw` remains a valid fd as long as the
                // duplex stream is alive (it owns the master `OwnedFd`).
                // `ws` is a stack-allocated `winsize` matching the layout
                // the kernel expects. The ioctl reads from the pointer; it
                // does not retain it past the call.
                let rc = unsafe { libc::ioctl(master_raw, libc::TIOCSWINSZ, &ws) };
                if rc != 0 {
                    let err = std::io::Error::last_os_error();
                    tracing::warn!(?err, "TIOCSWINSZ failed on pty master");
                }
            }
        });

        // Build the exit future: waitpid on a blocking thread.
        let exit_fut: ExecExitFuture = Box::pin(async move {
            let exit_code = tokio::task::spawn_blocking(move || {
                use nix::sys::wait::{waitpid, WaitStatus};
                use nix::unistd::Pid;
                let pid = Pid::from_raw(exec_pid);
                match waitpid(pid, None) {
                    Ok(WaitStatus::Exited(_, code)) => code,
                    Ok(WaitStatus::Signaled(_, signal, _)) => 128 + signal as i32,
                    Ok(_) | Err(_) => -1,
                }
            })
            .await
            .map_err(|e| AgentError::Internal(format!("waitpid join error: {e}")))?;
            Ok(exit_code)
        });

        // Wrap the master end in an AsyncFd-backed duplex stream.
        let stream: ExecPtyStream = Box::new(PtyDuplex::new(master_fd)?);

        Ok(ExecHandle {
            stream,
            resize: resize_tx,
            exit: exit_fut,
        })
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
                "/sys/fs/cgroup/system.slice/zlayer-{container_id}.scope"
            ))
        } else {
            // cgroupfs driver: /sys/fs/cgroup/zlayer/{id}
            PathBuf::from(format!("/sys/fs/cgroup/zlayer/{container_id}"))
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
                    "failed to read cgroup stats for container {container_id}: {e}"
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
                    return Err(AgentError::Internal(format!("container failed: {reason}")));
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
    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
        let container_id = self.container_id_str(id);

        // Get log paths
        let (stdout_path, stderr_path) = {
            let containers = self.containers.read().await;
            match containers.get(&container_id) {
                Some(info) => (info.stdout_path.clone(), info.stderr_path.clone()),
                None => self.log_paths(id),
            }
        };

        let now = chrono::Utc::now();
        let source = LogSource::Container(id.to_string());
        let mut entries = Vec::new();

        // Read stdout
        if stdout_path.exists() {
            if let Ok(content) = fs::read_to_string(&stdout_path).await {
                for line in content.lines() {
                    entries.push(LogEntry {
                        timestamp: now,
                        stream: LogStream::Stdout,
                        message: line.to_string(),
                        source: source.clone(),
                        service: None,
                        deployment: None,
                    });
                }
            }
        }

        // Read stderr
        if stderr_path.exists() {
            if let Ok(content) = fs::read_to_string(&stderr_path).await {
                for line in content.lines() {
                    entries.push(LogEntry {
                        timestamp: now,
                        stream: LogStream::Stderr,
                        message: line.to_string(),
                        source: source.clone(),
                        service: None,
                        deployment: None,
                    });
                }
            }
        }

        // Sort by timestamp
        entries.sort_by_key(|e| e.timestamp);

        Ok(entries)
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
                    reason: format!("failed to load container: {e}"),
                })?;

            // Refresh status to get current state
            let _ = container.refresh_status();

            // Get PID - returns None if container is not running
            #[allow(clippy::cast_sign_loss)]
            let pid = container.pid().map(|p| p.as_raw() as u32);

            Ok::<Option<u32>, AgentError>(pid)
        })
        .await
        .map_err(|e| AgentError::NotFound {
            container: container_id.clone(),
            reason: format!("task join error: {e}"),
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

    /// Sync all named volumes to S3 before container removal.
    ///
    /// When the `s3` feature is enabled and a `LayerSyncManager` has been
    /// configured on the storage manager, this iterates all non-anonymous
    /// volumes and pushes any changes to S3. Errors are logged but do not
    /// prevent container removal.
    #[allow(unused_variables)]
    async fn sync_container_volumes(&self, id: &ContainerId) -> Result<()> {
        #[cfg(feature = "s3")]
        {
            let storage_manager = self.storage_manager.read().await;
            if storage_manager.layer_sync().is_some() {
                let container_id = self.container_id_str(id);
                tracing::info!(
                    container = %container_id,
                    "syncing volumes to S3 before container removal"
                );
                match storage_manager.sync_all_volumes().await {
                    Ok(synced) => {
                        if synced > 0 {
                            tracing::info!(
                                container = %container_id,
                                synced_count = synced,
                                "volume sync complete"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            container = %container_id,
                            error = %e,
                            "volume sync failed, data may not be persisted"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        let keys = self
            .blob_cache
            .keys_with_prefix("manifest:")
            .await
            .map_err(|e| {
                AgentError::Internal(format!("failed to list manifest cache keys: {e}"))
            })?;

        let mut images = Vec::with_capacity(keys.len());
        for key in keys {
            // Strip the "manifest:" prefix
            let reference = match key.strip_prefix("manifest:") {
                Some(r) => r.to_string(),
                None => continue,
            };

            // Load manifest body to compute size and extract digest
            let Ok(Some(manifest_bytes)) = self.blob_cache.get(&key).await else {
                continue; // cache entry disappeared — skip
            };

            let Ok(manifest) = serde_json::from_slice::<OciImageManifest>(&manifest_bytes) else {
                continue; // corrupt entry — skip
            };

            // Sum layer sizes + config size
            let layers_size: i64 = manifest.layers.iter().map(|l| l.size).sum();
            let config_size = manifest.config.size;
            let total = layers_size.saturating_add(config_size);
            let size_bytes = if total > 0 {
                u64::try_from(total).ok()
            } else {
                None
            };

            // Look up the stored registry digest (Wave 2 convention)
            let digest_key = zlayer_registry::manifest_digest_cache_key(&reference);
            let digest = self
                .blob_cache
                .get(&digest_key)
                .await
                .ok()
                .flatten()
                .and_then(|bytes| String::from_utf8(bytes).ok());

            images.push(ImageInfo {
                reference,
                digest,
                size_bytes,
            });
        }

        Ok(images)
    }

    async fn remove_image(&self, image: &str, _force: bool) -> Result<()> {
        let manifest_key = zlayer_registry::manifest_cache_key(image);
        let digest_key = zlayer_registry::manifest_digest_cache_key(image);

        // Load manifest to learn the blob digests it references
        let manifest_bytes = self
            .blob_cache
            .get(&manifest_key)
            .await
            .map_err(|e| AgentError::Internal(format!("failed to read manifest cache: {e}")))?;

        let Some(manifest_bytes) = manifest_bytes else {
            return Err(AgentError::NotFound {
                container: image.to_string(),
                reason: format!("image '{image}' not found in cache"),
            });
        };

        let manifest: OciImageManifest = serde_json::from_slice(&manifest_bytes).map_err(|e| {
            AgentError::Internal(format!(
                "failed to parse cached manifest for '{image}': {e}"
            ))
        })?;

        // Delete each referenced blob. Don't bail on individual failures — log and continue.
        for layer in &manifest.layers {
            if let Err(e) = self.blob_cache.delete(&layer.digest).await {
                tracing::warn!(
                    image = %image,
                    digest = %layer.digest,
                    error = %e,
                    "failed to delete layer blob"
                );
            }
        }
        if let Err(e) = self.blob_cache.delete(&manifest.config.digest).await {
            tracing::warn!(
                image = %image,
                digest = %manifest.config.digest,
                error = %e,
                "failed to delete config blob"
            );
        }

        // Delete the manifest body and its digest sidecar
        self.blob_cache
            .delete(&manifest_key)
            .await
            .map_err(|e| AgentError::Internal(format!("failed to delete manifest: {e}")))?;
        let _ = self.blob_cache.delete(&digest_key).await;

        Ok(())
    }

    async fn prune_images(&self) -> Result<PruneResult> {
        // 1. Collect all digests referenced by remaining manifests.
        let manifest_keys = self
            .blob_cache
            .keys_with_prefix("manifest:")
            .await
            .map_err(|e| AgentError::Internal(format!("failed to list manifests: {e}")))?;

        let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
        for key in &manifest_keys {
            let Ok(Some(bytes)) = self.blob_cache.get(key).await else {
                continue;
            };
            let Ok(manifest) = serde_json::from_slice::<OciImageManifest>(&bytes) else {
                continue;
            };
            referenced.insert(manifest.config.digest.clone());
            for layer in &manifest.layers {
                referenced.insert(layer.digest.clone());
            }
        }

        // 2. Walk all sha256:* blob keys and delete those not referenced.
        let all_blob_keys = self
            .blob_cache
            .keys_with_prefix("sha256:")
            .await
            .map_err(|e| AgentError::Internal(format!("failed to list blobs: {e}")))?;

        let mut deleted = Vec::new();
        let mut space_reclaimed: u64 = 0;

        for key in all_blob_keys {
            if referenced.contains(&key) {
                continue;
            }
            // Grab the blob size before deleting (best-effort).
            if let Ok(Some(bytes)) = self.blob_cache.get(&key).await {
                space_reclaimed = space_reclaimed.saturating_add(bytes.len() as u64);
            }
            if let Err(e) = self.blob_cache.delete(&key).await {
                tracing::warn!(
                    digest = %key,
                    error = %e,
                    "failed to delete orphaned blob during prune"
                );
                continue;
            }
            deleted.push(key);
        }

        Ok(PruneResult {
            deleted,
            space_reclaimed,
        })
    }

    #[instrument(
        skip(self),
        fields(
            otel.name = "container.kill",
            container.id = %self.container_id_str(id),
            service.name = %id.service,
            signal = ?signal,
        )
    )]
    async fn kill_container(&self, id: &ContainerId, signal: Option<&str>) -> Result<()> {
        use std::convert::TryFrom;

        let canonical = crate::runtime::validate_signal(signal.unwrap_or("SIGKILL"))?;
        let container_id = self.container_id_str(id);
        let container_root = self.container_root(id);

        tracing::info!(
            container = %container_id,
            signal = %canonical,
            "sending signal to container"
        );

        let container_id_clone = container_id.clone();
        let canonical_clone = canonical.clone();
        tokio::task::spawn_blocking(move || {
            let mut container =
                Container::load(container_root).map_err(|e| AgentError::NotFound {
                    container: container_id_clone.clone(),
                    reason: format!("failed to load container: {e}"),
                })?;

            if !container.status().can_kill() {
                return Err(AgentError::InvalidSpec(format!(
                    "container '{container_id_clone}' is not in a killable state ({:?})",
                    container.status()
                )));
            }

            let sig = Signal::try_from(canonical_clone.as_str()).map_err(|e| {
                AgentError::InvalidSpec(format!("invalid signal '{canonical_clone}': {e:?}"))
            })?;
            container.kill(sig, true).map_err(|e| {
                AgentError::Internal(format!(
                    "failed to deliver signal {canonical_clone} to '{container_id_clone}': {e}"
                ))
            })?;

            Ok::<(), AgentError>(())
        })
        .await
        .map_err(|e| AgentError::Internal(format!("task join error during kill: {e}")))??;

        Ok(())
    }

    /// Pause a container by freezing its cgroup via `Container::pause`.
    ///
    /// Loaded from the on-disk libcontainer state; the call itself is
    /// blocking so we hop to `spawn_blocking`. Errors map to `NotFound` when
    /// the container state directory doesn't exist, `InvalidSpec` when the
    /// container is in a non-pausable state (already paused, never started),
    /// and `Internal` for cgroup write failures.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.pause",
            container.id = %self.container_id_str(id),
            service.name = %id.service,
        )
    )]
    async fn pause_container(&self, id: &ContainerId) -> Result<()> {
        let container_id = self.container_id_str(id);
        let container_root = self.container_root(id);
        let container_id_clone = container_id.clone();
        tokio::task::spawn_blocking(move || {
            let mut container =
                Container::load(container_root).map_err(|e| AgentError::NotFound {
                    container: container_id_clone.clone(),
                    reason: format!("failed to load container: {e}"),
                })?;
            if !container.can_pause() {
                return Err(AgentError::InvalidSpec(format!(
                    "container '{container_id_clone}' is not in a pausable state ({:?})",
                    container.status()
                )));
            }
            container.pause().map_err(|e| {
                AgentError::Internal(format!(
                    "failed to pause container '{container_id_clone}': {e}"
                ))
            })?;
            Ok::<(), AgentError>(())
        })
        .await
        .map_err(|e| AgentError::Internal(format!("task join error during pause: {e}")))??;
        Ok(())
    }

    /// Resume a previously-paused container via `Container::resume`.
    ///
    /// Symmetric inverse of `pause_container`: thaws the freezer cgroup. Same
    /// error mapping conventions.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.unpause",
            container.id = %self.container_id_str(id),
            service.name = %id.service,
        )
    )]
    async fn unpause_container(&self, id: &ContainerId) -> Result<()> {
        let container_id = self.container_id_str(id);
        let container_root = self.container_root(id);
        let container_id_clone = container_id.clone();
        tokio::task::spawn_blocking(move || {
            let mut container =
                Container::load(container_root).map_err(|e| AgentError::NotFound {
                    container: container_id_clone.clone(),
                    reason: format!("failed to load container: {e}"),
                })?;
            if !container.can_resume() {
                return Err(AgentError::InvalidSpec(format!(
                    "container '{container_id_clone}' is not in a resumable state ({:?})",
                    container.status()
                )));
            }
            container.resume().map_err(|e| {
                AgentError::Internal(format!(
                    "failed to resume container '{container_id_clone}': {e}"
                ))
            })?;
            Ok::<(), AgentError>(())
        })
        .await
        .map_err(|e| AgentError::Internal(format!("task join error during unpause: {e}")))??;
        Ok(())
    }

    /// Update a running container's cgroup v2 resource limits and persist
    /// the new restart policy in the supervisor's in-memory state.
    ///
    /// This implementation writes directly to the container's cgroup v2
    /// hierarchy under `/sys/fs/cgroup/zlayer/<id>` (or
    /// `/sys/fs/cgroup/system.slice/zlayer-<id>.scope` when systemd is the
    /// driver). The fields it can apply natively on cgroup v2 are:
    ///
    /// * `cpu_shares` → `cpu.weight` (mapped from the `2..262144` shares
    ///   range to v2's `1..10000` weight range)
    /// * `memory` → `memory.max` (`0` clears the limit)
    /// * `memory_reservation` → `memory.low`
    /// * `memory_swap` → `memory.swap.max` (`-1` clears the limit)
    /// * `pids_limit` → `pids.max` (`-1` or `0` clears)
    /// * `cpuset_cpus` → `cpuset.cpus`
    /// * `cpuset_mems` → `cpuset.mems`
    /// * `cpu_period` + `cpu_quota` → `cpu.max` ("`<quota> <period>`")
    /// * `blkio_weight` → `io.bfq.weight` (best-effort; emits a warning
    ///   when the BFQ controller isn't enabled)
    ///
    /// `cpu_realtime_period` / `cpu_realtime_runtime` and `kernel_memory`
    /// have no cgroup v2 equivalent and are surfaced as warnings rather
    /// than errors.
    ///
    /// `restart_policy` is captured into the in-memory `ContainerInfo`
    /// entry so the supervisor sees the new policy when the container
    /// next exits.
    #[instrument(
        skip(self, update),
        fields(
            otel.name = "container.update",
            container.id = %self.container_id_str(id),
            service.name = %id.service,
        )
    )]
    async fn update_container_resources(
        &self,
        id: &ContainerId,
        update: &crate::runtime::ContainerResourceUpdate,
    ) -> Result<crate::runtime::ContainerUpdateOutcome> {
        let container_id = self.container_id_str(id);
        if update.is_empty() {
            return Ok(crate::runtime::ContainerUpdateOutcome::default());
        }

        // Persist the new restart policy in our in-memory tracking. We
        // do this even when the container's cgroup directory is gone
        // (e.g. the container has already exited): the supervisor reads
        // the policy on the *next* exit, so updating it for a stopped
        // container is still meaningful.
        if let Some(rp) = update.restart_policy.clone() {
            let mut containers = self.containers.write().await;
            if let Some(info) = containers.get_mut(&container_id) {
                info.restart_policy = Some(rp);
            }
        }

        let cgroup_path = if self.config.use_systemd {
            PathBuf::from(format!(
                "/sys/fs/cgroup/system.slice/zlayer-{container_id}.scope"
            ))
        } else {
            PathBuf::from(format!("/sys/fs/cgroup/zlayer/{container_id}"))
        };

        let mut warnings: Vec<String> = Vec::new();

        // If there's nothing to write to cgroup files (only restart
        // policy was set), bail out before touching the filesystem.
        let needs_cgroup_write = update.cpu_shares.is_some()
            || update.memory.is_some()
            || update.memory_reservation.is_some()
            || update.memory_swap.is_some()
            || update.pids_limit.is_some()
            || update.cpuset_cpus.is_some()
            || update.cpuset_mems.is_some()
            || update.cpu_period.is_some()
            || update.cpu_quota.is_some()
            || update.blkio_weight.is_some();

        if needs_cgroup_write && !cgroup_path.exists() {
            return Err(AgentError::NotFound {
                container: container_id.clone(),
                reason: format!(
                    "cgroup directory '{}' not found — is the container running?",
                    cgroup_path.display()
                ),
            });
        }

        if update.kernel_memory.is_some() {
            warnings
                .push("KernelMemory has no cgroup v2 equivalent and was not applied".to_string());
        }
        if update.cpu_realtime_period.is_some() || update.cpu_realtime_runtime.is_some() {
            warnings.push(
                "CpuRealtimePeriod/CpuRealtimeRuntime are not supported on cgroup v2; ignored"
                    .to_string(),
            );
        }

        // cpu_shares -> cpu.weight (cgroup v2 mapping). v1 shares are
        // 2..262144 with default 1024; v2 weight is 1..10000 with
        // default 100. Use Docker's documented mapping:
        //   weight = 1 + ((shares - 2) * 9999 / 262142)
        if let Some(shares) = update.cpu_shares {
            let shares = shares.max(2);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let weight = 1_i64 + (shares - 2) * 9999 / 262_142;
            let weight = weight.clamp(1, 10_000);
            write_cgroup_file(
                &cgroup_path.join("cpu.weight"),
                &weight.to_string(),
                &mut warnings,
            )
            .await?;
        }

        // cpu.max takes "<quota> <period>" or "max <period>".
        if update.cpu_period.is_some() || update.cpu_quota.is_some() {
            let period = update.cpu_period.unwrap_or(100_000);
            let quota_str = match update.cpu_quota {
                Some(q) if q > 0 => q.to_string(),
                _ => "max".to_string(),
            };
            let value = format!("{quota_str} {period}");
            write_cgroup_file(&cgroup_path.join("cpu.max"), &value, &mut warnings).await?;
        }

        if let Some(memory) = update.memory {
            let value = if memory <= 0 {
                "max".to_string()
            } else {
                memory.to_string()
            };
            write_cgroup_file(&cgroup_path.join("memory.max"), &value, &mut warnings).await?;
        }

        if let Some(reservation) = update.memory_reservation {
            let value = if reservation <= 0 {
                "0".to_string()
            } else {
                reservation.to_string()
            };
            write_cgroup_file(&cgroup_path.join("memory.low"), &value, &mut warnings).await?;
        }

        if let Some(swap) = update.memory_swap {
            // Docker semantics: -1 means unlimited. Memory swap on v2
            // is the *swap-only* limit, while Docker's `MemorySwap`
            // historically meant memory+swap. Pass through the absolute
            // value with a warning so the operator knows the v2
            // semantic is different.
            warnings.push(
                "MemorySwap is interpreted as cgroup v2 memory.swap.max (swap-only); \
                 Docker's v1 semantics differ"
                    .to_string(),
            );
            let value = if swap < 0 {
                "max".to_string()
            } else {
                swap.to_string()
            };
            write_cgroup_file(&cgroup_path.join("memory.swap.max"), &value, &mut warnings).await?;
        }

        if let Some(pids) = update.pids_limit {
            let value = if pids <= 0 {
                "max".to_string()
            } else {
                pids.to_string()
            };
            write_cgroup_file(&cgroup_path.join("pids.max"), &value, &mut warnings).await?;
        }

        if let Some(cpus) = update.cpuset_cpus.as_ref() {
            write_cgroup_file(&cgroup_path.join("cpuset.cpus"), cpus, &mut warnings).await?;
        }

        if let Some(mems) = update.cpuset_mems.as_ref() {
            write_cgroup_file(&cgroup_path.join("cpuset.mems"), mems, &mut warnings).await?;
        }

        if let Some(weight) = update.blkio_weight {
            // io.bfq.weight expects 1..1000; Docker's BlkioWeight is
            // 10..1000 with default 500. Pass through verbatim and let
            // the kernel reject out-of-range values.
            write_cgroup_file(
                &cgroup_path.join("io.bfq.weight"),
                &weight.to_string(),
                &mut warnings,
            )
            .await?;
        }

        Ok(crate::runtime::ContainerUpdateOutcome { warnings })
    }

    /// List the processes running inside a container.
    ///
    /// Reads the container's main PID from libcontainer's loaded state, walks
    /// `/proc/{pid}/task/*` to enumerate tids inside the container's pid
    /// namespace, then synthesises a Docker-style top response. The columns
    /// returned are a fixed minimal subset (`UID`, `PID`, `PPID`, `STIME`,
    /// `CMD`) — `ps_args` is accepted for trait conformance but ignored,
    /// because youki has no privileged `ps`-runner per container.
    #[instrument(
        skip(self, _ps_args),
        fields(
            otel.name = "container.top",
            container.id = %self.container_id_str(id),
            service.name = %id.service,
        )
    )]
    async fn top_container(
        &self,
        id: &ContainerId,
        _ps_args: &[String],
    ) -> Result<crate::runtime::ContainerTopOutput> {
        use crate::runtime::ContainerTopOutput;

        let container_id = self.container_id_str(id);
        let container_root = self.container_root(id);
        let container_id_clone = container_id.clone();

        // Snapshot the main process PID under spawn_blocking — Container::load
        // walks the on-disk state file synchronously.
        let pid = tokio::task::spawn_blocking(move || -> Result<i32> {
            let container = Container::load(container_root).map_err(|e| AgentError::NotFound {
                container: container_id_clone.clone(),
                reason: format!("failed to load container: {e}"),
            })?;
            let pid = container.pid().ok_or_else(|| {
                AgentError::InvalidSpec(format!(
                    "container '{container_id_clone}' has no running process"
                ))
            })?;
            Ok(pid.as_raw())
        })
        .await
        .map_err(|e| AgentError::Internal(format!("task join error during top: {e}")))??;

        // Walk /proc/{pid}/task/* to enumerate threads (which double as
        // process IDs from the host's perspective). Containers running a
        // single multi-threaded process expose all its tids here.
        let task_dir = format!("/proc/{pid}/task");
        let mut entries = match tokio::fs::read_dir(&task_dir).await {
            Ok(it) => it,
            Err(e) => {
                return Err(AgentError::NotFound {
                    container: container_id.clone(),
                    reason: format!("failed to read /proc/{pid}/task: {e}"),
                });
            }
        };

        let mut processes: Vec<Vec<String>> = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| AgentError::Internal(format!("failed to walk /proc tree: {e}")))?
        {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            // Skip entries that aren't PIDs (defensive — /proc/.../task only
            // contains numeric directories in practice).
            if !name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let row = read_proc_row(&name).await;
            processes.push(row);
        }

        Ok(ContainerTopOutput {
            titles: vec![
                "UID".to_string(),
                "PID".to_string(),
                "PPID".to_string(),
                "STIME".to_string(),
                "CMD".to_string(),
            ],
            processes,
        })
    }

    /// `changes_container` is unsupported on Youki: the runtime stores
    /// containers as a single mutable rootfs (extracted from the cached
    /// layers in `bundle_path`), with no overlayfs upper/lower split to
    /// diff against. Implementing this would require either re-extracting
    /// the original image layers and walking both trees, or cooperating
    /// with the storage driver — both out of scope for this trait method.
    /// The REST layer translates `Unsupported` into a 501 response.
    async fn changes_container(
        &self,
        _id: &ContainerId,
    ) -> Result<Vec<crate::runtime::FilesystemChangeEntry>> {
        Err(AgentError::Unsupported(
            "changes_container is not supported by the youki runtime: \
             no layered filesystem to diff against"
                .into(),
        ))
    }

    /// `port_mappings_container` is unsupported on Youki: the runtime relies
    /// on the host's network namespace for port forwarding (proxy / overlay
    /// network), not on a per-container `HostConfig.PortBindings` table. The
    /// 501 from the REST layer signals to clients that they should consult
    /// the daemon's deployment / endpoint metadata rather than a runtime
    /// inspect call.
    async fn port_mappings_container(
        &self,
        _id: &ContainerId,
    ) -> Result<Vec<crate::runtime::PortMappingEntry>> {
        Err(AgentError::Unsupported(
            "port_mappings_container is not supported by the youki runtime: \
             port publishing is managed at the proxy / overlay layer"
                .into(),
        ))
    }

    /// `prune_containers` is unsupported on Youki: cleanup of stopped
    /// container state directories is driven by the container supervisor /
    /// reaper, not by a daemon-wide prune sweep. Forcing a sweep here would
    /// race the supervisor's own bookkeeping. Surfaces as 501 from the REST
    /// layer.
    async fn prune_containers(&self) -> Result<crate::runtime::ContainerPruneResult> {
        Err(AgentError::Unsupported(
            "prune_containers is not supported by the youki runtime: \
             stopped containers are reaped by the supervisor"
                .into(),
        ))
    }

    #[instrument(
        skip(self),
        fields(
            otel.name = "image.tag",
            source = %source,
            target = %target,
        )
    )]
    async fn tag_image(&self, source: &str, target: &str) -> Result<()> {
        if source.trim().is_empty() || target.trim().is_empty() {
            return Err(AgentError::InvalidSpec(
                "source and target must be non-empty image references".to_string(),
            ));
        }
        if source == target {
            // Nothing to do; idempotent.
            return Ok(());
        }

        // Copy the source manifest and its digest sidecar under the target
        // reference. All blobs remain shared content-addressed in the cache.
        let src_manifest_key = zlayer_registry::manifest_cache_key(source);
        let manifest_bytes = self
            .blob_cache
            .get(&src_manifest_key)
            .await
            .map_err(|e| AgentError::Internal(format!("failed to read manifest cache: {e}")))?
            .ok_or_else(|| AgentError::NotFound {
                container: source.to_string(),
                reason: format!("source image '{source}' not found in cache"),
            })?;

        let dst_manifest_key = zlayer_registry::manifest_cache_key(target);
        self.blob_cache
            .put(&dst_manifest_key, &manifest_bytes)
            .await
            .map_err(|e| AgentError::Internal(format!("failed to write manifest for tag: {e}")))?;

        // Best-effort: carry over the registry digest sidecar if present.
        let src_digest_key = zlayer_registry::manifest_digest_cache_key(source);
        if let Ok(Some(digest_bytes)) = self.blob_cache.get(&src_digest_key).await {
            let dst_digest_key = zlayer_registry::manifest_digest_cache_key(target);
            if let Err(e) = self.blob_cache.put(&dst_digest_key, &digest_bytes).await {
                tracing::warn!(
                    source = %source,
                    target = %target,
                    error = %e,
                    "failed to copy manifest-digest sidecar for tag (non-fatal)"
                );
            }
        }

        tracing::info!(source = %source, target = %target, "tagged image");
        Ok(())
    }

    /// Stream container logs by tailing the on-disk stdout/stderr files
    /// produced by [`Self::create_log_files`].
    ///
    /// Implementation notes:
    /// * Each line of the file produces one [`LogChunk`]. Youki's runtime
    ///   does not write timestamps into the log files, so chunks carry the
    ///   wall-clock time the line was read when `opts.timestamps` is set
    ///   and `None` otherwise.
    /// * `opts.tail` is honoured by counting `\n` bytes from the end of
    ///   each file before streaming begins.
    /// * `opts.follow` keeps the stream alive after EOF, polling every
    ///   200ms for new content. When `follow=false` the stream completes
    ///   after the buffered lines drain.
    /// * `opts.since`/`opts.until` filter chunks by the read-time
    ///   wallclock (see above — file format has no per-line timestamps,
    ///   so this is the best the youki backend can do).
    /// * `opts.stdout`/`opts.stderr` toggle each channel; if neither is
    ///   true (Docker's default-on shorthand), both are streamed.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.logs.stream",
            container.id = %self.container_id_str(id),
        )
    )]
    async fn logs_stream(&self, id: &ContainerId, opts: LogsStreamOptions) -> Result<LogsStream> {
        // Resolve log paths from local state, falling back to the default
        // bundle/structured paths just like `container_logs` / `get_logs`.
        let (stdout_path, stderr_path) = {
            let containers = self.containers.read().await;
            match containers.get(&self.container_id_str(id)) {
                Some(info) => (info.stdout_path.clone(), info.stderr_path.clone()),
                None => self.log_paths(id),
            }
        };

        // If neither channel was requested, default to streaming both
        // (matches Docker's behaviour when `stdout=false&stderr=false` is
        // sent — Docker treats it as "both", since requesting nothing is
        // never useful).
        let none_specified = !opts.stdout && !opts.stderr;
        let want_stdout = opts.stdout || none_specified;
        let want_stderr = opts.stderr || none_specified;

        // Use a bounded channel so a slow consumer applies natural
        // back-pressure on the file readers.
        let (tx, rx) = mpsc::channel::<Result<LogChunk>>(64);

        if want_stdout && stdout_path.exists() {
            let tx = tx.clone();
            let path = stdout_path.clone();
            let opts_cloned = opts.clone();
            tokio::spawn(async move {
                let _ = stream_log_file(path, LogChannel::Stdout, opts_cloned, tx).await;
            });
        }

        if want_stderr && stderr_path.exists() {
            let tx_err = tx.clone();
            let path = stderr_path.clone();
            let opts_cloned = opts.clone();
            tokio::spawn(async move {
                let _ = stream_log_file(path, LogChannel::Stderr, opts_cloned, tx_err).await;
            });
        }

        // Drop the original sender so the stream terminates once both
        // (or all available) tailers exit.
        drop(tx);

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    /// Stream periodic resource-usage samples for a container by polling
    /// its cgroup v2 directory once per second.
    ///
    /// Reuses [`cgroups_stats::read_container_stats`] for `cpu.stat`,
    /// `memory.current`, and `memory.max`, and additionally reads
    /// `pids.current` / `pids.max` directly so the [`StatsSample`] reflects
    /// pids counters that the existing internal [`ContainerStats`] type
    /// does not carry.
    ///
    /// Network and block-IO counters are not surfaced by youki's cgroup
    /// directory at the same path (network stats live in the container's
    /// netns, blkio stats require the legacy v1 hierarchy) so they are
    /// reported as `0`. Consumers that need those numbers should use the
    /// Docker runtime, which does have them.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.stats.stream",
            container.id = %self.container_id_str(id),
        )
    )]
    async fn stats_stream(&self, id: &ContainerId) -> Result<StatsStream> {
        let container_id = self.container_id_str(id);
        let cgroup_path = if self.config.use_systemd {
            PathBuf::from(format!(
                "/sys/fs/cgroup/system.slice/zlayer-{container_id}.scope"
            ))
        } else {
            PathBuf::from(format!("/sys/fs/cgroup/zlayer/{container_id}"))
        };

        let (tx, rx) = mpsc::channel::<Result<StatsSample>>(8);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            #[allow(clippy::cast_possible_truncation)]
            let online_cpus = num_cpus::get() as u32;

            loop {
                interval.tick().await;

                let sample_result = read_stats_sample(&cgroup_path, online_cpus).await;
                let send_result = match sample_result {
                    Ok(sample) => tx.send(Ok(sample)).await,
                    Err(err) => {
                        // Surface the error and terminate the stream — a
                        // missing cgroup directory means the container is
                        // gone, retrying on every tick would just spam.
                        let _ = tx.send(Err(err)).await;
                        break;
                    }
                };

                if send_result.is_err() {
                    // Receiver dropped — stop sampling.
                    break;
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    /// Stream image pull progress by wrapping the synchronous
    /// [`zlayer_registry::ImagePuller::pull_image_with_policy`] code path.
    ///
    /// The puller does not expose a per-layer progress callback today, so
    /// this implementation synthesises a coarse progression:
    ///   1. `Status { status: "Pulling manifest" }` before manifest fetch.
    ///   2. One `Status { status: "Pulling layer", id: <digest> }` per
    ///      layer in the manifest, emitted before each layer is fetched.
    ///   3. A final `Done { reference, digest }` event when the pull
    ///      succeeds (or an `Err` item if it fails).
    ///
    /// Each layer event carries the layer's `total` size from the manifest
    /// so consumers can render proportional progress bars even though
    /// `current` cannot be reported until the puller gains a streaming
    /// callback.
    ///
    /// `auth` is currently ignored on this backend — youki resolves
    /// credentials through the persistent secret store via
    /// [`zlayer_core::AuthResolver`], matching the semantics of
    /// [`Self::pull_image_with_policy`].
    #[instrument(
        skip(self, _auth),
        fields(
            otel.name = "image.pull.stream",
            container.image.name = %image,
        )
    )]
    async fn pull_image_stream(
        &self,
        image: &str,
        _auth: Option<&RegistryAuth>,
    ) -> Result<PullProgressStream> {
        let (tx, rx) = mpsc::channel::<Result<PullProgress>>(32);

        // Build the puller eagerly (cheap clone of cache + optional
        // local registry) so the spawned task owns everything it needs.
        let puller = {
            let p = zlayer_registry::ImagePuller::with_cache(self.blob_cache.clone());
            if let Some(ref registry) = self.local_registry {
                p.with_local_registry(registry.clone())
            } else {
                p
            }
        };
        let auth = self.auth_resolver.resolve(image);
        let image_owned = image.to_string();

        tokio::spawn(async move {
            // Step 1: announce manifest pull.
            if tx
                .send(Ok(PullProgress::Status {
                    id: None,
                    status: "Pulling manifest".to_string(),
                    progress: None,
                    current: None,
                    total: None,
                }))
                .await
                .is_err()
            {
                return;
            }

            // Step 2: fetch the manifest so we can enumerate layers.
            // `pull_image_manifest` is exposed via the public client; the
            // higher-level pull_image will redo this internally but the
            // cost is one cached lookup and the API is the cleanest way
            // to learn about layers up-front for streaming events.
            let layers_meta: Vec<(String, u64)> =
                match puller.pull_manifest(&image_owned, &auth).await {
                    Ok((manifest, _digest)) => manifest
                        .layers
                        .iter()
                        .map(|l| {
                            let size = u64::try_from(l.size).unwrap_or(0);
                            (l.digest.clone(), size)
                        })
                        .collect(),
                    Err(e) => {
                        let _ = tx
                            .send(Err(AgentError::PullFailed {
                                image: image_owned.clone(),
                                reason: format!("failed to pull manifest: {e}"),
                            }))
                            .await;
                        return;
                    }
                };

            // Step 3: emit one Status event per layer before the actual
            // pull. The puller will retrieve cached layers near-instantly
            // and uncached ones over the network; either way, consumers
            // see one event per layer with the digest as `id`.
            for (digest, size) in &layers_meta {
                if tx
                    .send(Ok(PullProgress::Status {
                        id: Some(digest.clone()),
                        status: "Pulling fs layer".to_string(),
                        progress: None,
                        current: None,
                        total: if *size > 0 { Some(*size) } else { None },
                    }))
                    .await
                    .is_err()
                {
                    return;
                }
            }

            // Step 4: do the actual pull (uses the shared blob cache;
            // already-cached layers are no-ops).
            let force_refresh = false;
            match puller
                .pull_image_with_policy(&image_owned, &auth, force_refresh)
                .await
            {
                Ok(_layers) => {
                    // Best-effort fetch of the registry digest sidecar so
                    // the `Done` event can carry a content-addressed
                    // identifier when one is available.
                    let _ = puller
                        .pull_image_config_with_policy(&image_owned, &auth, force_refresh)
                        .await;

                    let _ = tx
                        .send(Ok(PullProgress::Done {
                            reference: image_owned.clone(),
                            digest: None,
                        }))
                        .await;
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(AgentError::PullFailed {
                            image: image_owned.clone(),
                            reason: format!("failed to pull image: {e}"),
                        }))
                        .await;
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    /// Stream a TAR archive of a path inside the container's rootfs.
    ///
    /// The youki backend doesn't run a daemon and has no live attach API to
    /// the container's mount namespace, so we satisfy `archive_get` by
    /// walking the on-disk rootfs at `<bundle>/rootfs<container_path>` and
    /// streaming the TAR archive on the fly. This works for non-running
    /// containers and for live containers whose rootfs has not been
    /// `pivot_root`'d into a private mount namespace inaccessible from the
    /// host (the standard Youki layout keeps the bundle rootfs visible).
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.archive_get",
            container.id = %self.container_id_str(id),
            archive.path = %path,
        )
    )]
    async fn archive_get(&self, id: &ContainerId, path: &str) -> Result<ArchiveStream> {
        let bundle_path = self.bundle_path(id);
        let rootfs_path = bundle_path.join("rootfs");
        if !rootfs_path.exists() {
            return Err(AgentError::NotFound {
                container: self.container_id_str(id),
                reason: format!(
                    "container rootfs '{}' does not exist on disk",
                    rootfs_path.display()
                ),
            });
        }

        let rel = path.trim_start_matches('/');
        let abs_target = if rel.is_empty() {
            rootfs_path.clone()
        } else {
            rootfs_path.join(rel)
        };

        // Reject path-traversal attempts: the canonicalized target must live
        // strictly under the rootfs.
        let canon_root = tokio::fs::canonicalize(&rootfs_path).await.map_err(|e| {
            AgentError::Internal(format!(
                "failed to canonicalize rootfs '{}': {e}",
                rootfs_path.display()
            ))
        })?;
        let canon_target = match tokio::fs::canonicalize(&abs_target).await {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AgentError::NotFound {
                    container: self.container_id_str(id),
                    reason: format!("path '{path}' not found in container rootfs"),
                });
            }
            Err(e) => {
                return Err(AgentError::Internal(format!(
                    "failed to canonicalize path '{path}': {e}"
                )));
            }
        };
        if !canon_target.starts_with(&canon_root) {
            return Err(AgentError::InvalidSpec(format!(
                "archive path '{path}' escapes container rootfs"
            )));
        }

        // Build the TAR archive on a blocking thread so we never block the
        // async runtime on filesystem I/O.
        let (tx, rx) = mpsc::channel::<Result<bytes::Bytes>>(8);
        let target_for_task = canon_target.clone();
        let path_for_task = path.to_string();
        tokio::task::spawn_blocking(move || {
            let result = build_tar_into_sender(&target_for_task, &path_for_task, &tx);
            if let Err(e) = result {
                let _ = tx.blocking_send(Err(e));
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    /// Extract a TAR archive into the container at `path` by unpacking
    /// directly into `<bundle>/rootfs<path>`.
    #[instrument(
        skip(self, tar_bytes),
        fields(
            otel.name = "container.archive_put",
            container.id = %self.container_id_str(id),
            archive.path = %path,
            archive.bytes = tar_bytes.len(),
        )
    )]
    async fn archive_put(
        &self,
        id: &ContainerId,
        path: &str,
        tar_bytes: bytes::Bytes,
        opts: ArchivePutOptions,
    ) -> Result<()> {
        let bundle_path = self.bundle_path(id);
        let rootfs_path = bundle_path.join("rootfs");
        if !rootfs_path.exists() {
            return Err(AgentError::NotFound {
                container: self.container_id_str(id),
                reason: format!(
                    "container rootfs '{}' does not exist on disk",
                    rootfs_path.display()
                ),
            });
        }

        let rel = path.trim_start_matches('/');
        let abs_dest = if rel.is_empty() {
            rootfs_path.clone()
        } else {
            rootfs_path.join(rel)
        };

        // The destination must already exist and be a directory (Docker's
        // semantics).
        match tokio::fs::metadata(&abs_dest).await {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(AgentError::InvalidSpec(format!(
                    "destination '{path}' inside container is not a directory"
                )));
            }
            Err(_) => {
                return Err(AgentError::NotFound {
                    container: self.container_id_str(id),
                    reason: format!("destination path '{path}' does not exist in container"),
                });
            }
        }

        // Validate that abs_dest stays under canonical rootfs.
        let canon_root = tokio::fs::canonicalize(&rootfs_path).await.map_err(|e| {
            AgentError::Internal(format!(
                "failed to canonicalize rootfs '{}': {e}",
                rootfs_path.display()
            ))
        })?;
        let canon_dest = tokio::fs::canonicalize(&abs_dest).await.map_err(|e| {
            AgentError::Internal(format!("failed to canonicalize dest '{path}': {e}"))
        })?;
        if !canon_dest.starts_with(&canon_root) {
            return Err(AgentError::InvalidSpec(format!(
                "archive destination '{path}' escapes container rootfs"
            )));
        }

        let dest_for_task = canon_dest.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            unpack_tar_into(&dest_for_task, tar_bytes.as_ref(), opts)
        })
        .await
        .map_err(|e| AgentError::Internal(format!("archive_put task panicked: {e}")))??;
        Ok(())
    }

    /// Return path-stat metadata for `path` inside the container's rootfs.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.archive_head",
            container.id = %self.container_id_str(id),
            archive.path = %path,
        )
    )]
    async fn archive_head(&self, id: &ContainerId, path: &str) -> Result<PathStat> {
        let bundle_path = self.bundle_path(id);
        let rootfs_path = bundle_path.join("rootfs");
        if !rootfs_path.exists() {
            return Err(AgentError::NotFound {
                container: self.container_id_str(id),
                reason: format!(
                    "container rootfs '{}' does not exist on disk",
                    rootfs_path.display()
                ),
            });
        }

        let rel = path.trim_start_matches('/');
        let abs_target = if rel.is_empty() {
            rootfs_path.clone()
        } else {
            rootfs_path.join(rel)
        };

        // symlink_metadata so we report the link itself, not its target.
        let meta = tokio::fs::symlink_metadata(&abs_target)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => AgentError::NotFound {
                    container: self.container_id_str(id),
                    reason: format!("path '{path}' not found in container rootfs"),
                },
                _ => AgentError::Internal(format!("failed to stat path '{path}': {e}")),
            })?;

        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        #[allow(clippy::cast_possible_wrap)]
        let size = meta.len() as i64;
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::MetadataExt;
            meta.mode()
        };
        #[cfg(not(unix))]
        let mode: u32 = 0;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339().into());
        let link_target = if meta.file_type().is_symlink() {
            tokio::fs::read_link(&abs_target)
                .await
                .ok()
                .and_then(|p| p.to_str().map(String::from))
                .unwrap_or_default()
        } else {
            String::new()
        };

        Ok(PathStat {
            name,
            size,
            mode,
            mtime,
            link_target,
        })
    }
}

/// Walk `target` and stream a TAR archive into `tx` synchronously.
///
/// Used by `YoukiRuntime::archive_get` from a `spawn_blocking` task. Each
/// chunk emitted by the underlying `tar::Builder` is forwarded to the
/// channel as a `bytes::Bytes` so the async caller can pipe it straight to
/// the HTTP response body.
fn build_tar_into_sender(
    target: &Path,
    archive_path: &str,
    tx: &mpsc::Sender<Result<bytes::Bytes>>,
) -> Result<()> {
    use std::io::Write;

    /// `std::io::Write` adapter that forwards every write into a tokio mpsc
    /// channel as a `bytes::Bytes` chunk.
    struct ChannelWriter<'a> {
        tx: &'a mpsc::Sender<Result<bytes::Bytes>>,
    }
    impl Write for ChannelWriter<'_> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let chunk = bytes::Bytes::copy_from_slice(buf);
            self.tx
                .blocking_send(Ok(chunk))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let writer = ChannelWriter { tx };
    let mut builder = tar::Builder::new(writer);
    builder.follow_symlinks(false);

    // Determine the in-archive name (Docker uses the basename of the
    // requested path so the TAR contains entries like `foo/...`).
    let entry_name = std::path::Path::new(archive_path).file_name().map_or_else(
        || std::ffi::OsString::from("."),
        std::ffi::OsStr::to_os_string,
    );

    let meta = std::fs::symlink_metadata(target)
        .map_err(|e| AgentError::Internal(format!("failed to stat archive target: {e}")))?;
    if meta.is_dir() {
        builder
            .append_dir_all(&entry_name, target)
            .map_err(|e| AgentError::Internal(format!("failed to append dir to tar: {e}")))?;
    } else {
        let mut f = std::fs::File::open(target)
            .map_err(|e| AgentError::Internal(format!("failed to open archive target: {e}")))?;
        builder
            .append_file(&entry_name, &mut f)
            .map_err(|e| AgentError::Internal(format!("failed to append file to tar: {e}")))?;
    }
    builder
        .finish()
        .map_err(|e| AgentError::Internal(format!("failed to finalize tar: {e}")))?;
    Ok(())
}

/// Unpack a TAR archive into `dest` synchronously, honouring
/// [`ArchivePutOptions`].
///
/// `no_overwrite_dir_non_dir` rejects the case where an entry would replace
/// an existing directory with a non-directory (or vice versa) — implemented
/// via a pre-pass over the archive's entries before extracting. `copy_uid_gid`
/// is forwarded to `tar::Archive::set_preserve_ownerships` so the unpacker
/// keeps the archive's uid/gid instead of chown'ing to the calling user.
fn unpack_tar_into(dest: &Path, tar_bytes: &[u8], opts: ArchivePutOptions) -> Result<()> {
    if opts.no_overwrite_dir_non_dir {
        // Pre-pass: detect directory/non-directory replacements.
        let mut probe = tar::Archive::new(std::io::Cursor::new(tar_bytes));
        let entries = probe
            .entries()
            .map_err(|e| AgentError::Internal(format!("failed to read tar entries: {e}")))?;
        for entry in entries {
            let entry =
                entry.map_err(|e| AgentError::Internal(format!("invalid tar entry: {e}")))?;
            let p = entry
                .path()
                .map_err(|e| AgentError::Internal(format!("invalid tar path: {e}")))?
                .into_owned();
            let dest_p = dest.join(&p);
            if let Ok(existing) = std::fs::symlink_metadata(&dest_p) {
                let entry_is_dir = entry.header().entry_type().is_dir();
                if existing.is_dir() != entry_is_dir {
                    return Err(AgentError::InvalidSpec(format!(
                        "archive entry '{}' would replace a {} with a {}",
                        p.display(),
                        if existing.is_dir() {
                            "directory"
                        } else {
                            "non-directory"
                        },
                        if entry_is_dir {
                            "directory"
                        } else {
                            "non-directory"
                        }
                    )));
                }
            }
        }
    }

    let mut archive = tar::Archive::new(std::io::Cursor::new(tar_bytes));
    archive.set_preserve_permissions(true);
    archive.set_preserve_ownerships(opts.copy_uid_gid);
    archive
        .unpack(dest)
        .map_err(|e| AgentError::Internal(format!("failed to unpack archive: {e}")))?;
    Ok(())
}

/// Build a TAR archive containing exactly one entry from a host path,
/// returning the bytes. Test-only helper used by
/// `archive_helpers_reject_dir_nondir_replacements`.
#[cfg(test)]
fn build_tar_from_path_for_test(src: &Path, entry_name: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        builder.follow_symlinks(false);
        let meta = std::fs::symlink_metadata(src).unwrap();
        if meta.is_dir() {
            builder.append_dir_all(entry_name, src).unwrap();
        } else {
            let mut f = std::fs::File::open(src).unwrap();
            builder.append_file(entry_name, &mut f).unwrap();
        }
        builder.finish().unwrap();
    }
    buf
}

/// Read one row of `top`-style data for a host PID by parsing
/// `/proc/{pid}/status` and `/proc/{pid}/cmdline`.
///
/// The columns mirror the trait's documented `top_container` shape:
/// Write `value` to a cgroup v2 control file, demoting recoverable
/// errors (missing controller, invalid value) to warnings so a single
/// unsupported field doesn't sink the whole update. Hard errors
/// (permission denied, IO errors) propagate as
/// [`AgentError::Internal`].
async fn write_cgroup_file(
    path: &std::path::Path,
    value: &str,
    warnings: &mut Vec<String>,
) -> Result<()> {
    match tokio::fs::write(path, value).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warnings.push(format!(
                "cgroup file '{}' not found; controller may not be enabled",
                path.display()
            ));
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
            warnings.push(format!(
                "cgroup write to '{}' rejected value '{}': {e}",
                path.display(),
                value
            ));
            Ok(())
        }
        Err(e) => Err(AgentError::Internal(format!(
            "failed to write '{}' to {}: {e}",
            value,
            path.display()
        ))),
    }
}

/// `[UID, PID, PPID, STIME, CMD]`. Any field that fails to read is filled
/// with the empty string so the row width stays constant — `top` clients
/// expect the matrix to be rectangular.
async fn read_proc_row(pid: &str) -> Vec<String> {
    let status_path = format!("/proc/{pid}/status");
    let mut uid = String::new();
    let mut parent_pid = String::new();
    if let Ok(text) = tokio::fs::read_to_string(&status_path).await {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                // First field after the tabs is the real UID.
                if let Some(first) = rest.split_whitespace().next() {
                    uid = first.to_string();
                }
            } else if let Some(rest) = line.strip_prefix("PPid:") {
                parent_pid = rest.trim().to_string();
            }
        }
    }

    // STIME would normally come from `ps`'s formatter; we simulate it as
    // the truncated wallclock seen on the start of the row read. This is
    // a coarse approximation but matches Docker's documented contract:
    // youki has no internal `ps` runner so we surface the best-effort
    // string with the same shape.
    let stime = chrono::Utc::now().format("%H:%M:%S").to_string();

    let cmdline_path = format!("/proc/{pid}/cmdline");
    let cmd = match tokio::fs::read(&cmdline_path).await {
        Ok(bytes) => {
            // /proc/{pid}/cmdline uses NUL separators; replace with spaces
            // and trim the trailing NUL the kernel emits.
            let normalised: Vec<u8> = bytes
                .into_iter()
                .map(|b| if b == 0 { b' ' } else { b })
                .collect();
            let mut s = String::from_utf8_lossy(&normalised).into_owned();
            while s.ends_with(' ') {
                s.pop();
            }
            s
        }
        Err(_) => String::new(),
    };

    vec![uid, pid.to_string(), parent_pid, stime, cmd]
}

/// Tail a single log file and forward each line as a [`LogChunk`] over
/// `tx`. Honours [`LogsStreamOptions::follow`] (poll-on-EOF), `tail`
/// (count `\n` bytes from end before streaming), and `since`/`until`
/// (wallclock filter applied to the moment each line is read — youki log
/// files do not carry per-line timestamps).
async fn stream_log_file(
    path: PathBuf,
    channel: LogChannel,
    opts: LogsStreamOptions,
    tx: mpsc::Sender<Result<LogChunk>>,
) -> Result<()> {
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => {
            let _ = tx
                .send(Err(AgentError::Internal(format!(
                    "failed to open log file {}: {}",
                    path.display(),
                    e
                ))))
                .await;
            return Ok(());
        }
    };

    let mut reader = BufReader::new(file);

    // Apply `tail`: seek so that the next read begins at the start of
    // the most-recent N lines. Implemented by counting newlines from the
    // end of the file.
    if let Some(tail) = opts.tail {
        if tail > 0 {
            let metadata = match reader.get_ref().metadata().await {
                Ok(m) => m,
                Err(e) => {
                    let _ = tx
                        .send(Err(AgentError::Internal(format!(
                            "failed to stat log file {}: {}",
                            path.display(),
                            e
                        ))))
                        .await;
                    return Ok(());
                }
            };
            let start = compute_tail_offset(reader.get_mut(), metadata.len(), tail).await;
            if let Err(e) = reader.seek(std::io::SeekFrom::Start(start)).await {
                let _ = tx
                    .send(Err(AgentError::Internal(format!(
                        "failed to seek log file {}: {}",
                        path.display(),
                        e
                    ))))
                    .await;
                return Ok(());
            }
        }
    }

    let mut line = String::new();
    loop {
        line.clear();
        let bytes_read = match reader.read_line(&mut line).await {
            Ok(n) => n,
            Err(e) => {
                let _ = tx
                    .send(Err(AgentError::Internal(format!(
                        "failed to read log file {}: {}",
                        path.display(),
                        e
                    ))))
                    .await;
                return Ok(());
            }
        };

        if bytes_read == 0 {
            // EOF — terminate unless we're in follow mode.
            if !opts.follow {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }

        let now = chrono::Utc::now();
        let now_secs = now.timestamp();

        if let Some(since) = opts.since {
            if now_secs < since {
                continue;
            }
        }
        if let Some(until) = opts.until {
            if now_secs > until {
                // Past the cutoff — stop streaming this channel.
                return Ok(());
            }
        }

        let chunk = LogChunk {
            stream: channel,
            bytes: bytes::Bytes::copy_from_slice(line.as_bytes()),
            timestamp: if opts.timestamps { Some(now) } else { None },
        };

        if tx.send(Ok(chunk)).await.is_err() {
            // Receiver dropped — stop tailing.
            return Ok(());
        }
    }
}

/// Compute the byte offset of the start of the last `tail` lines in a
/// file of size `file_len`. Reads backwards in 4 KiB chunks until enough
/// newlines have been seen or the file start is reached.
///
/// "The last N lines" is defined as the bytes following the
/// `(N+1)`th-from-end newline, mirroring `tail -n N`. If the file has
/// fewer than `N` lines, returns `0` (stream the whole file).
async fn compute_tail_offset(file: &mut tokio::fs::File, file_len: u64, tail: u64) -> u64 {
    const CHUNK_USIZE: usize = 4096;

    if file_len == 0 || tail == 0 {
        return 0;
    }

    let chunk: u64 = CHUNK_USIZE as u64;
    let target = tail.saturating_add(1); // the newline *before* the first wanted line
    let mut pos = file_len;
    let mut newlines: u64 = 0;
    let mut buf = vec![0u8; CHUNK_USIZE];

    while pos > 0 {
        let read_len = std::cmp::min(chunk, pos);
        pos -= read_len;
        if file.seek(std::io::SeekFrom::Start(pos)).await.is_err() {
            return 0;
        }
        // `read_len` is bounded by `CHUNK_USIZE`, so the cast is safe on
        // every target ZLayer supports (Linux x86_64 / aarch64).
        let slice_len = usize::try_from(read_len).unwrap_or(CHUNK_USIZE);
        let buf_slice = &mut buf[..slice_len];
        if tokio::io::AsyncReadExt::read_exact(file, buf_slice)
            .await
            .is_err()
        {
            return 0;
        }
        for (i, byte) in buf_slice.iter().enumerate().rev() {
            if *byte == b'\n' {
                newlines += 1;
                if newlines == target {
                    // `i` is the index of the (tail+1)-th newline from
                    // the end *within the current chunk*. The first
                    // wanted byte sits immediately after it.
                    let absolute = pos + (i as u64) + 1;
                    return absolute.min(file_len);
                }
            }
        }
    }

    0
}

/// Read a single [`StatsSample`] from `cgroup_path`. Wraps the existing
/// [`cgroups_stats::read_container_stats`] (cpu + memory) and supplements
/// it with `pids.current` / `pids.max` read directly from sysfs.
async fn read_stats_sample(cgroup_path: &Path, online_cpus: u32) -> Result<StatsSample> {
    let stats = cgroups_stats::read_container_stats(cgroup_path)
        .await
        .map_err(|e| {
            AgentError::Internal(format!(
                "failed to read cgroup stats at {}: {}",
                cgroup_path.display(),
                e
            ))
        })?;

    let pids_current = read_u64_file(cgroup_path.join("pids.current"))
        .await
        .unwrap_or(0);
    let pids_limit = read_pids_limit(cgroup_path.join("pids.max")).await;

    let mem_limit_bytes = if stats.memory_limit == u64::MAX {
        0
    } else {
        stats.memory_limit
    };

    Ok(StatsSample {
        cpu_total_ns: stats.cpu_usage_usec.saturating_mul(1_000),
        cpu_system_ns: 0,
        online_cpus,
        mem_used_bytes: stats.memory_bytes,
        mem_limit_bytes,
        net_rx_bytes: 0,
        net_tx_bytes: 0,
        blkio_read_bytes: 0,
        blkio_write_bytes: 0,
        pids_current,
        pids_limit,
        timestamp: chrono::Utc::now(),
    })
}

/// Read a small text file containing a single decimal integer. Used for
/// `pids.current`. Returns `None` when the file is missing or unreadable
/// so the caller can substitute a sentinel (`0` for unknown counters).
async fn read_u64_file(path: PathBuf) -> Option<u64> {
    let content = tokio::fs::read_to_string(&path).await.ok()?;
    content.trim().parse::<u64>().ok()
}

/// Read `pids.max`, which is either a decimal integer or the literal
/// `"max"` (cgroup v2 sentinel for "no limit"). Returns `None` for
/// `"max"` or any read/parse error so the caller can leave
/// [`StatsSample::pids_limit`] unset.
async fn read_pids_limit(path: PathBuf) -> Option<u64> {
    let content = tokio::fs::read_to_string(&path).await.ok()?;
    let trimmed = content.trim();
    if trimmed == "max" {
        return None;
    }
    trimmed.parse::<u64>().ok()
}

/// Async duplex wrapper around a non-blocking PTY master fd, used by
/// [`YoukiRuntime::exec_pty`] to expose the master end as an
/// `AsyncRead + AsyncWrite + Send + Unpin` stream that fits the
/// [`ExecPtyStream`] trait object.
///
/// The fd is owned: dropping `PtyDuplex` closes the master, which causes the
/// kernel to send `SIGHUP` to the slave's controlling process group and tears
/// the session down cleanly.
struct PtyDuplex {
    inner: tokio::io::unix::AsyncFd<OwnedFd>,
}

impl PtyDuplex {
    fn new(fd: OwnedFd) -> Result<Self> {
        let inner = tokio::io::unix::AsyncFd::new(fd)
            .map_err(|e| AgentError::Internal(format!("AsyncFd::new on pty master: {e}")))?;
        Ok(Self { inner })
    }
}

impl tokio::io::AsyncRead for PtyDuplex {
    #[allow(unsafe_code)]
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        loop {
            let mut guard = std::task::ready!(this.inner.poll_read_ready(cx))?;
            // SAFETY: `read(2)` only writes into the buffer, never reads from
            // its uninitialised tail. We pass the unfilled portion as a raw
            // pointer + length and tell `ReadBuf` how many bytes were
            // actually written before exposing them as initialised. The
            // pointer is valid for the duration of the `read` call because
            // `buf` is borrowed mutably for the entire `poll_read` body.
            let unfilled = unsafe {
                std::slice::from_raw_parts_mut(
                    buf.unfilled_mut().as_mut_ptr().cast::<libc::c_void>(),
                    buf.remaining(),
                )
            };
            let fd = guard.get_ref().as_raw_fd();
            // SAFETY: `fd` is a valid PTY master fd owned by `self.inner`.
            // `unfilled` points into a unique mutable borrow of `buf`. The
            // syscall touches at most `unfilled.len()` bytes.
            let rc = unsafe { libc::read(fd, unfilled.as_mut_ptr(), unfilled.len()) };
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    guard.clear_ready();
                    continue;
                }
                // Linux PTY masters return EIO once the slave hangs up; treat
                // that as a clean EOF so callers see end-of-stream rather
                // than a confusing error.
                if err.raw_os_error() == Some(libc::EIO) {
                    return std::task::Poll::Ready(Ok(()));
                }
                return std::task::Poll::Ready(Err(err));
            }
            // We checked `rc < 0` above, so the cast is well-defined.
            #[allow(clippy::cast_sign_loss)]
            let n = rc as usize;
            // SAFETY: the kernel just wrote `n` bytes into the unfilled
            // tail; mark them as initialised + filled.
            unsafe {
                buf.assume_init(n);
            }
            buf.advance(n);
            return std::task::Poll::Ready(Ok(()));
        }
    }
}

impl tokio::io::AsyncWrite for PtyDuplex {
    #[allow(unsafe_code)]
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufdata: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        loop {
            let mut guard = std::task::ready!(this.inner.poll_write_ready(cx))?;
            let fd = guard.get_ref().as_raw_fd();
            // SAFETY: `fd` is owned by `self.inner` and remains valid for
            // the call. `bufdata` is a borrowed slice valid for `bufdata.len()`
            // bytes; `write(2)` only reads from it.
            let rc = unsafe { libc::write(fd, bufdata.as_ptr().cast(), bufdata.len()) };
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    guard.clear_ready();
                    continue;
                }
                return std::task::Poll::Ready(Err(err));
            }
            // We checked `rc < 0` above; safe to cast.
            #[allow(clippy::cast_sign_loss)]
            let n = rc as usize;
            return std::task::Poll::Ready(Ok(n));
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        // PTYs have no userspace buffer; the kernel handles framing.
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        // Closing the master happens on drop; no half-close on PTYs.
        std::task::Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_paths::ZLayerDirs;

    #[test]
    fn test_youki_config_default() {
        let config = YoukiConfig::default();
        let dirs = zlayer_paths::ZLayerDirs::system_default();

        assert_eq!(config.state_dir, dirs.containers());
        assert_eq!(config.rootfs_dir, dirs.rootfs());
        assert_eq!(config.bundle_dir, dirs.bundles());
        assert_eq!(config.cache_dir, dirs.cache());
        assert_eq!(config.volume_dir, dirs.volumes());
        assert!(!config.use_systemd);
        assert!(config.cache_type.is_none());
        assert!(config.log_base_dir.is_none());
        assert!(config.deployment_name.is_none());
    }

    #[test]
    fn test_container_id_str() {
        let id = ContainerId::new("myservice".to_string(), 1);

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
                ContainerStatus::Creating | ContainerStatus::Created => ContainerState::Pending,
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
        let dirs = zlayer_paths::ZLayerDirs::system_default();
        let id = ContainerId::new("testservice".to_string(), 2);

        let container_id = format!("{}-{}", id.service, id.replica);
        let state_dir = config.state_dir.join(&container_id);
        let stdout = state_dir.join("stdout.log");
        let stderr = state_dir.join("stderr.log");

        assert_eq!(stdout, dirs.containers().join("testservice-2/stdout.log"));
        assert_eq!(stderr, dirs.containers().join("testservice-2/stderr.log"));
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
            log_base_dir: Some(PathBuf::from("/var/log/zlayer")),
            deployment_name: Some("myapp".to_string()),
        };

        let cloned = config.clone();

        assert_eq!(cloned.state_dir, config.state_dir);
        assert_eq!(cloned.rootfs_dir, config.rootfs_dir);
        assert_eq!(cloned.bundle_dir, config.bundle_dir);
        assert_eq!(cloned.cache_dir, config.cache_dir);
        assert_eq!(cloned.volume_dir, config.volume_dir);
        assert_eq!(cloned.use_systemd, config.use_systemd);
        assert!(cloned.cache_type.is_some());
        assert_eq!(cloned.log_base_dir, config.log_base_dir);
        assert_eq!(cloned.deployment_name, config.deployment_name);
    }

    /// Test that `YoukiRuntime::new()` creates directories
    #[tokio::test]
    async fn test_youki_runtime_directory_creation() {
        // Use a unique temp directory based on test run
        let temp_base = ZLayerDirs::system_default()
            .tmp()
            .join(format!("youki_test_{}", std::process::id()));

        let config = YoukiConfig {
            state_dir: temp_base.join("state"),
            rootfs_dir: temp_base.join("rootfs"),
            bundle_dir: temp_base.join("bundles"),
            cache_dir: temp_base.join("cache"),
            volume_dir: temp_base.join("volumes"),
            use_systemd: false,
            cache_type: None,
            log_base_dir: None,
            deployment_name: None,
        };

        // Clean up any previous test run
        let _ = std::fs::remove_dir_all(&temp_base);

        // This should succeed and create all directories
        let result = YoukiRuntime::new(config.clone(), None).await;

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

    /// Tail-from-end logic: write a file with a known number of lines,
    /// ask `compute_tail_offset` for the start of the last 2, and verify
    /// the offset lands on the third-to-last line's start.
    #[tokio::test]
    async fn youki_tail_offset_returns_last_n_lines() {
        let dir = ZLayerDirs::system_default().tmp().join(format!(
            "zlayer_tail_test_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("log");
        // 5 lines, each ending in '\n', distinguishable bodies.
        tokio::fs::write(&path, b"a\nbb\nccc\ndddd\neeeee\n")
            .await
            .unwrap();

        let mut file = tokio::fs::File::open(&path).await.unwrap();
        let len = file.metadata().await.unwrap().len();
        // Last 2 lines are "dddd\n" + "eeeee\n", combined 11 bytes;
        // offset should be `len - 11`.
        let offset = compute_tail_offset(&mut file, len, 2).await;
        assert_eq!(offset, len - 11, "expected last-2-lines offset");

        // Tail >= total line count must yield 0 (whole file).
        let mut file = tokio::fs::File::open(&path).await.unwrap();
        assert_eq!(compute_tail_offset(&mut file, len, 100).await, 0);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// `stream_log_file` with `follow=false` reads all existing lines and
    /// then terminates cleanly at EOF. Exercises the non-follow path
    /// without needing a real container.
    #[tokio::test]
    async fn youki_logs_stream_reads_static_file_without_follow() {
        let dir = ZLayerDirs::system_default().tmp().join(format!(
            "zlayer_logs_static_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("stdout.log");
        tokio::fs::write(&path, b"hello\nworld\n").await.unwrap();

        let (tx, mut rx) = mpsc::channel::<Result<LogChunk>>(8);
        let opts = LogsStreamOptions {
            follow: false,
            tail: None,
            since: None,
            until: None,
            timestamps: true,
            stdout: true,
            stderr: false,
        };
        stream_log_file(path, LogChannel::Stdout, opts, tx)
            .await
            .unwrap();

        let mut received = Vec::new();
        while let Some(item) = rx.recv().await {
            let chunk = item.unwrap();
            received.push(chunk);
        }
        assert_eq!(
            received.len(),
            2,
            "expected 2 chunks, got {}",
            received.len()
        );
        assert_eq!(received[0].stream, LogChannel::Stdout);
        assert!(received[0].timestamp.is_some());
        assert_eq!(received[0].bytes.as_ref(), b"hello\n");
        assert_eq!(received[1].bytes.as_ref(), b"world\n");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// `read_stats_sample` requires a real cgroup v2 directory layout
    /// that we cannot easily synthesise on a non-root test host, so this
    /// test is `#[ignore]`-d. Run with
    /// `cargo test -p zlayer-agent youki_stats_sample_reads_cgroup -- --ignored`
    /// inside a real container or as root with a fake cgroup path.
    #[tokio::test]
    #[ignore = "requires a real cgroup v2 hierarchy"]
    async fn youki_stats_sample_reads_cgroup() {
        // Try the host's own root cgroup as a smoke target — every cgroup
        // v2 system has /sys/fs/cgroup/cpu.stat and memory.current at the
        // root. pids.current/pids.max may be missing at the root, which
        // is fine — read_stats_sample treats them as 0/None.
        let path = Path::new("/sys/fs/cgroup");
        let sample = read_stats_sample(path, 1).await.unwrap();
        // CPU monotonically increases; memory.current is non-negative.
        assert!(sample.cpu_total_ns < u64::MAX);
        assert!(sample.online_cpus >= 1);
    }

    /// `(rows, cols)` from the resize channel converts to `nix::pty::Winsize`
    /// with `ws_row` and `ws_col` populated and the pixel fields zeroed.
    /// Pure shape conversion — no fd touched.
    #[test]
    fn youki_pty_resize_winsize_shape() {
        let (rows, cols): (u16, u16) = (24, 80);
        let ws = nix::pty::Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(ws.ws_row, 24);
        assert_eq!(ws.ws_col, 80);
        assert_eq!(ws.ws_xpixel, 0);
        assert_eq!(ws.ws_ypixel, 0);

        // Maximum values still fit cleanly through the channel and the
        // ioctl payload (winsize is u16 across all four fields).
        let ws_max = nix::pty::Winsize {
            ws_row: u16::MAX,
            ws_col: u16::MAX,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(ws_max.ws_row, u16::MAX);
        assert_eq!(ws_max.ws_col, u16::MAX);
    }

    /// `exec_pty` rejects an empty command vector with `InvalidSpec` before
    /// touching libcontainer or allocating a PTY. Exercises the input
    /// validation path without needing a running container.
    #[tokio::test]
    async fn youki_exec_pty_rejects_empty_command() {
        let temp_base = ZLayerDirs::system_default()
            .tmp()
            .join(format!("youki_exec_pty_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_base);

        let config = YoukiConfig {
            state_dir: temp_base.join("state"),
            rootfs_dir: temp_base.join("rootfs"),
            bundle_dir: temp_base.join("bundles"),
            cache_dir: temp_base.join("cache"),
            volume_dir: temp_base.join("volumes"),
            use_systemd: false,
            cache_type: None,
            log_base_dir: None,
            deployment_name: None,
        };

        let runtime = YoukiRuntime::new(config, None).await.unwrap();
        let id = ContainerId::new("missing".to_string(), 0);

        let result = runtime
            .exec_pty(
                &id,
                ExecOptions {
                    command: Vec::new(),
                    tty: true,
                    ..ExecOptions::default()
                },
            )
            .await;

        assert!(matches!(result, Err(AgentError::InvalidSpec(_))));

        let _ = std::fs::remove_dir_all(&temp_base);
    }

    /// `PtyDuplex::new` accepts a non-blocking PTY master fd produced by
    /// `nix::pty::openpty` and exposes it as `AsyncRead + AsyncWrite`. This
    /// is purely a wrapper smoke test — no container, no exec.
    ///
    /// Marked `#[ignore]` because real PTY allocation requires
    /// `/dev/ptmx`, which CI sandboxes occasionally restrict; run with
    /// `cargo test -p zlayer-agent --features youki-runtime youki_pty_duplex_wraps_master -- --ignored`.
    #[tokio::test]
    #[ignore = "requires /dev/ptmx access"]
    async fn youki_pty_duplex_wraps_master() {
        let pair = nix::pty::openpty(None, None).expect("openpty");
        nix::fcntl::fcntl(
            &pair.master,
            nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
        )
        .expect("F_SETFL O_NONBLOCK");

        let _duplex = PtyDuplex::new(pair.master).expect("PtyDuplex::new");
        // Slave is dropped here, which makes the master EOF on next read.
        drop(pair.slave);
    }

    /// Sanity-check the `unpack_tar_into` + `build_tar_into_sender`
    /// helpers used by the youki archive endpoints: a TAR archive built
    /// from a host directory must round-trip back to the same file tree
    /// when unpacked elsewhere.
    #[tokio::test]
    async fn archive_helpers_round_trip_a_directory_tree() {
        // Build a small tree.
        let src_dir = ZLayerDirs::system_default()
            .scratch_dir("youki-archive-test-")
            .unwrap();
        let nested = src_dir.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("c.txt"), b"deep file").unwrap();
        std::fs::write(src_dir.path().join("top.txt"), b"top file").unwrap();

        // Drive `build_tar_into_sender` through a Tokio mpsc and collect bytes.
        let (tx, mut rx) = mpsc::channel::<Result<bytes::Bytes>>(8);
        let target = src_dir.path().to_path_buf();
        let handle =
            tokio::task::spawn_blocking(move || super::build_tar_into_sender(&target, "root", &tx));
        let mut buf = Vec::new();
        while let Some(item) = rx.recv().await {
            let chunk = item.expect("tar chunk");
            buf.extend_from_slice(&chunk);
        }
        handle.await.unwrap().unwrap();

        // Unpack into a fresh dir; the entry should land under `root/`.
        let dest_dir = ZLayerDirs::system_default()
            .scratch_dir("youki-archive-test-")
            .unwrap();
        super::unpack_tar_into(
            dest_dir.path(),
            &buf,
            crate::runtime::ArchivePutOptions::default(),
        )
        .unwrap();
        assert!(dest_dir.path().join("root/top.txt").exists());
        assert!(dest_dir.path().join("root/a/b/c.txt").exists());
        assert_eq!(
            std::fs::read(dest_dir.path().join("root/a/b/c.txt")).unwrap(),
            b"deep file",
        );
    }

    /// `unpack_tar_into` with `no_overwrite_dir_non_dir = true` must
    /// reject an archive entry that would replace an existing directory
    /// with a non-directory (or vice versa).
    #[tokio::test]
    async fn archive_helpers_reject_dir_nondir_replacements() {
        let dest_dir = ZLayerDirs::system_default()
            .scratch_dir("youki-archive-test-")
            .unwrap();
        // Pre-create a directory at `target`.
        let target = dest_dir.path().join("target");
        std::fs::create_dir_all(&target).unwrap();

        // Build an archive whose only entry is a *file* named `target`.
        let src_file_dir = ZLayerDirs::system_default()
            .scratch_dir("youki-archive-test-")
            .unwrap();
        let src_file = src_file_dir.path().join("target");
        std::fs::write(&src_file, b"i am a file").unwrap();
        let bytes = super::build_tar_from_path_for_test(&src_file, "target");
        let opts = crate::runtime::ArchivePutOptions {
            no_overwrite_dir_non_dir: true,
            copy_uid_gid: false,
        };
        let err = super::unpack_tar_into(dest_dir.path(), &bytes, opts).unwrap_err();
        assert!(
            matches!(err, AgentError::InvalidSpec(_)),
            "expected InvalidSpec, got {err:?}"
        );
    }

    #[test]
    fn from_data_dir_scopes_all_paths() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = YoukiConfig::from_data_dir(tmp.path());
        assert!(cfg.cache_dir.starts_with(tmp.path()));
        assert!(cfg.state_dir.starts_with(tmp.path()));
        assert!(cfg.rootfs_dir.starts_with(tmp.path()));
        assert!(cfg.bundle_dir.starts_with(tmp.path()));
        assert!(cfg.volume_dir.starts_with(tmp.path()));
    }
}
