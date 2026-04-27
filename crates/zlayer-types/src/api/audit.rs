//! Audit log API DTOs.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use utoipa::IntoParams;

/// Query parameters for `GET /api/v1/audit`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct AuditQuery {
    /// Filter by user id.
    #[serde(default)]
    pub user: Option<String>,
    /// Filter by resource kind.
    #[serde(default)]
    pub resource_kind: Option<String>,
    /// Only entries at or after this timestamp (RFC 3339).
    #[serde(default)]
    #[param(value_type = Option<String>)]
    pub since: Option<DateTime<Utc>>,
    /// Only entries at or before this timestamp (RFC 3339).
    #[serde(default)]
    #[param(value_type = Option<String>)]
    pub until: Option<DateTime<Utc>>,
    /// Maximum number of entries to return (default 100).
    #[serde(default)]
    pub limit: Option<usize>,
}
