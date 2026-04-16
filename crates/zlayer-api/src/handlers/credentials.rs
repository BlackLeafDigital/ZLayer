//! Credential management endpoints for registry and git credentials.
//!
//! Provides REST handlers for creating, listing, and deleting registry
//! (Docker/OCI) and git (PAT/SSH) credentials. Secrets are stored encrypted
//! via the underlying `zlayer-secrets` credential stores; only metadata is
//! returned in list/create responses.
//!
//! All mutating endpoints require the `admin` role. Read-only list endpoints
//! accept any authenticated actor.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;

/// State for credential endpoints.
#[derive(Clone)]
pub struct CredentialState {
    /// Registry credential store.
    pub registry_store:
        Arc<zlayer_secrets::RegistryCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>>,
    /// Git credential store.
    pub git_store:
        Arc<zlayer_secrets::GitCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>>,
}

impl CredentialState {
    /// Build a new credential state from both stores.
    #[must_use]
    pub fn new(
        registry_store: Arc<
            zlayer_secrets::RegistryCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>,
        >,
        git_store: Arc<
            zlayer_secrets::GitCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>,
        >,
    ) -> Self {
        Self {
            registry_store,
            git_store,
        }
    }
}

// ---- OpenAPI-compatible response types ----
//
// The `zlayer_secrets` types do not derive `ToSchema`, so we define local
// response structs that mirror them and can be registered in `openapi.rs`.

/// Registry credential metadata (returned by list/create; no password).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RegistryCredentialResponse {
    /// Unique identifier.
    pub id: String,
    /// Registry hostname, e.g. `"docker.io"`, `"ghcr.io"`.
    pub registry: String,
    /// Username for authentication.
    pub username: String,
    /// Authentication method.
    pub auth_type: RegistryAuthTypeSchema,
}

impl From<zlayer_secrets::RegistryCredential> for RegistryCredentialResponse {
    fn from(c: zlayer_secrets::RegistryCredential) -> Self {
        Self {
            id: c.id,
            registry: c.registry,
            username: c.username,
            auth_type: match c.auth_type {
                zlayer_secrets::RegistryAuthType::Basic => RegistryAuthTypeSchema::Basic,
                zlayer_secrets::RegistryAuthType::Token => RegistryAuthTypeSchema::Token,
            },
        }
    }
}

/// Authentication method for a registry credential (`OpenAPI` schema).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegistryAuthTypeSchema {
    /// HTTP Basic authentication (username + password).
    Basic,
    /// Bearer token authentication.
    Token,
}

/// Git credential metadata (returned by list/create; no secret value).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GitCredentialResponse {
    /// Unique identifier.
    pub id: String,
    /// Human-readable display label.
    pub name: String,
    /// Credential kind.
    pub kind: GitCredentialKindSchema,
}

impl From<zlayer_secrets::GitCredential> for GitCredentialResponse {
    fn from(c: zlayer_secrets::GitCredential) -> Self {
        Self {
            id: c.id,
            name: c.name,
            kind: match c.kind {
                zlayer_secrets::GitCredentialKind::Pat => GitCredentialKindSchema::Pat,
                zlayer_secrets::GitCredentialKind::SshKey => GitCredentialKindSchema::SshKey,
            },
        }
    }
}

/// Kind of git credential (`OpenAPI` schema).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitCredentialKindSchema {
    /// Personal access token.
    Pat,
    /// SSH private key.
    SshKey,
}

// ---- Request types ----

/// Body for `POST /api/v1/credentials/registry`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateRegistryCredentialRequest {
    /// Registry hostname (e.g. `"docker.io"`).
    pub registry: String,
    /// Username for authentication.
    pub username: String,
    /// Password or token value (stored encrypted, never returned).
    pub password: String,
    /// Authentication method.
    pub auth_type: RegistryAuthTypeSchema,
}

/// Body for `POST /api/v1/credentials/git`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateGitCredentialRequest {
    /// Human-readable label (e.g. `"GitHub PAT for ci"`).
    pub name: String,
    /// PAT or SSH key content (stored encrypted, never returned).
    pub value: String,
    /// Credential kind.
    pub kind: GitCredentialKindSchema,
}

// ---- Endpoints: Registry credentials ----

/// List all registry credentials (metadata only, no passwords).
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the credential store fails.
#[utoipa::path(
    get,
    path = "/api/v1/credentials/registry",
    responses(
        (status = 200, description = "List of registry credentials", body = Vec<RegistryCredentialResponse>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Credentials"
)]
pub async fn list_registry_credentials(
    _actor: AuthActor,
    State(state): State<CredentialState>,
) -> Result<Json<Vec<RegistryCredentialResponse>>> {
    let creds = state
        .registry_store
        .list()
        .await
        .map_err(|e| ApiError::Internal(format!("Registry credential store: {e}")))?;
    Ok(Json(creds.into_iter().map(Into::into).collect()))
}

/// Create a new registry credential. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins,
/// [`ApiError::BadRequest`] for missing fields, or
/// [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    post,
    path = "/api/v1/credentials/registry",
    request_body = CreateRegistryCredentialRequest,
    responses(
        (status = 201, description = "Registry credential created", body = RegistryCredentialResponse),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Credentials"
)]
pub async fn create_registry_credential(
    actor: AuthActor,
    State(state): State<CredentialState>,
    Json(req): Json<CreateRegistryCredentialRequest>,
) -> Result<(StatusCode, Json<RegistryCredentialResponse>)> {
    actor.require_admin()?;

    if req.registry.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Registry hostname is required".to_string(),
        ));
    }
    if req.username.trim().is_empty() {
        return Err(ApiError::BadRequest("Username is required".to_string()));
    }
    if req.password.is_empty() {
        return Err(ApiError::BadRequest("Password is required".to_string()));
    }

    let auth_type = match req.auth_type {
        RegistryAuthTypeSchema::Basic => zlayer_secrets::RegistryAuthType::Basic,
        RegistryAuthTypeSchema::Token => zlayer_secrets::RegistryAuthType::Token,
    };

    let cred = state
        .registry_store
        .create(&req.registry, &req.username, &req.password, auth_type)
        .await
        .map_err(|e| ApiError::Internal(format!("Registry credential store: {e}")))?;

    Ok((StatusCode::CREATED, Json(cred.into())))
}

/// Delete a registry credential. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins,
/// [`ApiError::NotFound`] when the credential does not exist, or
/// [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/credentials/registry/{id}",
    params(("id" = String, Path, description = "Registry credential id")),
    responses(
        (status = 204, description = "Registry credential deleted"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Credentials"
)]
pub async fn delete_registry_credential(
    actor: AuthActor,
    State(state): State<CredentialState>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    state.registry_store.delete(&id).await.map_err(|e| {
        if e.to_string().contains("not found") || e.to_string().contains("NotFound") {
            ApiError::NotFound(format!("Registry credential {id} not found"))
        } else {
            ApiError::Internal(format!("Registry credential store: {e}"))
        }
    })?;

    Ok(StatusCode::NO_CONTENT)
}

// ---- Endpoints: Git credentials ----

/// List all git credentials (metadata only, no secret values).
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the credential store fails.
#[utoipa::path(
    get,
    path = "/api/v1/credentials/git",
    responses(
        (status = 200, description = "List of git credentials", body = Vec<GitCredentialResponse>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Credentials"
)]
pub async fn list_git_credentials(
    _actor: AuthActor,
    State(state): State<CredentialState>,
) -> Result<Json<Vec<GitCredentialResponse>>> {
    let creds = state
        .git_store
        .list()
        .await
        .map_err(|e| ApiError::Internal(format!("Git credential store: {e}")))?;
    Ok(Json(creds.into_iter().map(Into::into).collect()))
}

/// Create a new git credential. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins,
/// [`ApiError::BadRequest`] for missing fields, or
/// [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    post,
    path = "/api/v1/credentials/git",
    request_body = CreateGitCredentialRequest,
    responses(
        (status = 201, description = "Git credential created", body = GitCredentialResponse),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Credentials"
)]
pub async fn create_git_credential(
    actor: AuthActor,
    State(state): State<CredentialState>,
    Json(req): Json<CreateGitCredentialRequest>,
) -> Result<(StatusCode, Json<GitCredentialResponse>)> {
    actor.require_admin()?;

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Credential name is required".to_string(),
        ));
    }
    if req.value.is_empty() {
        return Err(ApiError::BadRequest(
            "Credential value is required".to_string(),
        ));
    }

    let kind = match req.kind {
        GitCredentialKindSchema::Pat => zlayer_secrets::GitCredentialKind::Pat,
        GitCredentialKindSchema::SshKey => zlayer_secrets::GitCredentialKind::SshKey,
    };

    let cred = state
        .git_store
        .create(&req.name, &req.value, kind)
        .await
        .map_err(|e| ApiError::Internal(format!("Git credential store: {e}")))?;

    Ok((StatusCode::CREATED, Json(cred.into())))
}

/// Delete a git credential. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins,
/// [`ApiError::NotFound`] when the credential does not exist, or
/// [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/credentials/git/{id}",
    params(("id" = String, Path, description = "Git credential id")),
    responses(
        (status = 204, description = "Git credential deleted"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Credentials"
)]
pub async fn delete_git_credential(
    actor: AuthActor,
    State(state): State<CredentialState>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    state.git_store.delete(&id).await.map_err(|e| {
        if e.to_string().contains("not found") || e.to_string().contains("NotFound") {
            ApiError::NotFound(format!("Git credential {id} not found"))
        } else {
            ApiError::Internal(format!("Git credential store: {e}"))
        }
    })?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_registry_credential_request_deserialize() {
        let req: CreateRegistryCredentialRequest = serde_json::from_str(
            r#"{"registry":"docker.io","username":"user","password":"pw","auth_type":"basic"}"#,
        )
        .unwrap();
        assert_eq!(req.registry, "docker.io");
        assert_eq!(req.username, "user");
        assert_eq!(req.password, "pw");
        assert_eq!(req.auth_type, RegistryAuthTypeSchema::Basic);
    }

    #[test]
    fn test_create_registry_credential_request_token_type() {
        let req: CreateRegistryCredentialRequest = serde_json::from_str(
            r#"{"registry":"ghcr.io","username":"bot","password":"ghp_xxx","auth_type":"token"}"#,
        )
        .unwrap();
        assert_eq!(req.auth_type, RegistryAuthTypeSchema::Token);
    }

    #[test]
    fn test_create_git_credential_request_pat() {
        let req: CreateGitCredentialRequest =
            serde_json::from_str(r#"{"name":"GitHub PAT","value":"ghp_xxxx","kind":"pat"}"#)
                .unwrap();
        assert_eq!(req.name, "GitHub PAT");
        assert_eq!(req.value, "ghp_xxxx");
        assert_eq!(req.kind, GitCredentialKindSchema::Pat);
    }

    #[test]
    fn test_create_git_credential_request_ssh() {
        let req: CreateGitCredentialRequest = serde_json::from_str(
            r#"{"name":"Deploy key","value":"ssh-rsa AAAA...","kind":"ssh_key"}"#,
        )
        .unwrap();
        assert_eq!(req.kind, GitCredentialKindSchema::SshKey);
    }

    #[test]
    fn test_registry_auth_type_schema_roundtrip() {
        let basic = serde_json::to_string(&RegistryAuthTypeSchema::Basic).unwrap();
        assert_eq!(basic, "\"basic\"");
        let token = serde_json::to_string(&RegistryAuthTypeSchema::Token).unwrap();
        assert_eq!(token, "\"token\"");

        let parsed: RegistryAuthTypeSchema = serde_json::from_str(&basic).unwrap();
        assert_eq!(parsed, RegistryAuthTypeSchema::Basic);
    }

    #[test]
    fn test_git_credential_kind_schema_roundtrip() {
        let pat = serde_json::to_string(&GitCredentialKindSchema::Pat).unwrap();
        assert_eq!(pat, "\"pat\"");
        let ssh = serde_json::to_string(&GitCredentialKindSchema::SshKey).unwrap();
        assert_eq!(ssh, "\"ssh_key\"");

        let parsed: GitCredentialKindSchema = serde_json::from_str(&pat).unwrap();
        assert_eq!(parsed, GitCredentialKindSchema::Pat);
    }
}
