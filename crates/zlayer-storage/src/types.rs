//! Core types for layer storage

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Identifies a container's persistent layer state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ContainerLayerId {
    /// Service/deployment name
    pub service: String,
    /// Unique container instance ID
    pub instance_id: String,
}

impl ContainerLayerId {
    pub fn new(service: impl Into<String>, instance_id: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            instance_id: instance_id.into(),
        }
    }

    pub fn to_key(&self) -> String {
        format!("{}_{}", self.service, self.instance_id)
    }
}

impl std::fmt::Display for ContainerLayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.service, self.instance_id)
    }
}

/// Metadata for a persisted layer snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerSnapshot {
    /// SHA256 digest of the tarball (content-addressed key)
    pub digest: String,
    /// Size in bytes (uncompressed)
    pub size_bytes: u64,
    /// Size in bytes (compressed with zstd)
    pub compressed_size_bytes: u64,
    /// Timestamp when snapshot was created
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Number of files in the layer
    pub file_count: u64,
}

/// Tracks sync state between local and S3
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
    /// Container this state belongs to
    pub container_id: ContainerLayerId,
    /// Current local layer digest (None if no changes)
    pub local_digest: Option<String>,
    /// Last successfully uploaded digest
    pub remote_digest: Option<String>,
    /// Timestamp of last successful sync
    pub last_sync: Option<chrono::DateTime<chrono::Utc>>,
    /// In-progress upload state (for crash recovery)
    pub pending_upload: Option<PendingUpload>,
}

impl SyncState {
    pub fn new(container_id: ContainerLayerId) -> Self {
        Self {
            container_id,
            local_digest: None,
            remote_digest: None,
            last_sync: None,
            pending_upload: None,
        }
    }

    pub fn needs_upload(&self) -> bool {
        match (&self.local_digest, &self.remote_digest) {
            (Some(local), Some(remote)) => local != remote,
            (Some(_), None) => true,
            _ => false,
        }
    }
}

/// State for resumable multipart uploads
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingUpload {
    /// S3 multipart upload ID
    pub upload_id: String,
    /// S3 object key
    pub object_key: String,
    /// Total parts expected
    pub total_parts: u32,
    /// Parts successfully uploaded (part_number -> ETag)
    pub completed_parts: HashMap<u32, CompletedPart>,
    /// Part size in bytes
    pub part_size: u64,
    /// Local file path of tarball being uploaded
    pub local_tarball_path: PathBuf,
    /// Started at timestamp
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Digest of the layer being uploaded
    pub digest: String,
}

impl PendingUpload {
    pub fn missing_parts(&self) -> Vec<u32> {
        (1..=self.total_parts)
            .filter(|p| !self.completed_parts.contains_key(p))
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.completed_parts.len() == self.total_parts as usize
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedPart {
    pub part_number: u32,
    pub e_tag: String,
}
