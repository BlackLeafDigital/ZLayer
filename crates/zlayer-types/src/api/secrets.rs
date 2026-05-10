//! Secrets management API DTOs.
//!
//! Wire types for the secrets endpoints. Secret values are never exposed
//! through the API except via an explicit admin-only `?reveal=true` request —
//! only metadata is returned for listing and retrieval.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request to create or update a secret.
///
/// `scope` is optional and only honored on the legacy code path — when set
/// alongside `?environment=`, the request is rejected.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSecretRequest {
    /// The name of the secret.
    pub name: String,
    /// The secret value (will be encrypted at rest).
    pub value: String,
    /// Optional explicit scope (legacy form). Mutually exclusive with the
    /// `?environment=` query parameter.
    #[serde(default)]
    pub scope: Option<String>,
    /// Optional per-secret node affinity. `None` (default) = any node may
    /// host the decryptable form. When set, only matching nodes receive a
    /// wrap of the DEK material for this row, and the API gate filters
    /// reads accordingly. Ignored on standalone (non-clustered) daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_affinity: Option<crate::storage::NodeAffinity>,
}

/// Response containing secret metadata. Never includes the value unless
/// the caller is on the explicit `?reveal=true` admin path, in which case
/// `value` is populated.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SecretMetadataResponse {
    /// The name/identifier of the secret.
    pub name: String,
    /// Unix timestamp when the secret was created.
    pub created_at: i64,
    /// Unix timestamp when the secret was last updated.
    pub updated_at: i64,
    /// Version number of the secret (incremented on each update).
    pub version: u32,
    /// Plaintext value — populated only on `?reveal=true` admin reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// Request body for secret rotation.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RotateSecretRequest {
    /// The new secret value (will be encrypted at rest).
    pub value: String,
    /// Optional per-secret node affinity update. `None` here means "leave
    /// existing affinity unchanged"; to clear affinity explicitly, pass an
    /// empty selector via a separate update endpoint (Phase 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_affinity: Option<crate::storage::NodeAffinity>,
}

/// Response returned by the rotate endpoint.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RotateSecretResponse {
    /// The secret name.
    pub name: String,
    /// Version prior to rotation. `None` if the secret did not exist (won't
    /// happen today — rotate rejects missing secrets — but preserved for
    /// forward compatibility).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_version: Option<u32>,
    /// Version after rotation.
    pub new_version: u32,
}

/// Response for the batch reveal endpoint — returns every secret in an env as plaintext.
/// Admin-only for now (Phase 3 will gate this on per-env Read permission instead).
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RevealAllSecretsResponse {
    /// The environment id the secrets were revealed from.
    pub environment: String,
    /// Name → plaintext value map. Includes every secret in the scope.
    pub secrets: std::collections::HashMap<String, String>,
}

/// Result body for `POST /api/v1/secrets/bulk-import`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BulkImportResponse {
    /// Number of new secrets created.
    pub created: usize,
    /// Number of existing secrets updated.
    pub updated: usize,
    /// Per-line errors. Empty when every line parsed and stored cleanly.
    pub errors: Vec<String>,
}

/// Query for create / list / get / delete endpoints.
#[derive(Debug, Default, Deserialize)]
pub struct SecretsScopeQuery {
    /// Environment id whose namespace to operate in. Mutually exclusive
    /// with `scope`.
    #[serde(default)]
    pub environment: Option<String>,
    /// Explicit scope string (legacy). Mutually exclusive with `environment`.
    #[serde(default)]
    pub scope: Option<String>,
}

/// Query for `GET /api/v1/secrets/{name}` — extends the scope query with a
/// `reveal` flag for admin-only plaintext reads.
#[derive(Debug, Default, Deserialize)]
pub struct GetSecretQuery {
    /// Environment id whose namespace to read from. Mutually exclusive with `scope`.
    #[serde(default)]
    pub environment: Option<String>,
    /// Explicit scope string (legacy). Mutually exclusive with `environment`.
    #[serde(default)]
    pub scope: Option<String>,
    /// When true, include the plaintext value. Admin only.
    #[serde(default)]
    pub reveal: bool,
}

/// Query for `POST /api/v1/secrets/bulk-import` — `environment` is required.
#[derive(Debug, Deserialize)]
pub struct BulkImportQuery {
    /// Environment id to import the secrets into.
    pub environment: String,
}
