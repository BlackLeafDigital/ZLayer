//! Configuration types for layer storage

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
            state_db_path: PathBuf::from("/var/lib/zlayer/layer-state.redb"),
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
