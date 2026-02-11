//! Authentication endpoints

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use utoipa::ToSchema;

use crate::auth::{create_token, AuthState};
use crate::error::{ApiError, Result};

/// Token request
#[derive(Debug, Deserialize, ToSchema)]
pub struct TokenRequest {
    /// API key or username
    pub api_key: String,
    /// API secret or password
    pub api_secret: String,
}

/// Token response
#[derive(Debug, Serialize, ToSchema)]
pub struct TokenResponse {
    /// JWT access token
    pub access_token: String,
    /// Token type (always "Bearer")
    pub token_type: String,
    /// Expiration in seconds
    pub expires_in: u64,
}

/// Get an access token
#[utoipa::path(
    post,
    path = "/auth/token",
    request_body = TokenRequest,
    responses(
        (status = 200, description = "Token created", body = TokenResponse),
        (status = 401, description = "Invalid credentials"),
    ),
    tag = "Authentication"
)]
pub async fn get_token(
    State(auth): State<AuthState>,
    Json(request): Json<TokenRequest>,
) -> Result<Json<TokenResponse>> {
    // Validate against the credential store if available
    if let Some(cred_store) = &auth.credential_store {
        match cred_store
            .validate(&request.api_key, &request.api_secret)
            .await
        {
            Ok(Some(roles)) => {
                let expiry = Duration::from_secs(3600);
                let token = create_token(&auth.jwt_secret, &request.api_key, expiry, roles)?;

                return Ok(Json(TokenResponse {
                    access_token: token,
                    token_type: "Bearer".to_string(),
                    expires_in: expiry.as_secs(),
                }));
            }
            Ok(None) => {
                // Invalid credentials -- fall through to error
            }
            Err(e) => {
                tracing::error!(error = %e, "Credential store error during authentication");
                return Err(ApiError::Internal(
                    "Authentication backend error".to_string(),
                ));
            }
        }
    } else {
        // Dev fallback: accept "dev"/"dev-secret" when no credential store is configured
        if request.api_key == "dev" && request.api_secret == "dev-secret" {
            tracing::warn!("Using dev credentials -- NOT SAFE FOR PRODUCTION");
            let expiry = Duration::from_secs(3600);
            let token = create_token(&auth.jwt_secret, "dev", expiry, vec!["admin".to_string()])?;

            return Ok(Json(TokenResponse {
                access_token: token,
                token_type: "Bearer".to_string(),
                expires_in: expiry.as_secs(),
            }));
        }
    }

    Err(ApiError::Unauthorized("Invalid credentials".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_request_deserialize() {
        let json = r#"{"api_key": "test", "api_secret": "secret"}"#;
        let request: TokenRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.api_key, "test");
        assert_eq!(request.api_secret, "secret");
    }

    #[test]
    fn test_token_response_serialize() {
        let response = TokenResponse {
            access_token: "token123".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("token123"));
        assert!(json.contains("Bearer"));
    }
}
