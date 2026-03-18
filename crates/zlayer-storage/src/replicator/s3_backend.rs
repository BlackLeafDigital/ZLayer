//! S3 upload/download for ZQL database backups
//!
//! Handles uploading compressed snapshots to S3 and downloading for restore.

use crate::error::{LayerStorageError, Result};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3Client;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use tracing::{debug, info};

/// Replication metadata stored in S3
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationMetadata {
    /// Latest snapshot key
    pub latest_snapshot: Option<String>,
    /// Timestamp of latest snapshot
    pub latest_snapshot_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Total number of snapshots stored
    pub snapshot_count: u64,
    /// Database identifier (for validation)
    pub db_identifier: Option<String>,
    /// Last modified timestamp
    pub last_modified: chrono::DateTime<chrono::Utc>,
}

impl Default for ReplicationMetadata {
    fn default() -> Self {
        Self {
            latest_snapshot: None,
            latest_snapshot_time: None,
            snapshot_count: 0,
            db_identifier: None,
            last_modified: chrono::Utc::now(),
        }
    }
}

/// S3 backend for ZQL database replication
pub struct S3Backend {
    client: S3Client,
    bucket: String,
    prefix: String,
    compression_level: i32,
}

impl S3Backend {
    /// Create a new S3 backend
    ///
    /// # Arguments
    ///
    /// * `client` - Pre-configured S3 client
    /// * `bucket` - S3 bucket name
    /// * `prefix` - Key prefix for all objects
    /// * `compression_level` - Zstd compression level (1-22)
    pub fn new(client: S3Client, bucket: String, prefix: String, compression_level: i32) -> Self {
        Self {
            client,
            bucket,
            prefix,
            compression_level,
        }
    }

    /// Build the S3 key for a snapshot
    fn snapshot_key(&self, timestamp: &chrono::DateTime<chrono::Utc>) -> String {
        format!(
            "{}snapshots/{}.zql.tar.zst",
            self.prefix,
            timestamp.format("%Y%m%d_%H%M%S")
        )
    }

    /// Build the S3 key for metadata
    fn metadata_key(&self) -> String {
        format!("{}metadata.json", self.prefix)
    }

    /// Upload a database snapshot (tar+zstd archive of the ZQL data directory)
    pub async fn upload_snapshot(&self, data: &[u8]) -> Result<()> {
        let timestamp = chrono::Utc::now();
        let key = self.snapshot_key(&timestamp);

        info!(
            "Uploading snapshot to s3://{}/{} ({} bytes)",
            self.bucket,
            key,
            data.len()
        );

        // Compress the data
        let compressed = self.compress(data)?;

        #[allow(clippy::cast_precision_loss)]
        let reduction_pct = if data.is_empty() {
            0.0
        } else {
            (1.0 - (compressed.len() as f64 / data.len() as f64)) * 100.0
        };
        info!(
            "Compressed {} bytes to {} bytes ({:.1}% reduction)",
            data.len(),
            compressed.len(),
            reduction_pct,
        );

        // Upload to S3
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(compressed))
            .content_type("application/zstd")
            .send()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?;

        debug!("Snapshot uploaded: {}", key);
        Ok(())
    }

    /// Download the latest snapshot
    pub async fn download_latest_snapshot(&self) -> Result<Option<Vec<u8>>> {
        // Get metadata to find latest snapshot
        let metadata = self.get_metadata().await?;

        let snapshot_key = if let Some(key) = &metadata.latest_snapshot {
            key.clone()
        } else {
            // List snapshots to find the latest
            let snapshots = self.list_snapshots().await?;
            if snapshots.is_empty() {
                return Ok(None);
            }
            // Safe: we just checked `is_empty()`
            snapshots.into_iter().last().unwrap_or_default()
        };

        info!("Downloading snapshot: {}", snapshot_key);

        let response = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&snapshot_key)
            .send()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?;

        let compressed_bytes = response
            .body
            .collect()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?
            .into_bytes();

        // Decompress
        let decompressed = self.decompress(&compressed_bytes)?;

        info!(
            "Downloaded snapshot: {} bytes (compressed: {} bytes)",
            decompressed.len(),
            compressed_bytes.len()
        );

        Ok(Some(decompressed))
    }

    /// List all snapshot keys
    pub async fn list_snapshots(&self) -> Result<Vec<String>> {
        let prefix = format!("{}snapshots/", self.prefix);

        let mut keys = Vec::new();
        let mut continuation_token: Option<String> = None;

        loop {
            let mut request = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix);

            if let Some(token) = &continuation_token {
                request = request.continuation_token(token);
            }

            let response = request
                .send()
                .await
                .map_err(|e| LayerStorageError::S3(e.to_string()))?;

            for object in response.contents() {
                if let Some(key) = object.key() {
                    if key.ends_with(".zql.tar.zst") {
                        keys.push(key.to_string());
                    }
                }
            }

            if response.is_truncated().unwrap_or(false) {
                continuation_token = response.next_continuation_token().map(String::from);
            } else {
                break;
            }
        }

        // Sort by timestamp (embedded in key)
        keys.sort();

        Ok(keys)
    }

    /// Get replication metadata from S3
    pub async fn get_metadata(&self) -> Result<ReplicationMetadata> {
        let key = self.metadata_key();

        match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(response) => {
                let bytes = response
                    .body
                    .collect()
                    .await
                    .map_err(|e| LayerStorageError::S3(e.to_string()))?
                    .into_bytes();

                let metadata: ReplicationMetadata = serde_json::from_slice(&bytes)?;
                Ok(metadata)
            }
            Err(e) => {
                // Check if it's a not-found error
                if e.to_string().contains("NoSuchKey") || e.to_string().contains("404") {
                    Ok(ReplicationMetadata::default())
                } else {
                    Err(LayerStorageError::S3(e.to_string()))
                }
            }
        }
    }

    /// Update replication metadata in S3
    pub async fn update_metadata(&self) -> Result<()> {
        let key = self.metadata_key();

        // Get current metadata
        let mut metadata = self.get_metadata().await.unwrap_or_default();

        // Get latest snapshot
        let snapshots = self.list_snapshots().await?;
        if let Some(latest) = snapshots.last() {
            metadata.latest_snapshot = Some(latest.clone());
            metadata.latest_snapshot_time = Some(chrono::Utc::now());
        }
        metadata.snapshot_count = snapshots.len() as u64;
        metadata.last_modified = chrono::Utc::now();

        // Upload metadata
        let json = serde_json::to_vec_pretty(&metadata)?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(json))
            .content_type("application/json")
            .send()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?;

        debug!("Metadata updated");
        Ok(())
    }

    /// Compress data using zstd
    fn compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        let mut encoder = zstd::stream::Encoder::new(Vec::new(), self.compression_level)?;
        encoder.write_all(data)?;
        Ok(encoder.finish()?)
    }

    /// Decompress zstd data
    #[allow(clippy::unused_self)]
    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>> {
        let mut decoder = zstd::stream::Decoder::new(data)?;
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;
        Ok(decompressed)
    }

    /// Delete old snapshots, keeping only the most recent N
    #[allow(dead_code)]
    pub async fn cleanup_old_snapshots(&self, keep_count: usize) -> Result<usize> {
        let snapshots = self.list_snapshots().await?;

        if snapshots.len() <= keep_count {
            return Ok(0);
        }

        let to_delete = &snapshots[..snapshots.len() - keep_count];
        let mut deleted = 0;

        for key in to_delete {
            match self
                .client
                .delete_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
            {
                Ok(_) => {
                    debug!("Deleted old snapshot: {}", key);
                    deleted += 1;
                }
                Err(e) => {
                    debug!("Failed to delete snapshot {}: {}", key, e);
                }
            }
        }

        info!("Cleaned up {} old snapshots", deleted);
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_default() {
        let metadata = ReplicationMetadata::default();
        assert!(metadata.latest_snapshot.is_none());
        assert_eq!(metadata.snapshot_count, 0);
    }
}
