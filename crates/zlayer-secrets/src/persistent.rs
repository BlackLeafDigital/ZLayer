//! Persistent secrets storage using ZQL.
//!
//! Provides encrypted local storage for secrets with a single store containing
//! fields: storage_key, encrypted_value_hex, name, version, created_at, updated_at.
//!
//! - `storage_key`: Primary key in `{scope}:{name}` format
//! - `encrypted_value_hex`: XChaCha20-Poly1305 encrypted secret bytes (hex-encoded)
//! - `name`, `version`, `created_at`, `updated_at`: Metadata fields
//!
//! # Example
//!
//! ```no_run
//! use zlayer_secrets::{EncryptionKey, PersistentSecretsStore, Secret};
//! use zlayer_secrets::{SecretsProvider, SecretsStore};
//!
//! # async fn example() -> zlayer_secrets::Result<()> {
//! let key = EncryptionKey::generate();
//! let store = PersistentSecretsStore::open("/var/lib/zlayer/secrets", key).await?;
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

use async_trait::async_trait;
use tracing::{debug, info};

use crate::{
    EncryptionKey, Result, Secret, SecretMetadata, SecretsError, SecretsProvider, SecretsStore,
};

/// Default database directory name when a directory is provided.
const DEFAULT_DB_DIRNAME: &str = "secrets_zql";

/// Persistent secrets store backed by ZQL with encryption.
///
/// Secrets are encrypted using XChaCha20-Poly1305 before storage.
/// Metadata is stored alongside secrets for inspection and auditing.
pub struct PersistentSecretsStore {
    db: tokio::sync::Mutex<zql::Database>,
    key: EncryptionKey,
}

impl PersistentSecretsStore {
    /// Opens or creates a persistent secrets store at the given path.
    ///
    /// If `path` is a directory, the database will be created as
    /// `secrets_zql` inside that directory. If `path` is a file path,
    /// it will be used directly.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the database directory
    /// * `key` - Encryption key for encrypting/decrypting secrets
    ///
    /// # Errors
    ///
    /// Returns `SecretsError::Storage` if:
    /// - The database cannot be created or opened
    pub async fn open(path: impl AsRef<Path>, key: EncryptionKey) -> Result<Self> {
        let path = path.as_ref();

        // If the path is an existing directory, append the default database dirname
        let db_path = if path.is_dir() {
            path.join(DEFAULT_DB_DIRNAME)
        } else {
            path.to_path_buf()
        };

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| SecretsError::Storage(format!("Failed to create directory: {e}")))?;
        }

        let db = tokio::task::spawn_blocking(move || zql::Database::open(&db_path))
            .await
            .map_err(|e| SecretsError::Storage(format!("spawn_blocking failed: {e}")))?
            .map_err(|e| SecretsError::Storage(format!("Failed to open database: {e}")))?;

        info!("Opened persistent secrets store at {}", path.display());

        Ok(Self {
            db: tokio::sync::Mutex::new(db),
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

    /// Get the current timestamp as ISO 8601 string.
    #[allow(clippy::cast_possible_wrap)]
    fn now_iso8601() -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        chrono::DateTime::from_timestamp(now as i64, 0).map_or_else(
            || "1970-01-01T00:00:00Z".to_string(),
            |dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        )
    }

    /// Parse ISO 8601 timestamp to Unix timestamp.
    fn parse_timestamp(s: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.timestamp())
            .unwrap_or(0)
    }

    /// Escape a string for safe ZQL query embedding
    fn escape_str(s: &str) -> String {
        s.replace('\'', "''")
    }

    /// Query the database for a record by storage_key
    async fn get_record(&self, storage_key: &str) -> Result<Option<HashMap<String, String>>> {
        let mut db = self.db.lock().await;
        let result = db.query(&format!(
            "SELECT * FROM secrets WHERE storage_key = '{}'",
            Self::escape_str(storage_key)
        ));

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                if records.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(records[0].fields.clone()))
                }
            }
            Ok(_) => Ok(None),
            Err(_) => Ok(None),
        }
    }
}

#[async_trait]
impl SecretsProvider for PersistentSecretsStore {
    async fn get_secret(&self, scope: &str, name: &str) -> Result<Secret> {
        let storage_key = Self::make_key(scope, name);

        let record = self.get_record(&storage_key).await?;

        match record {
            Some(fields) => {
                let encrypted_hex = fields.get("encrypted_value_hex").ok_or_else(|| {
                    SecretsError::Storage("Missing encrypted_value_hex field".to_string())
                })?;

                let encrypted_value = hex::decode(encrypted_hex)
                    .map_err(|e| SecretsError::Storage(format!("Invalid hex encoding: {e}")))?;

                let decrypted = self.key.decrypt(&encrypted_value)?;

                let value = String::from_utf8(decrypted)
                    .map_err(|e| SecretsError::Decryption(format!("Invalid UTF-8: {e}")))?;

                debug!("Retrieved secret: {}", storage_key);
                Ok(Secret::new(value))
            }
            None => Err(SecretsError::NotFound {
                name: name.to_string(),
            }),
        }
    }

    async fn get_secrets(&self, scope: &str, names: &[&str]) -> Result<HashMap<String, Secret>> {
        let mut results = HashMap::with_capacity(names.len());

        for name in names {
            if let Ok(secret) = self.get_secret(scope, name).await {
                results.insert((*name).to_string(), secret);
            }
        }

        Ok(results)
    }

    async fn list_secrets(&self, scope: &str) -> Result<Vec<SecretMetadata>> {
        let prefix = format!("{scope}:");

        let mut db = self.db.lock().await;
        let result = db.query("SELECT * FROM secrets");

        match result {
            Ok(zql::query::executor::ExecResult::Retrieved(records)) => {
                let mut results = Vec::new();

                for record in &records {
                    if let Some(sk) = record.fields.get("storage_key") {
                        if sk.starts_with(&prefix) {
                            let name = record.fields.get("name").cloned().unwrap_or_default();
                            let version = record
                                .fields
                                .get("version")
                                .and_then(|v| v.parse::<i64>().ok())
                                .unwrap_or(1);
                            let created_at = record
                                .fields
                                .get("created_at")
                                .map(|s| Self::parse_timestamp(s))
                                .unwrap_or(0);
                            let updated_at = record
                                .fields
                                .get("updated_at")
                                .map(|s| Self::parse_timestamp(s))
                                .unwrap_or(0);

                            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                            results.push(SecretMetadata {
                                name,
                                version: version as u32,
                                created_at,
                                updated_at,
                            });
                        }
                    }
                }

                results.sort_by(|a, b| a.name.cmp(&b.name));

                debug!("Listed {} secrets in scope: {}", results.len(), scope);
                Ok(results)
            }
            _ => Ok(Vec::new()),
        }
    }

    async fn exists(&self, scope: &str, name: &str) -> Result<bool> {
        let storage_key = Self::make_key(scope, name);
        let record = self.get_record(&storage_key).await?;
        Ok(record.is_some())
    }
}

#[async_trait]
impl SecretsStore for PersistentSecretsStore {
    async fn set_secret(&self, scope: &str, name: &str, value: &Secret) -> Result<()> {
        let storage_key = Self::make_key(scope, name);

        // Encrypt the secret value
        let encrypted = self.key.encrypt(value.expose().as_bytes())?;
        let encrypted_hex = hex::encode(&encrypted);

        let now = Self::now_iso8601();

        // Check if secret exists to determine version
        let existing = self.get_record(&storage_key).await?;

        let mut db = self.db.lock().await;

        if let Some(existing_fields) = existing {
            // Update existing secret
            let version: i64 = existing_fields
                .get("version")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let new_version = version + 1;
            let created_at = existing_fields
                .get("created_at")
                .cloned()
                .unwrap_or_else(|| now.clone());

            // Delete old entry
            let _ = db.query(&format!(
                "DELETE FROM secrets WHERE storage_key = '{}'",
                Self::escape_str(&storage_key)
            ));

            // Insert updated entry
            db.query(&format!(
                "INSERT INTO secrets (storage_key, encrypted_value_hex, name, version, created_at, updated_at) VALUES ('{}', '{}', '{}', '{}', '{}', '{}')",
                Self::escape_str(&storage_key),
                Self::escape_str(&encrypted_hex),
                Self::escape_str(name),
                new_version,
                Self::escape_str(&created_at),
                Self::escape_str(&now)
            ))
            .map_err(|e| SecretsError::Storage(format!("Failed to update secret: {e}")))?;

            debug!("Updated secret: {} (version {})", storage_key, new_version);
        } else {
            // Insert new secret
            db.query(&format!(
                "INSERT INTO secrets (storage_key, encrypted_value_hex, name, version, created_at, updated_at) VALUES ('{}', '{}', '{}', '1', '{}', '{}')",
                Self::escape_str(&storage_key),
                Self::escape_str(&encrypted_hex),
                Self::escape_str(name),
                Self::escape_str(&now),
                Self::escape_str(&now)
            ))
            .map_err(|e| SecretsError::Storage(format!("Failed to insert secret: {e}")))?;

            debug!("Stored secret: {} (version 1)", storage_key);
        }

        Ok(())
    }

    async fn delete_secret(&self, scope: &str, name: &str) -> Result<()> {
        let storage_key = Self::make_key(scope, name);

        // Check existence first
        let exists = self.get_record(&storage_key).await?.is_some();
        if !exists {
            return Err(SecretsError::NotFound {
                name: name.to_string(),
            });
        }

        let mut db = self.db.lock().await;
        db.query(&format!(
            "DELETE FROM secrets WHERE storage_key = '{}'",
            Self::escape_str(&storage_key)
        ))
        .map_err(|e| SecretsError::Storage(format!("Failed to delete secret: {e}")))?;

        debug!("Deleted secret: {}", storage_key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_store() -> (PersistentSecretsStore, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_secrets_zql");
        let key = EncryptionKey::generate();
        let store = PersistentSecretsStore::open(&db_path, key).await.unwrap();
        (store, temp_dir)
    }

    #[tokio::test]
    async fn test_set_and_get_secret() {
        let (store, _temp) = create_test_store().await;

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
        let (store, _temp) = create_test_store().await;

        let result = store.get_secret("deployment/myapp", "nonexistent").await;
        assert!(matches!(result, Err(SecretsError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_exists() {
        let (store, _temp) = create_test_store().await;

        assert!(!store.exists("scope", "name").await.unwrap());

        let secret = Secret::new("value");
        store.set_secret("scope", "name", &secret).await.unwrap();

        assert!(store.exists("scope", "name").await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_secret() {
        let (store, _temp) = create_test_store().await;

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
        let (store, _temp) = create_test_store().await;

        let result = store.delete_secret("scope", "nonexistent").await;
        assert!(matches!(result, Err(SecretsError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_list_secrets() {
        let (store, _temp) = create_test_store().await;

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
        let (store, _temp) = create_test_store().await;

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
        let (store, _temp) = create_test_store().await;

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
        let db_path = temp_dir.path().join("persist_test_zql");

        // Use fixed key for persistence test
        let key_bytes = [42u8; 32];
        let key = EncryptionKey::from_bytes(&key_bytes).unwrap();

        // Write data
        {
            let store = PersistentSecretsStore::open(&db_path, key.clone())
                .await
                .unwrap();
            store
                .set_secret("scope", "persistent", &Secret::new("persistent-value"))
                .await
                .unwrap();
        }

        // Reopen and verify
        {
            let store = PersistentSecretsStore::open(&db_path, key).await.unwrap();
            let secret = store.get_secret("scope", "persistent").await.unwrap();
            assert_eq!(secret.expose(), "persistent-value");
        }
    }

    #[tokio::test]
    async fn test_wrong_key_fails_decryption() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("wrong_key_test_zql");

        // Write with one key
        let key1 = EncryptionKey::generate();
        {
            let store = PersistentSecretsStore::open(&db_path, key1).await.unwrap();
            store
                .set_secret("scope", "secret", &Secret::new("value"))
                .await
                .unwrap();
        }

        // Try to read with different key
        let key2 = EncryptionKey::generate();
        {
            let store = PersistentSecretsStore::open(&db_path, key2).await.unwrap();
            let result = store.get_secret("scope", "secret").await;
            assert!(result.is_err()); // Should fail decryption
        }
    }

    #[tokio::test]
    async fn test_open_with_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let key = EncryptionKey::generate();

        // Pass directory path instead of file path
        let store = PersistentSecretsStore::open(temp_dir.path(), key)
            .await
            .unwrap();

        store
            .set_secret("scope", "test", &Secret::new("value"))
            .await
            .unwrap();

        // Verify database directory was created
        let expected_path = temp_dir.path().join(DEFAULT_DB_DIRNAME);
        assert!(
            expected_path.exists(),
            "Database directory should be created at {:?}",
            expected_path
        );
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
        let (store, _temp) = create_test_store().await;

        let secret = Secret::new("");
        store.set_secret("scope", "empty", &secret).await.unwrap();

        let retrieved = store.get_secret("scope", "empty").await.unwrap();
        assert_eq!(retrieved.expose(), "");
    }

    #[tokio::test]
    async fn test_unicode_secret() {
        let (store, _temp) = create_test_store().await;

        let secret = Secret::new("hello world");
        store.set_secret("scope", "unicode", &secret).await.unwrap();

        let retrieved = store.get_secret("scope", "unicode").await.unwrap();
        assert_eq!(retrieved.expose(), "hello world");
    }

    #[tokio::test]
    async fn test_large_secret() {
        let (store, _temp) = create_test_store().await;

        // 1MB secret
        let large_value: String = "x".repeat(1024 * 1024);
        let secret = Secret::new(&large_value);
        store.set_secret("scope", "large", &secret).await.unwrap();

        let retrieved = store.get_secret("scope", "large").await.unwrap();
        assert_eq!(retrieved.expose().len(), 1024 * 1024);
    }
}
