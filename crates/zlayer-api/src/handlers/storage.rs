//! Storage replication status endpoint
//!
//! Provides a read-only endpoint for querying the SQLite-to-S3 replication
//! status. Returns whether replication is enabled, its current state, the
//! last sync timestamp, and the number of pending changes.

use std::sync::Arc;

use axum::{extract::State, Json};
pub use zlayer_types::api::storage::*;

use crate::auth::AuthUser;
use crate::error::Result;
use zlayer_storage::SqliteReplicator;

/// State for storage status endpoint
#[derive(Clone)]
pub struct StorageState {
    /// Optional `SQLite` replicator (None when S3 is not configured)
    pub replicator: Option<Arc<SqliteReplicator>>,
}

impl Default for StorageState {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageState {
    /// Create a new storage state with no replicator (disabled)
    #[must_use]
    pub fn new() -> Self {
        Self { replicator: None }
    }

    /// Create a storage state with a replicator
    #[must_use]
    pub fn with_replicator(replicator: Arc<SqliteReplicator>) -> Self {
        Self {
            replicator: Some(replicator),
        }
    }
}

/// Get storage replication status.
///
/// Returns the current state of SQLite-to-S3 replication including whether
/// it is enabled, the last sync time, and pending change count.
///
/// # Errors
///
/// Returns an error if authentication fails.
#[utoipa::path(
    get,
    path = "/api/v1/storage/status",
    responses(
        (status = 200, description = "Storage replication status", body = StorageStatusResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Storage"
)]
pub async fn get_storage_status(
    _user: AuthUser,
    State(state): State<StorageState>,
) -> Result<Json<StorageStatusResponse>> {
    let replication = match &state.replicator {
        Some(replicator) => {
            let status = replicator.status();
            let status_str = if status.running {
                "active"
            } else if status.failed_uploads > 0 {
                "error"
            } else {
                "disabled"
            };

            let last_sync = status.last_wal_sync.map(|ts| ts.to_rfc3339());

            ReplicationInfo {
                enabled: true,
                status: status_str.to_string(),
                last_sync,
                pending_changes: status.pending_segments,
            }
        }
        None => ReplicationInfo {
            enabled: false,
            status: "disabled".to_string(),
            last_sync: None,
            pending_changes: 0,
        },
    };

    Ok(Json(StorageStatusResponse { replication }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_state_default() {
        let state = StorageState::default();
        assert!(state.replicator.is_none());
    }

    #[test]
    fn test_storage_state_new() {
        let state = StorageState::new();
        assert!(state.replicator.is_none());
    }
}
