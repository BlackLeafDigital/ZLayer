//! Volume management endpoints
//!
//! Provides list and delete operations for named volumes managed by the daemon.
//! Volumes are directories on the host filesystem under the configured volume
//! base directory (typically `{data_dir}/volumes`).

use std::path::PathBuf;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::info;
use utoipa::ToSchema;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// State for volume management endpoints.
///
/// Holds the base directory where named volumes are stored. Each
/// subdirectory is treated as a named volume.
#[derive(Clone)]
pub struct VolumeApiState {
    /// Base volume directory (e.g. `{data_dir}/volumes`)
    pub volume_dir: PathBuf,
}

impl VolumeApiState {
    /// Create a new volume API state.
    #[must_use]
    pub fn new(volume_dir: PathBuf) -> Self {
        Self { volume_dir }
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// Summary of a single volume returned by the list endpoint.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct VolumeSummary {
    /// Volume name (directory name)
    pub name: String,
    /// Host filesystem path
    pub path: String,
    /// Approximate size in bytes (sum of files in the volume directory)
    pub size_bytes: Option<u64>,
}

/// Query parameters for the delete endpoint.
#[derive(Debug, Deserialize)]
pub struct DeleteVolumeQuery {
    /// Force removal even if the volume directory is non-empty.
    #[serde(default)]
    pub force: bool,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// List all volumes on disk.
///
/// Enumerates subdirectories under the volume base directory and returns
/// metadata for each one.
///
/// # Errors
///
/// Returns an error if the volume directory cannot be read.
#[utoipa::path(
    get,
    path = "/api/v1/volumes",
    responses(
        (status = 200, description = "List of volumes", body = Vec<VolumeSummary>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Volumes"
)]
pub async fn list_volumes(
    _user: AuthUser,
    State(state): State<VolumeApiState>,
) -> Result<Json<Vec<VolumeSummary>>> {
    let volume_dir = state.volume_dir.clone();

    // Read directory entries on a blocking thread to avoid blocking the runtime
    let volumes = tokio::task::spawn_blocking(
        move || -> std::result::Result<Vec<VolumeSummary>, std::io::Error> {
            let mut result = Vec::new();

            if !volume_dir.exists() {
                return Ok(result);
            }

            for entry in std::fs::read_dir(&volume_dir)? {
                let entry = entry?;
                let file_type = entry.file_type()?;
                if !file_type.is_dir() {
                    continue;
                }

                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path();
                let size_bytes = dir_size(&path).ok();

                result.push(VolumeSummary {
                    name,
                    path: path.display().to_string(),
                    size_bytes,
                });
            }

            result.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(result)
        },
    )
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to list volumes: {e}")))?
    .map_err(|e| ApiError::Internal(format!("Failed to read volume directory: {e}")))?;

    Ok(Json(volumes))
}

/// Delete a volume by name.
///
/// Removes a volume directory from disk. By default, only empty volumes
/// can be deleted. Use `?force=true` to remove non-empty volumes.
///
/// # Errors
///
/// Returns an error if the volume is not found or cannot be removed.
#[utoipa::path(
    delete,
    path = "/api/v1/volumes/{name}",
    params(
        ("name" = String, Path, description = "Volume name"),
        ("force" = bool, Query, description = "Force removal of non-empty volumes"),
    ),
    responses(
        (status = 204, description = "Volume deleted"),
        (status = 404, description = "Volume not found"),
        (status = 409, description = "Volume is non-empty (use ?force=true)"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer_auth" = [])),
    tag = "Volumes"
)]
pub async fn delete_volume(
    user: AuthUser,
    State(state): State<VolumeApiState>,
    Path(name): Path<String>,
    Query(query): Query<DeleteVolumeQuery>,
) -> Result<StatusCode> {
    user.require_role("operator")?;

    let volume_path = state.volume_dir.join(&name);

    if !volume_path.exists() || !volume_path.is_dir() {
        return Err(ApiError::NotFound(format!("Volume '{name}' not found")));
    }

    let force = query.force;
    let name_clone = name.clone();

    tokio::task::spawn_blocking(move || -> std::result::Result<(), ApiError> {
        // Check if directory is non-empty when not forcing
        if !force {
            let has_entries = std::fs::read_dir(&volume_path)
                .map_err(|e| ApiError::Internal(format!("Failed to read volume directory: {e}")))?
                .next()
                .is_some();

            if has_entries {
                return Err(ApiError::Conflict(format!(
                    "Volume '{name_clone}' is non-empty. Use ?force=true to remove anyway."
                )));
            }
        }

        std::fs::remove_dir_all(&volume_path).map_err(|e| {
            ApiError::Internal(format!("Failed to remove volume '{name_clone}': {e}"))
        })?;

        Ok(())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to delete volume: {e}")))??;

    info!(volume = %name, "Deleted volume");
    Ok(StatusCode::NO_CONTENT)
}

/// Calculate the total size of a directory tree.
fn dir_size(path: &std::path::Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total += dir_size(&entry.path()).unwrap_or(0);
        } else {
            total += metadata.len();
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_summary_serialization() {
        let summary = VolumeSummary {
            name: "test-vol".to_string(),
            path: "/data/volumes/test-vol".to_string(),
            size_bytes: Some(1024),
        };

        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("test-vol"));
        assert!(json.contains("1024"));
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
