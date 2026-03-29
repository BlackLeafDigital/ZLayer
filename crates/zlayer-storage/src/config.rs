//! Configuration types for layer storage

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

    /// Custom S3 endpoint URL (for S3-compatible storage like `MinIO`)
    pub endpoint_url: Option<String>,

    /// Local directory for staging tarballs before upload
    pub staging_dir: PathBuf,

    /// Local database path for sync state persistence (ZQL directory)
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
            state_db_path: zlayer_paths::ZLayerDirs::system_default()
                .data_dir()
                .join("layer-state-zql"),
            part_size_bytes: default_part_size(),
            max_concurrent_uploads: default_concurrent_uploads(),
            compression_level: default_compression_level(),
            sync_interval_secs: default_sync_interval(),
        }
    }
}

// ============================================================================
// ZQL Replicator Configuration
// ============================================================================

/// Configuration for ZQL database replication to S3
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZqlReplicatorConfig {
    /// Path to the ZQL database data directory
    pub data_dir: PathBuf,
    /// S3 bucket for storing backups
    pub s3_bucket: String,
    /// S3 key prefix for backup objects
    pub s3_prefix: String,
    /// Local directory for staging/caching snapshots
    pub cache_dir: PathBuf,
    /// Maximum cache size in bytes
    #[serde(default = "default_max_cache_size")]
    pub max_cache_size: u64,
    /// Whether to automatically restore from S3 on startup if no local DB
    #[serde(default)]
    pub auto_restore: bool,
    /// Interval in seconds between periodic snapshots
    #[serde(default = "default_snapshot_interval")]
    pub snapshot_interval_secs: u64,
}

fn default_max_cache_size() -> u64 {
    100 * 1024 * 1024 // 100MB
}

fn default_snapshot_interval() -> u64 {
    3600 // 1 hour
}

impl ZqlReplicatorConfig {
    /// Create a new replicator config with required fields
    pub fn new(
        data_dir: impl Into<PathBuf>,
        s3_bucket: impl Into<String>,
        s3_prefix: impl Into<String>,
    ) -> Self {
        let data_dir = data_dir.into();
        let cache_dir = data_dir.parent().map_or_else(
            || PathBuf::from("/tmp/zlayer-replicator/cache"),
            |p| p.join("replicator-cache"),
        );
        Self {
            data_dir,
            s3_bucket: s3_bucket.into(),
            s3_prefix: s3_prefix.into(),
            cache_dir,
            max_cache_size: default_max_cache_size(),
            auto_restore: false,
            snapshot_interval_secs: default_snapshot_interval(),
        }
    }

    /// Enable automatic restore from S3 on startup
    #[must_use]
    pub fn with_auto_restore(mut self, auto_restore: bool) -> Self {
        self.auto_restore = auto_restore;
        self
    }

    /// Set the local cache directory for staging snapshots
    #[must_use]
    pub fn with_cache_dir(mut self, cache_dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = cache_dir.into();
        self
    }

    /// Set the maximum cache size in bytes
    #[must_use]
    pub fn with_max_cache_size(mut self, max_cache_size: u64) -> Self {
        self.max_cache_size = max_cache_size;
        self
    }

    /// Set the snapshot interval in seconds
    #[must_use]
    pub fn with_snapshot_interval(mut self, secs: u64) -> Self {
        self.snapshot_interval_secs = secs;
        self
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
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Set the AWS region
    #[must_use]
    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Set a custom S3 endpoint URL
    #[must_use]
    pub fn with_endpoint_url(mut self, url: impl Into<String>) -> Self {
        self.endpoint_url = Some(url.into());
        self
    }

    /// Set the staging directory
    #[must_use]
    pub fn with_staging_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.staging_dir = path.into();
        self
    }

    /// Set the state database path
    #[must_use]
    pub fn with_state_db_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.state_db_path = path.into();
        self
    }

    /// Build the S3 object key for a given layer digest
    #[must_use]
    pub fn object_key(&self, digest: &str) -> String {
        format!("{}{}.tar.zst", self.prefix, digest)
    }

    /// Build the S3 object key for layer metadata
    #[must_use]
    pub fn metadata_key(&self, digest: &str) -> String {
        format!("{}{}.meta.json", self.prefix, digest)
    }
}
