//! Local write cache for network tolerance
//!
//! Buffers WAL segments locally when network is unavailable, maintaining FIFO order
//! for upload and respecting configurable size limits.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// A cached WAL segment entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Sequence number (frame count at time of capture)
    pub sequence: u64,
    /// WAL data
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
    /// Timestamp when cached
    pub cached_at: chrono::DateTime<chrono::Utc>,
    /// Number of upload attempts
    pub attempts: u32,
}

impl CacheEntry {
    /// Create a new cache entry
    pub fn new(sequence: u64, data: Vec<u8>) -> Self {
        Self {
            sequence,
            data,
            cached_at: chrono::Utc::now(),
            attempts: 0,
        }
    }

    /// Size of this entry in bytes
    pub fn size(&self) -> u64 {
        self.data.len() as u64
    }
}

/// Local write cache for WAL segments
///
/// Provides network tolerance by buffering WAL segments locally when S3 is
/// unavailable. Entries are stored in FIFO order and persisted to disk for
/// crash recovery.
pub struct WriteCache {
    /// Cache directory for persisted entries
    cache_dir: PathBuf,
    /// Maximum cache size in bytes
    max_size: u64,
    /// In-memory queue of entries
    entries: Arc<RwLock<VecDeque<CacheEntry>>>,
    /// Current total size in bytes
    current_size: Arc<AtomicU64>,
    /// Number of entries
    entry_count: Arc<AtomicUsize>,
}

impl WriteCache {
    /// Create a new write cache
    ///
    /// # Arguments
    ///
    /// * `cache_dir` - Directory for persisting cache entries
    /// * `max_size` - Maximum total size of cached data in bytes
    pub fn new(cache_dir: PathBuf, max_size: u64) -> Self {
        Self {
            cache_dir,
            max_size,
            entries: Arc::new(RwLock::new(VecDeque::new())),
            current_size: Arc::new(AtomicU64::new(0)),
            entry_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Load existing cache entries from disk
    #[allow(dead_code)]
    pub async fn load(&self) -> Result<()> {
        if !self.cache_dir.exists() {
            return Ok(());
        }

        let mut entries = self.entries.write().await;
        let mut dir = tokio::fs::read_dir(&self.cache_dir).await?;

        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("cache") {
                match tokio::fs::read(&path).await {
                    Ok(data) => match serde_json::from_slice::<CacheEntry>(&data) {
                        Ok(cache_entry) => {
                            let size = cache_entry.size();
                            entries.push_back(cache_entry);
                            self.current_size.fetch_add(size, Ordering::SeqCst);
                            self.entry_count.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(e) => {
                            warn!("Failed to parse cache entry {:?}: {}", path, e);
                        }
                    },
                    Err(e) => {
                        warn!("Failed to read cache entry {:?}: {}", path, e);
                    }
                }
            }
        }

        // Sort by sequence number
        entries
            .make_contiguous()
            .sort_by_key(|e| e.sequence);

        info!(
            "Loaded {} cache entries ({} bytes)",
            entries.len(),
            self.current_size.load(Ordering::SeqCst)
        );

        Ok(())
    }

    /// Add a new entry to the cache
    ///
    /// If the cache is full, this will evict the oldest entries to make room.
    pub async fn add(&self, sequence: u64, data: Vec<u8>) -> Result<()> {
        let entry = CacheEntry::new(sequence, data);
        let entry_size = entry.size();

        // Evict old entries if necessary
        while self.current_size.load(Ordering::SeqCst) + entry_size > self.max_size {
            if let Some(evicted) = self.pop_oldest_internal().await? {
                warn!(
                    "Cache full, evicting entry {} ({} bytes)",
                    evicted.sequence,
                    evicted.size()
                );
                self.remove_from_disk(&evicted).await?;
            } else {
                break;
            }
        }

        // Persist to disk
        let filename = format!("{:020}.cache", entry.sequence);
        let path = self.cache_dir.join(&filename);
        let serialized = serde_json::to_vec(&entry)?;
        tokio::fs::write(&path, &serialized).await?;

        // Add to in-memory queue
        let mut entries = self.entries.write().await;
        entries.push_back(entry);
        self.current_size.fetch_add(entry_size, Ordering::SeqCst);
        self.entry_count.fetch_add(1, Ordering::SeqCst);

        debug!(
            "Cached WAL segment {} ({} bytes, {} total entries)",
            sequence,
            entry_size,
            entries.len()
        );

        Ok(())
    }

    /// Pop the oldest entry from the cache (for upload)
    ///
    /// The entry is removed from the in-memory queue but NOT from disk.
    /// Call `remove()` after successful upload to clean up the disk file.
    pub async fn pop_oldest(&self) -> Result<Option<CacheEntry>> {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.pop_front() {
            let size = entry.size();
            self.current_size.fetch_sub(size, Ordering::SeqCst);
            self.entry_count.fetch_sub(1, Ordering::SeqCst);
            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }

    /// Internal pop without modifying counters (for eviction)
    async fn pop_oldest_internal(&self) -> Result<Option<CacheEntry>> {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.pop_front() {
            let size = entry.size();
            self.current_size.fetch_sub(size, Ordering::SeqCst);
            self.entry_count.fetch_sub(1, Ordering::SeqCst);
            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }

    /// Remove an entry from disk after successful upload
    pub async fn remove(&self, entry: &CacheEntry) -> Result<()> {
        self.remove_from_disk(entry).await
    }

    /// Remove entry file from disk
    async fn remove_from_disk(&self, entry: &CacheEntry) -> Result<()> {
        let filename = format!("{:020}.cache", entry.sequence);
        let path = self.cache_dir.join(&filename);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        Ok(())
    }

    /// Peek at the oldest entry without removing it
    #[allow(dead_code)]
    pub async fn peek_oldest(&self) -> Option<CacheEntry> {
        let entries = self.entries.read().await;
        entries.front().cloned()
    }

    /// Check if the cache is empty
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entry_count.load(Ordering::SeqCst) == 0
    }

    /// Get cache statistics
    pub fn stats(&self) -> (usize, u64) {
        (
            self.entry_count.load(Ordering::SeqCst),
            self.current_size.load(Ordering::SeqCst),
        )
    }

    /// Get the number of cached entries
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entry_count.load(Ordering::SeqCst)
    }

    /// Get the current cache size in bytes
    #[allow(dead_code)]
    pub fn size(&self) -> u64 {
        self.current_size.load(Ordering::SeqCst)
    }

    /// Get the maximum cache size in bytes
    #[allow(dead_code)]
    pub fn max_size(&self) -> u64 {
        self.max_size
    }

    /// Clear all entries from the cache
    #[allow(dead_code)]
    pub async fn clear(&self) -> Result<()> {
        let mut entries = self.entries.write().await;

        // Remove all disk files
        for entry in entries.iter() {
            let _ = self.remove_from_disk(entry).await;
        }

        entries.clear();
        self.current_size.store(0, Ordering::SeqCst);
        self.entry_count.store(0, Ordering::SeqCst);

        info!("Cache cleared");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_cache_add_and_pop() {
        let temp = TempDir::new().unwrap();
        let cache = WriteCache::new(temp.path().to_path_buf(), 1024 * 1024);

        // Add entries
        cache.add(1, vec![1, 2, 3]).await.unwrap();
        cache.add(2, vec![4, 5, 6]).await.unwrap();
        cache.add(3, vec![7, 8, 9]).await.unwrap();

        assert_eq!(cache.len(), 3);

        // Pop in FIFO order
        let entry1 = cache.pop_oldest().await.unwrap().unwrap();
        assert_eq!(entry1.sequence, 1);
        assert_eq!(entry1.data, vec![1, 2, 3]);

        let entry2 = cache.pop_oldest().await.unwrap().unwrap();
        assert_eq!(entry2.sequence, 2);

        // Remove from disk
        cache.remove(&entry1).await.unwrap();
        cache.remove(&entry2).await.unwrap();

        assert_eq!(cache.len(), 1);
    }

    #[tokio::test]
    async fn test_cache_eviction() {
        let temp = TempDir::new().unwrap();
        // Max size of 20 bytes
        let cache = WriteCache::new(temp.path().to_path_buf(), 20);

        // Add entries that exceed max size
        cache.add(1, vec![0; 10]).await.unwrap();
        cache.add(2, vec![0; 10]).await.unwrap();
        // This should evict entry 1
        cache.add(3, vec![0; 10]).await.unwrap();

        assert_eq!(cache.len(), 2);

        // Entry 1 should have been evicted
        let entry = cache.pop_oldest().await.unwrap().unwrap();
        assert_eq!(entry.sequence, 2);
    }

    #[tokio::test]
    async fn test_cache_persistence() {
        let temp = TempDir::new().unwrap();

        // Create and populate cache
        {
            let cache = WriteCache::new(temp.path().to_path_buf(), 1024 * 1024);
            cache.add(1, vec![1, 2, 3]).await.unwrap();
            cache.add(2, vec![4, 5, 6]).await.unwrap();
        }

        // Create new cache and load from disk
        {
            let cache = WriteCache::new(temp.path().to_path_buf(), 1024 * 1024);
            cache.load().await.unwrap();

            assert_eq!(cache.len(), 2);

            let entry = cache.pop_oldest().await.unwrap().unwrap();
            assert_eq!(entry.sequence, 1);
        }
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let temp = TempDir::new().unwrap();
        let cache = WriteCache::new(temp.path().to_path_buf(), 1024 * 1024);

        cache.add(1, vec![1, 2, 3]).await.unwrap();
        cache.add(2, vec![4, 5, 6]).await.unwrap();

        cache.clear().await.unwrap();

        assert!(cache.is_empty());
        assert_eq!(cache.size(), 0);
    }
}
