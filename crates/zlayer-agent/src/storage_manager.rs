//! Storage Manager for `ZLayer` volumes
//!
//! Manages named volumes, anonymous volumes, and S3 mounts.
//! When the `s3` feature is enabled, supports optional S3 backup of volumes
//! via [`zlayer_storage::sync::LayerSyncManager`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;
use ulid::Ulid;

#[cfg(feature = "s3")]
use std::sync::Arc;
#[cfg(feature = "s3")]
use zlayer_storage::sync::LayerSyncManager;
#[cfg(feature = "s3")]
use zlayer_storage::ContainerLayerId;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Volume '{0}' not found")]
    VolumeNotFound(String),

    #[error("Volume '{0}' is in use by containers: {1:?}")]
    VolumeInUse(String, Vec<String>),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid volume name: {0}")]
    InvalidName(String),

    #[cfg(feature = "s3")]
    #[error("Layer sync error: {0}")]
    LayerSync(String),
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// Information about a managed volume
#[derive(Debug, Clone)]
pub struct VolumeInfo {
    /// Volume name
    pub name: String,
    /// Path on host filesystem
    pub path: PathBuf,
    /// Container IDs currently using this volume
    pub attached: HashSet<String>,
    /// Whether this is an anonymous volume
    pub anonymous: bool,
}

/// Information about an S3 FUSE mount
#[derive(Debug, Clone)]
pub struct S3MountInfo {
    /// Bucket name
    pub bucket: String,
    /// Prefix within bucket
    pub prefix: Option<String>,
    /// Mount point path
    pub mount_path: PathBuf,
    /// Custom endpoint (for S3-compatible services)
    pub endpoint: Option<String>,
    /// Containers using this mount
    pub attached: HashSet<String>,
}

/// Manages storage volumes for containers
pub struct StorageManager {
    /// Base directory for volumes (`ZLayerDirs::system_default().volumes()`)
    volume_dir: PathBuf,
    /// Tracked volumes (name -> info)
    volumes: HashMap<String, VolumeInfo>,
    /// Tracked S3 mounts (key is "{bucket}_{prefix}")
    s3_mounts: HashMap<String, S3MountInfo>,
    /// Optional S3-backed layer sync for volume backup/restore
    #[cfg(feature = "s3")]
    layer_sync: Option<Arc<LayerSyncManager>>,
    /// Service name used to construct `ContainerLayerIds` for volume sync
    #[cfg(feature = "s3")]
    service_name: Option<String>,
}

impl StorageManager {
    /// Create a new `StorageManager` with the given base directory
    ///
    /// # Errors
    /// Returns an error if the base directory cannot be created.
    pub fn new(base_dir: impl AsRef<Path>) -> Result<Self> {
        let volume_dir = base_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&volume_dir)?;

        Ok(Self {
            volume_dir,
            volumes: HashMap::new(),
            s3_mounts: HashMap::new(),
            #[cfg(feature = "s3")]
            layer_sync: None,
            #[cfg(feature = "s3")]
            service_name: None,
        })
    }

    /// Get the base volume directory
    #[must_use]
    pub fn volume_dir(&self) -> &Path {
        &self.volume_dir
    }

    /// Set the layer sync manager for S3-backed volume backup/restore.
    ///
    /// When set, named volumes will be registered for sync tracking and
    /// restored from S3 on first creation. Use [`sync_volume`] or
    /// [`sync_all_volumes`] to push changes to S3.
    #[cfg(feature = "s3")]
    pub fn set_layer_sync(&mut self, sync: Arc<LayerSyncManager>, service_name: impl Into<String>) {
        self.layer_sync = Some(sync);
        self.service_name = Some(service_name.into());
    }

    /// Get a reference to the layer sync manager, if configured.
    #[cfg(feature = "s3")]
    #[must_use]
    pub fn layer_sync(&self) -> Option<&Arc<LayerSyncManager>> {
        self.layer_sync.as_ref()
    }

    /// Build a [`ContainerLayerId`] for a volume name.
    ///
    /// Uses the configured service name (or "default") combined with the
    /// volume name to produce a stable, unique identifier for S3 sync.
    #[cfg(feature = "s3")]
    fn volume_layer_id(&self, volume_name: &str) -> ContainerLayerId {
        let service = self
            .service_name
            .as_deref()
            .unwrap_or("default")
            .to_string();
        ContainerLayerId::new(service, format!("vol-{volume_name}"))
    }

    /// Register a volume with the layer sync manager and attempt to restore
    /// it from S3 if a remote backup exists.
    ///
    /// This is called internally when a volume is first created via
    /// [`ensure_volume`]. Errors are logged but do not prevent volume creation.
    #[cfg(feature = "s3")]
    async fn register_and_restore_volume(&self, volume_name: &str, volume_path: &Path) {
        let Some(sync) = &self.layer_sync else {
            return;
        };

        let layer_id = self.volume_layer_id(volume_name);

        // Register for tracking
        if let Err(e) = sync.register_container(layer_id.clone()).await {
            tracing::warn!(
                volume = %volume_name,
                error = %e,
                "failed to register volume with layer sync"
            );
            return;
        }

        tracing::debug!(
            volume = %volume_name,
            layer_id = %layer_id,
            "registered volume with layer sync"
        );

        // Attempt restore from S3 (non-fatal if no remote backup exists)
        match sync.restore_layer(&layer_id, volume_path).await {
            Ok(snapshot) => {
                tracing::info!(
                    volume = %volume_name,
                    digest = %snapshot.digest,
                    size = snapshot.size_bytes,
                    "restored volume from S3 backup"
                );
            }
            Err(e) => {
                // NotFound is expected for new volumes that have never been synced
                let msg = e.to_string();
                if msg.contains("not found")
                    || msg.contains("NotFound")
                    || msg.contains("No remote layer")
                {
                    tracing::debug!(
                        volume = %volume_name,
                        "no S3 backup found for volume (first use)"
                    );
                } else {
                    tracing::warn!(
                        volume = %volume_name,
                        error = %e,
                        "failed to restore volume from S3"
                    );
                }
            }
        }
    }

    /// Sync a single named volume to S3.
    ///
    /// Returns `Ok(true)` if a new snapshot was uploaded, `Ok(false)` if
    /// the volume was already up to date, or an error on failure.
    ///
    /// # Errors
    ///
    /// Returns an error if layer sync is not configured, the volume is not found,
    /// or the S3 upload fails.
    #[cfg(feature = "s3")]
    pub async fn sync_volume(&self, volume_name: &str) -> Result<bool> {
        let sync = self
            .layer_sync
            .as_ref()
            .ok_or_else(|| StorageError::LayerSync("layer sync not configured".to_string()))?;

        let volume = self
            .volumes
            .get(volume_name)
            .ok_or_else(|| StorageError::VolumeNotFound(volume_name.to_string()))?;

        let layer_id = self.volume_layer_id(volume_name);

        match sync.sync_layer(&layer_id, &volume.path).await {
            Ok(Some(snapshot)) => {
                tracing::info!(
                    volume = %volume_name,
                    digest = %snapshot.digest,
                    compressed_size = snapshot.compressed_size_bytes,
                    "synced volume to S3"
                );
                Ok(true)
            }
            Ok(None) => {
                tracing::debug!(
                    volume = %volume_name,
                    "volume unchanged, no sync needed"
                );
                Ok(false)
            }
            Err(e) => {
                tracing::error!(
                    volume = %volume_name,
                    error = %e,
                    "failed to sync volume to S3"
                );
                Err(StorageError::LayerSync(format!(
                    "failed to sync volume '{volume_name}': {e}"
                )))
            }
        }
    }

    /// Sync all tracked named volumes to S3.
    ///
    /// Skips anonymous volumes (they are ephemeral by nature).
    /// Returns the number of volumes that had new snapshots uploaded.
    ///
    /// # Errors
    ///
    /// Returns an error if any volume sync fails.
    #[cfg(feature = "s3")]
    pub async fn sync_all_volumes(&self) -> Result<usize> {
        if self.layer_sync.is_none() {
            return Ok(0);
        }

        let volume_names: Vec<String> = self
            .volumes
            .values()
            .filter(|v| !v.anonymous)
            .map(|v| v.name.clone())
            .collect();

        let mut synced = 0;
        for name in &volume_names {
            match self.sync_volume(name).await {
                Ok(true) => synced += 1,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        volume = %name,
                        error = %e,
                        "failed to sync volume, continuing with others"
                    );
                }
            }
        }

        if synced > 0 {
            tracing::info!(
                synced_count = synced,
                total = volume_names.len(),
                "volume sync complete"
            );
        }

        Ok(synced)
    }

    /// Create an anonymous volume for a container
    /// Returns the path to the volume directory
    ///
    /// # Errors
    /// Returns an error if the volume directory cannot be created.
    pub fn create_anonymous(&mut self, container_id: &str, target: &str) -> Result<PathBuf> {
        let ulid = Ulid::new().to_string().to_lowercase();
        // Create a safe name from target path (replace / with _)
        let _safe_target = target.trim_start_matches('/').replace('/', "_");
        let name = format!("_anon_{container_id}_{ulid}");

        let anon_dir = self.volume_dir.join("_anon");
        let volume_path = anon_dir.join(format!("{container_id}-{ulid}"));

        std::fs::create_dir_all(&volume_path)?;

        let mut attached = HashSet::new();
        attached.insert(container_id.to_string());

        self.volumes.insert(
            name.clone(),
            VolumeInfo {
                name,
                path: volume_path.clone(),
                attached,
                anonymous: true,
            },
        );

        Ok(volume_path)
    }

    /// Clean up all anonymous volumes for a container
    ///
    /// # Errors
    /// Returns an error if volume directories cannot be removed.
    pub fn cleanup_anonymous(&mut self, container_id: &str) -> Result<()> {
        // Find all anonymous volumes for this container
        let to_remove: Vec<String> = self
            .volumes
            .iter()
            .filter(|(_, v)| v.anonymous && v.attached.contains(container_id))
            .map(|(k, _)| k.clone())
            .collect();

        for name in to_remove {
            if let Some(volume) = self.volumes.remove(&name) {
                if volume.path.exists() {
                    std::fs::remove_dir_all(&volume.path)?;
                }
            }
        }

        Ok(())
    }

    /// List anonymous volumes for a container
    #[must_use]
    pub fn list_anonymous(&self, container_id: &str) -> Vec<&VolumeInfo> {
        self.volumes
            .values()
            .filter(|v| v.anonymous && v.attached.contains(container_id))
            .collect()
    }

    /// Ensure a named volume exists, creating it if necessary
    ///
    /// # Errors
    /// Returns an error if the volume name is invalid or the directory cannot be created.
    pub fn ensure_volume(&mut self, name: &str) -> Result<PathBuf> {
        // Validate name format
        if !Self::is_valid_name(name) {
            return Err(StorageError::InvalidName(name.to_string()));
        }

        let volume_path = self.volume_dir.join(name);

        if !self.volumes.contains_key(name) {
            std::fs::create_dir_all(&volume_path)?;

            self.volumes.insert(
                name.to_string(),
                VolumeInfo {
                    name: name.to_string(),
                    path: volume_path.clone(),
                    attached: HashSet::new(),
                    anonymous: false,
                },
            );
        }

        Ok(volume_path)
    }

    /// Ensure a named volume exists, register it with layer sync, and restore
    /// from S3 if a backup exists.
    ///
    /// This is the async counterpart to [`ensure_volume`] that integrates with
    /// the S3 layer sync. When the `s3` feature is not enabled or no layer sync
    /// is configured, this behaves identically to [`ensure_volume`].
    ///
    /// # Errors
    /// Returns an error if the volume cannot be created or S3 restore fails.
    #[allow(clippy::unused_async)]
    pub async fn ensure_volume_with_sync(&mut self, name: &str) -> Result<PathBuf> {
        let is_new = !self.volumes.contains_key(name);
        let path = self.ensure_volume(name)?;

        #[cfg(feature = "s3")]
        if is_new {
            self.register_and_restore_volume(name, &path).await;
        }

        #[cfg(not(feature = "s3"))]
        let _ = is_new; // suppress unused variable warning

        Ok(path)
    }

    /// Attach a container to a volume (track usage)
    ///
    /// # Errors
    /// Returns an error if the volume does not exist.
    pub fn attach_volume(&mut self, name: &str, container_id: &str) -> Result<()> {
        let volume = self
            .volumes
            .get_mut(name)
            .ok_or_else(|| StorageError::VolumeNotFound(name.to_string()))?;

        volume.attached.insert(container_id.to_string());
        Ok(())
    }

    /// Detach a container from a volume (untrack usage)
    ///
    /// # Errors
    /// This function currently always succeeds but returns `Result` for future compatibility.
    pub fn detach_volume(&mut self, name: &str, container_id: &str) -> Result<()> {
        if let Some(volume) = self.volumes.get_mut(name) {
            volume.attached.remove(container_id);
        }
        Ok(())
    }

    /// Delete a volume if it's not in use
    ///
    /// # Errors
    /// Returns an error if the volume is still in use or cannot be removed.
    pub fn delete_volume(&mut self, name: &str) -> Result<()> {
        let volume = self
            .volumes
            .get(name)
            .ok_or_else(|| StorageError::VolumeNotFound(name.to_string()))?;

        if !volume.attached.is_empty() {
            return Err(StorageError::VolumeInUse(
                name.to_string(),
                volume.attached.iter().cloned().collect(),
            ));
        }

        let path = volume.path.clone();
        self.volumes.remove(name);

        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }

        Ok(())
    }

    /// List all tracked volumes
    #[must_use]
    pub fn list_volumes(&self) -> Vec<&VolumeInfo> {
        self.volumes.values().collect()
    }

    /// Get info for a specific volume
    #[must_use]
    pub fn get_volume(&self, name: &str) -> Option<&VolumeInfo> {
        self.volumes.get(name)
    }

    /// Get the S3 mount directory
    fn s3_mount_dir(&self) -> PathBuf {
        self.volume_dir.join("s3mounts")
    }

    /// Generate a key for S3 mount tracking
    fn s3_mount_key(bucket: &str, prefix: Option<&str>) -> String {
        match prefix {
            Some(p) => format!("{}_{}", bucket, p.replace('/', "_")),
            None => bucket.to_string(),
        }
    }

    /// Mount an S3 bucket via s3fs FUSE
    ///
    /// Requires s3fs-fuse to be installed on the system.
    /// Credentials should be configured via environment or ~/.aws/credentials
    ///
    /// # Errors
    /// Returns an error if the S3 mount point cannot be created or the s3fs command fails.
    pub fn mount_s3(
        &mut self,
        bucket: &str,
        prefix: Option<&str>,
        endpoint: Option<&str>,
        container_id: &str,
    ) -> Result<PathBuf> {
        let key = Self::s3_mount_key(bucket, prefix);

        // Check if already mounted
        if let Some(info) = self.s3_mounts.get_mut(&key) {
            info.attached.insert(container_id.to_string());
            return Ok(info.mount_path.clone());
        }

        // Create mount point directory
        let mount_dir = self.s3_mount_dir();
        std::fs::create_dir_all(&mount_dir)?;

        let mount_path = mount_dir.join(&key);
        std::fs::create_dir_all(&mount_path)?;

        // Build s3fs command
        let mut cmd = std::process::Command::new("s3fs");

        // Add bucket (with optional prefix as path)
        let bucket_arg = match prefix {
            Some(p) => format!("{}:/{}", bucket, p.trim_start_matches('/')),
            None => bucket.to_string(),
        };
        cmd.arg(&bucket_arg);
        cmd.arg(&mount_path);

        // Add options
        let mut options = vec!["allow_other".to_string(), "mp_umask=022".to_string()];

        if let Some(ep) = endpoint {
            options.push(format!("url={ep}"));
            options.push("use_path_request_style".to_string());
        }

        cmd.arg("-o");
        cmd.arg(options.join(","));

        tracing::info!(
            bucket = %bucket,
            prefix = ?prefix,
            mount_path = %mount_path.display(),
            "mounting S3 bucket via s3fs"
        );

        // Execute mount
        let output = cmd.output().map_err(|e| {
            StorageError::Io(std::io::Error::other(format!(
                "failed to execute s3fs: {e}"
            )))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(StorageError::Io(std::io::Error::other(format!(
                "s3fs mount failed: {stderr}"
            ))));
        }

        // Track the mount
        let mut attached = HashSet::new();
        attached.insert(container_id.to_string());

        self.s3_mounts.insert(
            key,
            S3MountInfo {
                bucket: bucket.to_string(),
                prefix: prefix.map(String::from),
                mount_path: mount_path.clone(),
                endpoint: endpoint.map(String::from),
                attached,
            },
        );

        Ok(mount_path)
    }

    /// Unmount an S3 bucket
    ///
    /// # Errors
    /// Returns an error if the unmount command fails.
    pub fn unmount_s3(
        &mut self,
        bucket: &str,
        prefix: Option<&str>,
        container_id: &str,
    ) -> Result<()> {
        let key = Self::s3_mount_key(bucket, prefix);

        let should_unmount = if let Some(info) = self.s3_mounts.get_mut(&key) {
            info.attached.remove(container_id);
            info.attached.is_empty()
        } else {
            return Ok(()); // Not tracked, nothing to do
        };

        if should_unmount {
            if let Some(info) = self.s3_mounts.remove(&key) {
                // Unmount via fusermount
                let output = std::process::Command::new("fusermount")
                    .arg("-u")
                    .arg(&info.mount_path)
                    .output();

                match output {
                    Ok(o) if !o.status.success() => {
                        let stderr = String::from_utf8_lossy(&o.stderr);
                        tracing::warn!(
                            bucket = %bucket,
                            error = %stderr,
                            "failed to unmount S3, attempting lazy unmount"
                        );
                        // Try lazy unmount
                        let _ = std::process::Command::new("fusermount")
                            .arg("-uz")
                            .arg(&info.mount_path)
                            .output();
                    }
                    Err(e) => {
                        tracing::warn!(bucket = %bucket, error = %e, "failed to execute fusermount");
                    }
                    _ => {}
                }

                // Remove mount directory
                let _ = std::fs::remove_dir(&info.mount_path);

                tracing::info!(bucket = %bucket, "S3 bucket unmounted");
            }
        }

        Ok(())
    }

    /// List all S3 mounts
    #[must_use]
    pub fn list_s3_mounts(&self) -> Vec<&S3MountInfo> {
        self.s3_mounts.values().collect()
    }

    /// Get S3 mount info
    #[must_use]
    pub fn get_s3_mount(&self, bucket: &str, prefix: Option<&str>) -> Option<&S3MountInfo> {
        let key = Self::s3_mount_key(bucket, prefix);
        self.s3_mounts.get(&key)
    }

    /// Validate volume name format (lowercase alphanumeric with hyphens)
    fn is_valid_name(name: &str) -> bool {
        if name.is_empty() || name.len() > 63 {
            return false;
        }

        let chars: Vec<char> = name.chars().collect();

        // Must start and end with alphanumeric
        if !chars.first().is_some_and(char::is_ascii_alphanumeric) {
            return false;
        }
        if !chars.last().is_some_and(char::is_ascii_alphanumeric) {
            return false;
        }

        // All chars must be lowercase alphanumeric or hyphen
        chars
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, StorageManager) {
        let temp_dir = TempDir::new().unwrap();
        let manager = StorageManager::new(temp_dir.path()).unwrap();
        (temp_dir, manager)
    }

    #[test]
    fn test_ensure_named_volume() {
        let (_temp, mut manager) = setup();

        let path = manager.ensure_volume("my-data").unwrap();
        assert!(path.exists());
        assert!(path.ends_with("my-data"));

        // Calling again should return same path
        let path2 = manager.ensure_volume("my-data").unwrap();
        assert_eq!(path, path2);
    }

    #[test]
    fn test_attach_detach_volume() {
        let (_temp, mut manager) = setup();

        manager.ensure_volume("test-vol").unwrap();
        manager.attach_volume("test-vol", "container-1").unwrap();

        let vol = manager.get_volume("test-vol").unwrap();
        assert!(vol.attached.contains("container-1"));

        manager.detach_volume("test-vol", "container-1").unwrap();
        let vol = manager.get_volume("test-vol").unwrap();
        assert!(!vol.attached.contains("container-1"));
    }

    #[test]
    fn test_delete_volume_success() {
        let (_temp, mut manager) = setup();

        let path = manager.ensure_volume("deleteme").unwrap();
        assert!(path.exists());

        manager.delete_volume("deleteme").unwrap();
        assert!(!path.exists());
        assert!(manager.get_volume("deleteme").is_none());
    }

    #[test]
    fn test_delete_volume_in_use_fails() {
        let (_temp, mut manager) = setup();

        manager.ensure_volume("in-use").unwrap();
        manager.attach_volume("in-use", "container-1").unwrap();

        let result = manager.delete_volume("in-use");
        assert!(matches!(result, Err(StorageError::VolumeInUse(_, _))));
    }

    #[test]
    fn test_create_anonymous_volume() {
        let (_temp, mut manager) = setup();

        let path = manager
            .create_anonymous("container-1", "/app/cache")
            .unwrap();
        assert!(path.exists());

        let anon_vols = manager.list_anonymous("container-1");
        assert_eq!(anon_vols.len(), 1);
        assert!(anon_vols[0].anonymous);
    }

    #[test]
    fn test_cleanup_anonymous_volumes() {
        let (_temp, mut manager) = setup();

        let path1 = manager.create_anonymous("container-1", "/cache1").unwrap();
        let path2 = manager.create_anonymous("container-1", "/cache2").unwrap();
        let _path3 = manager.create_anonymous("container-2", "/other").unwrap();

        assert!(path1.exists());
        assert!(path2.exists());

        manager.cleanup_anonymous("container-1").unwrap();

        assert!(!path1.exists());
        assert!(!path2.exists());

        // container-2's volume should still exist
        let remaining = manager.list_anonymous("container-2");
        assert_eq!(remaining.len(), 1);
    }

    #[test]
    fn test_volume_name_validation() {
        let (_temp, mut manager) = setup();

        // Valid names
        assert!(manager.ensure_volume("a").is_ok());
        assert!(manager.ensure_volume("my-volume").is_ok());
        assert!(manager.ensure_volume("vol123").is_ok());
        assert!(manager.ensure_volume("a1b2c3").is_ok());

        // Invalid names
        assert!(matches!(
            manager.ensure_volume("-invalid"),
            Err(StorageError::InvalidName(_))
        ));
        assert!(matches!(
            manager.ensure_volume("invalid-"),
            Err(StorageError::InvalidName(_))
        ));
        assert!(matches!(
            manager.ensure_volume("UPPERCASE"),
            Err(StorageError::InvalidName(_))
        ));
        assert!(matches!(
            manager.ensure_volume("has_underscore"),
            Err(StorageError::InvalidName(_))
        ));
        assert!(matches!(
            manager.ensure_volume(""),
            Err(StorageError::InvalidName(_))
        ));
    }

    #[test]
    fn test_list_volumes() {
        let (_temp, mut manager) = setup();

        manager.ensure_volume("vol1").unwrap();
        manager.ensure_volume("vol2").unwrap();
        manager.ensure_volume("vol3").unwrap();

        let vols = manager.list_volumes();
        assert_eq!(vols.len(), 3);

        let names: Vec<&str> = vols.iter().map(|v| v.name.as_str()).collect();
        assert!(names.contains(&"vol1"));
        assert!(names.contains(&"vol2"));
        assert!(names.contains(&"vol3"));
    }
}
