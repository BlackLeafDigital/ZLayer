//! Persistent blob cache for OCI images using redb
//!
//! This module provides a persistent blob cache backed by redb for durability.
//! Blobs are stored with metadata for LRU eviction.

use crate::cache::{compute_digest, validate_digest, BlobCacheBackend};
use crate::error::{CacheError, Result};
use async_trait::async_trait;
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

/// Table for blob data: digest -> blob bytes
const BLOB_DATA: TableDefinition<&str, &[u8]> = TableDefinition::new("blob_data");

/// Table for blob metadata: digest -> serialized BlobMetadata
const BLOB_METADATA: TableDefinition<&str, &[u8]> = TableDefinition::new("blob_metadata");

/// Metadata stored for each blob to support LRU eviction
#[derive(Debug, Clone)]
struct BlobMetadata {
    /// Size of the blob in bytes
    size_bytes: u64,
    /// Unix timestamp when blob was created
    created_at: u64,
    /// Unix timestamp when blob was last accessed
    last_accessed: u64,
}

impl BlobMetadata {
    fn new(size_bytes: u64) -> Self {
        let now = current_timestamp();
        Self {
            size_bytes,
            created_at: now,
            last_accessed: now,
        }
    }

    fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(24);
        buf.extend_from_slice(&self.size_bytes.to_le_bytes());
        buf.extend_from_slice(&self.created_at.to_le_bytes());
        buf.extend_from_slice(&self.last_accessed.to_le_bytes());
        buf
    }

    fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < 24 {
            return None;
        }
        Some(Self {
            size_bytes: u64::from_le_bytes(data[0..8].try_into().ok()?),
            created_at: u64::from_le_bytes(data[8..16].try_into().ok()?),
            last_accessed: u64::from_le_bytes(data[16..24].try_into().ok()?),
        })
    }
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Default database filename used when a directory is provided
const DEFAULT_DB_FILENAME: &str = "blob_cache.redb";

/// Persistent blob cache for OCI images backed by redb
pub struct PersistentBlobCache {
    db: Arc<Database>,
    max_size_bytes: u64,
}

impl PersistentBlobCache {
    /// Create a new persistent cache at the given path
    ///
    /// If `path` is a directory, the cache database will be created as
    /// `blob_cache.redb` inside that directory. If `path` is a file path,
    /// it will be used directly as the database file.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, CacheError> {
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

        let db = Database::create(&db_path).map_err(|e| {
            CacheError::Database(format!("failed to open database at {:?}: {}", db_path, e))
        })?;

        // Initialize tables
        let write_txn = db.begin_write().map_err(|e| {
            CacheError::Database(format!("failed to begin write transaction: {}", e))
        })?;

        // Create tables if they don't exist
        {
            let _ = write_txn.open_table(BLOB_DATA).map_err(|e| {
                CacheError::Database(format!("failed to open blob_data table: {}", e))
            })?;
            let _ = write_txn.open_table(BLOB_METADATA).map_err(|e| {
                CacheError::Database(format!("failed to open blob_metadata table: {}", e))
            })?;
        }

        write_txn
            .commit()
            .map_err(|e| CacheError::Database(format!("failed to commit: {}", e)))?;

        info!("Opened persistent blob cache at {:?}", db_path);

        Ok(Self {
            db: Arc::new(db),
            max_size_bytes: 10 * 1024 * 1024 * 1024, // 10GB default
        })
    }

    /// Set maximum cache size in bytes
    pub fn with_max_size(mut self, max_size_bytes: u64) -> Self {
        self.max_size_bytes = max_size_bytes;
        self
    }

    /// Get a blob by digest
    pub fn get(&self, digest: &str) -> Result<Option<Vec<u8>>, CacheError> {
        validate_digest(digest)?;

        let read_txn = self.db.begin_read().map_err(|e| {
            CacheError::Database(format!("failed to begin read transaction: {}", e))
        })?;

        let table = read_txn
            .open_table(BLOB_DATA)
            .map_err(|e| CacheError::Database(format!("failed to open blob_data table: {}", e)))?;

        let result = table
            .get(digest)
            .map_err(|e| CacheError::Database(format!("failed to get blob: {}", e)))?
            .map(|v| v.value().to_vec());

        // Update last_accessed timestamp asynchronously (best effort)
        if result.is_some() {
            let _ = self.update_access_time(digest);
        }

        Ok(result)
    }

    /// Update the last_accessed timestamp for a blob
    fn update_access_time(&self, digest: &str) -> Result<(), CacheError> {
        let write_txn = self.db.begin_write().map_err(|e| {
            CacheError::Database(format!("failed to begin write transaction: {}", e))
        })?;

        {
            let mut meta_table = write_txn.open_table(BLOB_METADATA).map_err(|e| {
                CacheError::Database(format!("failed to open blob_metadata table: {}", e))
            })?;

            // Read metadata first, then update
            let metadata_bytes = meta_table
                .get(digest)
                .map_err(|e| CacheError::Database(format!("failed to get metadata: {}", e)))?
                .map(|v| v.value().to_vec());

            if let Some(bytes) = metadata_bytes {
                if let Some(mut metadata) = BlobMetadata::deserialize(&bytes) {
                    metadata.last_accessed = current_timestamp();
                    meta_table
                        .insert(digest, metadata.serialize().as_slice())
                        .map_err(|e| {
                            CacheError::Database(format!("failed to update metadata: {}", e))
                        })?;
                }
            }
        }

        write_txn
            .commit()
            .map_err(|e| CacheError::Database(format!("failed to commit: {}", e)))?;

        Ok(())
    }

    /// Put a blob into the cache
    pub fn put(&self, digest: &str, data: &[u8]) -> Result<(), CacheError> {
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

        let write_txn = self.db.begin_write().map_err(|e| {
            CacheError::Database(format!("failed to begin write transaction: {}", e))
        })?;

        {
            let mut data_table = write_txn.open_table(BLOB_DATA).map_err(|e| {
                CacheError::Database(format!("failed to open blob_data table: {}", e))
            })?;

            let mut meta_table = write_txn.open_table(BLOB_METADATA).map_err(|e| {
                CacheError::Database(format!("failed to open blob_metadata table: {}", e))
            })?;

            // Store blob data
            data_table
                .insert(digest, data)
                .map_err(|e| CacheError::Database(format!("failed to insert blob: {}", e)))?;

            // Store metadata
            let metadata = BlobMetadata::new(data.len() as u64);
            meta_table
                .insert(digest, metadata.serialize().as_slice())
                .map_err(|e| CacheError::Database(format!("failed to insert metadata: {}", e)))?;
        }

        write_txn
            .commit()
            .map_err(|e| CacheError::Database(format!("failed to commit: {}", e)))?;

        debug!("Stored blob {} ({} bytes)", digest, data.len());

        // Evict if needed (after commit to avoid holding transaction too long)
        self.evict_if_needed()?;

        Ok(())
    }

    /// Check if a blob exists in the cache
    pub fn contains(&self, digest: &str) -> Result<bool, CacheError> {
        validate_digest(digest)?;

        let read_txn = self.db.begin_read().map_err(|e| {
            CacheError::Database(format!("failed to begin read transaction: {}", e))
        })?;

        let table = read_txn
            .open_table(BLOB_DATA)
            .map_err(|e| CacheError::Database(format!("failed to open blob_data table: {}", e)))?;

        let exists = table
            .get(digest)
            .map_err(|e| CacheError::Database(format!("failed to check blob: {}", e)))?
            .is_some();

        Ok(exists)
    }

    /// Delete a blob from the cache
    pub fn delete(&self, digest: &str) -> Result<(), CacheError> {
        validate_digest(digest)?;

        let write_txn = self.db.begin_write().map_err(|e| {
            CacheError::Database(format!("failed to begin write transaction: {}", e))
        })?;

        {
            let mut data_table = write_txn.open_table(BLOB_DATA).map_err(|e| {
                CacheError::Database(format!("failed to open blob_data table: {}", e))
            })?;

            let mut meta_table = write_txn.open_table(BLOB_METADATA).map_err(|e| {
                CacheError::Database(format!("failed to open blob_metadata table: {}", e))
            })?;

            data_table
                .remove(digest)
                .map_err(|e| CacheError::Database(format!("failed to remove blob: {}", e)))?;

            meta_table
                .remove(digest)
                .map_err(|e| CacheError::Database(format!("failed to remove metadata: {}", e)))?;
        }

        write_txn
            .commit()
            .map_err(|e| CacheError::Database(format!("failed to commit: {}", e)))?;

        debug!("Deleted blob {}", digest);

        Ok(())
    }

    /// Get current cache size in bytes
    pub fn size(&self) -> Result<u64, CacheError> {
        let read_txn = self.db.begin_read().map_err(|e| {
            CacheError::Database(format!("failed to begin read transaction: {}", e))
        })?;

        let table = read_txn.open_table(BLOB_METADATA).map_err(|e| {
            CacheError::Database(format!("failed to open blob_metadata table: {}", e))
        })?;

        let mut total_size = 0u64;
        for entry in table
            .iter()
            .map_err(|e| CacheError::Database(format!("failed to iterate metadata: {}", e)))?
        {
            let (_, value) =
                entry.map_err(|e| CacheError::Database(format!("failed to read entry: {}", e)))?;
            if let Some(metadata) = BlobMetadata::deserialize(value.value()) {
                total_size += metadata.size_bytes;
            }
        }

        Ok(total_size)
    }

    /// Get number of blobs in the cache
    pub fn blob_count(&self) -> Result<u64, CacheError> {
        let read_txn = self.db.begin_read().map_err(|e| {
            CacheError::Database(format!("failed to begin read transaction: {}", e))
        })?;

        let table = read_txn
            .open_table(BLOB_DATA)
            .map_err(|e| CacheError::Database(format!("failed to open blob_data table: {}", e)))?;

        let count = table
            .len()
            .map_err(|e| CacheError::Database(format!("failed to get table length: {}", e)))?;

        Ok(count)
    }

    /// Clear all blobs from the cache
    pub fn clear(&self) -> Result<(), CacheError> {
        let write_txn = self.db.begin_write().map_err(|e| {
            CacheError::Database(format!("failed to begin write transaction: {}", e))
        })?;

        // Drop and recreate tables
        write_txn.delete_table(BLOB_DATA).map_err(|e| {
            CacheError::Database(format!("failed to delete blob_data table: {}", e))
        })?;
        write_txn.delete_table(BLOB_METADATA).map_err(|e| {
            CacheError::Database(format!("failed to delete blob_metadata table: {}", e))
        })?;

        // Recreate tables
        let _ = write_txn.open_table(BLOB_DATA).map_err(|e| {
            CacheError::Database(format!("failed to recreate blob_data table: {}", e))
        })?;
        let _ = write_txn.open_table(BLOB_METADATA).map_err(|e| {
            CacheError::Database(format!("failed to recreate blob_metadata table: {}", e))
        })?;

        write_txn
            .commit()
            .map_err(|e| CacheError::Database(format!("failed to commit: {}", e)))?;

        info!("Cleared all blobs from cache");

        Ok(())
    }

    /// Evict blobs using LRU if cache is over size limit
    fn evict_if_needed(&self) -> Result<(), CacheError> {
        let current_size = self.size()?;
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

        // Collect all metadata with digests, sorted by last_accessed (ascending = oldest first)
        let mut entries: Vec<(String, BlobMetadata)> = Vec::new();

        {
            let read_txn = self.db.begin_read().map_err(|e| {
                CacheError::Database(format!("failed to begin read transaction: {}", e))
            })?;

            let table = read_txn.open_table(BLOB_METADATA).map_err(|e| {
                CacheError::Database(format!("failed to open blob_metadata table: {}", e))
            })?;

            for entry in table
                .iter()
                .map_err(|e| CacheError::Database(format!("failed to iterate metadata: {}", e)))?
            {
                let (key, value) = entry
                    .map_err(|e| CacheError::Database(format!("failed to read entry: {}", e)))?;
                if let Some(metadata) = BlobMetadata::deserialize(value.value()) {
                    entries.push((key.value().to_string(), metadata));
                }
            }
        }

        // Sort by last_accessed (oldest first)
        entries.sort_by_key(|(_, m)| m.last_accessed);

        // Evict oldest entries until we reach target
        let mut evicted_size = 0u64;
        let mut evicted_count = 0u64;

        for (digest, metadata) in entries {
            if evicted_size >= to_evict {
                break;
            }

            self.delete(&digest)?;
            evicted_size += metadata.size_bytes;
            evicted_count += 1;
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
        // Delegate to the synchronous method
        PersistentBlobCache::get(self, digest)
    }

    async fn put(&self, digest: &str, data: &[u8]) -> Result<(), CacheError> {
        // Delegate to the synchronous method
        PersistentBlobCache::put(self, digest, data)
    }

    async fn contains(&self, digest: &str) -> Result<bool, CacheError> {
        // Delegate to the synchronous method
        PersistentBlobCache::contains(self, digest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::compute_digest;
    use tempfile::TempDir;

    fn create_test_cache() -> (PersistentBlobCache, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("test_cache.redb");
        let cache = PersistentBlobCache::open(&cache_path).unwrap();
        (cache, temp_dir)
    }

    #[test]
    fn test_put_get() {
        let (cache, _temp) = create_test_cache();

        let data = b"test data";
        let digest = compute_digest(data);

        cache.put(&digest, data).unwrap();
        let retrieved = cache.get(&digest).unwrap();
        assert_eq!(retrieved, Some(data.to_vec()));
    }

    #[test]
    fn test_contains() {
        let (cache, _temp) = create_test_cache();

        let data = b"test data";
        let digest = compute_digest(data);

        assert!(!cache.contains(&digest).unwrap());
        cache.put(&digest, data).unwrap();
        assert!(cache.contains(&digest).unwrap());
    }

    #[test]
    fn test_delete() {
        let (cache, _temp) = create_test_cache();

        let data = b"test data";
        let digest = compute_digest(data);

        cache.put(&digest, data).unwrap();
        assert!(cache.contains(&digest).unwrap());

        cache.delete(&digest).unwrap();
        assert!(!cache.contains(&digest).unwrap());
    }

    #[test]
    fn test_size() {
        let (cache, _temp) = create_test_cache();

        let data1 = b"test data 1";
        let digest1 = compute_digest(data1);
        let data2 = b"test data 2";
        let digest2 = compute_digest(data2);

        cache.put(&digest1, data1).unwrap();
        cache.put(&digest2, data2).unwrap();

        let size = cache.size().unwrap();
        assert_eq!(size, data1.len() as u64 + data2.len() as u64);
    }

    #[test]
    fn test_clear() {
        let (cache, _temp) = create_test_cache();

        let data = b"test data";
        let digest = compute_digest(data);

        cache.put(&digest, data).unwrap();
        assert!(cache.contains(&digest).unwrap());

        cache.clear().unwrap();
        assert!(!cache.contains(&digest).unwrap());
        assert_eq!(cache.size().unwrap(), 0);
    }

    #[test]
    fn test_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("persist_cache.redb");

        let data = b"persistent data";
        let digest = compute_digest(data);

        // Write data
        {
            let cache = PersistentBlobCache::open(&cache_path).unwrap();
            cache.put(&digest, data).unwrap();
        }

        // Reopen and verify data persists
        {
            let cache = PersistentBlobCache::open(&cache_path).unwrap();
            let retrieved = cache.get(&digest).unwrap();
            assert_eq!(retrieved, Some(data.to_vec()));
        }
    }

    #[test]
    fn test_eviction() {
        let (cache, _temp) = create_test_cache();
        let cache = cache.with_max_size(100); // 100 bytes max

        // Insert data that exceeds limit
        for i in 0..20 {
            let data = format!("data_{:02}", i);
            let digest = compute_digest(data.as_bytes());
            cache.put(&digest, data.as_bytes()).unwrap();

            // Add small delay to ensure different timestamps
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Cache should have evicted some entries
        let size = cache.size().unwrap();
        assert!(size <= 100, "Cache size {} should be <= 100", size);
    }

    #[test]
    fn test_invalid_digest() {
        let (cache, _temp) = create_test_cache();

        let result = cache.get("invalid_digest");
        assert!(result.is_err());

        let result = cache.put("md5:abc", b"data");
        assert!(result.is_err());
    }

    #[test]
    fn test_digest_mismatch() {
        let (cache, _temp) = create_test_cache();

        let data = b"test data";
        let wrong_digest =
            "sha256:0000000000000000000000000000000000000000000000000000000000000000";

        let result = cache.put(wrong_digest, data);
        assert!(result.is_err());
    }

    #[test]
    fn test_open_with_directory_path() {
        // Test that opening with a directory path works (appends default filename)
        let temp_dir = TempDir::new().unwrap();

        // Pass the directory path instead of a file path
        let cache = PersistentBlobCache::open(temp_dir.path()).unwrap();

        // Verify cache works
        let data = b"test data for directory path";
        let digest = compute_digest(data);

        cache.put(&digest, data).unwrap();
        let retrieved = cache.get(&digest).unwrap();
        assert_eq!(retrieved, Some(data.to_vec()));

        // Verify the database file was created with the default name
        let expected_db_path = temp_dir.path().join(DEFAULT_DB_FILENAME);
        assert!(
            expected_db_path.exists(),
            "Database file should be created at {:?}",
            expected_db_path
        );
    }

    #[test]
    fn test_lru_access_time_update() {
        let (cache, _temp) = create_test_cache();
        let cache = cache.with_max_size(150); // 150 bytes max

        // Create three blobs of 60 bytes each
        let data1 = vec![1u8; 60];
        let digest1 = compute_digest(&data1);
        let data2 = vec![2u8; 60];
        let digest2 = compute_digest(&data2);
        let data3 = vec![3u8; 60];
        let digest3 = compute_digest(&data3);

        // Add first two blobs (120 bytes total, under limit)
        cache.put(&digest1, &data1).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1)); // Use full second sleeps for timestamp resolution
        cache.put(&digest2, &data2).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));

        // Verify both exist
        assert!(cache.contains(&digest1).unwrap());
        assert!(cache.contains(&digest2).unwrap());

        // Access digest1 to update its access time (making it more recent than data2)
        let result = cache.get(&digest1).unwrap();
        assert!(result.is_some(), "data1 should exist");
        std::thread::sleep(std::time::Duration::from_secs(1));

        // Adding third blob brings us to 180 bytes (over 150 limit)
        // Eviction target is 90% of 150 = 135 bytes
        // Need to evict 180 - 135 = 45 bytes
        // Should evict data2 (60 bytes, the oldest), leaving data1 and data3
        cache.put(&digest3, &data3).unwrap();

        // Verify the eviction happened correctly
        let has_data1 = cache.contains(&digest1).unwrap();
        let has_data2 = cache.contains(&digest2).unwrap();
        let has_data3 = cache.contains(&digest3).unwrap();
        let final_size = cache.size().unwrap();

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
}
