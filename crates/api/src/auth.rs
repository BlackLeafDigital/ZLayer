//! JWT authentication for the ZLayer API
//!
//! Provides token creation, verification, and Axum request extractors.

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{header::AUTHORIZATION, request::Parts},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::warn;

use crate::error::ApiError;

/// JWT claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID or API key ID)
    pub sub: String,
    /// Expiration time (Unix timestamp)
    pub exp: u64,
    /// Issued at (Unix timestamp)
    pub iat: u64,
    /// Issuer
    pub iss: String,
    /// Roles/permissions
    #[serde(default)]
    pub roles: Vec<String>,
}

impl Claims {
    /// Create new claims
    pub fn new(subject: impl Into<String>, expiry: Duration, roles: Vec<String>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            sub: subject.into(),
            exp: now + expiry.as_secs(),
            iat: now,
            iss: "zlayer".to_string(),
            roles,
        }
    }

    /// Check if token is expired
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.exp < now
    }

    /// Check if the user has a specific role
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role || r == "admin")
    }
}

/// Create a JWT token
pub fn create_token(
    secret: &str,
    subject: impl Into<String>,
    expiry: Duration,
    roles: Vec<String>,
) -> Result<String, ApiError> {
    let claims = Claims::new(subject, expiry, roles);

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| ApiError::Internal(format!("Failed to create token: {}", e)))
}

/// Verify and decode a JWT token
pub fn verify_token(secret: &str, token: &str) -> Result<Claims, ApiError> {
    let mut validation = Validation::default();
    validation.set_issuer(&["zlayer"]);

    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|e| {
        warn!(error = %e, "Token verification failed");
        ApiError::Unauthorized(format!("Invalid token: {}", e))
    })
}

/// Authenticated user extracted from request
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub claims: Claims,
}

impl AuthUser {
    /// Get the user/subject ID
    pub fn id(&self) -> &str {
        &self.claims.sub
    }

    /// Check if user has a role
    pub fn has_role(&self, role: &str) -> bool {
        self.claims.has_role(role)
    }

    /// Require a specific role, returning Forbidden if not present
    pub fn require_role(&self, role: &str) -> Result<(), ApiError> {
        if self.has_role(role) {
            Ok(())
        } else {
            Err(ApiError::Forbidden(format!("Role '{}' required", role)))
        }
    }
}

/// State needed for authentication
#[derive(Clone)]
pub struct AuthState {
    pub jwt_secret: String,
}

/// Axum extractor for authenticated users
#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Get auth state from extensions
        let auth_state = parts
            .extensions
            .get::<AuthState>()
            .cloned()
            .ok_or_else(|| ApiError::Internal("Auth state not configured".to_string()))?;

        // Extract Authorization header
        let auth_header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| ApiError::Unauthorized("Missing Authorization header".to_string()))?;

        // Parse Bearer token
        let token = auth_header.strip_prefix("Bearer ").ok_or_else(|| {
            ApiError::Unauthorized("Invalid Authorization header format".to_string())
        })?;

        // Verify token
        let claims = verify_token(&auth_state.jwt_secret, token)?;

        if claims.is_expired() {
            return Err(ApiError::Unauthorized("Token expired".to_string()));
        }

        Ok(AuthUser { claims })
    }
}

/// Optional authentication extractor
/// Returns None if no auth header, error if invalid auth
#[derive(Debug, Clone)]
pub struct OptionalAuthUser(pub Option<AuthUser>);

#[async_trait]
impl<S> FromRequestParts<S> for OptionalAuthUser
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // Check if Authorization header exists
        if parts.headers.get(AUTHORIZATION).is_none() {
            return Ok(OptionalAuthUser(None));
        }

        // If header exists, it must be valid
        let user = AuthUser::from_request_parts(parts, state).await?;
        Ok(OptionalAuthUser(Some(user)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "test-secret-key-for-jwt-testing-purposes";

    #[test]
    fn test_create_and_verify_token() {
        let token = create_token(
            TEST_SECRET,
            "user123",
            Duration::from_secs(3600),
            vec!["reader".to_string()],
        )
        .unwrap();

        let claims = verify_token(TEST_SECRET, &token).unwrap();

        assert_eq!(claims.sub, "user123");
        assert!(!claims.is_expired());
        assert!(claims.has_role("reader"));
        assert!(!claims.has_role("admin"));
    }

    #[test]
    fn test_expired_token() {
        let claims = Claims {
            sub: "user123".to_string(),
            exp: 0, // Expired immediately
            iat: 0,
            iss: "zlayer".to_string(),
            roles: vec![],
        };

        assert!(claims.is_expired());
    }

    #[test]
    fn test_admin_role_overrides() {
        let claims = Claims {
            sub: "admin".to_string(),
            exp: u64::MAX,
            iat: 0,
            iss: "zlayer".to_string(),
            roles: vec!["admin".to_string()],
        };

        // Admin should have access to any role
        assert!(claims.has_role("admin"));
        assert!(claims.has_role("reader"));
        assert!(claims.has_role("writer"));
        assert!(claims.has_role("anything"));
    }

    #[test]
    fn test_invalid_token_fails() {
        let result = verify_token(TEST_SECRET, "invalid.token.here");
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_secret_fails() {
        let token =
            create_token(TEST_SECRET, "user123", Duration::from_secs(3600), vec![]).unwrap();

        let result = verify_token("wrong-secret", &token);
        assert!(result.is_err());
    }
}
