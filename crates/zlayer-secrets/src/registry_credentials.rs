//! Typed credential store for Docker/OCI registry authentication.
//!
//! Built on top of any [`SecretsStore`] implementation, this module provides
//! structured storage for registry credentials. Metadata (registry, username,
//! auth type) is stored as JSON in the `registry_credentials_meta` scope, while
//! the actual password/token is stored as a secret in the `registry_credentials`
//! scope. Both are keyed by a UUID identifier.
//!
//! # Example
//!
//! ```rust,ignore
//! use zlayer_secrets_zql::{EncryptionKey, PersistentSecretsStore};
//! use zlayer_secrets_zql::registry_credentials::{RegistryCredentialStore, RegistryAuthType};
//!
//! # async fn example() -> zlayer_secrets_zql::Result<()> {
//! let key = EncryptionKey::generate();
//! let secrets_dir = zlayer_paths::ZLayerDirs::system_default().secrets();
//! let store = PersistentSecretsStore::open(&secrets_dir, key).await?;
//! let reg_store = RegistryCredentialStore::new(store);
//!
//! let cred = reg_store.create("ghcr.io", "ci-bot", "ghp_xxxx", RegistryAuthType::Token).await?;
//! let password = reg_store.get_password(&cred.id).await?;
//! assert_eq!(password.expose(), "ghp_xxxx");
//! # Ok(())
//! # }
//! ```

use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use uuid::Uuid;

use crate::{Result, Secret, SecretsError, SecretsStore};

/// Scope used for storing registry password/token secrets.
const REGISTRY_CRED_SCOPE: &str = "registry_credentials";

/// Scope used for storing registry credential metadata (JSON).
const REGISTRY_CRED_META_SCOPE: &str = "registry_credentials_meta";

/// Docker/OCI registry credential metadata.
///
/// The actual password/token is stored separately as a [`Secret`] in the
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

/// Store for Docker/OCI registry credentials.
///
/// Wraps a [`SecretsStore`] and organises data into two scopes:
/// - **`registry_credentials_meta`**: JSON-serialised [`RegistryCredential`]
///   metadata (everything except the password).
/// - **`registry_credentials`**: The raw password/token as an encrypted secret.
pub struct RegistryCredentialStore<S: SecretsStore> {
    store: S,
}

impl<S: SecretsStore> std::fmt::Debug for RegistryCredentialStore<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryCredentialStore")
            .field("store", &"<secrets store>")
            .finish()
    }
}

impl<S: SecretsStore> RegistryCredentialStore<S> {
    /// Create a new registry credential store backed by the provided secrets store.
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Store a new registry credential.
    ///
    /// Generates a UUID for the credential, stores the password as an encrypted
    /// secret in the `registry_credentials` scope, and the metadata as JSON in
    /// the `registry_credentials_meta` scope.
    ///
    /// # Arguments
    /// * `registry` - Registry hostname (e.g. `"docker.io"`)
    /// * `username` - Username for authentication
    /// * `password` - Password or token value (stored encrypted)
    /// * `auth_type` - Authentication method
    ///
    /// # Errors
    /// Returns a [`SecretsError`] if serialisation or storage fails.
    pub async fn create(
        &self,
        registry: &str,
        username: &str,
        password: &str,
        auth_type: RegistryAuthType,
    ) -> Result<RegistryCredential> {
        let id = Uuid::new_v4().to_string();

        let cred = RegistryCredential {
            id: id.clone(),
            registry: registry.to_string(),
            username: username.to_string(),
            auth_type,
        };

        // Store metadata as JSON.
        let meta_json = serde_json::to_string(&cred)
            .map_err(|e| SecretsError::Storage(format!("failed to serialise credential: {e}")))?;
        self.store
            .set_secret(REGISTRY_CRED_META_SCOPE, &id, &Secret::new(meta_json))
            .await?;

        // Store the password/token as an encrypted secret.
        self.store
            .set_secret(REGISTRY_CRED_SCOPE, &id, &Secret::new(password))
            .await?;

        info!(id = %id, registry = %registry, username = %username, "Created registry credential");
        Ok(cred)
    }

    /// Retrieve credential metadata (without the password).
    ///
    /// Returns `None` if no credential with the given `id` exists.
    ///
    /// # Errors
    /// Returns a [`SecretsError`] on storage/decryption errors.
    pub async fn get(&self, id: &str) -> Result<Option<RegistryCredential>> {
        let secret = match self.store.get_secret(REGISTRY_CRED_META_SCOPE, id).await {
            Ok(s) => s,
            Err(SecretsError::NotFound { .. }) => {
                debug!(id = %id, "Registry credential not found");
                return Ok(None);
            }
            Err(e) => return Err(e),
        };

        let cred: RegistryCredential = serde_json::from_str(secret.expose()).map_err(|e| {
            SecretsError::Storage(format!("corrupt registry credential '{id}': {e}"))
        })?;

        Ok(Some(cred))
    }

    /// Retrieve the password/token for a registry credential.
    ///
    /// # Errors
    /// Returns [`SecretsError::NotFound`] if the credential does not exist.
    pub async fn get_password(&self, id: &str) -> Result<Secret> {
        self.store.get_secret(REGISTRY_CRED_SCOPE, id).await
    }

    /// List all registry credentials (metadata only, no passwords).
    ///
    /// # Errors
    /// Returns a [`SecretsError`] on storage/decryption errors.
    pub async fn list(&self) -> Result<Vec<RegistryCredential>> {
        let metas = self.store.list_secrets(REGISTRY_CRED_META_SCOPE).await?;

        let mut creds = Vec::with_capacity(metas.len());
        for meta in metas {
            if let Some(cred) = self.get(&meta.name).await? {
                creds.push(cred);
            }
        }

        Ok(creds)
    }

    /// Delete a registry credential and its associated secret.
    ///
    /// Both the metadata and the password secret are removed.
    ///
    /// # Errors
    /// Returns [`SecretsError::NotFound`] if the credential does not exist.
    pub async fn delete(&self, id: &str) -> Result<()> {
        // Delete metadata first; if it doesn't exist, the whole credential is missing.
        self.store
            .delete_secret(REGISTRY_CRED_META_SCOPE, id)
            .await?;

        // Delete the password secret. Ignore NotFound here in case only metadata
        // existed (defensive).
        match self.store.delete_secret(REGISTRY_CRED_SCOPE, id).await {
            Ok(()) | Err(SecretsError::NotFound { .. }) => {}
            Err(e) => return Err(e),
        }

        info!(id = %id, "Deleted registry credential");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EncryptionKey, PersistentSecretsStore};

    async fn create_test_store() -> (PersistentSecretsStore, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_registry_creds.sqlite");
        let key = EncryptionKey::generate();
        let store = PersistentSecretsStore::open(&db_path, key).await.unwrap();
        (store, temp_dir)
    }

    #[tokio::test]
    async fn test_create_and_get() {
        let (store, _temp) = create_test_store().await;
        let reg_store = RegistryCredentialStore::new(store);

        let cred = reg_store
            .create("ghcr.io", "ci-bot", "ghp_xxxx", RegistryAuthType::Token)
            .await
            .unwrap();

        assert_eq!(cred.registry, "ghcr.io");
        assert_eq!(cred.username, "ci-bot");
        assert_eq!(cred.auth_type, RegistryAuthType::Token);
        assert!(!cred.id.is_empty());

        let retrieved = reg_store.get(&cred.id).await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, cred.id);
        assert_eq!(retrieved.registry, "ghcr.io");
        assert_eq!(retrieved.username, "ci-bot");
        assert_eq!(retrieved.auth_type, RegistryAuthType::Token);
    }

    #[tokio::test]
    async fn test_get_password() {
        let (store, _temp) = create_test_store().await;
        let reg_store = RegistryCredentialStore::new(store);

        let cred = reg_store
            .create("docker.io", "user", "s3cret!", RegistryAuthType::Basic)
            .await
            .unwrap();

        let password = reg_store.get_password(&cred.id).await.unwrap();
        assert_eq!(password.expose(), "s3cret!");
    }

    #[tokio::test]
    async fn test_list() {
        let (store, _temp) = create_test_store().await;
        let reg_store = RegistryCredentialStore::new(store);

        reg_store
            .create("docker.io", "user1", "pw1", RegistryAuthType::Basic)
            .await
            .unwrap();
        reg_store
            .create("ghcr.io", "user2", "pw2", RegistryAuthType::Token)
            .await
            .unwrap();

        let list = reg_store.list().await.unwrap();
        assert_eq!(list.len(), 2);

        let registries: Vec<&str> = list.iter().map(|c| c.registry.as_str()).collect();
        assert!(registries.contains(&"docker.io"));
        assert!(registries.contains(&"ghcr.io"));
    }

    #[tokio::test]
    async fn test_delete() {
        let (store, _temp) = create_test_store().await;
        let reg_store = RegistryCredentialStore::new(store);

        let cred = reg_store
            .create("docker.io", "user", "pw", RegistryAuthType::Basic)
            .await
            .unwrap();

        reg_store.delete(&cred.id).await.unwrap();

        assert!(reg_store.get(&cred.id).await.unwrap().is_none());
        assert!(reg_store.get_password(&cred.id).await.is_err());
    }

    #[tokio::test]
    async fn test_create_overwrites() {
        let (store, _temp) = create_test_store().await;
        let reg_store = RegistryCredentialStore::new(store);

        let cred1 = reg_store
            .create("docker.io", "user", "pw1", RegistryAuthType::Basic)
            .await
            .unwrap();

        // Create a second credential -- gets its own id, so it's a separate entry,
        // not an overwrite by design. But if we manually set the same scope+key
        // it would overwrite. This test verifies two creates produce two entries.
        let cred2 = reg_store
            .create("docker.io", "user", "pw2", RegistryAuthType::Basic)
            .await
            .unwrap();

        assert_ne!(cred1.id, cred2.id);

        let pw1 = reg_store.get_password(&cred1.id).await.unwrap();
        let pw2 = reg_store.get_password(&cred2.id).await.unwrap();
        assert_eq!(pw1.expose(), "pw1");
        assert_eq!(pw2.expose(), "pw2");
    }

    #[tokio::test]
    async fn test_get_nonexistent_returns_none() {
        let (store, _temp) = create_test_store().await;
        let reg_store = RegistryCredentialStore::new(store);

        let result = reg_store.get("nonexistent-id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_password_nonexistent_returns_error() {
        let (store, _temp) = create_test_store().await;
        let reg_store = RegistryCredentialStore::new(store);

        let result = reg_store.get_password("nonexistent-id").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SecretsError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_delete_nonexistent_returns_error() {
        let (store, _temp) = create_test_store().await;
        let reg_store = RegistryCredentialStore::new(store);

        let result = reg_store.delete("nonexistent-id").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SecretsError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_serde_auth_type_roundtrip() {
        let basic = serde_json::to_string(&RegistryAuthType::Basic).unwrap();
        assert_eq!(basic, "\"basic\"");

        let token = serde_json::to_string(&RegistryAuthType::Token).unwrap();
        assert_eq!(token, "\"token\"");

        let parsed: RegistryAuthType = serde_json::from_str(&basic).unwrap();
        assert_eq!(parsed, RegistryAuthType::Basic);

        let parsed: RegistryAuthType = serde_json::from_str(&token).unwrap();
        assert_eq!(parsed, RegistryAuthType::Token);
    }
}
