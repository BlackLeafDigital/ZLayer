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
//! - `$secret://<env>/<KEY>` - Environment-scoped secret (requires an
//!   [`EnvScopeProvider`] wired via [`SecretsResolver::with_env_resolver`])
//! - `$secret://<env>/<KEY>/<field>` - With JSON field extraction

pub mod cluster_dek;
mod cluster_signer;
mod encryption;
mod error;
mod jwt;
mod key_manager;
pub mod node_effects;
mod provider;
pub mod raft_sm;
pub mod raft_store;
pub mod sealed;
mod types;
mod worker_bootstrap;
mod worker_ca;

#[cfg(feature = "persistent")]
pub mod client_keys;

#[cfg(feature = "persistent")]
mod persistent;

#[cfg(feature = "persistent")]
pub mod credentials;

#[cfg(feature = "persistent")]
pub mod registry_credentials;

#[cfg(feature = "persistent")]
pub mod git_credentials;

#[cfg(feature = "vault")]
mod vault;

pub use cluster_dek::ClusterDek;
pub use cluster_signer::{
    list_valid_pubkeys, load_signer_for_kid, prune_expired_grace, rotate_keystore, ClusterCa,
    ClusterSigner, FileBackend, KeystoreRotationResult, PubkeyInfo, PubkeyStatus, SigningBackend,
};
pub use encryption::EncryptionKey;
pub use error::{Result, SecretsError};
pub use jwt::{JwtSecretManager, ENV_JWT_SECRET};
pub use key_manager::{load_or_generate_node_keypair, node_secrets_key_path, KeyManager};
pub use node_effects::NodeSideEffects;
pub use provider::{EnvScopeProvider, SecretsProvider, SecretsResolver, SecretsStore};
pub use raft_sm::SecretsState;
pub use raft_store::{RaftSecretsHandle, RaftSecretsStore};
pub use sealed::{RecipientPrivateKey, RecipientPublicKey, SealedError, SealedSecret};
pub use types::{RotationResult, Secret, SecretMetadata, SecretRef, SecretScope};
pub use worker_bootstrap::{
    issue_worker_bootstrap_token, verify_worker_bootstrap_token, WorkerBootstrapClaims,
    WorkerBootstrapToken,
};
pub use worker_ca::{
    WorkerCa, DEFAULT_CA_VALIDITY_YEARS, DEFAULT_LEAF_VALIDITY_DAYS, WORKER_CA_CERT_FILE,
    WORKER_CA_KEY_FILE,
};

#[cfg(feature = "persistent")]
pub use client_keys::{ActorKind, ClientKeyStore, ClientPublicKey, PersistentClientKeyStore};

#[cfg(feature = "persistent")]
pub use persistent::PersistentSecretsStore;

#[cfg(feature = "persistent")]
pub use credentials::CredentialStore;

#[cfg(feature = "persistent")]
pub use git_credentials::{GitCredential, GitCredentialKind, GitCredentialStore};

#[cfg(feature = "persistent")]
pub use registry_credentials::{RegistryAuthType, RegistryCredential, RegistryCredentialStore};

#[cfg(feature = "vault")]
pub use vault::VaultSecretsProvider;
