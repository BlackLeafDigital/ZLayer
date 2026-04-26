//! Sync DTOs.
//!
//! Wire types for the sync CRUD + diff/apply endpoints. A sync resource
//! points at a directory within a project's git checkout that contains
//! `ZLayer` resource YAMLs. The diff endpoint scans the directory and
//! reports what would change; the apply endpoint actually reconciles.

use serde::{Deserialize, Serialize};
#[cfg(feature = "utoipa")]
use utoipa::ToSchema;

/// Body for `POST /api/v1/syncs`.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct CreateSyncRequest {
    /// Display name for this sync.
    pub name: String,
    /// Linked project id.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Path within the project's checkout to scan for resource YAMLs.
    pub git_path: String,
    /// Whether the sync should automatically apply on pull.
    #[serde(default)]
    pub auto_apply: Option<bool>,
    /// Whether `apply` should delete resources on the API that are missing
    /// from the manifest directory. Defaults to `false` (the safer choice).
    #[serde(default)]
    pub delete_missing: Option<bool>,
}

/// Result of reconciling a single resource during apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct SyncResourceResult {
    /// The source manifest path (or remote resource name for deletions).
    pub resource: String,
    /// Resource kind: `"deployment"`, `"job"`, `"cron"`, or other.
    pub kind: String,
    /// Action taken: `"create"`, `"update"`, `"delete"`, or `"skip"`.
    pub action: String,
    /// Outcome status: `"ok"` or `"error"`.
    pub status: String,
    /// Optional error message (`status == "error"`) or skip reason
    /// (`action == "skip"`). Omitted on successful `"ok"` results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SyncResourceResult {
    /// Build a successful result for the given resource/kind/action.
    #[must_use]
    pub fn ok(resource: &str, kind: &str, action: &str) -> Self {
        Self {
            resource: resource.to_string(),
            kind: kind.to_string(),
            action: action.to_string(),
            status: "ok".to_string(),
            error: None,
        }
    }

    /// Build an error result with a human-readable message.
    #[must_use]
    pub fn err(resource: &str, kind: &str, action: &str, message: String) -> Self {
        Self {
            resource: resource.to_string(),
            kind: kind.to_string(),
            action: action.to_string(),
            status: "error".to_string(),
            error: Some(message),
        }
    }

    /// Build a skip result with a reason recorded in `error`.
    #[must_use]
    pub fn skip(resource: &str, kind: &str, message: String) -> Self {
        Self {
            resource: resource.to_string(),
            kind: kind.to_string(),
            action: "skip".to_string(),
            status: "ok".to_string(),
            error: Some(message),
        }
    }
}

/// Response for a real apply. Reports per-resource outcomes, the current
/// commit SHA the sync was applied at (when known), and a short human-readable
/// summary for CLI display.
///
/// NOTE: This is a breaking change from the previous dry-run-only
/// `{ diff, message }` shape. Clients inspecting the apply response directly
/// must be updated — callers that only consumed the HTTP status stay working.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct SyncApplyResponse {
    /// Per-resource reconcile results.
    pub results: Vec<SyncResourceResult>,
    /// Commit SHA the sync was applied against (when resolvable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_sha: Option<String>,
    /// Aggregate summary suitable for CLI output, e.g.
    /// `"3 created, 2 updated, 1 deleted, 0 skipped"`.
    pub summary: String,
}

/// JSON-friendly wrapper around the sync diff output.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct SyncDiffResponse {
    /// Resources to create.
    pub to_create: Vec<SyncResourceResponse>,
    /// Resources to update.
    pub to_update: Vec<SyncResourceResponse>,
    /// Resource names to delete.
    pub to_delete: Vec<String>,
}

/// A single resource in the diff output.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct SyncResourceResponse {
    /// Source file name.
    pub file_path: String,
    /// Resource kind.
    pub kind: String,
    /// Resource name.
    pub name: String,
}
