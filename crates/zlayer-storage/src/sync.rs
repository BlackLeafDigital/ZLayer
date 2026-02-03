//! Sync manager for coordinating local/remote layer state
//!
//! Handles crash-tolerant uploads with resume capability using S3 multipart uploads.

use crate::config::LayerStorageConfig;
use crate::error::{LayerStorageError, Result};
use crate::snapshot::{calculate_directory_digest, create_snapshot, extract_snapshot};
use crate::types::{ContainerLayerId, LayerSnapshot, PendingUpload, SyncState};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart as S3CompletedPart};
use aws_sdk_s3::Client as S3Client;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;
use tracing::{debug, info, instrument, warn};

/// Manages layer synchronization between local storage and S3
pub struct LayerSyncManager {
    config: LayerStorageConfig,
    s3_client: S3Client,
    pool: SqlitePool,
    /// In-memory cache of sync states
    states: Arc<RwLock<HashMap<String, SyncState>>>,
}

impl LayerSyncManager {
    /// Create a new sync manager
    pub async fn new(config: LayerStorageConfig) -> Result<Self> {
        // Ensure directories exist
        tokio::fs::create_dir_all(&config.staging_dir).await?;
        if let Some(parent) = config.state_db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Initialize AWS SDK
        let mut aws_config_builder = aws_config::from_env();

        if let Some(region) = &config.region {
            aws_config_builder =
                aws_config_builder.region(aws_sdk_s3::config::Region::new(region.clone()));
        }

        let aws_config = aws_config_builder.load().await;

        let s3_config = if let Some(endpoint) = &config.endpoint_url {
            aws_sdk_s3::config::Builder::from(&aws_config)
                .endpoint_url(endpoint)
                .force_path_style(true)
                .build()
        } else {
            aws_sdk_s3::config::Builder::from(&aws_config).build()
        };

        let s3_client = S3Client::from_conf(s3_config);

        // Open/create SQLite database
        let db_url = format!("sqlite:{}?mode=rwc", config.state_db_path.display());
        let connect_options = SqliteConnectOptions::from_str(&db_url)
            .map_err(|e| LayerStorageError::Database(e.to_string()))?
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(connect_options)
            .await
            .map_err(|e| LayerStorageError::Database(e.to_string()))?;

        // Enable WAL mode for better concurrent access
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await
            .map_err(|e| LayerStorageError::Database(e.to_string()))?;

        // Create sync_state table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sync_state (
                container_key TEXT PRIMARY KEY NOT NULL,
                state_json TEXT NOT NULL,
                updated_at TEXT DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await
        .map_err(|e| LayerStorageError::Database(e.to_string()))?;

        // Load existing states into memory
        let states = Arc::new(RwLock::new(Self::load_all_states(&pool).await?));

        Ok(Self {
            config,
            s3_client,
            pool,
            states,
        })
    }

    async fn load_all_states(pool: &SqlitePool) -> Result<HashMap<String, SyncState>> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT container_key, state_json FROM sync_state")
                .fetch_all(pool)
                .await
                .map_err(|e| LayerStorageError::Database(e.to_string()))?;

        let mut states = HashMap::new();
        for (key, json) in rows {
            let state: SyncState = serde_json::from_str(&json)?;
            states.insert(key, state);
        }

        Ok(states)
    }

    async fn save_state(&self, state: &SyncState) -> Result<()> {
        let key = state.container_id.to_key();
        let value = serde_json::to_string(state)?;

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO sync_state (container_key, state_json, updated_at)
            VALUES (?, ?, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(&key)
        .bind(&value)
        .execute(&self.pool)
        .await
        .map_err(|e| LayerStorageError::Database(e.to_string()))?;

        Ok(())
    }

    /// Register a container for layer sync tracking
    #[instrument(skip(self))]
    pub async fn register_container(&self, container_id: ContainerLayerId) -> Result<()> {
        let key = container_id.to_key();
        let mut states = self.states.write().await;

        if let std::collections::hash_map::Entry::Vacant(e) = states.entry(key) {
            let state = SyncState::new(container_id);
            self.save_state(&state).await?;
            e.insert(state);
            info!("Registered new container for layer sync");
        }

        Ok(())
    }

    /// Check if a container's layer has changed and needs sync
    #[instrument(skip(self, upper_layer_path))]
    pub async fn check_for_changes(
        &self,
        container_id: &ContainerLayerId,
        upper_layer_path: impl AsRef<Path>,
    ) -> Result<bool> {
        let key = container_id.to_key();
        let states = self.states.read().await;

        let state = states
            .get(&key)
            .ok_or_else(|| LayerStorageError::NotFound(key.clone()))?;

        // Calculate current digest
        let current_digest = calculate_directory_digest(upper_layer_path)?;

        // Compare with stored digest
        Ok(state.local_digest.as_ref() != Some(&current_digest))
    }

    /// Create a snapshot and upload it to S3
    #[instrument(skip(self, upper_layer_path), fields(container = %container_id))]
    pub async fn sync_layer(
        &self,
        container_id: &ContainerLayerId,
        upper_layer_path: impl AsRef<Path>,
    ) -> Result<Option<LayerSnapshot>> {
        let upper_layer_path = upper_layer_path.as_ref();
        let key = container_id.to_key();

        // Check for pending upload to resume
        {
            let states = self.states.read().await;
            if let Some(state) = states.get(&key) {
                if let Some(pending) = &state.pending_upload {
                    info!("Found pending upload, attempting to resume");
                    return self.resume_upload(container_id, pending.clone()).await;
                }
            }
        }

        // Calculate current digest
        let current_digest = calculate_directory_digest(upper_layer_path)?;

        // Check if sync needed
        {
            let states = self.states.read().await;
            if let Some(state) = states.get(&key) {
                if state.remote_digest.as_ref() == Some(&current_digest) {
                    debug!("Layer already synced, no changes");
                    return Ok(None);
                }
            }
        }

        // Create snapshot
        let tarball_path = self
            .config
            .staging_dir
            .join(format!("{}.tar.zst", current_digest));

        let snapshot = tokio::task::spawn_blocking({
            let source = upper_layer_path.to_path_buf();
            let output = tarball_path.clone();
            let level = self.config.compression_level;
            move || create_snapshot(source, output, level)
        })
        .await
        .map_err(|e| LayerStorageError::Io(std::io::Error::other(e)))??;

        // Upload to S3
        self.upload_snapshot(container_id, &tarball_path, &snapshot)
            .await?;

        // Update state
        {
            let mut states = self.states.write().await;
            if let Some(state) = states.get_mut(&key) {
                state.local_digest = Some(snapshot.digest.clone());
                state.remote_digest = Some(snapshot.digest.clone());
                state.last_sync = Some(chrono::Utc::now());
                state.pending_upload = None;
                self.save_state(state).await?;
            }
        }

        // Clean up staging file
        let _ = tokio::fs::remove_file(&tarball_path).await;

        Ok(Some(snapshot))
    }

    /// Upload a snapshot to S3 using multipart upload
    #[instrument(skip(self, tarball_path, snapshot))]
    async fn upload_snapshot(
        &self,
        container_id: &ContainerLayerId,
        tarball_path: &Path,
        snapshot: &LayerSnapshot,
    ) -> Result<()> {
        let object_key = self.config.object_key(&snapshot.digest);
        let file_size = tokio::fs::metadata(tarball_path).await?.len();
        let part_size = self.config.part_size_bytes;
        let total_parts = file_size.div_ceil(part_size) as u32;

        info!(
            "Uploading {} ({} bytes) in {} parts",
            object_key, file_size, total_parts
        );

        // Initiate multipart upload
        let create_response = self
            .s3_client
            .create_multipart_upload()
            .bucket(&self.config.bucket)
            .key(&object_key)
            .content_type("application/zstd")
            .send()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?;

        let upload_id = create_response
            .upload_id()
            .ok_or_else(|| LayerStorageError::S3("No upload ID returned".to_string()))?
            .to_string();

        // Record pending upload for crash recovery
        let pending = PendingUpload {
            upload_id: upload_id.clone(),
            object_key: object_key.clone(),
            total_parts,
            completed_parts: HashMap::new(),
            part_size,
            local_tarball_path: tarball_path.to_path_buf(),
            started_at: chrono::Utc::now(),
            digest: snapshot.digest.clone(),
        };

        {
            let key = container_id.to_key();
            let mut states = self.states.write().await;
            if let Some(state) = states.get_mut(&key) {
                state.pending_upload = Some(pending.clone());
                self.save_state(state).await?;
            }
        }

        // Upload parts
        let completed_parts = self
            .upload_parts(
                tarball_path,
                &upload_id,
                &object_key,
                total_parts,
                part_size,
            )
            .await?;

        // Complete multipart upload
        let completed_upload = CompletedMultipartUpload::builder()
            .set_parts(Some(
                completed_parts
                    .into_iter()
                    .map(|(num, etag)| {
                        S3CompletedPart::builder()
                            .part_number(num as i32)
                            .e_tag(etag)
                            .build()
                    })
                    .collect(),
            ))
            .build();

        self.s3_client
            .complete_multipart_upload()
            .bucket(&self.config.bucket)
            .key(&object_key)
            .upload_id(&upload_id)
            .multipart_upload(completed_upload)
            .send()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?;

        // Upload metadata
        let metadata_key = self.config.metadata_key(&snapshot.digest);
        let metadata_json = serde_json::to_vec(snapshot)?;

        self.s3_client
            .put_object()
            .bucket(&self.config.bucket)
            .key(&metadata_key)
            .body(ByteStream::from(metadata_json))
            .content_type("application/json")
            .send()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?;

        info!("Upload complete: {}", object_key);
        Ok(())
    }

    /// Upload individual parts with progress tracking
    async fn upload_parts(
        &self,
        tarball_path: &Path,
        upload_id: &str,
        object_key: &str,
        total_parts: u32,
        part_size: u64,
    ) -> Result<Vec<(u32, String)>> {
        let mut completed = Vec::new();

        for part_number in 1..=total_parts {
            let offset = (part_number as u64 - 1) * part_size;

            // Read part data
            let mut file = File::open(tarball_path).await?;
            file.seek(std::io::SeekFrom::Start(offset)).await?;

            let mut buffer = vec![0u8; part_size as usize];
            let bytes_read = file.read(&mut buffer).await?;
            buffer.truncate(bytes_read);

            // Upload part
            let response = self
                .s3_client
                .upload_part()
                .bucket(&self.config.bucket)
                .key(object_key)
                .upload_id(upload_id)
                .part_number(part_number as i32)
                .body(ByteStream::from(buffer))
                .send()
                .await
                .map_err(|e| LayerStorageError::S3(e.to_string()))?;

            let etag = response
                .e_tag()
                .ok_or_else(|| LayerStorageError::S3("No ETag returned for part".to_string()))?
                .to_string();

            debug!("Uploaded part {}/{}: {}", part_number, total_parts, etag);
            completed.push((part_number, etag));
        }

        Ok(completed)
    }

    /// Resume an interrupted upload
    #[instrument(skip(self, pending))]
    async fn resume_upload(
        &self,
        container_id: &ContainerLayerId,
        pending: PendingUpload,
    ) -> Result<Option<LayerSnapshot>> {
        let missing = pending.missing_parts();

        if missing.is_empty() {
            // All parts uploaded, just need to complete
            info!("All parts uploaded, completing multipart upload");
        } else {
            info!("Resuming upload, {} parts remaining", missing.len());

            // Verify local file still exists
            if !pending.local_tarball_path.exists() {
                warn!("Local tarball missing, aborting upload and starting fresh");
                self.abort_upload(&pending).await?;

                let key = container_id.to_key();
                let mut states = self.states.write().await;
                if let Some(state) = states.get_mut(&key) {
                    state.pending_upload = None;
                    self.save_state(state).await?;
                }

                return Err(LayerStorageError::UploadInterrupted(
                    "Local tarball missing".to_string(),
                ));
            }

            // Upload missing parts
            for part_number in missing {
                let offset = (part_number as u64 - 1) * pending.part_size;

                let mut file = File::open(&pending.local_tarball_path).await?;
                file.seek(std::io::SeekFrom::Start(offset)).await?;

                let mut buffer = vec![0u8; pending.part_size as usize];
                let bytes_read = file.read(&mut buffer).await?;
                buffer.truncate(bytes_read);

                let response = self
                    .s3_client
                    .upload_part()
                    .bucket(&self.config.bucket)
                    .key(&pending.object_key)
                    .upload_id(&pending.upload_id)
                    .part_number(part_number as i32)
                    .body(ByteStream::from(buffer))
                    .send()
                    .await
                    .map_err(|e| LayerStorageError::S3(e.to_string()))?;

                let etag = response
                    .e_tag()
                    .ok_or_else(|| LayerStorageError::S3("No ETag returned".to_string()))?
                    .to_string();

                debug!("Uploaded part {}: {}", part_number, etag);
            }
        }

        // List parts to get all ETags
        let parts_response = self
            .s3_client
            .list_parts()
            .bucket(&self.config.bucket)
            .key(&pending.object_key)
            .upload_id(&pending.upload_id)
            .send()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?;

        let completed_parts: Vec<S3CompletedPart> = parts_response
            .parts()
            .iter()
            .map(|p| {
                S3CompletedPart::builder()
                    .part_number(p.part_number().unwrap_or(0))
                    .e_tag(p.e_tag().unwrap_or_default())
                    .build()
            })
            .collect();

        // Complete multipart upload
        let completed_upload = CompletedMultipartUpload::builder()
            .set_parts(Some(completed_parts))
            .build();

        self.s3_client
            .complete_multipart_upload()
            .bucket(&self.config.bucket)
            .key(&pending.object_key)
            .upload_id(&pending.upload_id)
            .multipart_upload(completed_upload)
            .send()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?;

        // Update state
        let key = container_id.to_key();
        {
            let mut states = self.states.write().await;
            if let Some(state) = states.get_mut(&key) {
                state.local_digest = Some(pending.digest.clone());
                state.remote_digest = Some(pending.digest.clone());
                state.last_sync = Some(chrono::Utc::now());
                state.pending_upload = None;
                self.save_state(state).await?;
            }
        }

        // Clean up staging file
        let _ = tokio::fs::remove_file(&pending.local_tarball_path).await;

        info!("Upload resumed and completed successfully");

        // Return snapshot metadata (fetch from S3)
        self.get_snapshot_metadata(&pending.digest).await.map(Some)
    }

    /// Abort a multipart upload
    async fn abort_upload(&self, pending: &PendingUpload) -> Result<()> {
        self.s3_client
            .abort_multipart_upload()
            .bucket(&self.config.bucket)
            .key(&pending.object_key)
            .upload_id(&pending.upload_id)
            .send()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?;

        Ok(())
    }

    /// Download and restore a layer from S3
    #[instrument(skip(self, target_path))]
    pub async fn restore_layer(
        &self,
        container_id: &ContainerLayerId,
        target_path: impl AsRef<Path>,
    ) -> Result<LayerSnapshot> {
        let target_path = target_path.as_ref();
        let key = container_id.to_key();

        // Get remote digest
        let remote_digest = {
            let states = self.states.read().await;
            states
                .get(&key)
                .and_then(|s| s.remote_digest.clone())
                .ok_or_else(|| {
                    LayerStorageError::NotFound(format!("No remote layer for {}", key))
                })?
        };

        info!("Restoring layer {} from S3", remote_digest);

        // Download tarball
        let tarball_path = self
            .config
            .staging_dir
            .join(format!("{}.tar.zst", remote_digest));

        let object_key = self.config.object_key(&remote_digest);
        let response = self
            .s3_client
            .get_object()
            .bucket(&self.config.bucket)
            .key(&object_key)
            .send()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?;

        // Stream to file
        let mut file = tokio::fs::File::create(&tarball_path).await?;
        let mut stream = response.body.into_async_read();
        tokio::io::copy(&mut stream, &mut file).await?;

        // Get snapshot metadata
        let snapshot = self.get_snapshot_metadata(&remote_digest).await?;

        // Extract
        tokio::task::spawn_blocking({
            let tarball = tarball_path.clone();
            let target = target_path.to_path_buf();
            let digest = remote_digest.clone();
            move || extract_snapshot(tarball, target, Some(&digest))
        })
        .await
        .map_err(|e| LayerStorageError::Io(std::io::Error::other(e)))??;

        // Update local digest
        {
            let mut states = self.states.write().await;
            if let Some(state) = states.get_mut(&key) {
                state.local_digest = Some(remote_digest);
                self.save_state(state).await?;
            }
        }

        // Clean up
        let _ = tokio::fs::remove_file(&tarball_path).await;

        info!("Layer restored successfully");
        Ok(snapshot)
    }

    /// Get snapshot metadata from S3
    async fn get_snapshot_metadata(&self, digest: &str) -> Result<LayerSnapshot> {
        let metadata_key = self.config.metadata_key(digest);

        let response = self
            .s3_client
            .get_object()
            .bucket(&self.config.bucket)
            .key(&metadata_key)
            .send()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?;

        let bytes = response
            .body
            .collect()
            .await
            .map_err(|e| LayerStorageError::S3(e.to_string()))?
            .into_bytes();

        serde_json::from_slice(&bytes).map_err(Into::into)
    }

    /// List all containers with sync state
    pub async fn list_containers(&self) -> Vec<ContainerLayerId> {
        let states = self.states.read().await;
        states.values().map(|s| s.container_id.clone()).collect()
    }

    /// Get sync state for a container
    pub async fn get_sync_state(&self, container_id: &ContainerLayerId) -> Option<SyncState> {
        let states = self.states.read().await;
        states.get(&container_id.to_key()).cloned()
    }
}

// Need to use tokio seek
use tokio::io::AsyncSeekExt;
