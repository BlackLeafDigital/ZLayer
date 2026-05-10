//! Git credential data shapes (the wire/storage form, not the store impl).
//!
//! Lifted into `zlayer-types` so cross-crate consumers can name these
//! without depending on `zlayer-secrets`. The store impl
//! (`GitCredentialStore`) stays in `zlayer-secrets` and consumes these
//! types from here.

use serde::{Deserialize, Serialize};

/// Git authentication credential metadata.
///
/// The actual PAT or SSH key is stored separately as a [`Secret`] in the
/// `git_credentials` scope, keyed by [`id`](GitCredential::id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCredential {
    /// Unique identifier (UUID v4).
    pub id: String,
    /// Human-readable display label, e.g. `"GitHub PAT for ci"`.
    pub name: String,
    /// Whether this credential is a personal access token or an SSH key.
    pub kind: GitCredentialKind,
}

/// The kind of Git credential.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GitCredentialKind {
    /// Personal access token.
    Pat,
    /// SSH private key.
    SshKey,
}
