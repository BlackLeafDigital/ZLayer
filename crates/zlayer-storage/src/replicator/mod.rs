//! SQLite WAL-based replication to S3
//!
//! Provides automatic backup and restore of SQLite databases to S3, using WAL
//! (Write-Ahead Logging) for incremental replication. This enables crash-tolerant
//! persistence and cross-node database restoration.
//!
//! # Features
//!
//! - **WAL Monitoring**: Detects database changes via WAL file modifications
//! - **Network Tolerance**: Local write cache buffers changes during network outages
//! - **Automatic Snapshots**: Periodic full database snapshots with configurable intervals
//! - **S3 Backend**: Stores snapshots and WAL segments in S3 with zstd compression
//! - **Auto-Restore**: Automatically restores from S3 on startup if local DB is missing
//!
//! # Architecture
//!
//! ```text
//! SQLite DB (WAL mode)
//!        |
//!        v
//! WAL Monitor (notify) --> Write Cache --> S3 Backend
//!        |                     |               |
//!        v                     v               v
//!   Frame Detection      FIFO Queue    Upload/Download
//! ```
//!
//! # Example
//!
//! ```rust,no_run
//! use zlayer_storage::replicator::{SqliteReplicator, SqliteReplicatorConfig};
//! use zlayer_storage::config::LayerStorageConfig;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let replicator_config = SqliteReplicatorConfig {
//!         db_path: "/var/lib/myapp/data.db".into(),
//!         s3_bucket: "my-bucket".to_string(),
//!         s3_prefix: "sqlite-backups/myapp/".to_string(),
//!         cache_dir: "/tmp/zlayer-replicator/cache".into(),
//!         max_cache_size: 100 * 1024 * 1024, // 100MB
//!         auto_restore: true,
//!         snapshot_interval_secs: 3600, // 1 hour
//!     };
//!
//!     let s3_config = LayerStorageConfig::new("my-bucket");
//!     let replicator = SqliteReplicator::new(replicator_config, &s3_config).await?;
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
mod wal_monitor;

pub use crate::config::{LayerStorageConfig, SqliteReplicatorConfig};
use crate::error::Result;
use aws_sdk_s3::Client as S3Client;
use cache::WriteCache;
use restore::RestoreManager;
use s3_backend::S3Backend;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, warn};
use wal_monitor::WalMonitor;

pub use cache::CacheEntry;
pub use s3_backend::ReplicationMetadata;
pub use wal_monitor::WalEvent;

/// Current replication status
#[derive(Debug, Clone)]
pub struct ReplicationStatus {
    /// Whether the replicator is running
    pub running: bool,
    /// Number of WAL segments pending upload
    pub pending_segments: usize,
    /// Total bytes pending upload
    pub pending_bytes: u64,
    /// Last successful snapshot timestamp
    pub last_snapshot: Option<chrono::DateTime<chrono::Utc>>,
    /// Last successful WAL sync timestamp
    pub last_wal_sync: Option<chrono::DateTime<chrono::Utc>>,
    /// Number of failed upload attempts
    pub failed_uploads: u64,
    /// Current WAL frame count
    pub wal_frame_count: u64,
}

/// SQLite WAL-based replicator to S3
///
/// Monitors a SQLite database's WAL file and replicates changes to S3 for
/// persistence and disaster recovery.
pub struct SqliteReplicator {
    config: SqliteReplicatorConfig,
    s3_backend: Arc<S3Backend>,
    cache: Arc<WriteCache>,
    wal_monitor: Arc<Mutex<Option<WalMonitor>>>,
    restore_manager: RestoreManager,

    // Runtime state
    running: Arc<AtomicBool>,
    last_snapshot: Arc<RwLock<Option<chrono::DateTime<chrono::Utc>>>>,
    last_wal_sync: Arc<RwLock<Option<chrono::DateTime<chrono::Utc>>>>,
    failed_uploads: Arc<AtomicU64>,

    // Shutdown channel
    shutdown_tx: mpsc::Sender<()>,
    /// Receiver for shutdown signals (used by future graceful shutdown handling)
    #[allow(dead_code)]
    shutdown_rx: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl SqliteReplicator {
    /// Create a new SQLite replicator
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
    pub async fn new(
        config: SqliteReplicatorConfig,
        s3_config: &LayerStorageConfig,
    ) -> Result<Self> {
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
            config.db_path.clone(),
            s3_backend.clone(),
            config.cache_dir.clone(),
        );

        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);

        Ok(Self {
            config,
            s3_backend,
            cache,
            wal_monitor: Arc::new(Mutex::new(None)),
            restore_manager,
            running: Arc::new(AtomicBool::new(false)),
            last_snapshot: Arc::new(RwLock::new(None)),
            last_wal_sync: Arc::new(RwLock::new(None)),
            failed_uploads: Arc::new(AtomicU64::new(0)),
            shutdown_tx,
            shutdown_rx: Arc::new(Mutex::new(shutdown_rx)),
        })
    }

    /// Start the replicator background tasks
    ///
    /// This spawns background tasks for:
    /// - WAL file monitoring and change detection
    /// - Cache upload worker (handles retries)
    /// - Periodic snapshot creation
    ///
    /// # Errors
    ///
    /// Returns an error if the WAL monitor cannot be started (e.g., database
    /// file doesn't exist).
    pub async fn start(&self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!(
            "Starting SQLite replicator for {}",
            self.config.db_path.display()
        );

        // Check if DB exists, optionally restore
        if !self.config.db_path.exists() {
            if self.config.auto_restore {
                info!("Database not found, attempting auto-restore from S3");
                match self.restore().await {
                    Ok(true) => info!("Database restored from S3"),
                    Ok(false) => info!("No backup found in S3, starting fresh"),
                    Err(e) => warn!("Auto-restore failed: {}", e),
                }
            } else {
                debug!("Database not found and auto_restore is disabled");
            }
        }

        self.running.store(true, Ordering::SeqCst);

        // Create WAL monitor
        let wal_path = self.wal_path();
        let wal_monitor = WalMonitor::new(wal_path.clone())?;
        *self.wal_monitor.lock().await = Some(wal_monitor);

        // Start WAL monitoring task
        self.spawn_wal_monitor_task().await;

        // Start cache upload worker
        self.spawn_upload_worker().await;

        // Start periodic snapshot task
        self.spawn_snapshot_task().await;

        info!("SQLite replicator started");
        Ok(())
    }

    /// Force flush all pending changes to S3
    ///
    /// Call this before shutdown to ensure all changes are persisted. This will:
    /// 1. Create a final snapshot
    /// 2. Upload all pending cache entries
    /// 3. Wait for uploads to complete
    ///
    /// # Errors
    ///
    /// Returns an error if the final snapshot or upload fails.
    pub async fn flush(&self) -> Result<()> {
        info!("Flushing SQLite replicator");

        // Signal shutdown
        self.running.store(false, Ordering::SeqCst);
        let _ = self.shutdown_tx.send(()).await;

        // Create final snapshot
        if self.config.db_path.exists() {
            self.create_snapshot().await?;
        }

        // Upload all pending cache entries
        while let Some(entry) = self.cache.pop_oldest().await? {
            match self.s3_backend.upload_wal_segment(&entry).await {
                Ok(()) => {
                    debug!("Flushed WAL segment {}", entry.sequence);
                    self.cache.remove(&entry).await?;
                }
                Err(e) => {
                    error!("Failed to flush WAL segment {}: {}", entry.sequence, e);
                    return Err(e);
                }
            }
        }

        info!("SQLite replicator flushed");
        Ok(())
    }

    /// Restore database from S3
    ///
    /// Downloads the latest snapshot and any subsequent WAL segments from S3,
    /// then applies them to reconstruct the database.
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if a backup was found and restored
    /// - `Ok(false)` if no backup was found in S3
    /// - `Err(_)` if restore failed
    pub async fn restore(&self) -> Result<bool> {
        self.restore_manager.restore().await
    }

    /// Get current replication status
    pub fn status(&self) -> ReplicationStatus {
        let cache = self.cache.clone();

        // Get cache stats synchronously from the last known state
        let (pending_segments, pending_bytes) = cache.stats();

        ReplicationStatus {
            running: self.running.load(Ordering::SeqCst),
            pending_segments,
            pending_bytes,
            last_snapshot: None, // Would need async to read
            last_wal_sync: None, // Would need async to read
            failed_uploads: self.failed_uploads.load(Ordering::SeqCst),
            wal_frame_count: 0, // Would need async to read from monitor
        }
    }

    /// Get the WAL file path for the database
    fn wal_path(&self) -> std::path::PathBuf {
        let mut wal_path = self.config.db_path.clone();
        let filename = wal_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        wal_path.set_file_name(format!("{filename}-wal"));
        wal_path
    }

    /// Create a full database snapshot
    async fn create_snapshot(&self) -> Result<()> {
        info!("Creating database snapshot");

        // Ensure parent directory exists
        if let Some(parent) = self.config.db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Read the entire database file
        let db_bytes = tokio::fs::read(&self.config.db_path).await?;

        // Upload to S3
        self.s3_backend.upload_snapshot(&db_bytes).await?;

        // Update metadata
        self.s3_backend.update_metadata(None).await?;

        // Update last snapshot time
        *self.last_snapshot.write().await = Some(chrono::Utc::now());

        info!("Database snapshot created successfully");
        Ok(())
    }

    /// Spawn the WAL monitoring task
    async fn spawn_wal_monitor_task(&self) {
        let running = self.running.clone();
        let wal_monitor = self.wal_monitor.clone();
        let cache = self.cache.clone();
        let wal_path = self.wal_path();

        tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                // Check if WAL monitor is initialized
                let monitor_guard = wal_monitor.lock().await;
                if let Some(monitor) = monitor_guard.as_ref() {
                    // Check for WAL changes
                    match monitor.check_for_changes().await {
                        Ok(Some(event)) => {
                            debug!("WAL change detected: {:?}", event);

                            // Read WAL file and add to cache
                            if wal_path.exists() {
                                match tokio::fs::read(&wal_path).await {
                                    Ok(wal_data) => {
                                        let sequence = event.frame_count;
                                        if let Err(e) = cache.add(sequence, wal_data).await {
                                            error!("Failed to cache WAL segment: {}", e);
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to read WAL file: {}", e);
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            // No changes
                        }
                        Err(e) => {
                            error!("WAL monitor error: {}", e);
                        }
                    }
                }
                drop(monitor_guard);

                // Poll interval
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        });
    }

    /// Spawn the cache upload worker
    async fn spawn_upload_worker(&self) {
        let running = self.running.clone();
        let cache = self.cache.clone();
        let s3_backend = self.s3_backend.clone();
        let failed_uploads = self.failed_uploads.clone();
        let last_wal_sync = self.last_wal_sync.clone();

        tokio::spawn(async move {
            let mut retry_delay = tokio::time::Duration::from_secs(1);
            let max_retry_delay = tokio::time::Duration::from_secs(60);

            while running.load(Ordering::SeqCst) {
                // Try to upload oldest entry
                match cache.pop_oldest().await {
                    Ok(Some(entry)) => {
                        match s3_backend.upload_wal_segment(&entry).await {
                            Ok(()) => {
                                debug!("Uploaded WAL segment {}", entry.sequence);
                                if let Err(e) = cache.remove(&entry).await {
                                    error!("Failed to remove cached entry: {}", e);
                                }
                                *last_wal_sync.write().await = Some(chrono::Utc::now());
                                retry_delay = tokio::time::Duration::from_secs(1);
                            }
                            Err(e) => {
                                warn!("Failed to upload WAL segment: {}", e);
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
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
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
    async fn spawn_snapshot_task(&self) {
        let running = self.running.clone();
        let interval = tokio::time::Duration::from_secs(self.config.snapshot_interval_secs);
        let db_path = self.config.db_path.clone();
        let s3_backend = self.s3_backend.clone();
        let last_snapshot = self.last_snapshot.clone();

        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(interval);
            interval_timer.tick().await; // Skip first immediate tick

            while running.load(Ordering::SeqCst) {
                interval_timer.tick().await;

                if !running.load(Ordering::SeqCst) {
                    break;
                }

                info!("Creating periodic snapshot");

                if !db_path.exists() {
                    debug!("Database file doesn't exist, skipping snapshot");
                    continue;
                }

                // Read and upload snapshot
                match tokio::fs::read(&db_path).await {
                    Ok(db_bytes) => match s3_backend.upload_snapshot(&db_bytes).await {
                        Ok(()) => {
                            info!("Periodic snapshot created");
                            if let Err(e) = s3_backend.update_metadata(None).await {
                                error!("Failed to update metadata: {}", e);
                            }
                            *last_snapshot.write().await = Some(chrono::Utc::now());
                        }
                        Err(e) => {
                            error!("Failed to upload snapshot: {}", e);
                        }
                    },
                    Err(e) => {
                        error!("Failed to read database for snapshot: {}", e);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wal_path_derivation() {
        let config = SqliteReplicatorConfig {
            db_path: std::path::PathBuf::from("/var/lib/myapp/data.db"),
            s3_bucket: "test".to_string(),
            s3_prefix: "test/".to_string(),
            cache_dir: std::path::PathBuf::from("/tmp/cache"),
            max_cache_size: 1024,
            auto_restore: false,
            snapshot_interval_secs: 3600,
        };

        // Verify config creation
        assert_eq!(config.s3_bucket, "test");
        assert_eq!(config.snapshot_interval_secs, 3600);

        // We can't fully test without async, but we can verify the path logic
        let mut wal_path = config.db_path.clone();
        let filename = wal_path.file_name().unwrap().to_string_lossy().to_string();
        wal_path.set_file_name(format!("{filename}-wal"));

        assert_eq!(
            wal_path,
            std::path::PathBuf::from("/var/lib/myapp/data.db-wal")
        );
    }
}
