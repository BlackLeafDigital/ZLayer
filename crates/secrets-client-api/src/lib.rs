//! Shared secret-fetching contract for `ZLayer` Secrets consumers.
//!
//! This crate holds *only* the data types and the async trait that a consumer
//! needs to read secrets through an abstract backend: [`SecretScope`],
//! [`Secret`], [`SecretsError`], and the [`SecretsClient`] trait. It carries
//! no transport, no crypto, and no `sqlx` — just the contract.
//!
//! ## Why this exists
//!
//! `ZBilling` reads every credential through a `SecretsClient` trait object
//! (`zbilling-secrets`) so its backend can be swapped without touching call
//! sites. The concrete `ZLayer`-backed implementation, `ZLayerSecretsClient`,
//! lives in the sibling `zlayer-secrets-client` crate. Putting the trait +
//! types here — rather than in either `zbilling-secrets` or the heavy
//! `zlayer-secrets` crate — means:
//!
//! - `zlayer-secrets-client` can implement the exact same shape `ZBilling`
//!   expects **without** `ZBilling` having to depend on `ZLayer` (and without
//!   `ZLayer` depending on `ZBilling`).
//! - Consumers avoid pulling in `zlayer-secrets`' crypto/`sqlx` dependency
//!   tree just to call `get()`.
//!
//! The type definitions are kept byte-for-byte compatible with
//! `zbilling-secrets`' own copy so that, when `ZBilling` adopts `ZLayer`
//! Secrets, the swap is a one-line backend change in `main.rs` (construct a
//! `ZLayerSecretsClient` instead of an `EnvVarSecretsClient`); both satisfy a
//! `SecretsClient` trait of identical shape.

use async_trait::async_trait;
use std::fmt;
use thiserror::Error;

// ---------------------------------------------------------------------------
// SecretScope
// ---------------------------------------------------------------------------

/// Logical grouping for a secret. Backends use this to namespace lookups
/// (env-var prefix, vault path, `ZLayer` secret scope, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SecretScope {
    Stripe,
    License,
    ZRelay,
    Postgres,
    Custom(String),
}

impl SecretScope {
    /// Stable lowercase identifier for the scope. Used for `Display` and any
    /// path-style formatting (including the `?scope=` query the `ZLayer`
    /// Secrets API expects).
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            SecretScope::Stripe => "stripe",
            SecretScope::License => "license",
            SecretScope::ZRelay => "zrelay",
            SecretScope::Postgres => "postgres",
            SecretScope::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for SecretScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Secret
// ---------------------------------------------------------------------------

/// Newtype wrapping a secret string. Its `Debug` impl never prints the
/// contents — use [`Secret::expose`] when you actually need the value.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Borrow the inner secret string. Caller is responsible for not leaking
    /// the returned reference into logs.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Consume the secret and return the inner string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret([REDACTED, len={}])", self.0.len())
    }
}

impl From<String> for Secret {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Secret {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum SecretsError {
    #[error("secret not found: scope={scope}, name={name}")]
    NotFound { scope: String, name: String },
    #[error("backend read failed: {0}")]
    ReadFailed(String),
}

// ---------------------------------------------------------------------------
// SecretsClient trait
// ---------------------------------------------------------------------------

/// Async secret-fetching backend. Implementations must be `Send + Sync` so
/// `Arc<dyn SecretsClient>` can be shared across tokio tasks and axum state.
#[async_trait]
pub trait SecretsClient: Send + Sync {
    /// Fetch a secret. Returns `Ok(None)` when the secret is not present
    /// (consumer decides whether absence is an error). Returns `Err(_)` only
    /// on backend failure (network, decryption, etc.).
    async fn get(&self, scope: &SecretScope, name: &str) -> Result<Option<Secret>, SecretsError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_as_str_and_display_are_stable() {
        assert_eq!(SecretScope::Stripe.as_str(), "stripe");
        assert_eq!(SecretScope::License.as_str(), "license");
        assert_eq!(SecretScope::ZRelay.as_str(), "zrelay");
        assert_eq!(SecretScope::Postgres.as_str(), "postgres");
        assert_eq!(SecretScope::Custom("foo".into()).as_str(), "foo");
        assert_eq!(format!("{}", SecretScope::Stripe), "stripe");
        assert_eq!(format!("{}", SecretScope::Custom("bar".into())), "bar");
    }

    #[test]
    fn secret_debug_redacts() {
        let s = Secret::new("hunter2");
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("hunter2"), "leaked secret: {dbg}");
        assert!(dbg.contains("REDACTED"), "missing REDACTED marker: {dbg}");
    }

    #[test]
    fn secret_expose_and_into_string_roundtrip() {
        assert_eq!(Secret::new("x").expose(), "x");
        assert_eq!(Secret::from("y").into_string(), "y");
        assert_eq!(Secret::from(String::from("z")).expose(), "z");
    }

    #[test]
    fn not_found_error_display_includes_scope_and_name() {
        let err = SecretsError::NotFound {
            scope: "stripe".to_string(),
            name: "api-key".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("stripe"));
        assert!(s.contains("api-key"));
    }
}
