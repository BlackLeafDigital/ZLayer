//! `ZLayer` Secrets Management
//!
//! Provides secure storage and retrieval of secrets for container workloads.
//!
//! ## Scoping
//! Secrets are organized hierarchically:
//! - Deployment-level: Shared by all services in a deployment
//! - Service-level: Specific to a single service
//!
//! ## Syntax
//! - `$S:secret-name` - Deployment-level secret
//! - `$S:@service/secret-name` - Service-specific secret

mod encryption;
mod error;
mod key_manager;
mod provider;
mod types;

#[cfg(feature = "persistent")]
mod persistent;

#[cfg(feature = "vault")]
mod vault;

pub use encryption::EncryptionKey;
pub use error::{Result, SecretsError};
pub use key_manager::KeyManager;
pub use provider::{SecretsProvider, SecretsResolver, SecretsStore};
pub use types::{Secret, SecretMetadata, SecretRef, SecretScope};

#[cfg(feature = "persistent")]
pub use persistent::PersistentSecretsStore;

#[cfg(feature = "vault")]
pub use vault::VaultSecretsProvider;
