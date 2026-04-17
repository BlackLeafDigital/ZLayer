//! Wire types for the Secrets page.
//!
//! Mirrors the daemon's `SecretMetadataResponse` and `StoredEnvironment`
//! shapes, kept local so the hydrate/WASM build does not need to pull in
//! `zlayer-api` (SSR-only).
//!
//! ## Daemon reality
//!
//! - `GET /api/v1/secrets` returns `Vec<SecretMetadataResponse>` — each entry
//!   has `name`, `created_at` / `updated_at` as unix seconds (i64), and a
//!   `version` counter. There is no server-side "id", "masked value", or
//!   "description" — the name is the identity within a scope.
//! - Reveal is a query flag on the single-secret read:
//!   `GET /api/v1/secrets/{name}?reveal=true&environment={id}`; the
//!   response is a `SecretMetadataResponse` whose `value` field is
//!   populated.
//! - Bulk import is `POST /api/v1/secrets/bulk-import?environment={id}`
//!   with a raw `.env`-formatted body (not JSON).
//!
//! The wire types here reflect those shapes exactly — no synthetic fields.

use serde::{Deserialize, Serialize};

/// A secret as returned by `/api/v1/secrets`.
///
/// Values are never sent on list — only metadata. The plaintext `value`
/// only arrives on the single-secret reveal path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireSecret {
    /// Secret identifier within a scope. Also the path segment used by
    /// single-secret endpoints.
    pub name: String,
    /// Unix seconds — when the secret was first created.
    pub created_at: i64,
    /// Unix seconds — last update.
    pub updated_at: i64,
    /// Monotonic version counter, incremented on each update.
    pub version: u32,
    /// Plaintext — only populated when the response came from a reveal
    /// read. Always absent on list responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// An environment as returned by `/api/v1/environments`.
///
/// Timestamps round-trip as RFC-3339 strings since the daemon serialises
/// `DateTime<Utc>` that way.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireEnvironment {
    /// UUID identifier.
    pub id: String,
    /// Display name (unique within a project scope).
    pub name: String,
    /// Optional owning project id. `None` = global environment.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Free-form description.
    #[serde(default)]
    pub description: Option<String>,
    /// RFC-3339 creation timestamp.
    pub created_at: String,
    /// RFC-3339 last-updated timestamp.
    pub updated_at: String,
}

/// Result body returned by `POST /api/v1/secrets/bulk-import`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BulkImportResult {
    /// How many new secrets were written.
    pub created: usize,
    /// How many existing secrets were overwritten.
    pub updated: usize,
    /// Per-line error messages. Empty on a clean import.
    pub errors: Vec<String>,
}
