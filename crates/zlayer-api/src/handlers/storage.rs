//! Storage replication status endpoint
//!
//! Provides a read-only endpoint for querying the storage replication status.
//! When a [`ZqlReplicator`](zlayer_storage::ZqlReplicator) is configured, this
//! returns live replication metrics; otherwise it reports "disabled".

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use zlayer_storage::ZqlReplicator;

use crate::auth::AuthUser;
use crate::error::Result;

/// Replication detail within the storage status response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReplicationInfo {
    /// Whether replication is configured
    pub enabled: bool,
    /// Current status: "active", "disabled", or "error"
    pub status: String,
    /// ISO-8601 timestamp of the last successful sync, or null
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<String>,
    /// Number of WAL segments pending upload
    pub pending_changes: usize,
}

/// Storage status response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StorageStatusResponse {
    /// Replication status details
    pub replication: ReplicationInfo,
}

/// State for storage status endpoint
#[derive(Clone)]
pub struct StorageState {
    replicator: Option<Arc<ZqlReplicator>>,
}

impl Default for StorageState {
    fn default() -> Self {
        Self::new(None)
    }
}

impl StorageState {
    /// Create a new storage state.
    ///
    /// Pass `Some(replicator)` to report live replication metrics, or `None`
    /// to report replication as disabled.
    #[must_use]
    pub fn new(replicator: Option<Arc<ZqlReplicator>>) -> Self {
        Self { replicator }
    }
}

/// Get storage replication status
///
/// Returns the current state of storage replication. When a
/// [`ZqlReplicator`] is configured, this returns live metrics;
/// otherwise it reports "disabled".
///
/// # Errors
///
/// Returns an error if the handler fails to build the status response.
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
    let replication = if let Some(ref replicator) = state.replicator {
        let rs = replicator.status();
        ReplicationInfo {
            enabled: true,
            status: if rs.running {
                "active".to_string()
            } else {
                "disabled".to_string()
            },
            last_sync: rs.last_snapshot.map(|t| t.to_rfc3339()),
            pending_changes: rs.pending_entries,
        }
    } else {
        ReplicationInfo {
            enabled: false,
            status: "disabled".to_string(),
            last_sync: None,
            pending_changes: 0,
        }
    };

    Ok(Json(StorageStatusResponse { replication }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::no_effect_underscore_binding)]
    fn test_storage_state_default() {
        let _state = StorageState::default();
    }

    #[test]
    fn test_storage_state_new() {
        let _state = StorageState::new(None);
    }

    #[test]
    fn test_replication_info_disabled_serialize() {
        let info = ReplicationInfo {
            enabled: false,
            status: "disabled".to_string(),
            last_sync: None,
            pending_changes: 0,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"enabled\":false"));
        assert!(json.contains("\"disabled\""));
        assert!(json.contains("\"pending_changes\":0"));
        // last_sync should be omitted when None
        assert!(!json.contains("last_sync"));
    }

    #[test]
    fn test_replication_info_active_serialize() {
        let info = ReplicationInfo {
            enabled: true,
            status: "active".to_string(),
            last_sync: Some("2025-01-15T10:30:00+00:00".to_string()),
            pending_changes: 3,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"enabled\":true"));
        assert!(json.contains("\"active\""));
        assert!(json.contains("2025-01-15T10:30:00+00:00"));
        assert!(json.contains("\"pending_changes\":3"));
    }

    #[test]
    fn test_storage_status_response_serialize() {
        let response = StorageStatusResponse {
            replication: ReplicationInfo {
                enabled: false,
                status: "disabled".to_string(),
                last_sync: None,
                pending_changes: 0,
            },
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"replication\""));
        assert!(json.contains("\"enabled\":false"));
    }
}
