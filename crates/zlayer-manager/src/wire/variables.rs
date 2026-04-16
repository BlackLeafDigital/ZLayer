//! Wire type for the Variables page.
//!
//! Mirrors `zlayer_api::storage::StoredVariable`, but defined locally so
//! the hydrate/WASM build doesn't need `zlayer-api` (which is SSR-only).

use serde::{Deserialize, Serialize};

/// A stored variable as returned by `/api/v1/variables`.
///
/// Note: the daemon's `StoredVariable` type uses `DateTime<Utc>` for the
/// timestamps but serialises them as RFC-3339 strings — so receiving them
/// here as `String` round-trips lossless.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireVariable {
    /// UUID identifier.
    pub id: String,
    /// Variable name (unique within a scope).
    pub name: String,
    /// Plaintext value.
    pub value: String,
    /// Optional project-scope id. `None` = global variable.
    #[serde(default)]
    pub scope: Option<String>,
    /// RFC-3339 creation timestamp.
    pub created_at: String,
    /// RFC-3339 last-updated timestamp.
    pub updated_at: String,
}
