//! Secrets management endpoints
//!
//! Provides CRUD operations for secrets management. Secret values are never
//! exposed through the API - only metadata is returned for listing and retrieval.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use zlayer_secrets::{Secret, SecretMetadata, SecretsStore};

/// Request to create or update a secret
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSecretRequest {
    /// The name of the secret
    pub name: String,
    /// The secret value (will be encrypted at rest)
    pub value: String,
}

/// Response containing secret metadata (never the value)
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SecretMetadataResponse {
    /// The name/identifier of the secret
    pub name: String,
    /// Unix timestamp when the secret was created
    pub created_at: i64,
    /// Unix timestamp when the secret was last updated
    pub updated_at: i64,
    /// Version number of the secret (incremented on each update)
    pub version: u32,
}

impl From<SecretMetadata> for SecretMetadataResponse {
    fn from(metadata: SecretMetadata) -> Self {
        Self {
            name: metadata.name,
            created_at: metadata.created_at,
            updated_at: metadata.updated_at,
            version: metadata.version,
        }
    }
}

impl From<&SecretMetadata> for SecretMetadataResponse {
    fn from(metadata: &SecretMetadata) -> Self {
        Self {
            name: metadata.name.clone(),
            created_at: metadata.created_at,
            updated_at: metadata.updated_at,
            version: metadata.version,
        }
    }
}

/// State for secrets endpoints
#[derive(Clone)]
pub struct SecretsState {
    /// Secrets store for CRUD operations
    pub store: Arc<dyn SecretsStore + Send + Sync>,
}

impl SecretsState {
    /// Create a new secrets state with the given store
    pub fn new(store: Arc<dyn SecretsStore + Send + Sync>) -> Self {
        Self { store }
    }
}

/// Create or update a secret
///
/// Stores a new secret or updates an existing one. The secret value is encrypted
/// at rest and the version number is incremented on updates.
#[utoipa::path(
    post,
    path = "/api/v1/secrets",
    request_body = CreateSecretRequest,
    responses(
        (status = 201, description = "Secret created", body = SecretMetadataResponse),
        (status = 200, description = "Secret updated", body = SecretMetadataResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer_auth" = [])),
    tag = "Secrets"
)]
pub async fn create_secret(
    user: AuthUser,
    State(state): State<SecretsState>,
    Json(request): Json<CreateSecretRequest>,
) -> Result<(StatusCode, Json<SecretMetadataResponse>)> {
    // Require operator role for creating secrets
    user.require_role("operator")?;

    // Validate the secret name
    if request.name.is_empty() {
        return Err(ApiError::BadRequest(
            "Secret name cannot be empty".to_string(),
        ));
    }

    if request.name.len() > 256 {
        return Err(ApiError::BadRequest(
            "Secret name cannot exceed 256 characters".to_string(),
        ));
    }

    // Check if secret exists to determine if this is create or update
    let scope = user.id();
    let exists = state
        .store
        .exists(scope, &request.name)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to check secret existence: {}", e)))?;

    // Store the secret
    let secret = Secret::new(&request.value);
    state
        .store
        .set_secret(scope, &request.name, &secret)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to store secret: {}", e)))?;

    // Get the metadata to return
    let metadata_list = state
        .store
        .list_secrets(scope)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list secrets: {}", e)))?;

    let metadata = metadata_list
        .into_iter()
        .find(|m| m.name == request.name)
        .ok_or_else(|| {
            ApiError::Internal("Secret was stored but metadata not found".to_string())
        })?;

    let status = if exists {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };

    Ok((status, Json(SecretMetadataResponse::from(metadata))))
}

/// List all secrets for the authenticated user
///
/// Returns metadata for all secrets in the user's scope. Secret values are
/// never exposed through this endpoint.
#[utoipa::path(
    get,
    path = "/api/v1/secrets",
    responses(
        (status = 200, description = "List of secret metadata", body = Vec<SecretMetadataResponse>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Secrets"
)]
pub async fn list_secrets(
    user: AuthUser,
    State(state): State<SecretsState>,
) -> Result<Json<Vec<SecretMetadataResponse>>> {
    let scope = user.id();

    let metadata_list = state
        .store
        .list_secrets(scope)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list secrets: {}", e)))?;

    let response: Vec<SecretMetadataResponse> = metadata_list
        .iter()
        .map(SecretMetadataResponse::from)
        .collect();

    Ok(Json(response))
}

/// Get metadata for a specific secret
///
/// Returns metadata for a single secret. The secret value is never exposed
/// through this endpoint.
#[utoipa::path(
    get,
    path = "/api/v1/secrets/{name}",
    params(
        ("name" = String, Path, description = "Secret name"),
    ),
    responses(
        (status = 200, description = "Secret metadata", body = SecretMetadataResponse),
        (status = 404, description = "Secret not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Secrets"
)]
pub async fn get_secret_metadata(
    user: AuthUser,
    State(state): State<SecretsState>,
    Path(name): Path<String>,
) -> Result<Json<SecretMetadataResponse>> {
    let scope = user.id();

    // Check if the secret exists
    let exists = state
        .store
        .exists(scope, &name)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to check secret existence: {}", e)))?;

    if !exists {
        return Err(ApiError::NotFound(format!("Secret '{}' not found", name)));
    }

    // Get the metadata from the list
    let metadata_list = state
        .store
        .list_secrets(scope)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list secrets: {}", e)))?;

    let metadata = metadata_list
        .into_iter()
        .find(|m| m.name == name)
        .ok_or_else(|| ApiError::NotFound(format!("Secret '{}' not found", name)))?;

    Ok(Json(SecretMetadataResponse::from(metadata)))
}

/// Delete a secret
///
/// Permanently removes a secret from the store.
#[utoipa::path(
    delete,
    path = "/api/v1/secrets/{name}",
    params(
        ("name" = String, Path, description = "Secret name"),
    ),
    responses(
        (status = 204, description = "Secret deleted"),
        (status = 404, description = "Secret not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer_auth" = [])),
    tag = "Secrets"
)]
pub async fn delete_secret(
    user: AuthUser,
    State(state): State<SecretsState>,
    Path(name): Path<String>,
) -> Result<StatusCode> {
    // Require operator role for deleting secrets
    user.require_role("operator")?;

    let scope = user.id();

    // Check if the secret exists first
    let exists = state
        .store
        .exists(scope, &name)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to check secret existence: {}", e)))?;

    if !exists {
        return Err(ApiError::NotFound(format!("Secret '{}' not found", name)));
    }

    // Delete the secret
    state
        .store
        .delete_secret(scope, &name)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete secret: {}", e)))?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_metadata_response_from() {
        let metadata = SecretMetadata {
            name: "test-secret".to_string(),
            created_at: 1234567890,
            updated_at: 1234567900,
            version: 3,
        };

        let response = SecretMetadataResponse::from(metadata);

        assert_eq!(response.name, "test-secret");
        assert_eq!(response.created_at, 1234567890);
        assert_eq!(response.updated_at, 1234567900);
        assert_eq!(response.version, 3);
    }

    #[test]
    fn test_create_secret_request_deserialize() {
        let json = r#"{"name": "api-key", "value": "secret-value-123"}"#;
        let request: CreateSecretRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.name, "api-key");
        assert_eq!(request.value, "secret-value-123");
    }

    #[test]
    fn test_secret_metadata_response_serialize() {
        let response = SecretMetadataResponse {
            name: "db-password".to_string(),
            created_at: 1000,
            updated_at: 2000,
            version: 5,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("db-password"));
        assert!(json.contains("1000"));
        assert!(json.contains("2000"));
        assert!(json.contains("5"));
        // Ensure the value is NOT in the response
        assert!(!json.contains("value"));
    }
}
