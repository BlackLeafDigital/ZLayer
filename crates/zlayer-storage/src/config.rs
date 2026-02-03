//! Configuration types for layer storage

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ============================================================================
// SQLite Replicator Configuration
// ============================================================================

/// Configuration for SQLite WAL-based replication to S3
///
/// This configuration controls how the SQLite replicator monitors database changes
/// and syncs them to S3 for persistence and disaster recovery.
///
/// # Example
///
/// ```rust
/// use zlayer_storage::config::SqliteReplicatorConfig;
/// use std::path::PathBuf;
///
/// let config = SqliteReplicatorConfig {
///     db_path: PathBuf::from("/var/lib/myapp/data.db"),
///     s3_bucket: "my-bucket".to_string(),
///     s3_prefix: "sqlite-backups/myapp/".to_string(),
///     cache_dir: PathBuf::from("/tmp/zlayer-replicator/cache"),
///     max_cache_size: 100 * 1024 * 1024, // 100MB
///     auto_restore: true,
///     snapshot_interval_secs: 3600,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqliteReplicatorConfig {
    /// Path to the SQLite database file to replicate
    pub db_path: PathBuf,

    /// S3 bucket for storing backups
    pub s3_bucket: String,

    /// S3 key prefix for this database's backups
    ///
    /// The replicator will create the following structure under this prefix:
    /// - `{prefix}/snapshots/` - Full database snapshots
    /// - `{prefix}/wal/` - WAL segments
    /// - `{prefix}/metadata.json` - Replication metadata
    pub s3_prefix: String,

    /// Local directory for caching WAL segments before upload
    ///
    /// This provides network tolerance - if S3 is temporarily unavailable,
    /// WAL segments are cached here and uploaded when connectivity returns.
    pub cache_dir: PathBuf,

    /// Maximum size of the local cache in bytes
    ///
    /// When the cache exceeds this size, the oldest entries will be evicted
    /// (and lost if not yet uploaded). Default: 100MB
    #[serde(default = "default_max_cache_size")]
    pub max_cache_size: u64,

    /// Whether to automatically restore from S3 on startup if local DB is missing
    ///
    /// When enabled, the replicator will download and restore the database
    /// from S3 if the local file doesn't exist. Default: true
    #[serde(default = "default_auto_restore")]
    pub auto_restore: bool,

    /// Interval between full database snapshots in seconds
    ///
    /// Snapshots provide a restore point and allow cleanup of old WAL segments.
    /// Default: 3600 (1 hour)
    #[serde(default = "default_snapshot_interval")]
    pub snapshot_interval_secs: u64,
}

fn default_max_cache_size() -> u64 {
    100 * 1024 * 1024 // 100MB
}

fn default_auto_restore() -> bool {
    true
}

fn default_snapshot_interval() -> u64 {
    3600 // 1 hour
}

impl Default for SqliteReplicatorConfig {
    fn default() -> Self {
        Self {
            db_path: PathBuf::new(),
            s3_bucket: String::new(),
            s3_prefix: "sqlite-replication/".to_string(),
            cache_dir: PathBuf::from("/tmp/zlayer-replicator/cache"),
            max_cache_size: default_max_cache_size(),
            auto_restore: default_auto_restore(),
            snapshot_interval_secs: default_snapshot_interval(),
        }
    }
}

impl SqliteReplicatorConfig {
    /// Create a new config with the required fields
    pub fn new(
        db_path: impl Into<PathBuf>,
        s3_bucket: impl Into<String>,
        s3_prefix: impl Into<String>,
    ) -> Self {
        Self {
            db_path: db_path.into(),
            s3_bucket: s3_bucket.into(),
            s3_prefix: s3_prefix.into(),
            ..Default::default()
        }
    }

    /// Set the cache directory
    pub fn with_cache_dir(mut self, cache_dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = cache_dir.into();
        self
    }

    /// Set the maximum cache size
    pub fn with_max_cache_size(mut self, size: u64) -> Self {
        self.max_cache_size = size;
        self
    }

    /// Set whether to auto-restore on startup
    pub fn with_auto_restore(mut self, auto_restore: bool) -> Self {
        self.auto_restore = auto_restore;
        self
    }

    /// Set the snapshot interval
    pub fn with_snapshot_interval(mut self, interval_secs: u64) -> Self {
        self.snapshot_interval_secs = interval_secs;
        self
    }
}

// ============================================================================
// Layer Storage Configuration
// ============================================================================

/// Configuration for S3-backed layer storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerStorageConfig {
    /// S3 bucket name for storing layers
    pub bucket: String,

    /// S3 key prefix for layer objects (e.g., "layers/")
    #[serde(default = "default_prefix")]
    pub prefix: String,

    /// AWS region (if not using environment/profile defaults)
    pub region: Option<String>,

    /// Custom S3 endpoint URL (for S3-compatible storage like MinIO)
    pub endpoint_url: Option<String>,

    /// Local directory for staging tarballs before upload
    pub staging_dir: PathBuf,

    /// Local database path for sync state persistence
    pub state_db_path: PathBuf,

    /// Multipart upload part size in bytes (default: 64MB)
    #[serde(default = "default_part_size")]
    pub part_size_bytes: u64,

    /// Maximum concurrent part uploads
    #[serde(default = "default_concurrent_uploads")]
    pub max_concurrent_uploads: usize,

    /// Compression level for zstd (1-22, default: 3)
    #[serde(default = "default_compression_level")]
    pub compression_level: i32,

    /// Sync interval in seconds (how often to check for changes)
    #[serde(default = "default_sync_interval")]
    pub sync_interval_secs: u64,
}

fn default_prefix() -> String {
    "layers/".to_string()
}

fn default_part_size() -> u64 {
    64 * 1024 * 1024 // 64MB
}

fn default_concurrent_uploads() -> usize {
    4
}

fn default_compression_level() -> i32 {
    3
}

fn default_sync_interval() -> u64 {
    30
}

impl Default for LayerStorageConfig {
    fn default() -> Self {
        Self {
            bucket: String::new(),
            prefix: default_prefix(),
            region: None,
            endpoint_url: None,
            staging_dir: PathBuf::from("/tmp/zlayer-storage/staging"),
            state_db_path: PathBuf::from("/var/lib/zlayer/layer-state.sqlite"),
            part_size_bytes: default_part_size(),
            max_concurrent_uploads: default_concurrent_uploads(),
            compression_level: default_compression_level(),
            sync_interval_secs: default_sync_interval(),
        }
    }
}

impl LayerStorageConfig {
    /// Create a new config with the required bucket name
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
            ..Default::default()
        }
    }

    /// Set the S3 key prefix
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Set the AWS region
    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Set a custom S3 endpoint URL
    pub fn with_endpoint_url(mut self, url: impl Into<String>) -> Self {
        self.endpoint_url = Some(url.into());
        self
    }

    /// Set the staging directory
    pub fn with_staging_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.staging_dir = path.into();
        self
    }

    /// Set the state database path
    pub fn with_state_db_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.state_db_path = path.into();
        self
    }

    /// Build the S3 object key for a given layer digest
    pub fn object_key(&self, digest: &str) -> String {
        format!("{}{}.tar.zst", self.prefix, digest)
    }

    /// Build the S3 object key for layer metadata
    pub fn metadata_key(&self, digest: &str) -> String {
        format!("{}{}.meta.json", self.prefix, digest)
    }
}
