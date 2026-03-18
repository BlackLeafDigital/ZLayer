//! Auto-restore from S3
//!
//! Handles restoring ZQL databases from S3 backups. Downloads the latest
//! snapshot (a tar+zstd archive of the ZQL data directory) and extracts it
//! to reconstruct the database.

use super::s3_backend::S3Backend;
use crate::error::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

/// Manager for ZQL database restoration from S3
pub struct RestoreManager {
    /// Path to the ZQL data directory
    data_dir: PathBuf,
    /// S3 backend for downloads
    s3_backend: Arc<S3Backend>,
    /// Temporary directory for intermediate files
    temp_dir: PathBuf,
}

impl RestoreManager {
    /// Create a new restore manager
    ///
    /// # Arguments
    ///
    /// * `data_dir` - Path to the ZQL data directory to restore into
    /// * `s3_backend` - S3 backend for downloading backups
    /// * `temp_dir` - Temporary directory for intermediate files
    pub fn new(data_dir: PathBuf, s3_backend: Arc<S3Backend>, temp_dir: PathBuf) -> Self {
        Self {
            data_dir,
            s3_backend,
            temp_dir,
        }
    }

    /// Restore the ZQL database from S3
    ///
    /// Downloads the latest snapshot (a tar+zstd archive) and extracts it
    /// into the configured data directory.
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if a backup was found and restored
    /// - `Ok(false)` if no backup was found
    /// - `Err(_)` if restoration failed
    pub async fn restore(&self) -> Result<bool> {
        info!("Starting ZQL database restoration from S3");

        // Ensure temp directory exists
        tokio::fs::create_dir_all(&self.temp_dir).await?;

        // Download latest snapshot
        let Some(snapshot_data) = self.s3_backend.download_latest_snapshot().await? else {
            info!("No snapshot found in S3");
            return Ok(false);
        };

        info!("Downloaded snapshot: {} bytes", snapshot_data.len());

        // Write snapshot to a temp file
        let temp_tarball = self.temp_dir.join("restore.zql.tar.zst");
        tokio::fs::write(&temp_tarball, &snapshot_data).await?;

        // Ensure target data directory exists
        tokio::fs::create_dir_all(&self.data_dir).await?;

        // Extract the tarball into the data directory
        let tarball_path = temp_tarball.clone();
        let target_dir = self.data_dir.clone();
        tokio::task::spawn_blocking(move || {
            crate::snapshot::extract_snapshot(tarball_path, target_dir, None)
        })
        .await
        .map_err(|e| {
            crate::error::LayerStorageError::RestoreFailed(format!("extract task panicked: {e}"))
        })??;

        // Clean up temp file
        let _ = tokio::fs::remove_file(&temp_tarball).await;

        info!(
            "ZQL database restored successfully to {}",
            self.data_dir.display()
        );

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn test_data_dir_path() {
        let data_dir = PathBuf::from("/var/lib/app/zql-data");
        assert_eq!(data_dir.file_name().unwrap().to_str().unwrap(), "zql-data");
    }
}
