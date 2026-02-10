//! Credential store for API authentication.
//!
//! Built on top of [`PersistentSecretsStore`], this module provides API-key
//! based authentication with Argon2id password hashing.
//!
//! Credentials are stored in the `credentials` scope of the secrets store.
//! Each credential is a JSON object containing the argon2id hash of the
//! API secret and an array of roles.
//!
//! # Example
//!
//! ```rust,ignore
//! use zlayer_secrets::credentials::CredentialStore;
//! use zlayer_secrets::{EncryptionKey, PersistentSecretsStore};
//!
//! # async fn example() -> zlayer_secrets::Result<()> {
//! let key = EncryptionKey::generate();
//! let store = PersistentSecretsStore::open("/var/lib/zlayer/secrets", key).await?;
//! let cred_store = CredentialStore::new(store);
//!
//! // Create an API key
//! cred_store.create_api_key("admin", "super-secret-password", &["admin"]).await?;
//!
//! // Validate credentials
//! let roles = cred_store.validate("admin", "super-secret-password").await?;
//! assert!(roles.is_some());
//! # Ok(())
//! # }
//! ```

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::{Result, Secret, SecretsError, SecretsStore};

/// The scope used for storing API credentials in the secrets store.
const CREDENTIALS_SCOPE: &str = "credentials";

/// Stored credential record (JSON-serialised inside the encrypted secret).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCredential {
    /// Argon2id hash of the API secret / password.
    hash: String,
    /// Roles assigned to this credential (e.g. `["admin"]`).
    roles: Vec<String>,
}

/// Credential store for API key authentication.
///
/// Wraps a [`SecretsStore`] implementation and stores credentials as encrypted
/// JSON blobs keyed by API key name under the `credentials` scope.
pub struct CredentialStore<S: SecretsStore> {
    store: S,
}

impl<S: SecretsStore> std::fmt::Debug for CredentialStore<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialStore")
            .field("store", &"<secrets store>")
            .finish()
    }
}

impl<S: SecretsStore> CredentialStore<S> {
    /// Create a new credential store backed by the provided secrets store.
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Validate an API key and secret pair.
    ///
    /// Returns `Some(roles)` if the credentials are valid, `None` if invalid.
    ///
    /// # Arguments
    /// * `api_key` - The API key (used as the secret name in the store)
    /// * `api_secret` - The password/secret to verify against the stored hash
    ///
    /// # Errors
    /// Returns a `SecretsError` if there is a storage or decryption error
    /// (NOT for invalid credentials -- that returns `Ok(None)`).
    pub async fn validate(&self, api_key: &str, api_secret: &str) -> Result<Option<Vec<String>>> {
        // Look up the credential by API key
        let secret = match self.store.get_secret(CREDENTIALS_SCOPE, api_key).await {
            Ok(s) => s,
            Err(SecretsError::NotFound { .. }) => {
                debug!(api_key = %api_key, "Credential not found");
                return Ok(None);
            }
            Err(e) => return Err(e),
        };

        // Deserialise the stored credential
        let stored: StoredCredential = serde_json::from_str(secret.expose()).map_err(|e| {
            SecretsError::Storage(format!("corrupt credential record for '{api_key}': {e}"))
        })?;

        // Verify the password against the stored argon2id hash
        let parsed_hash = PasswordHash::new(&stored.hash).map_err(|e| {
            SecretsError::Storage(format!("invalid password hash for '{api_key}': {e}"))
        })?;

        let argon2 = Argon2::default();
        if argon2
            .verify_password(api_secret.as_bytes(), &parsed_hash)
            .is_ok()
        {
            debug!(api_key = %api_key, "Credential validated successfully");
            Ok(Some(stored.roles))
        } else {
            debug!(api_key = %api_key, "Invalid password");
            Ok(None)
        }
    }

    /// Create a new API key credential.
    ///
    /// The password is hashed with Argon2id before storage. If a credential
    /// with the same key already exists, it will be overwritten.
    ///
    /// # Arguments
    /// * `api_key` - The API key identifier
    /// * `password` - The password/secret to hash and store
    /// * `roles` - Roles assigned to this credential
    ///
    /// # Errors
    /// Returns a `SecretsError` if hashing or storage fails.
    pub async fn create_api_key(
        &self,
        api_key: &str,
        password: &str,
        roles: &[&str],
    ) -> Result<()> {
        // Hash the password with Argon2id
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| SecretsError::Encryption(format!("failed to hash password: {e}")))?
            .to_string();

        let credential = StoredCredential {
            hash,
            roles: roles.iter().map(|r| (*r).to_string()).collect(),
        };

        let json = serde_json::to_string(&credential)
            .map_err(|e| SecretsError::Storage(format!("failed to serialise credential: {e}")))?;

        self.store
            .set_secret(CREDENTIALS_SCOPE, api_key, &Secret::new(json))
            .await?;

        info!(api_key = %api_key, roles = ?roles, "Created API key credential");
        Ok(())
    }

    /// Delete an API key credential.
    ///
    /// # Arguments
    /// * `api_key` - The API key to delete
    ///
    /// # Errors
    /// Returns `SecretsError::NotFound` if the credential doesn't exist.
    pub async fn delete_api_key(&self, api_key: &str) -> Result<()> {
        self.store.delete_secret(CREDENTIALS_SCOPE, api_key).await?;
        info!(api_key = %api_key, "Deleted API key credential");
        Ok(())
    }

    /// Check if an API key credential exists.
    ///
    /// # Errors
    /// Returns a `SecretsError` if there is a storage or decryption error.
    pub async fn exists(&self, api_key: &str) -> Result<bool> {
        self.store.exists(CREDENTIALS_SCOPE, api_key).await
    }

    /// Ensure a default admin credential exists.
    ///
    /// If no credential with the given `api_key` exists, one is created with
    /// the provided password and `["admin"]` role. Returns the password that
    /// was set (either the provided one or the existing one if already present).
    ///
    /// # Arguments
    /// * `api_key` - The admin API key name (e.g. "admin")
    /// * `password` - The password to use if the credential doesn't exist
    ///
    /// # Returns
    /// `true` if a new credential was created, `false` if one already existed.
    ///
    /// # Errors
    /// Returns a `SecretsError` if hashing or storage fails during creation.
    pub async fn ensure_admin(&self, api_key: &str, password: &str) -> Result<bool> {
        if self.exists(api_key).await? {
            debug!(api_key = %api_key, "Admin credential already exists");
            return Ok(false);
        }

        self.create_api_key(api_key, password, &["admin"]).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EncryptionKey, PersistentSecretsStore};

    async fn create_test_store() -> (PersistentSecretsStore, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_creds.sqlite");
        let key = EncryptionKey::generate();
        let store = PersistentSecretsStore::open(&db_path, key).await.unwrap();
        (store, temp_dir)
    }

    #[tokio::test]
    async fn test_create_and_validate() {
        let (store, _temp) = create_test_store().await;
        let cred_store = CredentialStore::new(store);

        cred_store
            .create_api_key("test-key", "test-secret", &["admin", "reader"])
            .await
            .unwrap();

        // Valid credentials
        let roles = cred_store
            .validate("test-key", "test-secret")
            .await
            .unwrap();
        assert!(roles.is_some());
        let roles = roles.unwrap();
        assert!(roles.contains(&"admin".to_string()));
        assert!(roles.contains(&"reader".to_string()));
    }

    #[tokio::test]
    async fn test_validate_wrong_password() {
        let (store, _temp) = create_test_store().await;
        let cred_store = CredentialStore::new(store);

        cred_store
            .create_api_key("test-key", "correct-password", &["admin"])
            .await
            .unwrap();

        let roles = cred_store
            .validate("test-key", "wrong-password")
            .await
            .unwrap();
        assert!(roles.is_none());
    }

    #[tokio::test]
    async fn test_validate_nonexistent_key() {
        let (store, _temp) = create_test_store().await;
        let cred_store = CredentialStore::new(store);

        let roles = cred_store
            .validate("nonexistent", "password")
            .await
            .unwrap();
        assert!(roles.is_none());
    }

    #[tokio::test]
    async fn test_exists() {
        let (store, _temp) = create_test_store().await;
        let cred_store = CredentialStore::new(store);

        assert!(!cred_store.exists("test-key").await.unwrap());

        cred_store
            .create_api_key("test-key", "password", &["admin"])
            .await
            .unwrap();

        assert!(cred_store.exists("test-key").await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_api_key() {
        let (store, _temp) = create_test_store().await;
        let cred_store = CredentialStore::new(store);

        cred_store
            .create_api_key("delete-me", "password", &["admin"])
            .await
            .unwrap();
        assert!(cred_store.exists("delete-me").await.unwrap());

        cred_store.delete_api_key("delete-me").await.unwrap();
        assert!(!cred_store.exists("delete-me").await.unwrap());
    }

    #[tokio::test]
    async fn test_ensure_admin_creates() {
        let (store, _temp) = create_test_store().await;
        let cred_store = CredentialStore::new(store);

        let created = cred_store
            .ensure_admin("admin", "admin-password")
            .await
            .unwrap();
        assert!(created);

        // Should be able to validate
        let roles = cred_store
            .validate("admin", "admin-password")
            .await
            .unwrap();
        assert!(roles.is_some());
        assert!(roles.unwrap().contains(&"admin".to_string()));
    }

    #[tokio::test]
    async fn test_ensure_admin_skips_existing() {
        let (store, _temp) = create_test_store().await;
        let cred_store = CredentialStore::new(store);

        // Create first
        cred_store
            .create_api_key("admin", "original-password", &["admin"])
            .await
            .unwrap();

        // ensure_admin should not overwrite
        let created = cred_store
            .ensure_admin("admin", "new-password")
            .await
            .unwrap();
        assert!(!created);

        // Original password should still work
        let roles = cred_store
            .validate("admin", "original-password")
            .await
            .unwrap();
        assert!(roles.is_some());

        // New password should NOT work
        let roles = cred_store.validate("admin", "new-password").await.unwrap();
        assert!(roles.is_none());
    }

    #[tokio::test]
    async fn test_overwrite_credential() {
        let (store, _temp) = create_test_store().await;
        let cred_store = CredentialStore::new(store);

        cred_store
            .create_api_key("key", "password1", &["reader"])
            .await
            .unwrap();

        // Overwrite with new password and roles
        cred_store
            .create_api_key("key", "password2", &["admin"])
            .await
            .unwrap();

        // Old password should NOT work
        let roles = cred_store.validate("key", "password1").await.unwrap();
        assert!(roles.is_none());

        // New password should work with new roles
        let roles = cred_store.validate("key", "password2").await.unwrap();
        assert!(roles.is_some());
        let roles = roles.unwrap();
        assert!(roles.contains(&"admin".to_string()));
        assert!(!roles.contains(&"reader".to_string()));
    }
}
