//! User group API DTOs.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Body for `POST /api/v1/groups`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateGroupRequest {
    /// Group name.
    pub name: String,
    /// Free-form description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Body for `PATCH /api/v1/groups/{id}`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateGroupRequest {
    /// Updated name.
    #[serde(default)]
    pub name: Option<String>,
    /// Updated description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Body for `POST /api/v1/groups/{id}/members`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AddMemberRequest {
    /// The user id to add.
    pub user_id: String,
}

/// Response for member list queries.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GroupMembersResponse {
    /// Group id.
    pub group_id: String,
    /// List of user ids in the group.
    pub members: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_group_request_deserialize() {
        let json = r#"{"name": "developers"}"#;
        let req: CreateGroupRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "developers");
        assert!(req.description.is_none());
    }

    #[test]
    fn test_create_group_request_with_description() {
        let json = r#"{"name": "devs", "description": "Developer team"}"#;
        let req: CreateGroupRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "devs");
        assert_eq!(req.description.as_deref(), Some("Developer team"));
    }

    #[test]
    fn test_update_group_request_partial() {
        let req: UpdateGroupRequest = serde_json::from_str("{}").unwrap();
        assert!(req.name.is_none());
        assert!(req.description.is_none());
    }

    #[test]
    fn test_add_member_request_deserialize() {
        let json = r#"{"user_id": "u-123"}"#;
        let req: AddMemberRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.user_id, "u-123");
    }
}
