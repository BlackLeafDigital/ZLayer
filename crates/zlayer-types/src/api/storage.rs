//! Storage replication API DTOs.
//!
//! Wire types for the storage replication status endpoint. These are the
//! response shapes consumed by both the daemon and SDK clients.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

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

#[cfg(test)]
mod tests {
    use super::*;

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
