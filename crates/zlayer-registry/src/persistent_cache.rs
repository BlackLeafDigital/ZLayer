//! Persistent blob cache for OCI images using SQLx with SQLite
//!
//! This module provides a persistent blob cache backed by SQLite for durability.
//! Uses WAL mode for concurrent multi-process access.
//! Blobs are stored with metadata for LRU eviction.

use crate::cache::{compute_digest, validate_digest, BlobCacheBackend};
use crate::error::{CacheError, Result};
use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

/// Default database filename used when a directory is provided
const DEFAULT_DB_FILENAME: &str = "blob_cache.sqlite";

fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Persistent blob cache for OCI images backed by SQLite
pub struct PersistentBlobCache {
    pool: SqlitePool,
    max_size_bytes: u64,
}

impl PersistentBlobCache {
    /// Create a new persistent cache at the given path
    ///
    /// If `path` is a directory, the cache database will be created as
    /// `blob_cache.sqlite` inside that directory. If `path` is a file path,
    /// it will be used directly as the database file.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, CacheError> {
        let path = path.as_ref();

        // If the path is an existing directory, append the default database filename
        let db_path = if path.is_dir() {
            path.join(DEFAULT_DB_FILENAME)
        } else {
            path.to_path_buf()
        };

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Build connection options with WAL mode and busy timeout
        let connect_options =
            SqliteConnectOptions::from_str(&format!("sqlite:{}?mode=rwc", db_path.display()))
                .map_err(|e| CacheError::Database(format!("invalid database path: {}", e)))?
                .pragma("journal_mode", "WAL")
                .pragma("busy_timeout", "5000")
                .pragma("synchronous", "NORMAL")
                .create_if_missing(true);

        // Create connection pool
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(connect_options)
            .await
            .map_err(|e| CacheError::Database(format!("failed to open database: {}", e)))?;

        // Initialize schema
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS blobs (
                digest TEXT PRIMARY KEY NOT NULL,
                data BLOB NOT NULL,
                size_bytes INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                last_accessed INTEGER NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .map_err(|e| CacheError::Database(format!("failed to create blobs table: {}", e)))?;

        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_blobs_last_accessed ON blobs(last_accessed)
            "#,
        )
        .execute(&pool)
        .await
        .map_err(|e| CacheError::Database(format!("failed to create index: {}", e)))?;

        info!("Opened persistent blob cache at {:?}", db_path);

        Ok(Self {
            pool,
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

        let result: Option<Vec<u8>> = sqlx::query_scalar("SELECT data FROM blobs WHERE digest = ?")
            .bind(digest)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| CacheError::Database(format!("failed to get blob: {}", e)))?;

        // Update last_accessed timestamp asynchronously (best effort)
        if result.is_some() {
            let _ = self.update_access_time(digest).await;
        }

        Ok(result)
    }

    /// Update the last_accessed timestamp for a blob
    async fn update_access_time(&self, digest: &str) -> Result<(), CacheError> {
        let now = current_timestamp();

        sqlx::query("UPDATE blobs SET last_accessed = ? WHERE digest = ?")
            .bind(now)
            .bind(digest)
            .execute(&self.pool)
            .await
            .map_err(|e| CacheError::Database(format!("failed to update access time: {}", e)))?;

        Ok(())
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
        let size_bytes = data.len() as i64;

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO blobs (digest, data, size_bytes, created_at, last_accessed)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(digest)
        .bind(data)
        .bind(size_bytes)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| CacheError::Database(format!("failed to insert blob: {}", e)))?;

        debug!("Stored blob {} ({} bytes)", digest, data.len());

        // Evict if needed (after insert to avoid holding transaction too long)
        self.evict_if_needed().await?;

        Ok(())
    }

    /// Check if a blob exists in the cache
    pub async fn contains(&self, digest: &str) -> Result<bool, CacheError> {
        validate_digest(digest)?;

        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM blobs WHERE digest = ?)")
                .bind(digest)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| CacheError::Database(format!("failed to check blob: {}", e)))?;

        Ok(exists)
    }

    /// Delete a blob from the cache
    pub async fn delete(&self, digest: &str) -> Result<(), CacheError> {
        validate_digest(digest)?;

        sqlx::query("DELETE FROM blobs WHERE digest = ?")
            .bind(digest)
            .execute(&self.pool)
            .await
            .map_err(|e| CacheError::Database(format!("failed to delete blob: {}", e)))?;

        debug!("Deleted blob {}", digest);

        Ok(())
    }

    /// Get current cache size in bytes
    pub async fn size(&self) -> Result<u64, CacheError> {
        let total: Option<i64> = sqlx::query_scalar("SELECT SUM(size_bytes) FROM blobs")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| CacheError::Database(format!("failed to get cache size: {}", e)))?;

        Ok(total.unwrap_or(0) as u64)
    }

    /// Get number of blobs in the cache
    pub async fn blob_count(&self) -> Result<u64, CacheError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blobs")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| CacheError::Database(format!("failed to get blob count: {}", e)))?;

        Ok(count as u64)
    }

    /// Clear all blobs from the cache
    pub async fn clear(&self) -> Result<(), CacheError> {
        sqlx::query("DELETE FROM blobs")
            .execute(&self.pool)
            .await
            .map_err(|e| CacheError::Database(format!("failed to clear cache: {}", e)))?;

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

        // Get oldest entries sorted by last_accessed
        let entries: Vec<(String, i64)> =
            sqlx::query_as("SELECT digest, size_bytes FROM blobs ORDER BY last_accessed ASC")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| {
                    CacheError::Database(format!("failed to get blobs for eviction: {}", e))
                })?;

        // Evict oldest entries until we reach target
        let mut evicted_size = 0u64;
        let mut evicted_count = 0u64;
        let mut digests_to_delete = Vec::new();

        for (digest, size_bytes) in entries {
            if evicted_size >= to_evict {
                break;
            }

            digests_to_delete.push(digest);
            evicted_size += size_bytes as u64;
            evicted_count += 1;
        }

        // Delete in batch for efficiency
        for digest in &digests_to_delete {
            sqlx::query("DELETE FROM blobs WHERE digest = ?")
                .bind(digest)
                .execute(&self.pool)
                .await
                .map_err(|e| CacheError::Database(format!("failed to delete blob: {}", e)))?;
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
        let cache_path = temp_dir.path().join("test_cache.sqlite");
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
        let cache_path = temp_dir.path().join("persist_cache.sqlite");

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
    async fn test_eviction() {
        let (cache, _temp) = create_test_cache().await;
        let cache = cache.with_max_size(100); // 100 bytes max

        // Insert data that exceeds limit
        for i in 0..20 {
            let data = format!("data_{:02}", i);
            let digest = compute_digest(data.as_bytes());
            cache.put(&digest, data.as_bytes()).await.unwrap();

            // Add small delay to ensure different timestamps
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // Cache should have evicted some entries
        let size = cache.size().await.unwrap();
        assert!(size <= 100, "Cache size {} should be <= 100", size);
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
    async fn test_open_with_directory_path() {
        // Test that opening with a directory path works (appends default filename)
        let temp_dir = TempDir::new().unwrap();

        // Pass the directory path instead of a file path
        let cache = PersistentBlobCache::open(temp_dir.path()).await.unwrap();

        // Verify cache works
        let data = b"test data for directory path";
        let digest = compute_digest(data);

        cache.put(&digest, data).await.unwrap();
        let retrieved = cache.get(&digest).await.unwrap();
        assert_eq!(retrieved, Some(data.to_vec()));

        // Verify the database file was created with the default name
        let expected_db_path = temp_dir.path().join(DEFAULT_DB_FILENAME);
        assert!(
            expected_db_path.exists(),
            "Database file should be created at {:?}",
            expected_db_path
        );
    }

    #[tokio::test]
    async fn test_lru_access_time_update() {
        let (cache, _temp) = create_test_cache().await;
        let cache = cache.with_max_size(150); // 150 bytes max

        // Create three blobs of 60 bytes each
        let data1 = vec![1u8; 60];
        let digest1 = compute_digest(&data1);
        let data2 = vec![2u8; 60];
        let digest2 = compute_digest(&data2);
        let data3 = vec![3u8; 60];
        let digest3 = compute_digest(&data3);

        // Add first two blobs (120 bytes total, under limit)
        cache.put(&digest1, &data1).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await; // Use full second sleeps for timestamp resolution
        cache.put(&digest2, &data2).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // Verify both exist
        assert!(cache.contains(&digest1).await.unwrap());
        assert!(cache.contains(&digest2).await.unwrap());

        // Access digest1 to update its access time (making it more recent than data2)
        let result = cache.get(&digest1).await.unwrap();
        assert!(result.is_some(), "data1 should exist");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // Adding third blob brings us to 180 bytes (over 150 limit)
        // Eviction target is 90% of 150 = 135 bytes
        // Need to evict 180 - 135 = 45 bytes
        // Should evict data2 (60 bytes, the oldest), leaving data1 and data3
        cache.put(&digest3, &data3).await.unwrap();

        // Verify the eviction happened correctly
        let has_data1 = cache.contains(&digest1).await.unwrap();
        let has_data2 = cache.contains(&digest2).await.unwrap();
        let has_data3 = cache.contains(&digest3).await.unwrap();
        let final_size = cache.size().await.unwrap();

        // data3 (just added) should always remain
        assert!(
            has_data3,
            "data3 should remain (just added). data1={}, data2={}, data3={}, size={}",
            has_data1, has_data2, has_data3, final_size
        );

        // At least one should be evicted
        assert!(
            !has_data1 || !has_data2,
            "At least one of data1 or data2 should be evicted. data1={}, data2={}, data3={}, size={}",
            has_data1, has_data2, has_data3, final_size
        );

        // data2 (oldest) should be evicted before data1 (accessed recently)
        if !has_data1 && has_data2 {
            panic!(
                "LRU eviction failed: data1 (recently accessed) was evicted but data2 (oldest) was kept. data1={}, data2={}, data3={}, size={}",
                has_data1, has_data2, has_data3, final_size
            );
        }
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
