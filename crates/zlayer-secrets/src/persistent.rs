//! Persistent secrets storage using redb.
//!
//! Provides encrypted local storage for secrets with two tables:
//! - `secrets`: Encrypted secret values (scope:name -> encrypted bytes)
//! - `metadata`: JSON-encoded secret metadata (scope:name -> JSON)
//!
//! # Example
//!
//! ```no_run
//! use zlayer_secrets::{EncryptionKey, PersistentSecretsStore, Secret};
//! use zlayer_secrets::{SecretsProvider, SecretsStore};
//!
//! # async fn example() -> zlayer_secrets::Result<()> {
//! let key = EncryptionKey::generate();
//! let store = PersistentSecretsStore::open("/var/lib/zlayer/secrets", key)?;
//!
//! // Store a secret
//! let secret = Secret::new("my-password");
//! store.set_secret("deployment/myapp", "db-password", &secret).await?;
//!
//! // Retrieve a secret
//! let retrieved = store.get_secret("deployment/myapp", "db-password").await?;
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use redb::{Database, ReadableTable, TableDefinition};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::{
    EncryptionKey, Result, Secret, SecretMetadata, SecretsError, SecretsProvider, SecretsStore,
};

/// Table for encrypted secret values: key -> encrypted bytes.
const SECRETS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("secrets");

/// Table for secret metadata: key -> JSON-encoded `SecretMetadata`.
const METADATA_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("metadata");

/// Default database filename when a directory is provided.
const DEFAULT_DB_FILENAME: &str = "secrets.redb";

/// Persistent secrets store backed by redb with encryption.
///
/// Secrets are encrypted using XChaCha20-Poly1305 before storage.
/// Metadata is stored as JSON for inspection and auditing.
///
/// The store uses `tokio::sync::RwLock` to allow concurrent reads while
/// ensuring exclusive access during writes.
pub struct PersistentSecretsStore {
    db: Arc<RwLock<Database>>,
    key: EncryptionKey,
}

impl PersistentSecretsStore {
    /// Opens or creates a persistent secrets store at the given path.
    ///
    /// If `path` is a directory, the database file will be created as
    /// `secrets.redb` inside that directory. If `path` is a file path,
    /// it will be used directly.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the database file or directory
    /// * `key` - Encryption key for encrypting/decrypting secrets
    ///
    /// # Errors
    ///
    /// Returns `SecretsError::Storage` if:
    /// - The database cannot be created or opened
    /// - Table initialization fails
    pub fn open(path: impl AsRef<Path>, key: EncryptionKey) -> Result<Self> {
        let path = path.as_ref();

        // If the path is an existing directory, append the default database filename
        let db_path = if path.is_dir() {
            path.join(DEFAULT_DB_FILENAME)
        } else {
            path.to_path_buf()
        };

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| SecretsError::Storage(format!("Failed to create directory: {e}")))?;
        }

        let db = Database::create(&db_path).map_err(|e| {
            SecretsError::Storage(format!(
                "Failed to open database at {}: {e}",
                db_path.display()
            ))
        })?;

        // Initialize tables
        let write_txn = db.begin_write().map_err(|e| {
            SecretsError::Storage(format!("Failed to begin write transaction: {e}"))
        })?;

        {
            let _ = write_txn
                .open_table(SECRETS_TABLE)
                .map_err(|e| SecretsError::Storage(format!("Failed to open secrets table: {e}")))?;
            let _ = write_txn.open_table(METADATA_TABLE).map_err(|e| {
                SecretsError::Storage(format!("Failed to open metadata table: {e}"))
            })?;
        }

        write_txn
            .commit()
            .map_err(|e| SecretsError::Storage(format!("Failed to commit: {e}")))?;

        info!("Opened persistent secrets store at {}", db_path.display());

        Ok(Self {
            db: Arc::new(RwLock::new(db)),
            key,
        })
    }

    /// Constructs a storage key from scope and name.
    ///
    /// Format: `{scope}:{name}`
    #[inline]
    fn make_key(scope: &str, name: &str) -> String {
        format!("{scope}:{name}")
    }
}

#[async_trait]
impl SecretsProvider for PersistentSecretsStore {
    async fn get_secret(&self, scope: &str, name: &str) -> Result<Secret> {
        let storage_key = Self::make_key(scope, name);

        let db = self.db.read().await;

        let read_txn = db
            .begin_read()
            .map_err(|e| SecretsError::Storage(format!("Failed to begin read transaction: {e}")))?;

        let table = read_txn
            .open_table(SECRETS_TABLE)
            .map_err(|e| SecretsError::Storage(format!("Failed to open secrets table: {e}")))?;

        match table.get(storage_key.as_str()) {
            Ok(Some(encrypted_value)) => {
                let decrypted = self.key.decrypt(encrypted_value.value())?;

                let value = String::from_utf8(decrypted)
                    .map_err(|e| SecretsError::Decryption(format!("Invalid UTF-8: {e}")))?;

                debug!("Retrieved secret: {}", storage_key);
                Ok(Secret::new(value))
            }
            Ok(None) => Err(SecretsError::NotFound {
                name: name.to_string(),
            }),
            Err(e) => Err(SecretsError::Storage(format!("Failed to get secret: {e}"))),
        }
    }

    async fn get_secrets(&self, scope: &str, names: &[&str]) -> Result<HashMap<String, Secret>> {
        let mut results = HashMap::with_capacity(names.len());

        for name in names {
            // For batch retrieval, we silently skip missing secrets
            // rather than returning an error
            if let Ok(secret) = self.get_secret(scope, name).await {
                results.insert((*name).to_string(), secret);
            }
        }

        Ok(results)
    }

    async fn list_secrets(&self, scope: &str) -> Result<Vec<SecretMetadata>> {
        let prefix = format!("{scope}:");

        let db = self.db.read().await;

        let read_txn = db
            .begin_read()
            .map_err(|e| SecretsError::Storage(format!("Failed to begin read transaction: {e}")))?;

        let table = read_txn
            .open_table(METADATA_TABLE)
            .map_err(|e| SecretsError::Storage(format!("Failed to open metadata table: {e}")))?;

        let mut results = Vec::new();

        for item in table
            .iter()
            .map_err(|e| SecretsError::Storage(format!("Failed to iterate: {e}")))?
        {
            let (key, value) =
                item.map_err(|e| SecretsError::Storage(format!("Failed to read entry: {e}")))?;
            let key_str = key.value();

            if key_str.starts_with(&prefix) {
                let metadata: SecretMetadata = serde_json::from_slice(value.value())
                    .map_err(|e| SecretsError::Storage(format!("Failed to parse metadata: {e}")))?;
                results.push(metadata);
            }
        }

        // Sort by name for consistent ordering
        results.sort_by(|a, b| a.name.cmp(&b.name));

        debug!("Listed {} secrets in scope: {}", results.len(), scope);
        Ok(results)
    }

    async fn exists(&self, scope: &str, name: &str) -> Result<bool> {
        let storage_key = Self::make_key(scope, name);

        let db = self.db.read().await;

        let read_txn = db
            .begin_read()
            .map_err(|e| SecretsError::Storage(format!("Failed to begin read transaction: {e}")))?;

        let table = read_txn
            .open_table(SECRETS_TABLE)
            .map_err(|e| SecretsError::Storage(format!("Failed to open secrets table: {e}")))?;

        match table.get(storage_key.as_str()) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(SecretsError::Storage(format!(
                "Failed to check existence: {e}"
            ))),
        }
    }
}

#[async_trait]
impl SecretsStore for PersistentSecretsStore {
    async fn set_secret(&self, scope: &str, name: &str, value: &Secret) -> Result<()> {
        let storage_key = Self::make_key(scope, name);

        // Encrypt the secret value
        let encrypted = self.key.encrypt(value.expose().as_bytes())?;

        let db = self.db.write().await;

        // Get or create metadata
        let metadata = {
            let read_txn = db.begin_read().map_err(|e| {
                SecretsError::Storage(format!("Failed to begin read transaction: {e}"))
            })?;

            let table = read_txn.open_table(METADATA_TABLE).map_err(|e| {
                SecretsError::Storage(format!("Failed to open metadata table: {e}"))
            })?;

            match table.get(storage_key.as_str()) {
                Ok(Some(existing)) => {
                    let mut meta: SecretMetadata = serde_json::from_slice(existing.value())
                        .map_err(|e| {
                            SecretsError::Storage(format!("Failed to parse metadata: {e}"))
                        })?;
                    meta.update();
                    meta
                }
                Ok(None) => SecretMetadata::new(name),
                Err(e) => {
                    return Err(SecretsError::Storage(format!(
                        "Failed to get metadata: {e}"
                    )))
                }
            }
        };

        let metadata_bytes = serde_json::to_vec(&metadata)
            .map_err(|e| SecretsError::Storage(format!("Failed to serialize metadata: {e}")))?;

        // Write both secret and metadata
        let write_txn = db.begin_write().map_err(|e| {
            SecretsError::Storage(format!("Failed to begin write transaction: {e}"))
        })?;

        {
            let mut secrets_table = write_txn
                .open_table(SECRETS_TABLE)
                .map_err(|e| SecretsError::Storage(format!("Failed to open secrets table: {e}")))?;

            secrets_table
                .insert(storage_key.as_str(), encrypted.as_slice())
                .map_err(|e| SecretsError::Storage(format!("Failed to insert secret: {e}")))?;

            let mut metadata_table = write_txn.open_table(METADATA_TABLE).map_err(|e| {
                SecretsError::Storage(format!("Failed to open metadata table: {e}"))
            })?;

            metadata_table
                .insert(storage_key.as_str(), metadata_bytes.as_slice())
                .map_err(|e| SecretsError::Storage(format!("Failed to insert metadata: {e}")))?;
        }

        write_txn
            .commit()
            .map_err(|e| SecretsError::Storage(format!("Failed to commit: {e}")))?;

        debug!(
            "Stored secret: {} (version {})",
            storage_key, metadata.version
        );
        Ok(())
    }

    async fn delete_secret(&self, scope: &str, name: &str) -> Result<()> {
        let storage_key = Self::make_key(scope, name);

        let db = self.db.write().await;

        let write_txn = db.begin_write().map_err(|e| {
            SecretsError::Storage(format!("Failed to begin write transaction: {e}"))
        })?;

        {
            let mut secrets_table = write_txn
                .open_table(SECRETS_TABLE)
                .map_err(|e| SecretsError::Storage(format!("Failed to open secrets table: {e}")))?;

            let existed = secrets_table
                .remove(storage_key.as_str())
                .map_err(|e| SecretsError::Storage(format!("Failed to remove secret: {e}")))?
                .is_some();

            if !existed {
                return Err(SecretsError::NotFound {
                    name: name.to_string(),
                });
            }

            let mut metadata_table = write_txn.open_table(METADATA_TABLE).map_err(|e| {
                SecretsError::Storage(format!("Failed to open metadata table: {e}"))
            })?;

            let _ = metadata_table
                .remove(storage_key.as_str())
                .map_err(|e| SecretsError::Storage(format!("Failed to remove metadata: {e}")))?;
        }

        write_txn
            .commit()
            .map_err(|e| SecretsError::Storage(format!("Failed to commit: {e}")))?;

        debug!("Deleted secret: {}", storage_key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_store() -> (PersistentSecretsStore, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_secrets.redb");
        let key = EncryptionKey::generate();
        let store = PersistentSecretsStore::open(&db_path, key).unwrap();
        (store, temp_dir)
    }

    #[tokio::test]
    async fn test_set_and_get_secret() {
        let (store, _temp) = create_test_store();

        let secret = Secret::new("super-secret-value");
        store
            .set_secret("deployment/myapp", "db-password", &secret)
            .await
            .unwrap();

        let retrieved = store
            .get_secret("deployment/myapp", "db-password")
            .await
            .unwrap();
        assert_eq!(retrieved.expose(), "super-secret-value");
    }

    #[tokio::test]
    async fn test_get_nonexistent_secret() {
        let (store, _temp) = create_test_store();

        let result = store.get_secret("deployment/myapp", "nonexistent").await;
        assert!(matches!(result, Err(SecretsError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_exists() {
        let (store, _temp) = create_test_store();

        assert!(!store.exists("scope", "name").await.unwrap());

        let secret = Secret::new("value");
        store.set_secret("scope", "name", &secret).await.unwrap();

        assert!(store.exists("scope", "name").await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_secret() {
        let (store, _temp) = create_test_store();

        let secret = Secret::new("to-be-deleted");
        store
            .set_secret("scope", "deleteme", &secret)
            .await
            .unwrap();
        assert!(store.exists("scope", "deleteme").await.unwrap());

        store.delete_secret("scope", "deleteme").await.unwrap();
        assert!(!store.exists("scope", "deleteme").await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let (store, _temp) = create_test_store();

        let result = store.delete_secret("scope", "nonexistent").await;
        assert!(matches!(result, Err(SecretsError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_list_secrets() {
        let (store, _temp) = create_test_store();

        // Add secrets to different scopes
        store
            .set_secret("scope1", "secret-a", &Secret::new("a"))
            .await
            .unwrap();
        store
            .set_secret("scope1", "secret-b", &Secret::new("b"))
            .await
            .unwrap();
        store
            .set_secret("scope2", "secret-c", &Secret::new("c"))
            .await
            .unwrap();

        // List scope1 - should only see 2 secrets
        let list = store.list_secrets("scope1").await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "secret-a");
        assert_eq!(list[1].name, "secret-b");

        // List scope2 - should only see 1 secret
        let list = store.list_secrets("scope2").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "secret-c");
    }

    #[tokio::test]
    async fn test_get_secrets_batch() {
        let (store, _temp) = create_test_store();

        store
            .set_secret("scope", "a", &Secret::new("value-a"))
            .await
            .unwrap();
        store
            .set_secret("scope", "b", &Secret::new("value-b"))
            .await
            .unwrap();
        store
            .set_secret("scope", "c", &Secret::new("value-c"))
            .await
            .unwrap();

        let results = store
            .get_secrets("scope", &["a", "c", "nonexistent"])
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        assert_eq!(results.get("a").unwrap().expose(), "value-a");
        assert_eq!(results.get("c").unwrap().expose(), "value-c");
        assert!(!results.contains_key("nonexistent"));
    }

    #[tokio::test]
    async fn test_update_increments_version() {
        let (store, _temp) = create_test_store();

        store
            .set_secret("scope", "versioned", &Secret::new("v1"))
            .await
            .unwrap();

        let list = store.list_secrets("scope").await.unwrap();
        assert_eq!(list[0].version, 1);

        store
            .set_secret("scope", "versioned", &Secret::new("v2"))
            .await
            .unwrap();

        let list = store.list_secrets("scope").await.unwrap();
        assert_eq!(list[0].version, 2);
    }

    #[tokio::test]
    async fn test_persistence() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("persist_test.redb");

        // Use fixed key for persistence test
        let key_bytes = [42u8; 32];
        let key = EncryptionKey::from_bytes(&key_bytes).unwrap();

        // Write data
        {
            let store = PersistentSecretsStore::open(&db_path, key.clone()).unwrap();
            store
                .set_secret("scope", "persistent", &Secret::new("persistent-value"))
                .await
                .unwrap();
        }

        // Reopen and verify
        {
            let store = PersistentSecretsStore::open(&db_path, key).unwrap();
            let secret = store.get_secret("scope", "persistent").await.unwrap();
            assert_eq!(secret.expose(), "persistent-value");
        }
    }

    #[tokio::test]
    async fn test_wrong_key_fails_decryption() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("wrong_key_test.redb");

        // Write with one key
        let key1 = EncryptionKey::generate();
        {
            let store = PersistentSecretsStore::open(&db_path, key1).unwrap();
            store
                .set_secret("scope", "secret", &Secret::new("value"))
                .await
                .unwrap();
        }

        // Try to read with different key
        let key2 = EncryptionKey::generate();
        {
            let store = PersistentSecretsStore::open(&db_path, key2).unwrap();
            let result = store.get_secret("scope", "secret").await;
            assert!(result.is_err()); // Should fail decryption
        }
    }

    #[tokio::test]
    async fn test_open_with_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let key = EncryptionKey::generate();

        // Pass directory path instead of file path
        let store = PersistentSecretsStore::open(temp_dir.path(), key).unwrap();

        store
            .set_secret("scope", "test", &Secret::new("value"))
            .await
            .unwrap();

        // Verify database file was created
        let expected_path = temp_dir.path().join(DEFAULT_DB_FILENAME);
        assert!(expected_path.exists());
    }

    #[test]
    fn test_make_key() {
        assert_eq!(
            PersistentSecretsStore::make_key("scope", "name"),
            "scope:name"
        );
        assert_eq!(
            PersistentSecretsStore::make_key("deployment/app", "secret"),
            "deployment/app:secret"
        );
    }

    #[tokio::test]
    async fn test_empty_secret() {
        let (store, _temp) = create_test_store();

        let secret = Secret::new("");
        store.set_secret("scope", "empty", &secret).await.unwrap();

        let retrieved = store.get_secret("scope", "empty").await.unwrap();
        assert_eq!(retrieved.expose(), "");
    }

    #[tokio::test]
    async fn test_unicode_secret() {
        let (store, _temp) = create_test_store();

        let secret = Secret::new("hello world");
        store.set_secret("scope", "unicode", &secret).await.unwrap();

        let retrieved = store.get_secret("scope", "unicode").await.unwrap();
        assert_eq!(retrieved.expose(), "hello world");
    }

    #[tokio::test]
    async fn test_large_secret() {
        let (store, _temp) = create_test_store();

        // 1MB secret
        let large_value: String = "x".repeat(1024 * 1024);
        let secret = Secret::new(&large_value);
        store.set_secret("scope", "large", &secret).await.unwrap();

        let retrieved = store.get_secret("scope", "large").await.unwrap();
        assert_eq!(retrieved.expose().len(), 1024 * 1024);
    }
}
