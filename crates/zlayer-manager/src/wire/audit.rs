//! Wire types for the Audit page.
//!
//! Mirror of `zlayer_api::storage::AuditEntry` (the daemon's stored audit
//! row) and the filter shape used by
//! `GET /api/v1/audit?user=&resource_kind=&since=&until=&limit=`. Defined
//! locally so the hydrate/WASM build doesn't need `zlayer-api` (which is
//! SSR-only).
//!
//! Field shape was cross-checked against
//! `crates/zlayer-api/src/storage/mod.rs::AuditEntry` and
//! `crates/zlayer-api/src/handlers/audit.rs::AuditQuery`.
//!
//! Daemon emits `created_at` as the RFC-3339 timestamp field name on the
//! wire; we keep that name here so serde round-trips are byte-identical.
//! `user_id` and `resource_kind` are NON-optional `String` on the daemon
//! (not `Option<String>`), so the wire type matches that shape.
//! `resource_id`, `details`, `ip`, and `user_agent` are all optional and
//! default to `None` when absent.
//!
//! The daemon's list endpoint returns a bare `Vec<AuditEntry>` JSON array
//! (no `{entries, total}` envelope), so there's no `WireAuditPage` type —
//! the Manager paginates client-side by asking for `limit = per_page + 1`
//! and peeking at the last row to decide whether a "Next" page exists.

use serde::{Deserialize, Serialize};

/// A single audit log row, mirrored from `zlayer_api::storage::AuditEntry`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireAuditEntry {
    /// UUID identifier of this audit entry.
    pub id: String,
    /// The user who performed the action. Non-optional on the daemon.
    pub user_id: String,
    /// The action performed (`"create"`, `"update"`, `"delete"`, etc.).
    pub action: String,
    /// The kind of resource acted upon (`"deployment"`, `"user"`, ...).
    pub resource_kind: String,
    /// The specific resource id, when applicable.
    #[serde(default)]
    pub resource_id: Option<String>,
    /// Free-form context JSON. Preserved as `serde_json::Value` so the
    /// Manager can pretty-print it in the row-expand panel.
    #[serde(default)]
    pub details: Option<serde_json::Value>,
    /// Client IP address, when known.
    #[serde(default)]
    pub ip: Option<String>,
    /// Client user-agent string, when known.
    #[serde(default)]
    pub user_agent: Option<String>,
    /// When the action occurred — RFC-3339 UTC.
    pub created_at: String,
}

/// Filter shape for `GET /api/v1/audit`. Passed from the client to the
/// server function and translated to query params there.
///
/// Timestamps are RFC-3339 UTC strings (the daemon parses them via
/// `chrono::DateTime<Utc>` serde). An empty string in any `Option`
/// variant means "no filter" and is flattened to `None` before the
/// server-fn call, so the page code can lift raw `<input>` values into
/// this shape without filtering empty strings at every call site.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WireAuditFilter {
    /// Filter by user id.
    #[serde(default)]
    pub user_id: Option<String>,
    /// Filter by resource kind.
    #[serde(default)]
    pub resource_kind: Option<String>,
    /// Only entries at or after this timestamp (RFC-3339 UTC).
    #[serde(default)]
    pub since: Option<String>,
    /// Only entries at or before this timestamp (RFC-3339 UTC).
    #[serde(default)]
    pub until: Option<String>,
    /// Maximum number of entries to return.
    #[serde(default)]
    pub limit: Option<usize>,
}
