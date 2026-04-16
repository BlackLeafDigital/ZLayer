//! Wire types for the Permissions page.
//!
//! Mirror of `zlayer_api::storage::StoredPermission` plus the
//! `GrantPermissionRequest` body from
//! `crates/zlayer-api/src/handlers/permissions.rs`. Defined locally so the
//! hydrate/WASM build doesn't need `zlayer-api` (which is SSR-only).
//!
//! Field shape was cross-checked against
//! `crates/zlayer-api/src/storage/mod.rs`
//! (`SubjectKind`, `PermissionLevel`, `StoredPermission`) and
//! `crates/zlayer-api/src/handlers/permissions.rs`
//! (`GrantPermissionRequest`, `ListPermissionsQuery`).
//!
//! The daemon serialises `SubjectKind` and `PermissionLevel` with
//! `#[serde(rename_all = "snake_case")]`, so the string values are
//! `"user" | "group"` and `"none" | "read" | "execute" | "write"`. We keep
//! these as plain `String` on the wire (not a typed enum) so new levels or
//! subject kinds can be rendered without a forced wire-type bump — the
//! UI already renders unknown values with a neutral badge class.
//!
//! `DateTime<Utc>` is serialised as RFC-3339, so receiving `created_at`
//! here as `String` round-trips cleanly.

use serde::{Deserialize, Serialize};

/// A stored permission grant as returned by `/api/v1/permissions`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WirePermission {
    /// UUID identifier of this permission grant.
    pub id: String,
    /// Subject kind — `"user"` or `"group"`.
    pub subject_kind: String,
    /// The user or group id.
    pub subject_id: String,
    /// Kind of resource — e.g. `"deployment"`, `"project"`, `"secret"`.
    pub resource_kind: String,
    /// Specific resource id, or `None` for a wildcard grant (all resources of
    /// that kind).
    #[serde(default)]
    pub resource_id: Option<String>,
    /// Access level — `"none" | "read" | "execute" | "write"`.
    pub level: String,
    /// RFC-3339 creation timestamp.
    pub created_at: String,
}

/// Request body for `POST /api/v1/permissions`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireGrantPermissionRequest {
    /// Subject kind — `"user"` or `"group"`.
    pub subject_kind: String,
    /// The user or group id.
    pub subject_id: String,
    /// Kind of resource — e.g. `"deployment"`, `"project"`, `"secret"`.
    pub resource_kind: String,
    /// Specific resource id, or `None` for a wildcard grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    /// Access level — `"read" | "execute" | "write"`.
    /// ("none" is technically accepted by the schema but never useful as a grant.)
    pub level: String,
}
