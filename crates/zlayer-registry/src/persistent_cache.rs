//! Persistent blob cache for OCI images using ZQL
//!
//! This module provides a persistent blob cache backed by ZQL for durability.
//! Blobs are stored with metadata for LRU eviction. Binary data is base64-encoded
//! for storage in ZQL's string-based fields.

use crate::cache::{compute_digest, validate_digest, BlobCacheBackend};
use crate::error::{CacheError, Result};
use async_trait::async_trait;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

/// Default database directory name used when a directory is provided
const DEFAULT_DB_DIRNAME: &str = "blob_cache_zql";

fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Encode binary data as hex for ZQL storage
fn encode_blob(data: &[u8]) -> String {
    hex::encode(data)
}

/// Decode hex-encoded data back to bytes
fn decode_blob(encoded: &str) -> std::result::Result<Vec<u8>, CacheError> {
    hex::decode(encoded)
        .map_err(|e| CacheError::Database(format!("failed to decode blob data: {}", e)))
}

/// Persistent blob cache for OCI images backed by ZQL
pub struct PersistentBlobCache {
    db: tokio::sync::Mutex<zql::Database>,
    max_size_bytes: u64,
}

impl PersistentBlobCache {
    /// Create a new persistent cache at the given path
    ///
    /// If `path` is a directory, the cache database will be created as
    /// `blob_cache_zql` inside that directory. If `path` is a file path,
    /// it will be used directly as the database directory.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, CacheError> {
        let path = path.as_ref();

        // If the path is an existing directory, append the default database dirname
        let db_path = if path.is_dir() {
            path.join(DEFAULT_DB_DIRNAME)
        } else {
            path.to_path_buf()
        };

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let db = tokio::task::spawn_blocking(move || zql::Database::open(&db_path))
            .await
            .map_err(|e| CacheError::Database(format!("spawn_blocking failed: {}", e)))?
            .map_err(|e| CacheError::Database(format!("failed to open database: {}", e)))?;

        info!("Opened persistent blob cache at {:?}", path);

        Ok(Self {
            db: tokio::sync::Mutex::new(db),
            max_size_bytes: 10 * 1024 * 1024 * 1024, // 10GB default
        })
    }

    /// Set maximum cache size in bytes
    #[must_use]
    pub fn with_max_size(mut self, max_size_bytes: u64) -> Self {
        self.max_size_bytes = max_size_bytes;
        self
    }

    /// Get a blob by digest
    pub async fn get(&self, digest: &str) -> Result<Option<Vec<u8>>, CacheError> {
        validate_digest(digest)?;

        let mut db = self.db.lock().await;
        let result = db.query(&format!(
            "SELECT * FROM blobs WHERE digest = '{}'",
            digest.replace('\'', "''")
        ));

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                if records.is_empty() {
                    return Ok(None);
                }
                let record = &records[0];
                if let Some(encoded_data) = record.fields.get("data_hex") {
                    let data = decode_blob(encoded_data)?;

                    // Update last_accessed timestamp (best effort)
                    let now = current_timestamp();
                    let _ = db.query(&format!(
                        "DELETE FROM blobs WHERE digest = '{}'",
                        digest.replace('\'', "''")
                    ));
                    let size_bytes = data.len();
                    let created_at = record
                        .fields
                        .get("created_at")
                        .map(|s| s.as_str())
                        .unwrap_or("0");
                    let _ = db.query(&format!(
                        "INSERT INTO blobs (digest, data_hex, size_bytes, created_at, last_accessed) VALUES ('{}', '{}', '{}', '{}', '{}')",
                        digest.replace('\'', "''"),
                        encoded_data.replace('\'', "''"),
                        size_bytes,
                        created_at,
                        now
                    ));

                    Ok(Some(data))
                } else {
                    Ok(None)
                }
            }
            Ok(_) => Ok(None),
            Err(_) => Ok(None),
        }
    }

    /// Put a blob into the cache
    pub async fn put(&self, digest: &str, data: &[u8]) -> Result<(), CacheError> {
        validate_digest(digest)?;

        // Verify digest matches data (skip for manifest cache keys)
        if !digest.starts_with("manifest:") {
            let actual_digest = compute_digest(data);
            if actual_digest != digest {
                return Err(CacheError::Corrupted(format!(
                    "digest mismatch: expected {}, got {}",
                    digest, actual_digest
                )));
            }
        }

        let now = current_timestamp();
        let size_bytes = data.len();
        let encoded_data = encode_blob(data);

        let mut db = self.db.lock().await;

        // Delete existing entry (upsert)
        let _ = db.query(&format!(
            "DELETE FROM blobs WHERE digest = '{}'",
            digest.replace('\'', "''")
        ));

        // Insert
        db.query(&format!(
            "INSERT INTO blobs (digest, data_hex, size_bytes, created_at, last_accessed) VALUES ('{}', '{}', '{}', '{}', '{}')",
            digest.replace('\'', "''"),
            encoded_data.replace('\'', "''"),
            size_bytes,
            now,
            now
        ))
        .map_err(|e| CacheError::Database(format!("failed to insert blob: {}", e)))?;

        debug!("Stored blob {} ({} bytes)", digest, data.len());

        // Drop lock before eviction (eviction re-acquires)
        drop(db);

        // Evict if needed
        self.evict_if_needed().await?;

        Ok(())
    }

    /// Check if a blob exists in the cache
    pub async fn contains(&self, digest: &str) -> Result<bool, CacheError> {
        validate_digest(digest)?;

        let mut db = self.db.lock().await;
        let result = db.query(&format!(
            "SELECT * FROM blobs WHERE digest = '{}'",
            digest.replace('\'', "''")
        ));

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => Ok(!records.is_empty()),
            _ => Ok(false),
        }
    }

    /// Delete a blob from the cache
    pub async fn delete(&self, digest: &str) -> Result<(), CacheError> {
        validate_digest(digest)?;

        let mut db = self.db.lock().await;
        db.query(&format!(
            "DELETE FROM blobs WHERE digest = '{}'",
            digest.replace('\'', "''")
        ))
        .map_err(|e| CacheError::Database(format!("failed to delete blob: {}", e)))?;

        debug!("Deleted blob {}", digest);

        Ok(())
    }

    /// Get current cache size in bytes
    pub async fn size(&self) -> Result<u64, CacheError> {
        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM blobs");

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                let mut total: u64 = 0;
                for record in &records {
                    if let Some(size_str) = record.fields.get("size_bytes") {
                        if let Ok(size) = size_str.parse::<u64>() {
                            total += size;
                        }
                    }
                }
                Ok(total)
            }
            _ => Ok(0),
        }
    }

    /// Get number of blobs in the cache
    pub async fn blob_count(&self) -> Result<u64, CacheError> {
        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM blobs");

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => Ok(records.len() as u64),
            _ => Ok(0),
        }
    }

    /// Clear all blobs from the cache
    pub async fn clear(&self) -> Result<(), CacheError> {
        // Get all digests then delete each one
        let digests: Vec<String> = {
            let mut db = self.db.lock().await;
            let result = db.query("SELECT * FROM blobs");
            match result {
                Ok(zql::query::executor::ExecResult::Retrieved(records)) => records
                    .iter()
                    .filter_map(|r| r.fields.get("digest").cloned())
                    .collect(),
                _ => Vec::new(),
            }
        };

        let mut db = self.db.lock().await;
        for digest in &digests {
            let _ = db.query(&format!(
                "DELETE FROM blobs WHERE digest = '{}'",
                digest.replace('\'', "''")
            ));
        }

        info!("Cleared all blobs from cache");

        Ok(())
    }

    /// Evict blobs using LRU if cache is over size limit
    async fn evict_if_needed(&self) -> Result<(), CacheError> {
        let current_size = self.size().await?;
        if current_size <= self.max_size_bytes {
            return Ok(());
        }

        // Target: evict until we're at 90% capacity
        let target_size = (self.max_size_bytes as f64 * 0.9) as u64;
        let to_evict = current_size.saturating_sub(target_size);

        info!(
            "Cache size {} exceeds limit {}, evicting {} bytes",
            current_size, self.max_size_bytes, to_evict
        );

        // Get all entries with their timestamps for sorting
        let mut entries: Vec<(String, u64, i64)> = {
            let mut db = self.db.lock().await;
            let result = db.query("SELECT * FROM blobs");
            match result {
                Ok(zql::query::executor::ExecResult::Retrieved(records)) => records
                    .iter()
                    .filter_map(|r| {
                        let digest = r.fields.get("digest")?.clone();
                        let size = r
                            .fields
                            .get("size_bytes")
                            .and_then(|s| s.parse::<u64>().ok())?;
                        let last_accessed = r
                            .fields
                            .get("last_accessed")
                            .and_then(|s| s.parse::<i64>().ok())
                            .unwrap_or(0);
                        Some((digest, size, last_accessed))
                    })
                    .collect(),
                _ => Vec::new(),
            }
        };

        // Sort by last_accessed ASC (oldest first)
        entries.sort_by_key(|(_d, _s, ts)| *ts);

        // Evict oldest entries until we reach target
        let mut evicted_size = 0u64;
        let mut evicted_count = 0u64;
        let mut digests_to_delete = Vec::new();

        for (digest, size_bytes, _) in &entries {
            if evicted_size >= to_evict {
                break;
            }

            digests_to_delete.push(digest.clone());
            evicted_size += size_bytes;
            evicted_count += 1;
        }

        // Delete in batch
        let mut db = self.db.lock().await;
        for digest in &digests_to_delete {
            let _ = db.query(&format!(
                "DELETE FROM blobs WHERE digest = '{}'",
                digest.replace('\'', "''")
            ));
        }

        info!(
            "Evicted {} blobs ({} bytes) from cache",
            evicted_count, evicted_size
        );

        Ok(())
    }
}

#[async_trait]
impl BlobCacheBackend for PersistentBlobCache {
    async fn get(&self, digest: &str) -> Result<Option<Vec<u8>>, CacheError> {
        PersistentBlobCache::get(self, digest).await
    }

    async fn put(&self, digest: &str, data: &[u8]) -> Result<(), CacheError> {
        PersistentBlobCache::put(self, digest, data).await
    }

    async fn contains(&self, digest: &str) -> Result<bool, CacheError> {
        PersistentBlobCache::contains(self, digest).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::compute_digest;
    use tempfile::TempDir;

    async fn create_test_cache() -> (PersistentBlobCache, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("test_cache_zql");
        let cache = PersistentBlobCache::open(&cache_path).await.unwrap();
        (cache, temp_dir)
    }

    #[tokio::test]
    async fn test_put_get() {
        let (cache, _temp) = create_test_cache().await;

        let data = b"test data";
        let digest = compute_digest(data);

        cache.put(&digest, data).await.unwrap();
        let retrieved = cache.get(&digest).await.unwrap();
        assert_eq!(retrieved, Some(data.to_vec()));
    }

    #[tokio::test]
    async fn test_contains() {
        let (cache, _temp) = create_test_cache().await;

        let data = b"test data";
        let digest = compute_digest(data);

        assert!(!cache.contains(&digest).await.unwrap());
        cache.put(&digest, data).await.unwrap();
        assert!(cache.contains(&digest).await.unwrap());
    }

    #[tokio::test]
    async fn test_delete() {
        let (cache, _temp) = create_test_cache().await;

        let data = b"test data";
        let digest = compute_digest(data);

        cache.put(&digest, data).await.unwrap();
        assert!(cache.contains(&digest).await.unwrap());

        cache.delete(&digest).await.unwrap();
        assert!(!cache.contains(&digest).await.unwrap());
    }

    #[tokio::test]
    async fn test_size() {
        let (cache, _temp) = create_test_cache().await;

        let data1 = b"test data 1";
        let digest1 = compute_digest(data1);
        let data2 = b"test data 2";
        let digest2 = compute_digest(data2);

        cache.put(&digest1, data1).await.unwrap();
        cache.put(&digest2, data2).await.unwrap();

        let size = cache.size().await.unwrap();
        assert_eq!(size, data1.len() as u64 + data2.len() as u64);
    }

    #[tokio::test]
    async fn test_clear() {
        let (cache, _temp) = create_test_cache().await;

        let data = b"test data";
        let digest = compute_digest(data);

        cache.put(&digest, data).await.unwrap();
        assert!(cache.contains(&digest).await.unwrap());

        cache.clear().await.unwrap();
        assert!(!cache.contains(&digest).await.unwrap());
        assert_eq!(cache.size().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("persist_cache_zql");

        let data = b"persistent data";
        let digest = compute_digest(data);

        // Write data
        {
            let cache = PersistentBlobCache::open(&cache_path).await.unwrap();
            cache.put(&digest, data).await.unwrap();
        }

        // Reopen and verify data persists
        {
            let cache = PersistentBlobCache::open(&cache_path).await.unwrap();
            let retrieved = cache.get(&digest).await.unwrap();
            assert_eq!(retrieved, Some(data.to_vec()));
        }
    }

    #[tokio::test]
    async fn test_invalid_digest() {
        let (cache, _temp) = create_test_cache().await;

        let result = cache.get("invalid_digest").await;
        assert!(result.is_err());

        let result = cache.put("md5:abc", b"data").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_digest_mismatch() {
        let (cache, _temp) = create_test_cache().await;

        let data = b"test data";
        let wrong_digest =
            "sha256:0000000000000000000000000000000000000000000000000000000000000000";

        let result = cache.put(wrong_digest, data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_blob_count() {
        let (cache, _temp) = create_test_cache().await;

        assert_eq!(cache.blob_count().await.unwrap(), 0);

        let data1 = b"test data 1";
        let digest1 = compute_digest(data1);
        cache.put(&digest1, data1).await.unwrap();
        assert_eq!(cache.blob_count().await.unwrap(), 1);

        let data2 = b"test data 2";
        let digest2 = compute_digest(data2);
        cache.put(&digest2, data2).await.unwrap();
        assert_eq!(cache.blob_count().await.unwrap(), 2);

        cache.delete(&digest1).await.unwrap();
        assert_eq!(cache.blob_count().await.unwrap(), 1);
    }
}
