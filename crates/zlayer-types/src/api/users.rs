//! User management DTOs.

use serde::{Deserialize, Serialize};

use crate::storage::UserRole;

/// Body for `POST /api/v1/users`.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateUserRequest {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub role: UserRole,
}

/// Body for `PATCH /api/v1/users/{id}`. All fields are optional so a caller
/// can update a single attribute without touching the others.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UpdateUserRequest {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub role: Option<UserRole>,
    #[serde(default)]
    pub is_active: Option<bool>,
}

/// Body for `POST /api/v1/users/{id}/password`.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SetPasswordRequest {
    /// New password.
    pub new_password: String,
    /// Required when the caller is setting their own password (non-admin
    /// path). Admins changing someone else's password may omit this.
    #[serde(default)]
    pub current_password: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_user_request_deserialize() {
        let json = r#"{"email": "a@b.com", "password": "pw", "role": "user"}"#;
        let req: CreateUserRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.email, "a@b.com");
        assert_eq!(req.role, UserRole::User);
        assert!(req.display_name.is_none());
    }

    #[test]
    fn test_create_user_request_admin_role() {
        let json = r#"{"email":"a@b.com","password":"pw","role":"admin","display_name":"A"}"#;
        let req: CreateUserRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.role, UserRole::Admin);
        assert_eq!(req.display_name.as_deref(), Some("A"));
    }

    #[test]
    fn test_update_user_request_partial() {
        // All fields optional — empty body is valid
        let req: UpdateUserRequest = serde_json::from_str("{}").unwrap();
        assert!(req.display_name.is_none());
        assert!(req.role.is_none());
        assert!(req.is_active.is_none());
    }

    #[test]
    fn test_set_password_request_deserialize() {
        let req: SetPasswordRequest =
            serde_json::from_str(r#"{"new_password":"np","current_password":"cp"}"#).unwrap();
        assert_eq!(req.new_password, "np");
        assert_eq!(req.current_password.as_deref(), Some("cp"));

        let req_admin: SetPasswordRequest =
            serde_json::from_str(r#"{"new_password":"np"}"#).unwrap();
        assert!(req_admin.current_password.is_none());
    }
}
