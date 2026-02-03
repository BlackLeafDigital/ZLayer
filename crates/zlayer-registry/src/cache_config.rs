//! Unified cache configuration system
//!
//! This module provides a unified way to configure and instantiate cache backends
//! from environment variables or programmatic configuration.

use crate::cache::{BlobCache, BlobCacheBackend};
use crate::error::CacheError;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "persistent")]
use crate::persistent_cache::PersistentBlobCache;

#[cfg(feature = "s3")]
use crate::s3_cache::{S3BlobCache, S3CacheConfig};

/// Default cache directory for persistent cache
const DEFAULT_CACHE_DIR: &str = "/var/lib/zlayer/cache";

/// Default cache database filename
const DEFAULT_CACHE_DB: &str = "blobs.sqlite";

/// Cache type configuration
#[derive(Debug, Clone)]
pub enum CacheType {
    /// In-memory cache (non-persistent, cleared on restart)
    Memory,

    /// Persistent disk-based cache using SQLite
    #[cfg(feature = "persistent")]
    Persistent {
        /// Path to the cache database file
        path: PathBuf,
    },

    /// S3-compatible storage backend
    #[cfg(feature = "s3")]
    S3(S3CacheConfig),
}

impl Default for CacheType {
    fn default() -> Self {
        #[cfg(feature = "persistent")]
        {
            CacheType::Persistent {
                path: PathBuf::from(DEFAULT_CACHE_DIR).join(DEFAULT_CACHE_DB),
            }
        }
        #[cfg(not(feature = "persistent"))]
        {
            CacheType::Memory
        }
    }
}

impl CacheType {
    /// Create cache configuration from environment variables
    ///
    /// # Environment Variables
    ///
    /// - `ZLAYER_CACHE_TYPE`: Cache type - "memory", "persistent", or "s3" (default: "persistent")
    /// - `ZLAYER_CACHE_DIR`: Path for persistent cache directory (default: /var/lib/zlayer/cache)
    /// - `ZLAYER_S3_BUCKET`: S3 bucket name (required for s3 type)
    /// - `ZLAYER_S3_REGION`: AWS region (optional)
    /// - `ZLAYER_S3_ENDPOINT`: Custom S3 endpoint for S3-compatible services
    /// - `ZLAYER_S3_PREFIX`: Key prefix for blobs (default: "zlayer/blobs/")
    /// - `ZLAYER_S3_PATH_STYLE`: Set to "true" to use path-style URLs
    ///
    /// # Errors
    ///
    /// Returns `CacheError` if:
    /// - `ZLAYER_CACHE_TYPE` is "s3" but `ZLAYER_S3_BUCKET` is not set
    /// - `ZLAYER_CACHE_TYPE` is an unknown value
    pub fn from_env() -> Result<Self, CacheError> {
        let cache_type = std::env::var("ZLAYER_CACHE_TYPE")
            .unwrap_or_else(|_| "persistent".to_string())
            .to_lowercase();

        match cache_type.as_str() {
            "memory" => Ok(CacheType::Memory),

            "persistent" => {
                #[cfg(feature = "persistent")]
                {
                    let cache_dir = std::env::var("ZLAYER_CACHE_DIR")
                        .unwrap_or_else(|_| DEFAULT_CACHE_DIR.to_string());
                    let path = PathBuf::from(cache_dir).join(DEFAULT_CACHE_DB);
                    Ok(CacheType::Persistent { path })
                }
                #[cfg(not(feature = "persistent"))]
                {
                    Err(CacheError::Database(
                        "persistent cache requires the 'persistent' feature".to_string(),
                    ))
                }
            }

            "s3" => {
                #[cfg(feature = "s3")]
                {
                    let bucket = std::env::var("ZLAYER_S3_BUCKET").map_err(|_| {
                        CacheError::Database(
                            "ZLAYER_S3_BUCKET environment variable is required for S3 cache"
                                .to_string(),
                        )
                    })?;

                    let mut config = S3CacheConfig::new(bucket);

                    if let Ok(region) = std::env::var("ZLAYER_S3_REGION") {
                        config = config.with_region(region);
                    }

                    if let Ok(endpoint) = std::env::var("ZLAYER_S3_ENDPOINT") {
                        config = config.with_endpoint(endpoint);
                    }

                    if let Ok(prefix) = std::env::var("ZLAYER_S3_PREFIX") {
                        config = config.with_prefix(prefix);
                    }

                    if let Ok(path_style) = std::env::var("ZLAYER_S3_PATH_STYLE") {
                        if path_style.to_lowercase() == "true" || path_style == "1" {
                            config = config.with_path_style();
                        }
                    }

                    Ok(CacheType::S3(config))
                }
                #[cfg(not(feature = "s3"))]
                {
                    Err(CacheError::Database(
                        "S3 cache requires the 's3' feature".to_string(),
                    ))
                }
            }

            unknown => Err(CacheError::Database(format!(
                "unknown cache type '{}', expected 'memory', 'persistent', or 's3'",
                unknown
            ))),
        }
    }

    /// Build a cache backend from this configuration
    ///
    /// # Errors
    ///
    /// Returns `CacheError` if the cache backend fails to initialize
    pub async fn build(&self) -> Result<Arc<Box<dyn BlobCacheBackend>>, CacheError> {
        match self {
            CacheType::Memory => {
                let cache = BlobCache::new()?;
                Ok(Arc::new(Box::new(cache) as Box<dyn BlobCacheBackend>))
            }

            #[cfg(feature = "persistent")]
            CacheType::Persistent { path } => {
                let cache = PersistentBlobCache::open(path).await?;
                Ok(Arc::new(Box::new(cache) as Box<dyn BlobCacheBackend>))
            }

            #[cfg(feature = "s3")]
            CacheType::S3(config) => {
                let cache = S3BlobCache::new(config.clone()).await?;
                Ok(Arc::new(Box::new(cache) as Box<dyn BlobCacheBackend>))
            }
        }
    }

    /// Create a memory cache configuration
    pub fn memory() -> Self {
        CacheType::Memory
    }

    /// Create a persistent cache configuration with default path
    #[cfg(feature = "persistent")]
    pub fn persistent() -> Self {
        CacheType::Persistent {
            path: PathBuf::from(DEFAULT_CACHE_DIR).join(DEFAULT_CACHE_DB),
        }
    }

    /// Create a persistent cache configuration with custom path
    #[cfg(feature = "persistent")]
    pub fn persistent_at(path: impl Into<PathBuf>) -> Self {
        CacheType::Persistent { path: path.into() }
    }

    /// Create an S3 cache configuration
    #[cfg(feature = "s3")]
    pub fn s3(config: S3CacheConfig) -> Self {
        CacheType::S3(config)
    }

    /// Check if this is a memory cache
    pub fn is_memory(&self) -> bool {
        matches!(self, CacheType::Memory)
    }

    /// Check if this is a persistent cache
    #[cfg(feature = "persistent")]
    pub fn is_persistent(&self) -> bool {
        matches!(self, CacheType::Persistent { .. })
    }

    /// Check if this is an S3 cache
    #[cfg(feature = "s3")]
    pub fn is_s3(&self) -> bool {
        matches!(self, CacheType::S3(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;

    /// Environment variable names used by cache configuration
    const ENV_CACHE_TYPE: &str = "ZLAYER_CACHE_TYPE";
    const ENV_CACHE_DIR: &str = "ZLAYER_CACHE_DIR";
    const ENV_S3_BUCKET: &str = "ZLAYER_S3_BUCKET";
    const ENV_S3_REGION: &str = "ZLAYER_S3_REGION";
    const ENV_S3_ENDPOINT: &str = "ZLAYER_S3_ENDPOINT";
    const ENV_S3_PREFIX: &str = "ZLAYER_S3_PREFIX";
    const ENV_S3_PATH_STYLE: &str = "ZLAYER_S3_PATH_STYLE";

    /// Guard that clears environment variables on drop to ensure test isolation.
    struct EnvGuard;

    impl EnvGuard {
        fn new() -> Self {
            // Clear any existing env vars at test start
            Self::clear_all();
            Self
        }

        fn clear_all() {
            env::remove_var(ENV_CACHE_TYPE);
            env::remove_var(ENV_CACHE_DIR);
            env::remove_var(ENV_S3_BUCKET);
            env::remove_var(ENV_S3_REGION);
            env::remove_var(ENV_S3_ENDPOINT);
            env::remove_var(ENV_S3_PREFIX);
            env::remove_var(ENV_S3_PATH_STYLE);
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            Self::clear_all();
        }
    }

    #[test]
    fn test_default_cache_type() {
        let cache = CacheType::default();

        #[cfg(feature = "persistent")]
        assert!(cache.is_persistent());

        #[cfg(not(feature = "persistent"))]
        assert!(cache.is_memory());
    }

    #[test]
    fn test_memory_cache() {
        let cache = CacheType::memory();
        assert!(cache.is_memory());
    }

    #[cfg(feature = "persistent")]
    #[test]
    fn test_persistent_cache() {
        let cache = CacheType::persistent();
        assert!(cache.is_persistent());

        let custom = CacheType::persistent_at("/tmp/test.sqlite");
        if let CacheType::Persistent { path } = custom {
            assert_eq!(path, PathBuf::from("/tmp/test.sqlite"));
        } else {
            panic!("expected Persistent variant");
        }
    }

    #[cfg(feature = "s3")]
    #[test]
    fn test_s3_cache() {
        let config = S3CacheConfig::new("test-bucket");
        let cache = CacheType::s3(config);
        assert!(cache.is_s3());
    }

    #[test]
    #[serial]
    fn test_from_env_memory() {
        let _guard = EnvGuard::new();

        env::set_var(ENV_CACHE_TYPE, "memory");

        let cache = CacheType::from_env().unwrap();
        assert!(cache.is_memory());
    }

    #[test]
    #[serial]
    fn test_from_env_unknown_type() {
        let _guard = EnvGuard::new();

        env::set_var(ENV_CACHE_TYPE, "invalid");

        let result = CacheType::from_env();
        assert!(result.is_err());
    }

    #[cfg(feature = "s3")]
    #[test]
    #[serial]
    fn test_from_env_s3_missing_bucket() {
        let _guard = EnvGuard::new();

        env::set_var(ENV_CACHE_TYPE, "s3");

        let result = CacheType::from_env();
        assert!(result.is_err());
    }

    #[cfg(feature = "s3")]
    #[test]
    #[serial]
    fn test_from_env_s3_with_all_options() {
        let _guard = EnvGuard::new();

        env::set_var(ENV_CACHE_TYPE, "s3");
        env::set_var(ENV_S3_BUCKET, "my-bucket");
        env::set_var(ENV_S3_REGION, "us-west-2");
        env::set_var(ENV_S3_ENDPOINT, "https://s3.example.com");
        env::set_var(ENV_S3_PREFIX, "custom/prefix/");
        env::set_var(ENV_S3_PATH_STYLE, "true");

        let cache = CacheType::from_env().unwrap();
        if let CacheType::S3(config) = cache {
            assert_eq!(config.bucket, "my-bucket");
            assert_eq!(config.region, Some("us-west-2".to_string()));
            assert_eq!(config.endpoint, Some("https://s3.example.com".to_string()));
            assert_eq!(config.prefix, "custom/prefix/");
            assert!(config.path_style);
        } else {
            panic!("expected S3 variant");
        }
    }

    #[tokio::test]
    async fn test_build_memory_cache() {
        let cache_type = CacheType::memory();
        let cache = cache_type.build().await.unwrap();

        // Test basic operations
        let test_data = b"test data for cache";
        let digest = crate::cache::compute_digest(test_data);

        cache.put(&digest, test_data).await.unwrap();
        let retrieved = cache.get(&digest).await.unwrap();
        assert_eq!(retrieved, Some(test_data.to_vec()));
    }

    #[cfg(feature = "persistent")]
    #[tokio::test]
    async fn test_build_persistent_cache() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("test.sqlite");

        let cache_type = CacheType::persistent_at(&cache_path);
        let cache = cache_type.build().await.unwrap();

        // Test basic operations
        let test_data = b"persistent test data";
        let digest = crate::cache::compute_digest(test_data);

        cache.put(&digest, test_data).await.unwrap();
        let retrieved = cache.get(&digest).await.unwrap();
        assert_eq!(retrieved, Some(test_data.to_vec()));
    }
}
