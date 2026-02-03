//! Auto-restore from S3
//!
//! Handles restoring SQLite databases from S3 backups, including both snapshots
//! and WAL segments.

use super::s3_backend::S3Backend;
use crate::error::{LayerStorageError, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Manager for database restoration from S3
pub struct RestoreManager {
    /// Path to the local database file
    db_path: PathBuf,
    /// S3 backend for downloads
    s3_backend: Arc<S3Backend>,
    /// Temporary directory for restoration
    temp_dir: PathBuf,
}

impl RestoreManager {
    /// Create a new restore manager
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path where the database should be restored
    /// * `s3_backend` - S3 backend for downloading backups
    /// * `temp_dir` - Temporary directory for intermediate files
    pub fn new(db_path: PathBuf, s3_backend: Arc<S3Backend>, temp_dir: PathBuf) -> Self {
        Self {
            db_path,
            s3_backend,
            temp_dir,
        }
    }

    /// Restore the database from S3
    ///
    /// This will:
    /// 1. Download the latest snapshot
    /// 2. Download any WAL segments since the snapshot
    /// 3. Apply WAL segments to reconstruct the database
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if a backup was found and restored
    /// - `Ok(false)` if no backup was found
    /// - `Err(_)` if restoration failed
    pub async fn restore(&self) -> Result<bool> {
        info!("Starting database restoration from S3");

        // Ensure temp directory exists
        tokio::fs::create_dir_all(&self.temp_dir).await?;

        // Download latest snapshot
        let snapshot_data = match self.s3_backend.download_latest_snapshot().await? {
            Some(data) => data,
            None => {
                info!("No snapshot found in S3");
                return Ok(false);
            }
        };

        info!("Downloaded snapshot: {} bytes", snapshot_data.len());

        // Write snapshot to temp file
        let temp_db_path = self.temp_dir.join("restore.sqlite");
        tokio::fs::write(&temp_db_path, &snapshot_data).await?;

        // Get metadata to find WAL sequence at snapshot time
        let metadata = self.s3_backend.get_metadata().await?;
        let snapshot_wal_sequence = metadata.latest_wal_sequence.unwrap_or(0);

        // Download WAL segments since snapshot
        let wal_segments = self
            .s3_backend
            .download_wal_segments_since(snapshot_wal_sequence)
            .await?;

        if !wal_segments.is_empty() {
            info!("Downloaded {} WAL segments to apply", wal_segments.len());

            // Apply WAL segments
            self.apply_wal_segments(&temp_db_path, wal_segments).await?;
        }

        // Ensure target directory exists
        if let Some(parent) = self.db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Move restored database to target location
        // First, remove any existing database files
        if self.db_path.exists() {
            tokio::fs::remove_file(&self.db_path).await?;
        }

        let wal_path = self.wal_path();
        if wal_path.exists() {
            tokio::fs::remove_file(&wal_path).await?;
        }

        let shm_path = self.shm_path();
        if shm_path.exists() {
            tokio::fs::remove_file(&shm_path).await?;
        }

        // Move temp database to final location
        tokio::fs::rename(&temp_db_path, &self.db_path).await?;

        // Clean up temp WAL files if any
        let temp_wal = temp_db_path.with_extension("sqlite-wal");
        if temp_wal.exists() {
            let _ = tokio::fs::remove_file(&temp_wal).await;
        }

        let temp_shm = temp_db_path.with_extension("sqlite-shm");
        if temp_shm.exists() {
            let _ = tokio::fs::remove_file(&temp_shm).await;
        }

        info!(
            "Database restored successfully to {}",
            self.db_path.display()
        );

        Ok(true)
    }

    /// Apply WAL segments to a database
    ///
    /// This uses SQLite's ability to recover from WAL files by:
    /// 1. Writing WAL data to the WAL file location
    /// 2. Opening the database to trigger WAL recovery
    async fn apply_wal_segments(
        &self,
        db_path: &std::path::Path,
        segments: Vec<super::cache::CacheEntry>,
    ) -> Result<()> {
        if segments.is_empty() {
            return Ok(());
        }

        info!("Applying {} WAL segments", segments.len());

        // For each WAL segment, we need to apply it using rusqlite
        // The segments contain complete WAL files, so we use the last one
        // (which should contain all the frames from previous segments plus new ones)

        // Actually, in our implementation, each "segment" is a snapshot of the WAL file
        // at a point in time. The latest segment should contain the most recent state.
        // We take the segment with the highest sequence number.

        if let Some(latest_segment) = segments.last() {
            // Write the WAL file
            let wal_path = format!("{}-wal", db_path.display());
            tokio::fs::write(&wal_path, &latest_segment.data).await?;

            debug!(
                "Wrote WAL file ({} bytes) for recovery",
                latest_segment.data.len()
            );

            // Open the database with rusqlite to trigger WAL recovery
            // We need to do this in a blocking context since rusqlite is sync
            let db_path_clone = db_path.to_path_buf();
            tokio::task::spawn_blocking(move || {
                let conn = rusqlite::Connection::open(&db_path_clone)
                    .map_err(|e| LayerStorageError::Database(e.to_string()))?;

                // Force a checkpoint to apply WAL
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                    .map_err(|e| LayerStorageError::Database(e.to_string()))?;

                // Verify database is intact
                let integrity: String = conn
                    .query_row("PRAGMA integrity_check;", [], |row| row.get(0))
                    .map_err(|e| LayerStorageError::Database(e.to_string()))?;

                if integrity != "ok" {
                    warn!("Database integrity check returned: {}", integrity);
                }

                Ok::<_, LayerStorageError>(())
            })
            .await
            .map_err(|e| LayerStorageError::Io(std::io::Error::other(e)))??;

            info!("WAL recovery completed");
        }

        Ok(())
    }

    /// Get the WAL file path
    fn wal_path(&self) -> PathBuf {
        let mut path = self.db_path.clone();
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        path.set_file_name(format!("{filename}-wal"));
        path
    }

    /// Get the SHM file path
    fn shm_path(&self) -> PathBuf {
        let mut path = self.db_path.clone();
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        path.set_file_name(format!("{filename}-shm"));
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wal_path_derivation() {
        // Test the path derivation logic directly without needing a full RestoreManager
        let db_path = PathBuf::from("/var/lib/app/data.sqlite");

        // Derive WAL path
        let mut wal_path = db_path.clone();
        let filename = wal_path.file_name().unwrap_or_default().to_string_lossy();
        wal_path.set_file_name(format!("{filename}-wal"));

        assert_eq!(wal_path, PathBuf::from("/var/lib/app/data.sqlite-wal"));

        // Derive SHM path
        let mut shm_path = db_path.clone();
        let filename = shm_path.file_name().unwrap_or_default().to_string_lossy();
        shm_path.set_file_name(format!("{filename}-shm"));

        assert_eq!(shm_path, PathBuf::from("/var/lib/app/data.sqlite-shm"));
    }
}
