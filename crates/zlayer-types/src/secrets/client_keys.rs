//! Client public key data shapes for sealed-box recipient registration.
//!
//! Lifted into `zlayer-types` so cross-crate consumers can name these
//! without depending on `zlayer-secrets`. The persistent store and the
//! `ClientKeyStore` trait stay in `zlayer-secrets` and consume these from
//! here.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::secrets::error::{Result, SecretsError};

/// Required length, in bytes, of an X25519 / Curve25519 public key.
pub const PUBLIC_KEY_LEN: usize = 32;

/// The kind of actor a registered client key belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    /// A human user (matches the `users` table in the auth backend).
    User,
    /// A programmatic API key (matches the `api_keys` table).
    ApiKey,
}

impl ActorKind {
    /// Database / wire representation of this actor kind.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::ApiKey => "api_key",
        }
    }

    /// Parses an [`ActorKind`] from its database / wire string form.
    ///
    /// # Errors
    ///
    /// Returns [`SecretsError::Storage`] if `s` is not one of the
    /// recognized values (`"user"` or `"api_key"`).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "user" => Ok(Self::User),
            "api_key" => Ok(Self::ApiKey),
            other => Err(SecretsError::Storage(format!(
                "invalid actor_kind in client_public_keys row: {other:?}"
            ))),
        }
    }
}

/// A registered client public key bound to an actor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientPublicKey {
    /// Opaque, unique identifier (`ck_<32-hex>`). Surfaced to clients so
    /// they can reference the key on subsequent requests.
    pub key_id: String,
    /// Whether the owning actor is a `User` or an `ApiKey`.
    pub actor_kind: ActorKind,
    /// Identifier of the owning actor (UUID/text, opaque to this crate).
    pub actor_id: String,
    /// The 32-byte X25519 public key bytes.
    pub public_key: Vec<u8>,
    /// Optional human-readable label (e.g. browser/device name).
    pub label: Option<String>,
    /// When the key was registered.
    pub created_at: DateTime<Utc>,
    /// Most recent successful use (sealed-box decrypt), if any.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Soft-delete timestamp; `None` means the key is still active.
    pub revoked_at: Option<DateTime<Utc>>,
}
