//! Local blob cache for OCI images
//!
//! Simplified cache implementation using std::collections::HashMap.

use crate::error::{CacheError, Result};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

/// Local blob cache for OCI images (in-memory)
pub struct BlobCache {
    blobs: Arc<RwLock<std::collections::HashMap<String, Vec<u8>>>>,
    max_size_bytes: u64,
}

impl BlobCache {
    /// Create a new in-memory cache
    pub fn new() -> Result<Self, CacheError> {
        Ok(Self {
            blobs: Arc::new(RwLock::new(std::collections::HashMap::new())),
            max_size_bytes: 10 * 1024 * 1024 * 1024, // 10GB default
        })
    }

    /// Open or create a cache at the given path (creates in-memory for now)
    pub fn open<P: AsRef<Path>>(_path: P) -> Result<Self, CacheError> {
        Self::new()
    }

    /// Set maximum cache size in bytes
    pub fn with_max_size(mut self, max_size_bytes: u64) -> Self {
        self.max_size_bytes = max_size_bytes;
        self
    }

    /// Get a blob by digest
    pub fn get(&self, digest: &str) -> Result<Option<Vec<u8>>, CacheError> {
        validate_digest(digest)?;
        let blobs = self
            .blobs
            .read()
            .map_err(|e| CacheError::Database(format!("failed to acquire read lock: {}", e)))?;
        Ok(blobs.get(digest).cloned())
    }

    /// Put a blob into the cache
    pub fn put(&self, digest: &str, data: &[u8]) -> Result<(), CacheError> {
        validate_digest(digest)?;

        // Verify digest matches data
        let actual_digest = compute_digest(data);
        if actual_digest != digest {
            return Err(CacheError::Corrupted(format!(
                "digest mismatch: expected {}, got {}",
                digest, actual_digest
            )));
        }

        {
            let mut blobs = self.blobs.write().map_err(|e| {
                CacheError::Database(format!("failed to acquire write lock: {}", e))
            })?;
            blobs.insert(digest.to_string(), data.to_vec());
        } // Write lock released here

        // Now safe to call evict_if_needed() with no locks held
        self.evict_if_needed()?;

        Ok(())
    }

    /// Check if a blob exists in the cache
    pub fn contains(&self, digest: &str) -> Result<bool, CacheError> {
        validate_digest(digest)?;
        let blobs = self
            .blobs
            .read()
            .map_err(|e| CacheError::Database(format!("failed to acquire read lock: {}", e)))?;
        Ok(blobs.contains_key(digest))
    }

    /// Delete a blob from the cache
    pub fn delete(&self, digest: &str) -> Result<(), CacheError> {
        validate_digest(digest)?;
        let mut blobs = self
            .blobs
            .write()
            .map_err(|e| CacheError::Database(format!("failed to acquire write lock: {}", e)))?;
        blobs.remove(digest);
        Ok(())
    }

    /// Get current cache size in bytes
    pub fn size(&self) -> Result<u64, CacheError> {
        let blobs = self
            .blobs
            .read()
            .map_err(|e| CacheError::Database(format!("failed to acquire read lock: {}", e)))?;
        let size = blobs.values().map(|v| v.len() as u64).sum();
        Ok(size)
    }

    /// Clear all blobs from the cache
    pub fn clear(&self) -> Result<(), CacheError> {
        let mut blobs = self
            .blobs
            .write()
            .map_err(|e| CacheError::Database(format!("failed to acquire write lock: {}", e)))?;
        blobs.clear();
        Ok(())
    }

    /// Evict blobs if cache is over size limit
    fn evict_if_needed(&self) -> Result<(), CacheError> {
        let current_size = self.size()?;
        if current_size <= self.max_size_bytes {
            return Ok(());
        }

        // Simple eviction: clear if significantly over
        if current_size > self.max_size_bytes * 2 {
            self.clear()?;
        }

        Ok(())
    }
}

impl Default for BlobCache {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

/// Validate that a digest string is properly formatted
fn validate_digest(digest: &str) -> Result<(), CacheError> {
    if !digest.starts_with("sha256:") {
        return Err(CacheError::InvalidDigest(
            "digest must start with sha256:".to_string(),
        ));
    }

    let hex_part = &digest[7..];
    if hex_part.len() != 64 {
        return Err(CacheError::InvalidDigest(
            "sha256 digest must be 64 hex characters".to_string(),
        ));
    }

    if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CacheError::InvalidDigest(
            "digest must contain only hex characters".to_string(),
        ));
    }

    Ok(())
}

/// Compute SHA-256 digest of data
pub fn compute_digest(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    format!("sha256:{}", hex::encode(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_digest() {
        let data = b"hello world";
        let digest = compute_digest(data);
        assert!(digest.starts_with("sha256:"));
        assert_eq!(digest.len(), 7 + 64);
    }

    #[test]
    fn test_validate_digest() {
        assert!(validate_digest(
            "sha256:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
        )
        .is_ok());
        assert!(validate_digest("md5:123456").is_err());
        assert!(validate_digest("sha256:123").is_err());
    }

    #[test]
    fn test_cache_put_get() {
        let cache = BlobCache::new().unwrap();

        let data = b"test data";
        let digest = compute_digest(data);

        // Put and get
        cache.put(&digest, data).unwrap();
        let retrieved = cache.get(&digest).unwrap();
        assert_eq!(retrieved, Some(data.to_vec()));

        // Contains
        assert!(cache.contains(&digest).unwrap());

        // Delete
        cache.delete(&digest).unwrap();
        assert!(!cache.contains(&digest).unwrap());
    }
}
