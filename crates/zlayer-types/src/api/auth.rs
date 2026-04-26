//! Authentication DTOs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::storage::StoredUser;

/// Token request
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct TokenRequest {
    /// API key or username
    pub api_key: String,
    /// API secret or password
    pub api_secret: String,
}

/// Token response
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct TokenResponse {
    /// JWT access token
    pub access_token: String,
    /// Token type (always "Bearer")
    pub token_type: String,
    /// Expiration in seconds
    pub expires_in: u64,
}

/// Request for `POST /auth/bootstrap`.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct BootstrapRequest {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// Response shape used by login + bootstrap + me.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct UserView {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub is_active: bool,
    #[cfg_attr(feature = "utoipa", schema(value_type = Option<String>, example = "2026-04-15T12:00:00Z"))]
    pub last_login_at: Option<DateTime<Utc>>,
}

impl From<&StoredUser> for UserView {
    fn from(u: &StoredUser) -> Self {
        Self {
            id: u.id.clone(),
            email: u.email.clone(),
            display_name: u.display_name.clone(),
            role: u.role.to_string(),
            is_active: u.is_active,
            last_login_at: u.last_login_at,
        }
    }
}

/// Response for `/auth/login` and `/auth/bootstrap`.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct LoginResponse {
    pub user: UserView,
    pub csrf_token: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CsrfResponse {
    pub csrf_token: String,
}
