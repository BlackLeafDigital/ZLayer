//! Typed credential store for Git authentication (PAT or SSH key).
//!
//! Built on top of any [`SecretsStore`] implementation, this module provides
//! structured storage for Git credentials. Metadata (name, kind) is stored as
//! JSON in the `git_credentials_meta` scope, while the actual PAT or SSH key
//! is stored as a secret in the `git_credentials` scope. Both are keyed by a
//! UUID identifier.
//!
//! # Example
//!
//! ```rust,ignore
//! use zlayer_secrets_zql::{EncryptionKey, PersistentSecretsStore};
//! use zlayer_secrets_zql::git_credentials::{GitCredentialStore, GitCredentialKind};
//!
//! # async fn example() -> zlayer_secrets_zql::Result<()> {
//! let key = EncryptionKey::generate();
//! let secrets_dir = zlayer_paths::ZLayerDirs::system_default().secrets();
//! let store = PersistentSecretsStore::open(&secrets_dir, key).await?;
//! let git_store = GitCredentialStore::new(store);
//!
//! let cred = git_store.create("GitHub PAT for ci", "ghp_xxxx", GitCredentialKind::Pat).await?;
//! let value = git_store.get_value(&cred.id).await?;
//! assert_eq!(value.expose(), "ghp_xxxx");
//! # Ok(())
//! # }
//! ```

use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use uuid::Uuid;

use crate::{Result, Secret, SecretsError, SecretsStore};

/// Scope used for storing Git credential secrets (PAT / SSH key).
const GIT_CRED_SCOPE: &str = "git_credentials";

/// Scope used for storing Git credential metadata (JSON).
const GIT_CRED_META_SCOPE: &str = "git_credentials_meta";

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

/// Store for Git authentication credentials.
///
/// Wraps a [`SecretsStore`] and organises data into two scopes:
/// - **`git_credentials_meta`**: JSON-serialised [`GitCredential`] metadata
///   (everything except the secret value).
/// - **`git_credentials`**: The raw PAT or SSH key as an encrypted secret.
pub struct GitCredentialStore<S: SecretsStore> {
    store: S,
}

impl<S: SecretsStore> std::fmt::Debug for GitCredentialStore<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitCredentialStore")
            .field("store", &"<secrets store>")
            .finish()
    }
}

impl<S: SecretsStore> GitCredentialStore<S> {
    /// Create a new Git credential store backed by the provided secrets store.
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Store a new Git credential.
    ///
    /// Generates a UUID for the credential, stores the PAT/SSH key as an
    /// encrypted secret in the `git_credentials` scope, and the metadata as
    /// JSON in the `git_credentials_meta` scope.
    ///
    /// # Arguments
    /// * `name` - Human-readable label (e.g. `"GitHub PAT for ci"`)
    /// * `value` - The PAT or SSH key content (stored encrypted)
    /// * `kind` - Whether this is a PAT or SSH key
    ///
    /// # Errors
    /// Returns a [`SecretsError`] if serialisation or storage fails.
    pub async fn create(
        &self,
        name: &str,
        value: &str,
        kind: GitCredentialKind,
    ) -> Result<GitCredential> {
        let id = Uuid::new_v4().to_string();

        let cred = GitCredential {
            id: id.clone(),
            name: name.to_string(),
            kind,
        };

        // Store metadata as JSON.
        let meta_json = serde_json::to_string(&cred)
            .map_err(|e| SecretsError::Storage(format!("failed to serialise credential: {e}")))?;
        self.store
            .set_secret(GIT_CRED_META_SCOPE, &id, &Secret::new(meta_json))
            .await?;

        // Store the PAT / SSH key as an encrypted secret.
        self.store
            .set_secret(GIT_CRED_SCOPE, &id, &Secret::new(value))
            .await?;

        info!(id = %id, name = %name, kind = ?kind, "Created git credential");
        Ok(cred)
    }

    /// Retrieve credential metadata (without the secret value).
    ///
    /// Returns `None` if no credential with the given `id` exists.
    ///
    /// # Errors
    /// Returns a [`SecretsError`] on storage/decryption errors.
    pub async fn get(&self, id: &str) -> Result<Option<GitCredential>> {
        let secret = match self.store.get_secret(GIT_CRED_META_SCOPE, id).await {
            Ok(s) => s,
            Err(SecretsError::NotFound { .. }) => {
                debug!(id = %id, "Git credential not found");
                return Ok(None);
            }
            Err(e) => return Err(e),
        };

        let cred: GitCredential = serde_json::from_str(secret.expose())
            .map_err(|e| SecretsError::Storage(format!("corrupt git credential '{id}': {e}")))?;

        Ok(Some(cred))
    }

    /// Retrieve the PAT or SSH key for a Git credential.
    ///
    /// # Errors
    /// Returns [`SecretsError::NotFound`] if the credential does not exist.
    pub async fn get_value(&self, id: &str) -> Result<Secret> {
        self.store.get_secret(GIT_CRED_SCOPE, id).await
    }

    /// List all Git credentials (metadata only, no secret values).
    ///
    /// # Errors
    /// Returns a [`SecretsError`] on storage/decryption errors.
    pub async fn list(&self) -> Result<Vec<GitCredential>> {
        let metas = self.store.list_secrets(GIT_CRED_META_SCOPE).await?;

        let mut creds = Vec::with_capacity(metas.len());
        for meta in metas {
            if let Some(cred) = self.get(&meta.name).await? {
                creds.push(cred);
            }
        }

        Ok(creds)
    }

    /// Delete a Git credential and its associated secret.
    ///
    /// Both the metadata and the secret value are removed.
    ///
    /// # Errors
    /// Returns [`SecretsError::NotFound`] if the credential does not exist.
    pub async fn delete(&self, id: &str) -> Result<()> {
        // Delete metadata first; if it doesn't exist, the whole credential is missing.
        self.store.delete_secret(GIT_CRED_META_SCOPE, id).await?;

        // Delete the secret value. Ignore NotFound here in case only metadata
        // existed (defensive).
        match self.store.delete_secret(GIT_CRED_SCOPE, id).await {
            Ok(()) | Err(SecretsError::NotFound { .. }) => {}
            Err(e) => return Err(e),
        }

        info!(id = %id, "Deleted git credential");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EncryptionKey, PersistentSecretsStore};

    async fn create_test_store() -> (PersistentSecretsStore, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_git_creds.sqlite");
        let key = EncryptionKey::generate();
        let store = PersistentSecretsStore::open(&db_path, key).await.unwrap();
        (store, temp_dir)
    }

    #[tokio::test]
    async fn test_create_and_get() {
        let (store, _temp) = create_test_store().await;
        let git_store = GitCredentialStore::new(store);

        let cred = git_store
            .create("GitHub PAT for ci", "ghp_xxxx", GitCredentialKind::Pat)
            .await
            .unwrap();

        assert_eq!(cred.name, "GitHub PAT for ci");
        assert_eq!(cred.kind, GitCredentialKind::Pat);
        assert!(!cred.id.is_empty());

        let retrieved = git_store.get(&cred.id).await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, cred.id);
        assert_eq!(retrieved.name, "GitHub PAT for ci");
        assert_eq!(retrieved.kind, GitCredentialKind::Pat);
    }

    #[tokio::test]
    async fn test_get_value() {
        let (store, _temp) = create_test_store().await;
        let git_store = GitCredentialStore::new(store);

        let cred = git_store
            .create(
                "My SSH key",
                "-----BEGIN OPENSSH PRIVATE KEY-----\n...",
                GitCredentialKind::SshKey,
            )
            .await
            .unwrap();

        let value = git_store.get_value(&cred.id).await.unwrap();
        assert_eq!(value.expose(), "-----BEGIN OPENSSH PRIVATE KEY-----\n...");
    }

    #[tokio::test]
    async fn test_list() {
        let (store, _temp) = create_test_store().await;
        let git_store = GitCredentialStore::new(store);

        git_store
            .create("PAT 1", "token1", GitCredentialKind::Pat)
            .await
            .unwrap();
        git_store
            .create("SSH key 1", "key1", GitCredentialKind::SshKey)
            .await
            .unwrap();

        let list = git_store.list().await.unwrap();
        assert_eq!(list.len(), 2);

        let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"PAT 1"));
        assert!(names.contains(&"SSH key 1"));
    }

    #[tokio::test]
    async fn test_delete() {
        let (store, _temp) = create_test_store().await;
        let git_store = GitCredentialStore::new(store);

        let cred = git_store
            .create("To delete", "token", GitCredentialKind::Pat)
            .await
            .unwrap();

        git_store.delete(&cred.id).await.unwrap();

        assert!(git_store.get(&cred.id).await.unwrap().is_none());
        assert!(git_store.get_value(&cred.id).await.is_err());
    }

    #[tokio::test]
    async fn test_create_multiple_same_name() {
        let (store, _temp) = create_test_store().await;
        let git_store = GitCredentialStore::new(store);

        let cred1 = git_store
            .create("Same name", "val1", GitCredentialKind::Pat)
            .await
            .unwrap();
        let cred2 = git_store
            .create("Same name", "val2", GitCredentialKind::Pat)
            .await
            .unwrap();

        // Different UUIDs, both accessible.
        assert_ne!(cred1.id, cred2.id);

        let v1 = git_store.get_value(&cred1.id).await.unwrap();
        let v2 = git_store.get_value(&cred2.id).await.unwrap();
        assert_eq!(v1.expose(), "val1");
        assert_eq!(v2.expose(), "val2");
    }

    #[tokio::test]
    async fn test_get_nonexistent_returns_none() {
        let (store, _temp) = create_test_store().await;
        let git_store = GitCredentialStore::new(store);

        let result = git_store.get("nonexistent-id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_value_nonexistent_returns_error() {
        let (store, _temp) = create_test_store().await;
        let git_store = GitCredentialStore::new(store);

        let result = git_store.get_value("nonexistent-id").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SecretsError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_delete_nonexistent_returns_error() {
        let (store, _temp) = create_test_store().await;
        let git_store = GitCredentialStore::new(store);

        let result = git_store.delete("nonexistent-id").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SecretsError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_serde_credential_kind_roundtrip() {
        let pat = serde_json::to_string(&GitCredentialKind::Pat).unwrap();
        assert_eq!(pat, "\"pat\"");

        let ssh = serde_json::to_string(&GitCredentialKind::SshKey).unwrap();
        assert_eq!(ssh, "\"ssh_key\"");

        let parsed: GitCredentialKind = serde_json::from_str(&pat).unwrap();
        assert_eq!(parsed, GitCredentialKind::Pat);

        let parsed: GitCredentialKind = serde_json::from_str(&ssh).unwrap();
        assert_eq!(parsed, GitCredentialKind::SshKey);
    }
}
