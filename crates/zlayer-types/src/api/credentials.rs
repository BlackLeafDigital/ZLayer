//! Credential management DTOs (registry and git credentials).
//!
//! These types mirror the credential records stored by `zlayer-secrets`
//! but are defined here as wire types so they can be reused by clients
//! without depending on the secrets crate.

use serde::{Deserialize, Serialize};

/// Registry credential metadata (returned by list/create; no password).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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

/// Authentication method for a registry credential (`OpenAPI` schema).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RegistryAuthTypeSchema {
    /// HTTP Basic authentication (username + password).
    Basic,
    /// Bearer token authentication.
    Token,
}

/// Git credential metadata (returned by list/create; no secret value).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct GitCredentialResponse {
    /// Unique identifier.
    pub id: String,
    /// Human-readable display label.
    pub name: String,
    /// Credential kind.
    pub kind: GitCredentialKindSchema,
}

/// Kind of git credential (`OpenAPI` schema).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum GitCredentialKindSchema {
    /// Personal access token.
    Pat,
    /// SSH private key.
    SshKey,
}

/// Body for `POST /api/v1/credentials/registry`.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CreateGitCredentialRequest {
    /// Human-readable label (e.g. `"GitHub PAT for ci"`).
    pub name: String,
    /// PAT or SSH key content (stored encrypted, never returned).
    pub value: String,
    /// Credential kind.
    pub kind: GitCredentialKindSchema,
}
