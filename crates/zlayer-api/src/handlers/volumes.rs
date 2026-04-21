//! Volume management endpoints
//!
//! Provides create, list, inspect, and delete operations for named volumes
//! managed by the daemon. Volumes are directories on the host filesystem
//! under the configured volume base directory (typically
//! `{data_dir}/volumes`). Each volume may have a sidecar metadata file
//! (`.metadata.json`) recording creation parameters (labels, size, tier,
//! timestamp) — older volumes created implicitly by mounting a
//! `StorageSpec::Named` on first container start will lack the sidecar and
//! expose synthesized defaults.

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

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
// Constants
// ---------------------------------------------------------------------------

/// Sidecar metadata filename inside each volume directory. Hidden (leading
/// dot) so it does not collide with user-visible content and `dir_size`
/// can optionally ignore it.
const METADATA_FILENAME: &str = ".metadata.json";

// ---------------------------------------------------------------------------
// Usage source
// ---------------------------------------------------------------------------

/// Extension point for computing `in_use_by`.
///
/// The volumes API does not itself track which containers mount which
/// volume — the container registry lives in [`ContainerApiState`] and, via
/// [`ServiceManager`], the deployment-managed replicas. Consumers that want
/// `in_use_by` populated supply an implementation via
/// [`VolumeApiState::with_usage_source`].
///
/// Implementations must be cheap (read-side only); the list handler calls
/// [`Self::containers_using`] once per volume.
///
/// When no source is wired, `in_use_by` is always empty and the volume can
/// be deleted without a `force=true` override.
///
/// [`ContainerApiState`]: crate::handlers::containers::ContainerApiState
/// [`ServiceManager`]: zlayer_agent::ServiceManager
#[async_trait::async_trait]
pub trait VolumeUsageSource: Send + Sync {
    /// Return the container IDs that currently mount the given volume name.
    async fn containers_using(&self, volume_name: &str) -> Vec<String>;
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// State for volume management endpoints.
///
/// Holds the base directory where named volumes are stored. Each
/// subdirectory is treated as a named volume. Optionally holds a
/// [`VolumeUsageSource`] for populating `in_use_by` on responses.
#[derive(Clone)]
pub struct VolumeApiState {
    /// Base volume directory (e.g. `{data_dir}/volumes`).
    pub volume_dir: PathBuf,
    /// Optional source for `in_use_by` computation. When `None`, responses
    /// always report an empty `in_use_by` list.
    pub usage_source: Option<Arc<dyn VolumeUsageSource>>,
}

impl VolumeApiState {
    /// Create a new volume API state with no usage source wired.
    #[must_use]
    pub fn new(volume_dir: PathBuf) -> Self {
        Self {
            volume_dir,
            usage_source: None,
        }
    }

    /// Attach a [`VolumeUsageSource`] for populating `in_use_by`.
    #[must_use]
    pub fn with_usage_source(mut self, source: Arc<dyn VolumeUsageSource>) -> Self {
        self.usage_source = Some(source);
        self
    }
}

// ---------------------------------------------------------------------------
// Sidecar metadata
// ---------------------------------------------------------------------------

/// On-disk representation of `.metadata.json` inside a volume directory.
///
/// This is intentionally private to the handler — the wire format is
/// [`VolumeInfo`], which is reconstructed from sidecar + filesystem state
/// on every read.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VolumeMetadata {
    /// Labels supplied at create time.
    #[serde(default)]
    labels: HashMap<String, String>,
    /// Optional size hint supplied at create time (human-readable string
    /// like `"512Mi"`). Not enforced as a quota today — recorded for
    /// display and future quota enforcement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    size: Option<String>,
    /// Optional storage tier label (matches `zlayer_spec::StorageTier`
    /// serialization: `"local"` | `"cached"` | `"network"`). Default
    /// `"local"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier: Option<String>,
    /// RFC 3339 creation timestamp.
    created_at: String,
}

/// Read the sidecar metadata for a volume directory. Returns `Ok(None)`
/// when the sidecar is absent (legacy / implicit volume); returns an error
/// only on unexpected I/O or parse failures.
fn read_sidecar(volume_path: &FsPath) -> std::io::Result<Option<VolumeMetadata>> {
    let sidecar = volume_path.join(METADATA_FILENAME);
    let raw = match std::fs::read(&sidecar) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    match serde_json::from_slice::<VolumeMetadata>(&raw) {
        Ok(meta) => Ok(Some(meta)),
        Err(e) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to parse {METADATA_FILENAME}: {e}"),
        )),
    }
}

/// Write the sidecar metadata into a volume directory. Overwrites any
/// existing sidecar.
fn write_sidecar(volume_path: &FsPath, meta: &VolumeMetadata) -> std::io::Result<()> {
    let sidecar = volume_path.join(METADATA_FILENAME);
    let bytes = serde_json::to_vec_pretty(meta).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to serialize metadata: {e}"),
        )
    })?;
    std::fs::write(&sidecar, bytes)
}

// ---------------------------------------------------------------------------
// Name validation
// ---------------------------------------------------------------------------

/// Validate a volume name.
///
/// Rules (mirroring Docker / OCI conventions and the directory-name
/// constraints we apply on-disk):
/// - 1 to 64 characters.
/// - First character must be `[a-z0-9]`.
/// - Remaining characters must be `[a-z0-9_-]`.
///
/// Equivalent regex: `^[a-z0-9][a-z0-9_-]{0,63}$`.
fn validate_volume_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(ApiError::BadRequest("volume name is required".to_string()));
    }
    if name.len() > 64 {
        return Err(ApiError::BadRequest(format!(
            "volume name '{name}' exceeds 64 characters"
        )));
    }
    let mut chars = name.chars();
    let first = chars.next().expect("non-empty checked above");
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(ApiError::BadRequest(format!(
            "volume name '{name}' must start with [a-z0-9]"
        )));
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
            return Err(ApiError::BadRequest(format!(
                "volume name '{name}' contains invalid character '{c}'; allowed: [a-z0-9_-]"
            )));
        }
    }
    Ok(())
}

/// Validate a tier string against the known values in
/// [`zlayer_spec::StorageTier`]. Accepts `"local"`, `"cached"`, `"network"`
/// (case-sensitive, `snake_case`, matching `StorageTier`'s serde rename).
fn validate_tier(tier: &str) -> Result<()> {
    match tier {
        "local" | "cached" | "network" => Ok(()),
        other => Err(ApiError::BadRequest(format!(
            "invalid tier '{other}'; expected one of: local, cached, network"
        ))),
    }
}

/// Validate a size string (humansize-style: `"512Mi"`, `"10Gi"`, etc.)
/// using the shared validator in `zlayer-spec`.
fn validate_size(size: &str) -> Result<()> {
    zlayer_spec::validate_memory_format(size)
        .map_err(|e| ApiError::BadRequest(format!("invalid size '{size}': {e}")))
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/volumes`.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
    /// [`VolumeUsageSource`] is wired.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub in_use_by: Vec<String>,
}

/// Legacy response shape kept for backwards compatibility with older SDK
/// consumers that deserialize strictly. New consumers should use
/// [`VolumeInfo`]. `list_volumes` now returns [`VolumeInfo`] which is a
/// strict superset of the fields in `VolumeSummary`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build a [`VolumeInfo`] from an on-disk volume directory. Reads the
/// sidecar when present; synthesizes reasonable defaults otherwise.
fn volume_info_from_disk(name: String, path: &FsPath) -> std::io::Result<VolumeInfo> {
    let size_bytes = dir_size(path).ok();

    let sidecar = read_sidecar(path)?;
    let (labels, created_at) = if let Some(meta) = sidecar {
        (meta.labels, meta.created_at)
    } else {
        // Legacy / implicit volume: derive created_at from directory
        // mtime, fall back to "now" if that fails.
        let created_at = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| {
                chrono::DateTime::<chrono::Utc>::from_timestamp(
                    i64::try_from(d.as_secs()).ok()?,
                    d.subsec_nanos(),
                )
            })
            .map_or_else(|| chrono::Utc::now().to_rfc3339(), |dt| dt.to_rfc3339());
        (HashMap::new(), created_at)
    };

    Ok(VolumeInfo {
        name,
        path: path.display().to_string(),
        size_bytes,
        labels,
        created_at,
        in_use_by: Vec::new(),
    })
}

/// Calculate the total size of a directory tree. Skips the sidecar file so
/// size reflects user-visible content only.
fn dir_size(path: &FsPath) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let entry_name = entry.file_name();
        if entry_name == METADATA_FILENAME && entry.path().is_file() {
            continue;
        }
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total = total.saturating_add(dir_size(&entry.path()).unwrap_or(0));
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

/// Check whether a volume directory contains any user content (ignoring
/// the sidecar).
fn has_user_content(path: &FsPath) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_name() == METADATA_FILENAME && entry.path().is_file() {
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Create a new named volume.
///
/// Creates `state.volume_dir/{name}` on disk and writes a
/// `.metadata.json` sidecar capturing labels, size, tier, and the
/// creation timestamp.
///
/// # Errors
///
/// - 400 Bad Request — invalid name, size, or tier.
/// - 403 Forbidden — caller lacks the `operator` role.
/// - 409 Conflict — a volume with this name already exists.
/// - 500 Internal — filesystem errors.
#[utoipa::path(
    post,
    path = "/api/v1/volumes",
    request_body = CreateVolumeRequest,
    responses(
        (status = 201, description = "Volume created", body = VolumeInfo),
        (status = 400, description = "Bad request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 409, description = "Volume already exists"),
    ),
    security(("bearer_auth" = [])),
    tag = "Volumes"
)]
pub async fn create_volume(
    user: AuthUser,
    State(state): State<VolumeApiState>,
    Json(req): Json<CreateVolumeRequest>,
) -> Result<(StatusCode, Json<VolumeInfo>)> {
    user.require_role("operator")?;

    // Validation (synchronous / cheap — do before blocking dispatch).
    validate_volume_name(&req.name)?;
    if let Some(size) = req.size.as_deref() {
        validate_size(size)?;
    }
    if let Some(tier) = req.tier.as_deref() {
        validate_tier(tier)?;
    }

    let volume_dir = state.volume_dir.clone();
    let name = req.name.clone();
    let labels = req.labels.unwrap_or_default();
    let size = req.size.clone();
    let tier = req.tier.clone().or_else(|| Some("local".to_string()));
    let created_at = chrono::Utc::now().to_rfc3339();

    let name_for_task = name.clone();
    let created_at_for_task = created_at.clone();
    let labels_for_task = labels.clone();
    let size_for_task = size.clone();
    let tier_for_task = tier.clone();

    let (path, info) = tokio::task::spawn_blocking(
        move || -> std::result::Result<(PathBuf, VolumeInfo), ApiError> {
            // Ensure parent exists (first-ever volume creation).
            std::fs::create_dir_all(&volume_dir).map_err(|e| {
                ApiError::Internal(format!(
                    "failed to create volume base dir '{}': {e}",
                    volume_dir.display()
                ))
            })?;

            let volume_path = volume_dir.join(&name_for_task);

            // Duplicate check — directory existing means the volume exists
            // already, regardless of whether a sidecar is present.
            if volume_path.exists() {
                return Err(ApiError::Conflict(format!(
                    "volume '{name_for_task}' already exists"
                )));
            }

            std::fs::create_dir(&volume_path).map_err(|e| {
                ApiError::Internal(format!(
                    "failed to create volume dir '{}': {e}",
                    volume_path.display()
                ))
            })?;

            let meta = VolumeMetadata {
                labels: labels_for_task.clone(),
                size: size_for_task,
                tier: tier_for_task,
                created_at: created_at_for_task.clone(),
            };

            if let Err(e) = write_sidecar(&volume_path, &meta) {
                // Roll back the directory we just created so the error is
                // not "half-created volume".
                let _ = std::fs::remove_dir_all(&volume_path);
                return Err(ApiError::Internal(format!(
                    "failed to write volume metadata: {e}"
                )));
            }

            let info = VolumeInfo {
                name: name_for_task,
                path: volume_path.display().to_string(),
                size_bytes: Some(0),
                labels: labels_for_task,
                created_at: created_at_for_task,
                in_use_by: Vec::new(),
            };

            Ok((volume_path, info))
        },
    )
    .await
    .map_err(|e| ApiError::Internal(format!("volume create task join error: {e}")))??;

    info!(volume = %info.name, path = %path.display(), "Created volume");
    Ok((StatusCode::CREATED, Json(info)))
}

/// List all volumes on disk.
///
/// Enumerates subdirectories under the volume base directory, reads each
/// sidecar when present, and returns a [`VolumeInfo`] for each entry
/// (including `in_use_by` when a [`VolumeUsageSource`] is wired).
///
/// # Errors
///
/// Returns an error if the volume directory cannot be read.
#[utoipa::path(
    get,
    path = "/api/v1/volumes",
    responses(
        (status = 200, description = "List of volumes", body = Vec<VolumeInfo>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Volumes"
)]
pub async fn list_volumes(
    _user: AuthUser,
    State(state): State<VolumeApiState>,
) -> Result<Json<Vec<VolumeInfo>>> {
    let volume_dir = state.volume_dir.clone();

    // Read directory entries and sidecars on a blocking thread to avoid
    // stalling the runtime.
    let mut volumes = tokio::task::spawn_blocking(
        move || -> std::result::Result<Vec<VolumeInfo>, std::io::Error> {
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
                let info = volume_info_from_disk(name, &path)?;
                result.push(info);
            }

            result.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(result)
        },
    )
    .await
    .map_err(|e| ApiError::Internal(format!("failed to list volumes: {e}")))?
    .map_err(|e| ApiError::Internal(format!("failed to read volume directory: {e}")))?;

    // Populate `in_use_by` asynchronously on the runtime (the source may
    // take locks we don't want to hold inside spawn_blocking).
    if let Some(source) = state.usage_source.as_ref() {
        for vol in &mut volumes {
            vol.in_use_by = source.containers_using(&vol.name).await;
        }
    }

    Ok(Json(volumes))
}

/// Inspect a single volume by name.
///
/// Reads the sidecar when present, synthesizes defaults for legacy
/// volumes, and populates `in_use_by` when a [`VolumeUsageSource`] is
/// wired.
///
/// # Errors
///
/// - 404 Not Found — no such volume.
/// - 500 Internal — filesystem or parse errors.
#[utoipa::path(
    get,
    path = "/api/v1/volumes/{name}",
    params(
        ("name" = String, Path, description = "Volume name"),
    ),
    responses(
        (status = 200, description = "Volume metadata", body = VolumeInfo),
        (status = 404, description = "Volume not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Volumes"
)]
pub async fn get_volume(
    _user: AuthUser,
    State(state): State<VolumeApiState>,
    Path(name): Path<String>,
) -> Result<Json<VolumeInfo>> {
    validate_volume_name(&name)?;

    let volume_path = state.volume_dir.join(&name);
    if !volume_path.exists() || !volume_path.is_dir() {
        return Err(ApiError::NotFound(format!("volume '{name}' not found")));
    }

    let name_for_task = name.clone();
    let mut info =
        tokio::task::spawn_blocking(move || -> std::result::Result<VolumeInfo, std::io::Error> {
            volume_info_from_disk(name_for_task, &volume_path)
        })
        .await
        .map_err(|e| ApiError::Internal(format!("failed to inspect volume: {e}")))?
        .map_err(|e| ApiError::Internal(format!("failed to read volume metadata: {e}")))?;

    if let Some(source) = state.usage_source.as_ref() {
        info.in_use_by = source.containers_using(&name).await;
    }

    Ok(Json(info))
}

/// Delete a volume by name.
///
/// By default, refuses to remove a volume that is non-empty OR that any
/// container currently reports mounting. `?force=true` overrides both
/// checks.
///
/// # Errors
///
/// - 404 Not Found — no such volume.
/// - 409 Conflict — non-empty or in-use without `?force=true`.
/// - 403 Forbidden — caller lacks the `operator` role.
#[utoipa::path(
    delete,
    path = "/api/v1/volumes/{name}",
    params(
        ("name" = String, Path, description = "Volume name"),
        ("force" = bool, Query, description = "Force removal of non-empty or in-use volumes"),
    ),
    responses(
        (status = 204, description = "Volume deleted"),
        (status = 404, description = "Volume not found"),
        (status = 409, description = "Volume is non-empty or in use (use ?force=true)"),
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
        return Err(ApiError::NotFound(format!("volume '{name}' not found")));
    }

    // Usage check (async, runs on the runtime).
    if !query.force {
        if let Some(source) = state.usage_source.as_ref() {
            let in_use_by = source.containers_using(&name).await;
            if !in_use_by.is_empty() {
                return Err(ApiError::Conflict(format!(
                    "volume '{name}' is in use by {} container(s): {}. Use ?force=true to remove anyway.",
                    in_use_by.len(),
                    in_use_by.join(", ")
                )));
            }
        }
    }

    let force = query.force;
    let name_clone = name.clone();

    tokio::task::spawn_blocking(move || -> std::result::Result<(), ApiError> {
        if !force
            && has_user_content(&volume_path)
                .map_err(|e| ApiError::Internal(format!("failed to read volume directory: {e}")))?
        {
            return Err(ApiError::Conflict(format!(
                "volume '{name_clone}' is non-empty. Use ?force=true to remove anyway."
            )));
        }

        std::fs::remove_dir_all(&volume_path).map_err(|e| {
            ApiError::Internal(format!("failed to remove volume '{name_clone}': {e}"))
        })?;

        Ok(())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("failed to delete volume: {e}")))??;

    info!(volume = %name, "Deleted volume");
    Ok(StatusCode::NO_CONTENT)
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

    #[test]
    fn test_validate_volume_name_accepts_valid() {
        assert!(validate_volume_name("a").is_ok());
        assert!(validate_volume_name("pg-data").is_ok());
        assert!(validate_volume_name("cache_v1").is_ok());
        assert!(validate_volume_name("0-legacy").is_ok());
        assert!(validate_volume_name("abcdefghijklmnopqrstuvwxyz0123456789_-").is_ok());
    }

    #[test]
    fn test_validate_volume_name_rejects_empty() {
        let err = validate_volume_name("").unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn test_validate_volume_name_rejects_uppercase() {
        assert!(validate_volume_name("MyVolume").is_err());
        assert!(validate_volume_name("my-Volume").is_err());
    }

    #[test]
    fn test_validate_volume_name_rejects_invalid_characters() {
        assert!(validate_volume_name("my vol").is_err());
        assert!(validate_volume_name("my.vol").is_err());
        assert!(validate_volume_name("my/vol").is_err());
        assert!(validate_volume_name("my:vol").is_err());
    }

    #[test]
    fn test_validate_volume_name_rejects_leading_dash_or_underscore() {
        assert!(validate_volume_name("-foo").is_err());
        assert!(validate_volume_name("_foo").is_err());
    }

    #[test]
    fn test_validate_volume_name_rejects_too_long() {
        let long = "a".repeat(65);
        assert!(validate_volume_name(&long).is_err());
    }

    #[test]
    fn test_validate_tier_accepts_known() {
        assert!(validate_tier("local").is_ok());
        assert!(validate_tier("cached").is_ok());
        assert!(validate_tier("network").is_ok());
    }

    #[test]
    fn test_validate_tier_rejects_unknown() {
        assert!(validate_tier("Local").is_err());
        assert!(validate_tier("ssd").is_err());
        assert!(validate_tier("").is_err());
    }

    #[test]
    fn test_validate_size_accepts_valid() {
        assert!(validate_size("512Mi").is_ok());
        assert!(validate_size("10Gi").is_ok());
    }

    #[test]
    fn test_validate_size_rejects_invalid() {
        assert!(validate_size("512MB").is_err());
        assert!(validate_size("512").is_err());
    }

    #[test]
    fn test_sidecar_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let vol = tmp.path().join("vol1");
        std::fs::create_dir(&vol).unwrap();

        let meta = VolumeMetadata {
            labels: HashMap::from([
                ("owner".to_string(), "zarc".to_string()),
                ("team".to_string(), "platform".to_string()),
            ]),
            size: Some("2Gi".to_string()),
            tier: Some("local".to_string()),
            created_at: "2026-04-20T12:34:56Z".to_string(),
        };

        write_sidecar(&vol, &meta).unwrap();

        let read = read_sidecar(&vol).unwrap().expect("sidecar should exist");
        assert_eq!(read.labels, meta.labels);
        assert_eq!(read.size, meta.size);
        assert_eq!(read.tier, meta.tier);
        assert_eq!(read.created_at, meta.created_at);
    }

    #[test]
    fn test_sidecar_absent_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let vol = tmp.path().join("legacy-vol");
        std::fs::create_dir(&vol).unwrap();

        assert!(read_sidecar(&vol).unwrap().is_none());
    }

    #[test]
    fn test_volume_info_from_disk_with_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let vol = tmp.path().join("pg-data");
        std::fs::create_dir(&vol).unwrap();

        let meta = VolumeMetadata {
            labels: HashMap::from([("app".to_string(), "postgres".to_string())]),
            size: Some("10Gi".to_string()),
            tier: Some("local".to_string()),
            created_at: "2026-04-20T00:00:00Z".to_string(),
        };
        write_sidecar(&vol, &meta).unwrap();

        // Add some user content.
        std::fs::write(vol.join("data.bin"), b"hello world").unwrap();

        let info = volume_info_from_disk("pg-data".to_string(), &vol).unwrap();
        assert_eq!(info.name, "pg-data");
        assert_eq!(info.labels.get("app").map(String::as_str), Some("postgres"));
        assert_eq!(info.created_at, "2026-04-20T00:00:00Z");
        // size excludes the sidecar.
        assert_eq!(info.size_bytes, Some(b"hello world".len() as u64));
    }

    #[test]
    fn test_volume_info_from_disk_legacy_no_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let vol = tmp.path().join("legacy");
        std::fs::create_dir(&vol).unwrap();
        std::fs::write(vol.join("x"), b"abc").unwrap();

        let info = volume_info_from_disk("legacy".to_string(), &vol).unwrap();
        assert_eq!(info.name, "legacy");
        assert!(info.labels.is_empty());
        // created_at is derived from mtime and should be RFC 3339 parseable.
        chrono::DateTime::parse_from_rfc3339(&info.created_at)
            .expect("synthesized created_at must be RFC 3339");
        assert_eq!(info.size_bytes, Some(3));
    }

    #[test]
    fn test_has_user_content_ignores_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let vol = tmp.path().join("empty-with-meta");
        std::fs::create_dir(&vol).unwrap();
        let meta = VolumeMetadata {
            labels: HashMap::new(),
            size: None,
            tier: Some("local".to_string()),
            created_at: "2026-04-20T00:00:00Z".to_string(),
        };
        write_sidecar(&vol, &meta).unwrap();

        // Only the sidecar is present — should report "no user content".
        assert!(!has_user_content(&vol).unwrap());

        // Add real content — should flip to true.
        std::fs::write(vol.join("x"), b"x").unwrap();
        assert!(has_user_content(&vol).unwrap());
    }
}
