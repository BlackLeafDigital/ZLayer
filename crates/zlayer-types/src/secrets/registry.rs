//! Registry credential data shapes (the wire/storage form, not the store impl).
//!
//! Lifted into `zlayer-types` so cross-crate consumers can name these
//! without depending on `zlayer-secrets`. The store impl
//! (`RegistryCredentialStore`) stays in `zlayer-secrets` and consumes these
//! types from here.

use serde::{Deserialize, Serialize};

/// Docker/OCI registry credential metadata.
///
/// The actual password/token is stored separately as a secret in the
/// `registry_credentials` scope, keyed by [`id`](RegistryCredential::id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryCredential {
    /// Unique identifier (UUID v4).
    pub id: String,
    /// Registry hostname, e.g. `"docker.io"`, `"ghcr.io"`.
    pub registry: String,
    /// Username for authentication.
    pub username: String,
    /// Whether this credential uses basic auth or a bearer token.
    pub auth_type: RegistryAuthType,
}

/// Authentication method for a registry credential.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RegistryAuthType {
    /// HTTP Basic authentication (username + password).
    Basic,
    /// Bearer token authentication.
    Token,
}
