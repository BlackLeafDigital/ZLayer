//! ZQL database replication to S3
//!
//! Provides automatic backup and restore of ZQL databases to S3, using a
//! [`ChangeListener`]-based dirty flag to detect mutations. This enables
//! crash-tolerant persistence and cross-node database restoration.
//!
//! # Features
//!
//! - **Change Detection**: Receives notifications from ZQL via [`DirtyFlagListener`]
//! - **Network Tolerance**: Local write cache buffers snapshots during network outages
//! - **Automatic Snapshots**: Periodic compressed snapshots with configurable intervals
//! - **S3 Backend**: Stores snapshots in S3 with zstd compression
//! - **Auto-Restore**: Automatically restores from S3 on startup if no local DB
//!
//! # Architecture
//!
//! ```text
//! ZQL Database
//!        |
//!        v
//! DirtyFlagListener (AtomicBool) --> ZqlReplicator
//!                                        |
//!                              +---------+---------+
//!                              |                   |
//!                              v                   v
//!                     Periodic Snapshot       Cache Upload
//!                     (tar+zstd data_dir)    Worker (S3)
//! ```
//!
//! # Example
//!
//! ```rust,no_run
//! use zlayer_storage_zql::replicator::{ZqlReplicator, ZqlReplicatorConfig, DirtyFlagListener};
//! use zlayer_storage_zql::config::LayerStorageConfig;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let replicator_config = ZqlReplicatorConfig::new(
//!         "/var/lib/myapp/zql-data",
//!         "my-bucket",
//!         "zql-backups/myapp/",
//!     )
//!     .with_auto_restore(true)
//!     .with_cache_dir("/tmp/zlayer-replicator/cache");
//!
//!     let s3_config = LayerStorageConfig::new("my-bucket");
//!     let replicator = ZqlReplicator::new(replicator_config, &s3_config).await?;
//!
//!     // Register the dirty flag on your ZQL Database
//!     // let listener = DirtyFlagListener::new(replicator.dirty_flag());
//!     // db.add_change_listener(Box::new(listener));
//!
//!     // Optionally restore from S3 on startup
//!     replicator.restore().await?;
//!
//!     // Start background replication
//!     replicator.start().await?;
//!
//!     // ... run your application ...
//!
//!     // Graceful shutdown
//!     replicator.flush().await?;
//!     Ok(())
//! }
//! ```

mod cache;
mod restore;
mod s3_backend;

pub use crate::config::{LayerStorageConfig, ZqlReplicatorConfig};
use crate::error::Result;
use aws_sdk_s3::Client as S3Client;
use cache::WriteCache;
use restore::RestoreManager;
use s3_backend::S3Backend;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, warn};
use zql::events::{ChangeEvent, ChangeListener};

pub use cache::CacheEntry;
pub use s3_backend::ReplicationMetadata;

/// A [`ChangeListener`] that sets an [`AtomicBool`] dirty flag on any mutation.
///
/// This bridges the synchronous ZQL write path with the async replicator.
/// Register an instance on the [`zql::Database`] via
/// [`add_change_listener`](zql::Database::add_change_listener) so the
/// [`ZqlReplicator`] knows when to create a new snapshot.
pub struct DirtyFlagListener {
    dirty: Arc<AtomicBool>,
}

impl DirtyFlagListener {
    /// Create a new listener that sets the given dirty flag on every mutation.
    #[must_use]
    pub fn new(dirty: Arc<AtomicBool>) -> Self {
        Self { dirty }
    }
}

impl ChangeListener for DirtyFlagListener {
    fn on_change(&self, _event: ChangeEvent) {
        self.dirty.store(true, Ordering::Release);
    }
}

/// Current replication status
#[derive(Debug, Clone)]
pub struct ReplicationStatus {
    /// Whether the replicator is running
    pub running: bool,
    /// Number of snapshot entries pending upload
    pub pending_entries: usize,
    /// Total bytes pending upload
    pub pending_bytes: u64,
    /// Last successful snapshot timestamp
    pub last_snapshot: Option<chrono::DateTime<chrono::Utc>>,
    /// Number of failed upload attempts
    pub failed_uploads: u64,
}

/// ZQL database replicator to S3
///
/// Monitors a ZQL database for mutations via a dirty flag (set by
/// [`DirtyFlagListener`]) and periodically creates compressed snapshots of the
/// data directory, uploading them to S3 for persistence and disaster recovery.
///
/// **Important**: The `ZqlReplicator` does *not* hold a reference to the
/// `Database`. The caller is responsible for:
/// 1. Creating the replicator
/// 2. Getting the dirty flag via [`dirty_flag()`](Self::dirty_flag)
/// 3. Creating a [`DirtyFlagListener`] with that flag
/// 4. Registering it on the `Database` via `db.add_change_listener()`
pub struct ZqlReplicator {
    config: ZqlReplicatorConfig,
    s3_backend: Arc<S3Backend>,
    cache: Arc<WriteCache>,
    restore_manager: RestoreManager,

    /// Dirty flag set by the [`DirtyFlagListener`] when any mutation occurs
    dirty: Arc<AtomicBool>,

    // Runtime state
    running: Arc<AtomicBool>,
    last_snapshot: Arc<RwLock<Option<chrono::DateTime<chrono::Utc>>>>,
    failed_uploads: Arc<AtomicU64>,

    // Shutdown channel
    shutdown_tx: mpsc::Sender<()>,
    /// Receiver for shutdown signals (used by future graceful shutdown handling)
    #[allow(dead_code)]
    shutdown_rx: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl ZqlReplicator {
    /// Create a new ZQL replicator
    ///
    /// # Arguments
    ///
    /// * `config` - Replicator configuration
    /// * `s3_config` - S3/Layer storage configuration for credentials and endpoint
    ///
    /// # Errors
    ///
    /// Returns an error if the cache directory cannot be created or if S3 client
    /// initialization fails.
    pub async fn new(config: ZqlReplicatorConfig, s3_config: &LayerStorageConfig) -> Result<Self> {
        // Ensure cache directory exists
        tokio::fs::create_dir_all(&config.cache_dir).await?;

        // Initialize S3 client
        let mut aws_config_builder = aws_config::from_env();

        if let Some(region) = &s3_config.region {
            aws_config_builder =
                aws_config_builder.region(aws_sdk_s3::config::Region::new(region.clone()));
        }

        let aws_config = aws_config_builder.load().await;

        let s3_client_config = if let Some(endpoint) = &s3_config.endpoint_url {
            aws_sdk_s3::config::Builder::from(&aws_config)
                .endpoint_url(endpoint)
                .force_path_style(true)
                .build()
        } else {
            aws_sdk_s3::config::Builder::from(&aws_config).build()
        };

        let s3_client = S3Client::from_conf(s3_client_config);

        // Create S3 backend
        let s3_backend = Arc::new(S3Backend::new(
            s3_client,
            config.s3_bucket.clone(),
            config.s3_prefix.clone(),
            s3_config.compression_level,
        ));

        // Create write cache
        let cache = Arc::new(WriteCache::new(
            config.cache_dir.clone(),
            config.max_cache_size,
        ));

        // Create restore manager
        let restore_manager = RestoreManager::new(
            config.data_dir.clone(),
            s3_backend.clone(),
            config.cache_dir.clone(),
        );

        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);

        Ok(Self {
            config,
            s3_backend,
            cache,
            restore_manager,
            dirty: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            last_snapshot: Arc::new(RwLock::new(None)),
            failed_uploads: Arc::new(AtomicU64::new(0)),
            shutdown_tx,
            shutdown_rx: Arc::new(Mutex::new(shutdown_rx)),
        })
    }

    /// Returns the dirty flag shared with [`DirtyFlagListener`].
    ///
    /// The caller should create a `DirtyFlagListener` with this flag and
    /// register it on the ZQL `Database` so mutation events flow to the
    /// replicator.
    #[must_use]
    pub fn dirty_flag(&self) -> Arc<AtomicBool> {
        self.dirty.clone()
    }

    /// Start the replicator background tasks
    ///
    /// This spawns background tasks for:
    /// - Periodic snapshot creation (checks dirty flag each interval)
    /// - Cache upload worker (handles retries with exponential backoff)
    ///
    /// # Errors
    ///
    /// Returns an error if the data directory does not exist and auto-restore
    /// fails.
    pub async fn start(&self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!(
            "Starting ZQL replicator for {}",
            self.config.data_dir.display()
        );

        // Check if data directory exists, optionally restore
        if !self.config.data_dir.exists() {
            if self.config.auto_restore {
                info!("Data directory not found, attempting auto-restore from S3");
                match self.restore().await {
                    Ok(true) => info!("Database restored from S3"),
                    Ok(false) => info!("No backup found in S3, starting fresh"),
                    Err(e) => warn!("Auto-restore failed: {}", e),
                }
            } else {
                debug!("Data directory not found and auto_restore is disabled");
            }
        }

        self.running.store(true, Ordering::SeqCst);

        // Start cache upload worker
        self.spawn_upload_worker();

        // Start periodic snapshot task
        self.spawn_snapshot_task();

        info!("ZQL replicator started");
        Ok(())
    }

    /// Force flush all pending changes to S3
    ///
    /// Call this before shutdown to ensure all changes are persisted. This will:
    /// 1. Create a final snapshot if dirty
    /// 2. Upload all pending cache entries
    /// 3. Wait for uploads to complete
    ///
    /// # Errors
    ///
    /// Returns an error if the final snapshot or upload fails.
    pub async fn flush(&self) -> Result<()> {
        info!("Flushing ZQL replicator");

        // Signal shutdown
        self.running.store(false, Ordering::SeqCst);
        let _ = self.shutdown_tx.send(()).await;

        // Create final snapshot if data directory exists
        if self.config.data_dir.exists() {
            self.create_snapshot().await?;
        }

        // Upload all pending cache entries
        while let Some(entry) = self.cache.pop_oldest().await? {
            match self.s3_backend.upload_snapshot(&entry.data).await {
                Ok(()) => {
                    debug!("Flushed cached snapshot {}", entry.sequence);
                    self.cache.remove(&entry).await?;
                }
                Err(e) => {
                    error!("Failed to flush cached snapshot {}: {}", entry.sequence, e);
                    return Err(e);
                }
            }
        }

        info!("ZQL replicator flushed");
        Ok(())
    }

    /// Restore database from S3
    ///
    /// Downloads the latest snapshot (a tar+zstd archive of the ZQL data
    /// directory) and extracts it to the configured `data_dir`.
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if a backup was found and restored
    /// - `Ok(false)` if no backup was found in S3
    /// - `Err(_)` if restore failed
    ///
    /// # Errors
    ///
    /// Returns an error if downloading or extracting the snapshot fails.
    pub async fn restore(&self) -> Result<bool> {
        self.restore_manager.restore().await
    }

    /// Get current replication status
    #[must_use]
    pub fn status(&self) -> ReplicationStatus {
        let (pending_entries, pending_bytes) = self.cache.stats();

        ReplicationStatus {
            running: self.running.load(Ordering::SeqCst),
            pending_entries,
            pending_bytes,
            last_snapshot: None, // Would need async to read
            failed_uploads: self.failed_uploads.load(Ordering::SeqCst),
        }
    }

    /// Create a snapshot of the ZQL data directory
    ///
    /// Tars and compresses the data directory, then uploads the resulting
    /// archive to S3. The caller is responsible for flushing the `Database`
    /// memtable before calling this (they own the `Database`).
    async fn create_snapshot(&self) -> Result<()> {
        info!(
            "Creating ZQL snapshot from {}",
            self.config.data_dir.display()
        );

        // Create staging path for the tarball
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let staging_path = self
            .config
            .cache_dir
            .join(format!("{timestamp}.zql.tar.zst"));

        // Create compressed tarball of the data directory
        let data_dir = self.config.data_dir.clone();
        let output = staging_path.clone();
        let snapshot_info = tokio::task::spawn_blocking(move || {
            crate::snapshot::create_snapshot(data_dir, output, 3)
        })
        .await
        .map_err(|e| {
            crate::error::LayerStorageError::Io(std::io::Error::other(format!(
                "snapshot task panicked: {e}"
            )))
        })??;

        // Read the tarball bytes
        let tarball_bytes = tokio::fs::read(&staging_path).await?;

        // Upload to S3
        match self.s3_backend.upload_snapshot(&tarball_bytes).await {
            Ok(()) => {
                // Update metadata
                if let Err(e) = self.s3_backend.update_metadata().await {
                    error!("Failed to update metadata: {}", e);
                }
                *self.last_snapshot.write().await = Some(chrono::Utc::now());
                info!(
                    "ZQL snapshot uploaded ({} bytes compressed, {} files)",
                    snapshot_info.compressed_size_bytes, snapshot_info.file_count
                );
            }
            Err(e) => {
                warn!("Failed to upload snapshot to S3, caching locally: {}", e);
                // Cache for later upload
                #[allow(clippy::cast_sign_loss)]
                let seq = chrono::Utc::now().timestamp_millis() as u64;
                if let Err(cache_err) = self.cache.add(seq, tarball_bytes).await {
                    error!("Failed to cache snapshot: {}", cache_err);
                }
            }
        }

        // Clean up staging file
        let _ = tokio::fs::remove_file(&staging_path).await;

        Ok(())
    }

    /// Spawn the cache upload worker
    fn spawn_upload_worker(&self) {
        let running = self.running.clone();
        let cache = self.cache.clone();
        let s3_backend = self.s3_backend.clone();
        let failed_uploads = self.failed_uploads.clone();

        tokio::spawn(async move {
            let mut retry_delay = tokio::time::Duration::from_secs(1);
            let max_retry_delay = tokio::time::Duration::from_secs(60);

            while running.load(Ordering::SeqCst) {
                // Try to upload oldest cached entry
                match cache.pop_oldest().await {
                    Ok(Some(entry)) => {
                        match s3_backend.upload_snapshot(&entry.data).await {
                            Ok(()) => {
                                debug!("Uploaded cached snapshot {}", entry.sequence);
                                if let Err(e) = cache.remove(&entry).await {
                                    error!("Failed to remove cached entry: {}", e);
                                }
                                retry_delay = tokio::time::Duration::from_secs(1);
                            }
                            Err(e) => {
                                warn!("Failed to upload cached snapshot: {}", e);
                                failed_uploads.fetch_add(1, Ordering::SeqCst);

                                // Re-add to cache for retry
                                if let Err(e) = cache.add(entry.sequence, entry.data).await {
                                    error!("Failed to re-cache entry: {}", e);
                                }

                                // Exponential backoff
                                tokio::time::sleep(retry_delay).await;
                                retry_delay = std::cmp::min(retry_delay * 2, max_retry_delay);
                            }
                        }
                    }
                    Ok(None) => {
                        // No entries to upload, wait a bit
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    }
                    Err(e) => {
                        error!("Cache error: {}", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    }
                }
            }
        });
    }

    /// Spawn the periodic snapshot task
    fn spawn_snapshot_task(&self) {
        let running = self.running.clone();
        let dirty = self.dirty.clone();
        let interval = tokio::time::Duration::from_secs(self.config.snapshot_interval_secs);
        let data_dir = self.config.data_dir.clone();
        let cache_dir = self.config.cache_dir.clone();
        let s3_backend = self.s3_backend.clone();
        let last_snapshot = self.last_snapshot.clone();
        let cache = self.cache.clone();

        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(interval);
            interval_timer.tick().await; // Skip first immediate tick

            while running.load(Ordering::SeqCst) {
                interval_timer.tick().await;

                if !running.load(Ordering::SeqCst) {
                    break;
                }

                // Only snapshot if data has changed
                if !dirty.swap(false, Ordering::AcqRel) {
                    debug!("No mutations since last snapshot, skipping");
                    continue;
                }

                info!("Creating periodic ZQL snapshot");

                if !data_dir.exists() {
                    debug!("Data directory doesn't exist, skipping snapshot");
                    continue;
                }

                // Create compressed tarball
                let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
                let staging_path = cache_dir.join(format!("{timestamp}.zql.tar.zst"));

                let snap_data_dir = data_dir.clone();
                let snap_output = staging_path.clone();
                let snapshot_result = tokio::task::spawn_blocking(move || {
                    crate::snapshot::create_snapshot(snap_data_dir, snap_output, 3)
                })
                .await;

                let snapshot_info = match snapshot_result {
                    Ok(Ok(info)) => info,
                    Ok(Err(e)) => {
                        error!("Failed to create snapshot: {}", e);
                        continue;
                    }
                    Err(e) => {
                        error!("Snapshot task panicked: {}", e);
                        continue;
                    }
                };

                // Read tarball and upload
                match tokio::fs::read(&staging_path).await {
                    Ok(tarball_bytes) => match s3_backend.upload_snapshot(&tarball_bytes).await {
                        Ok(()) => {
                            info!(
                                "Periodic snapshot uploaded ({} bytes, {} files)",
                                snapshot_info.compressed_size_bytes, snapshot_info.file_count
                            );
                            if let Err(e) = s3_backend.update_metadata().await {
                                error!("Failed to update metadata: {}", e);
                            }
                            *last_snapshot.write().await = Some(chrono::Utc::now());
                        }
                        Err(e) => {
                            warn!("Failed to upload snapshot, caching: {}", e);
                            #[allow(clippy::cast_sign_loss)]
                            let seq = chrono::Utc::now().timestamp_millis() as u64;
                            if let Err(cache_err) = cache.add(seq, tarball_bytes).await {
                                error!("Failed to cache snapshot: {}", cache_err);
                            }
                        }
                    },
                    Err(e) => {
                        error!("Failed to read snapshot tarball: {}", e);
                    }
                }

                // Clean up staging file
                let _ = tokio::fs::remove_file(&staging_path).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dirty_flag_listener() {
        let dirty = Arc::new(AtomicBool::new(false));
        let listener = DirtyFlagListener::new(dirty.clone());

        assert!(!dirty.load(Ordering::Acquire));

        let event = ChangeEvent::new(
            1,
            "test_store".to_string(),
            zql::events::MutationKind::Insert,
            vec![42],
            1,
        );
        listener.on_change(event);

        assert!(dirty.load(Ordering::Acquire));
    }

    #[test]
    fn test_dirty_flag_reset() {
        let dirty = Arc::new(AtomicBool::new(false));
        let listener = DirtyFlagListener::new(dirty.clone());

        let event = ChangeEvent::new(
            1,
            "test".to_string(),
            zql::events::MutationKind::Insert,
            vec![],
            1,
        );
        listener.on_change(event);
        assert!(dirty.load(Ordering::Acquire));

        // Simulate the replicator resetting the flag
        dirty.store(false, Ordering::Release);
        assert!(!dirty.load(Ordering::Acquire));
    }

    #[test]
    fn test_replication_status_default() {
        let status = ReplicationStatus {
            running: false,
            pending_entries: 0,
            pending_bytes: 0,
            last_snapshot: None,
            failed_uploads: 0,
        };
        assert!(!status.running);
        assert_eq!(status.pending_entries, 0);
    }
}
