//! Volume management API DTOs.
//!
//! Wire-format types shared between the daemon's `/api/v1/volumes`
//! endpoints and SDK clients. Moved out of `zlayer-api` so SDK crates can
//! depend on them without pulling in the full server stack.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
#[cfg(feature = "utoipa")]
use utoipa::ToSchema;

/// Request body for `POST /api/v1/volumes`.
#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct CreateVolumeRequest {
    /// Volume name. Required. Must match `^[a-z0-9][a-z0-9_-]{0,63}$`.
    pub name: String,
    /// Optional size hint (humansize format: `"512Mi"`, `"10Gi"`).
    /// Recorded in the sidecar for display and future quota enforcement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// Optional storage tier. Accepts `"local"` (default), `"cached"`,
    /// `"network"`, matching `zlayer_spec::StorageTier`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// Optional labels to attach to the volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<HashMap<String, String>>,
}

/// Full volume response shape used by the list, inspect, and create
/// endpoints.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct VolumeInfo {
    /// Volume name (directory name).
    pub name: String,
    /// Host filesystem path.
    pub path: String,
    /// Approximate size in bytes (sum of regular files in the volume
    /// directory). `None` when the directory could not be walked, or for
    /// freshly created empty volumes where `0` would be equally
    /// informative.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Labels from the sidecar. Empty when no sidecar is present.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    /// RFC 3339 creation timestamp. For volumes without a sidecar this is
    /// the directory's mtime (best-effort).
    pub created_at: String,
    /// Container IDs currently mounting this volume. Empty when no
    /// `VolumeUsageSource` is wired.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub in_use_by: Vec<String>,
}

/// Legacy response shape kept for backwards compatibility with older SDK
/// consumers that deserialize strictly. New consumers should use
/// [`VolumeInfo`]. `list_volumes` now returns [`VolumeInfo`] which is a
/// strict superset of the fields in `VolumeSummary`.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct VolumeSummary {
    /// Volume name (directory name).
    pub name: String,
    /// Host filesystem path.
    pub path: String,
    /// Approximate size in bytes.
    pub size_bytes: Option<u64>,
}

/// Query parameters for the delete endpoint.
#[derive(Debug, Deserialize)]
pub struct DeleteVolumeQuery {
    /// Force removal of a volume even when it is non-empty OR currently
    /// in use by one or more containers. Default `false`.
    #[serde(default)]
    pub force: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_info_serialization() {
        let info = VolumeInfo {
            name: "test-vol".to_string(),
            path: "/data/volumes/test-vol".to_string(),
            size_bytes: Some(1024),
            labels: HashMap::from([("owner".to_string(), "zarc".to_string())]),
            created_at: "2026-04-20T00:00:00Z".to_string(),
            in_use_by: vec!["c1".to_string()],
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("test-vol"));
        assert!(json.contains("1024"));
        assert!(json.contains("owner"));
        assert!(json.contains("in_use_by"));
    }

    #[test]
    fn test_volume_summary_legacy_serialization() {
        // Ensure the legacy DTO still serializes cleanly for any SDK that
        // hasn't migrated yet.
        let summary = VolumeSummary {
            name: "legacy".to_string(),
            path: "/data/volumes/legacy".to_string(),
            size_bytes: Some(1024),
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("legacy"));
    }

    #[test]
    fn test_delete_volume_query_defaults() {
        let query: DeleteVolumeQuery = serde_json::from_str("{}").unwrap();
        assert!(!query.force);
    }

    #[test]
    fn test_delete_volume_query_force() {
        let query: DeleteVolumeQuery = serde_json::from_str(r#"{"force": true}"#).unwrap();
        assert!(query.force);
    }
}
