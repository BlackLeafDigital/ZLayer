//! Audit log query endpoint.
//!
//! Routes:
//!
//! ```text
//! GET /api/v1/audit?user={id}&resource_kind={k}&since={ts}&until={ts}&limit={n}
//!     -> list (admin)
//! ```
//!
//! Only admins may view the audit log.

use std::sync::Arc;

use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;
use crate::storage::{AuditEntry, AuditFilter, AuditStorage};

/// State for audit endpoints.
#[derive(Clone)]
pub struct AuditState {
    /// Underlying audit storage backend.
    pub store: Arc<dyn AuditStorage>,
}

impl AuditState {
    /// Build a new state from an audit store.
    #[must_use]
    pub fn new(store: Arc<dyn AuditStorage>) -> Self {
        Self { store }
    }
}

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

// ---- Endpoint ----

/// List audit log entries. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin, or
/// [`ApiError::Internal`] if the store fails.
#[utoipa::path(
    get,
    path = "/api/v1/audit",
    params(AuditQuery),
    responses(
        (status = 200, description = "List audit entries", body = Vec<AuditEntry>),
        (status = 403, description = "Admin role required"),
    ),
    tag = "Audit"
)]
pub async fn list_audit(
    actor: AuthActor,
    State(state): State<AuditState>,
    axum::extract::Query(q): axum::extract::Query<AuditQuery>,
) -> Result<Json<Vec<AuditEntry>>> {
    actor.require_admin()?;

    let filter = AuditFilter {
        user_id: q.user,
        resource_kind: q.resource_kind,
        since: q.since,
        until: q.until,
        limit: q.limit.unwrap_or(100),
    };

    let entries = state
        .store
        .list(filter)
        .await
        .map_err(|e| ApiError::Internal(format!("Audit store: {e}")))?;

    Ok(Json(entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_query_defaults() {
        let q: AuditQuery = serde_json::from_str("{}").unwrap();
        assert!(q.user.is_none());
        assert!(q.resource_kind.is_none());
        assert!(q.since.is_none());
        assert!(q.until.is_none());
        assert!(q.limit.is_none());
    }

    #[test]
    fn test_audit_query_with_params() {
        let q: AuditQuery =
            serde_json::from_str(r#"{"user": "u-1", "resource_kind": "deployment", "limit": 50}"#)
                .unwrap();
        assert_eq!(q.user.as_deref(), Some("u-1"));
        assert_eq!(q.resource_kind.as_deref(), Some("deployment"));
        assert_eq!(q.limit, Some(50));
    }
}
