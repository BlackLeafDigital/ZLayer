//! Secrets domain wire/storage types.
//!
//! Lifted from `zlayer-secrets` so cross-crate consumers (`zlayer-api`,
//! `zlayer-agent`, the CLI) can name secrets shapes without depending on
//! `zlayer-secrets`. The `zlayer-secrets` crate re-exports these for
//! backward compatibility and continues to own the trait definitions,
//! crypto, and persistent-store implementations.

pub mod client_keys;
pub mod error;
pub mod git;
pub mod registry;
pub mod sealed;
pub mod types;

// Convenience flat re-exports so callers can `use zlayer_types::secrets::Secret;`
// rather than reaching into submodule paths.
pub use client_keys::{ActorKind, ClientPublicKey, PUBLIC_KEY_LEN};
pub use error::{Result, SecretsError};
pub use git::{GitCredential, GitCredentialKind};
pub use registry::{RegistryAuthType, RegistryCredential};
pub use sealed::{RecipientPublicKey, SealedError, SealedSecret};
pub use types::{RotationResult, Secret, SecretMetadata, SecretRef, SecretScope};
