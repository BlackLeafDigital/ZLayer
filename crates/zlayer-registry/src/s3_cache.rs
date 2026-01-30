//! S3-backed blob cache for OCI images
//!
//! Provides S3 storage backend for image layers with optional local cache tier.

use crate::cache::BlobCacheBackend;
use crate::error::CacheError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

/// Result type alias for S3 cache operations
pub type Result<T> = std::result::Result<T, CacheError>;

/// Configuration for S3 blob cache
#[derive(Debug, Clone)]
pub struct S3CacheConfig {
    /// S3 bucket name
    pub bucket: String,
    /// Key prefix for blobs (e.g., "zlayer/blobs/")
    pub prefix: String,
    /// AWS region (optional, uses SDK default if not set)
    pub region: Option<String>,
    /// Custom S3 endpoint (for S3-compatible services like R2, B2, MinIO)
    pub endpoint: Option<String>,
    /// Use path-style URLs (required for some S3-compatible services)
    pub path_style: bool,
    /// Local cache directory for read-through caching
    pub local_cache_dir: Option<PathBuf>,
}

impl S3CacheConfig {
    /// Create a new S3 cache config for AWS S3
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
            prefix: "zlayer/blobs/".to_string(),
            region: None,
            endpoint: None,
            path_style: false,
            local_cache_dir: None,
        }
    }

    /// Create config for Cloudflare R2
    pub fn cloudflare_r2(bucket: impl Into<String>, account_id: impl AsRef<str>) -> Self {
        Self {
            bucket: bucket.into(),
            prefix: "zlayer/blobs/".to_string(),
            region: Some("auto".to_string()),
            endpoint: Some(format!(
                "https://{}.r2.cloudflarestorage.com",
                account_id.as_ref()
            )),
            path_style: true,
            local_cache_dir: None,
        }
    }

    /// Create config for Backblaze B2
    pub fn backblaze_b2(bucket: impl Into<String>, region: impl Into<String>) -> Self {
        let region = region.into();
        Self {
            bucket: bucket.into(),
            prefix: "zlayer/blobs/".to_string(),
            region: Some(region.clone()),
            endpoint: Some(format!("https://s3.{}.backblazeb2.com", region)),
            path_style: false,
            local_cache_dir: None,
        }
    }

    /// Create config for Wasabi
    pub fn wasabi(bucket: impl Into<String>, region: impl Into<String>) -> Self {
        let region = region.into();
        Self {
            bucket: bucket.into(),
            prefix: "zlayer/blobs/".to_string(),
            region: Some(region.clone()),
            endpoint: Some(format!("https://s3.{}.wasabisys.com", region)),
            path_style: false,
            local_cache_dir: None,
        }
    }

    /// Set custom prefix
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Set region
    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Set custom endpoint
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Enable path-style URLs
    pub fn with_path_style(mut self) -> Self {
        self.path_style = true;
        self
    }

    /// Enable local cache read-through
    pub fn with_local_cache(mut self, dir: impl Into<PathBuf>) -> Self {
        self.local_cache_dir = Some(dir.into());
        self
    }

    /// Get the full key for a digest
    pub fn key_for_digest(&self, digest: &str) -> String {
        format!("{}{}", self.prefix, digest.replace(':', "/"))
    }
}

impl Default for S3CacheConfig {
    fn default() -> Self {
        Self::new("zlayer-cache")
    }
}

/// S3-backed blob cache with optional local read-through cache
pub struct S3BlobCache {
    /// S3 client
    client: Client,
    /// Configuration
    config: S3CacheConfig,
    /// Local in-memory cache for hot blobs
    local_cache: Option<RwLock<HashMap<String, Vec<u8>>>>,
}

impl S3BlobCache {
    /// Create a new S3 blob cache
    pub async fn new(config: S3CacheConfig) -> Result<Self> {
        let sdk_config = Self::build_sdk_config(&config).await?;

        let mut s3_config = aws_sdk_s3::config::Builder::from(&sdk_config);

        // Set custom endpoint if configured
        if let Some(ref endpoint) = config.endpoint {
            s3_config = s3_config.endpoint_url(endpoint);
        }

        // Set path-style if configured
        if config.path_style {
            s3_config = s3_config.force_path_style(true);
        }

        let client = Client::from_conf(s3_config.build());

        // Initialize local cache if configured
        let local_cache = if config.local_cache_dir.is_some() {
            Some(RwLock::new(HashMap::new()))
        } else {
            None
        };

        Ok(Self {
            client,
            config,
            local_cache,
        })
    }

    /// Build AWS SDK config
    async fn build_sdk_config(config: &S3CacheConfig) -> Result<aws_config::SdkConfig> {
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());

        if let Some(ref region) = config.region {
            loader = loader.region(aws_config::Region::new(region.clone()));
        }

        Ok(loader.load().await)
    }

    /// Get a blob by digest
    pub async fn get(&self, digest: &str) -> Result<Option<Vec<u8>>> {
        crate::cache::validate_digest(digest)?;

        // Check local cache first
        if let Some(ref local) = self.local_cache {
            if let Ok(cache) = local.read() {
                if let Some(data) = cache.get(digest) {
                    tracing::debug!(digest = %digest, "S3 cache hit (local)");
                    return Ok(Some(data.clone()));
                }
            }
        }

        // Fetch from S3
        let key = self.config.key_for_digest(digest);

        let result = self
            .client
            .get_object()
            .bucket(&self.config.bucket)
            .key(&key)
            .send()
            .await;

        match result {
            Ok(output) => {
                let data = output
                    .body
                    .collect()
                    .await
                    .map_err(|e| {
                        CacheError::Io(std::io::Error::other(format!(
                            "failed to read S3 response body: {}",
                            e
                        )))
                    })?
                    .into_bytes()
                    .to_vec();

                // Verify digest
                let actual_digest = crate::cache::compute_digest(&data);
                if actual_digest != digest {
                    return Err(CacheError::Corrupted(format!(
                        "S3 blob digest mismatch: expected {}, got {}",
                        digest, actual_digest
                    )));
                }

                // Populate local cache
                if let Some(ref local) = self.local_cache {
                    if let Ok(mut cache) = local.write() {
                        cache.insert(digest.to_string(), data.clone());
                    }
                }

                tracing::debug!(digest = %digest, "S3 cache hit (remote)");
                Ok(Some(data))
            }
            Err(e) => {
                // Check if it's a not-found error
                if let aws_sdk_s3::error::SdkError::ServiceError(ref service_err) = e {
                    if service_err.err().is_no_such_key() {
                        tracing::debug!(digest = %digest, "S3 cache miss");
                        return Ok(None);
                    }
                }
                Err(CacheError::Io(std::io::Error::other(format!(
                    "S3 get failed: {}",
                    e
                ))))
            }
        }
    }

    /// Put a blob into the cache
    pub async fn put(&self, digest: &str, data: &[u8]) -> Result<()> {
        crate::cache::validate_digest(digest)?;

        // Verify digest matches data
        let actual_digest = crate::cache::compute_digest(data);
        if actual_digest != digest {
            return Err(CacheError::Corrupted(format!(
                "digest mismatch: expected {}, got {}",
                digest, actual_digest
            )));
        }

        let key = self.config.key_for_digest(digest);

        self.client
            .put_object()
            .bucket(&self.config.bucket)
            .key(&key)
            .body(ByteStream::from(data.to_vec()))
            .content_type("application/octet-stream")
            .send()
            .await
            .map_err(|e| CacheError::Io(std::io::Error::other(format!("S3 put failed: {}", e))))?;

        // Update local cache
        if let Some(ref local) = self.local_cache {
            if let Ok(mut cache) = local.write() {
                cache.insert(digest.to_string(), data.to_vec());
            }
        }

        tracing::debug!(digest = %digest, bucket = %self.config.bucket, "stored blob in S3");
        Ok(())
    }

    /// Check if a blob exists in the cache
    pub async fn contains(&self, digest: &str) -> Result<bool> {
        crate::cache::validate_digest(digest)?;

        // Check local cache first
        if let Some(ref local) = self.local_cache {
            if let Ok(cache) = local.read() {
                if cache.contains_key(digest) {
                    return Ok(true);
                }
            }
        }

        // Check S3
        let key = self.config.key_for_digest(digest);

        let result = self
            .client
            .head_object()
            .bucket(&self.config.bucket)
            .key(&key)
            .send()
            .await;

        match result {
            Ok(_) => Ok(true),
            Err(e) => {
                if let aws_sdk_s3::error::SdkError::ServiceError(ref service_err) = e {
                    if service_err.err().is_not_found() {
                        return Ok(false);
                    }
                }
                Err(CacheError::Io(std::io::Error::other(format!(
                    "S3 head failed: {}",
                    e
                ))))
            }
        }
    }

    /// Clear local cache
    pub fn clear_local_cache(&self) {
        if let Some(ref local) = self.local_cache {
            if let Ok(mut cache) = local.write() {
                cache.clear();
            }
        }
    }
}

#[async_trait::async_trait]
impl BlobCacheBackend for S3BlobCache {
    async fn get(&self, digest: &str) -> std::result::Result<Option<Vec<u8>>, CacheError> {
        S3BlobCache::get(self, digest).await
    }

    async fn put(&self, digest: &str, data: &[u8]) -> std::result::Result<(), CacheError> {
        S3BlobCache::put(self, digest, data).await
    }

    async fn contains(&self, digest: &str) -> std::result::Result<bool, CacheError> {
        S3BlobCache::contains(self, digest).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_for_digest() {
        let config = S3CacheConfig::new("mybucket").with_prefix("blobs/");
        let key = config.key_for_digest("sha256:abc123");
        assert_eq!(key, "blobs/sha256/abc123");
    }

    #[test]
    fn test_cloudflare_r2_config() {
        let config = S3CacheConfig::cloudflare_r2("mybucket", "abc123accountid");
        assert_eq!(
            config.endpoint,
            Some("https://abc123accountid.r2.cloudflarestorage.com".to_string())
        );
        assert!(config.path_style);
    }

    #[test]
    fn test_backblaze_b2_config() {
        let config = S3CacheConfig::backblaze_b2("mybucket", "us-west-001");
        assert_eq!(
            config.endpoint,
            Some("https://s3.us-west-001.backblazeb2.com".to_string())
        );
    }
}
