//! Permission grant/revoke/list API DTOs.

use serde::{Deserialize, Serialize};
#[cfg(feature = "utoipa")]
use utoipa::{IntoParams, ToSchema};

use crate::storage::{PermissionLevel, SubjectKind};

/// Query parameters for `GET /api/v1/permissions`.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(IntoParams))]
pub struct ListPermissionsQuery {
    /// Filter by user id.
    #[serde(default)]
    pub user: Option<String>,
    /// Filter by group id.
    #[serde(default)]
    pub group: Option<String>,
}

/// Query parameters for `GET /api/v1/permissions/by-resource`.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(IntoParams))]
pub struct ListByResourceQuery {
    /// Resource kind (e.g. `"environment"`).
    pub kind: String,
    /// Specific resource id. Omit for wildcard grants only.
    #[serde(default)]
    pub id: Option<String>,
}

/// Body for `POST /api/v1/permissions`.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct GrantPermissionRequest {
    /// Whether the subject is a user or a group.
    pub subject_kind: SubjectKind,
    /// The user or group id.
    pub subject_id: String,
    /// The kind of resource (e.g. `"deployment"`, `"project"`, `"secret"`).
    pub resource_kind: String,
    /// A specific resource id, or omit for a wildcard (all resources of that kind).
    #[serde(default)]
    pub resource_id: Option<String>,
    /// The access level to grant.
    pub level: PermissionLevel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grant_permission_request_deserialize() {
        let json = r#"{
            "subject_kind": "user",
            "subject_id": "u-1",
            "resource_kind": "deployment",
            "resource_id": "d-1",
            "level": "write"
        }"#;
        let req: GrantPermissionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.subject_kind, SubjectKind::User);
        assert_eq!(req.subject_id, "u-1");
        assert_eq!(req.resource_kind, "deployment");
        assert_eq!(req.resource_id.as_deref(), Some("d-1"));
        assert_eq!(req.level, PermissionLevel::Write);
    }

    #[test]
    fn test_grant_permission_request_wildcard() {
        let json = r#"{
            "subject_kind": "group",
            "subject_id": "g-1",
            "resource_kind": "project",
            "level": "read"
        }"#;
        let req: GrantPermissionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.subject_kind, SubjectKind::Group);
        assert!(req.resource_id.is_none());
        assert_eq!(req.level, PermissionLevel::Read);
    }

    #[test]
    fn test_list_permissions_query_user() {
        let q: ListPermissionsQuery = serde_json::from_str(r#"{"user": "u-1"}"#).unwrap();
        assert_eq!(q.user.as_deref(), Some("u-1"));
        assert!(q.group.is_none());
    }

    #[test]
    fn test_list_permissions_query_group() {
        let q: ListPermissionsQuery = serde_json::from_str(r#"{"group": "g-1"}"#).unwrap();
        assert!(q.user.is_none());
        assert_eq!(q.group.as_deref(), Some("g-1"));
    }
}
