//! Persistent secrets storage using `SQLite` (via `SQLx`).
//!
//! Provides encrypted local storage for secrets with a single table combining
//! secret values and metadata:
//! - `storage_key`: Primary key in `{scope}:{name}` format
//! - `encrypted_value`: XChaCha20-Poly1305 encrypted secret bytes
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
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use tracing::{debug, info};

use crate::{
    EncryptionKey, Result, Secret, SecretMetadata, SecretsError, SecretsProvider, SecretsStore,
};

/// Default database filename when a directory is provided.
const DEFAULT_DB_FILENAME: &str = "secrets.sqlite";

/// SQL schema for the secrets table.
const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS secrets (
    storage_key TEXT PRIMARY KEY NOT NULL,
    encrypted_value BLOB NOT NULL,
    name TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_secrets_prefix ON secrets(storage_key);
";

/// Persistent secrets store backed by `SQLite` with encryption.
///
/// Secrets are encrypted using XChaCha20-Poly1305 before storage.
/// Metadata is stored alongside secrets for inspection and auditing.
///
/// The store uses `SQLite` with WAL mode for concurrent access.
pub struct PersistentSecretsStore {
    pool: SqlitePool,
    key: EncryptionKey,
}

impl PersistentSecretsStore {
    /// Opens or creates a persistent secrets store at the given path.
    ///
    /// If `path` is a directory, the database file will be created as
    /// `secrets.sqlite` inside that directory. If `path` is a file path,
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
    /// - Schema initialization fails
    pub async fn open(path: impl AsRef<Path>, key: EncryptionKey) -> Result<Self> {
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

        // Configure SQLite connection with WAL mode for better concurrency
        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(std::time::Duration::from_secs(30));

        // Create connection pool
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(|e| {
                SecretsError::Storage(format!(
                    "Failed to open database at {}: {e}",
                    db_path.display()
                ))
            })?;

        // Initialize schema
        sqlx::query(SCHEMA)
            .execute(&pool)
            .await
            .map_err(|e| SecretsError::Storage(format!("Failed to initialize schema: {e}")))?;

        info!("Opened persistent secrets store at {}", db_path.display());

        Ok(Self { pool, key })
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
        // Convert to ISO 8601 format for SQLite TEXT storage
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
}

#[async_trait]
impl SecretsProvider for PersistentSecretsStore {
    async fn get_secret(&self, scope: &str, name: &str) -> Result<Secret> {
        let storage_key = Self::make_key(scope, name);

        let row = sqlx::query("SELECT encrypted_value FROM secrets WHERE storage_key = ?")
            .bind(&storage_key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| SecretsError::Storage(format!("Failed to query secret: {e}")))?;

        match row {
            Some(row) => {
                let encrypted_value: Vec<u8> = row
                    .try_get("encrypted_value")
                    .map_err(|e| SecretsError::Storage(format!("Failed to read value: {e}")))?;

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
            // For batch retrieval, we silently skip missing secrets
            // rather than returning an error
            if let Ok(secret) = self.get_secret(scope, name).await {
                results.insert((*name).to_string(), secret);
            }
        }

        Ok(results)
    }

    async fn list_secrets(&self, scope: &str) -> Result<Vec<SecretMetadata>> {
        let prefix = format!("{scope}:%");

        let rows = sqlx::query(
            "SELECT name, version, created_at, updated_at FROM secrets WHERE storage_key LIKE ? ORDER BY name",
        )
        .bind(&prefix)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SecretsError::Storage(format!("Failed to list secrets: {e}")))?;

        let mut results = Vec::with_capacity(rows.len());

        for row in rows {
            let name: String = row
                .try_get("name")
                .map_err(|e| SecretsError::Storage(format!("Failed to read name: {e}")))?;
            let version: i64 = row
                .try_get("version")
                .map_err(|e| SecretsError::Storage(format!("Failed to read version: {e}")))?;
            let created_at: String = row
                .try_get("created_at")
                .map_err(|e| SecretsError::Storage(format!("Failed to read created_at: {e}")))?;
            let updated_at: String = row
                .try_get("updated_at")
                .map_err(|e| SecretsError::Storage(format!("Failed to read updated_at: {e}")))?;

            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            results.push(SecretMetadata {
                name,
                version: version as u32,
                created_at: Self::parse_timestamp(&created_at),
                updated_at: Self::parse_timestamp(&updated_at),
            });
        }

        debug!("Listed {} secrets in scope: {}", results.len(), scope);
        Ok(results)
    }

    async fn exists(&self, scope: &str, name: &str) -> Result<bool> {
        let storage_key = Self::make_key(scope, name);

        let row = sqlx::query("SELECT 1 FROM secrets WHERE storage_key = ?")
            .bind(&storage_key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| SecretsError::Storage(format!("Failed to check existence: {e}")))?;

        Ok(row.is_some())
    }
}

#[async_trait]
impl SecretsStore for PersistentSecretsStore {
    async fn set_secret(&self, scope: &str, name: &str, value: &Secret) -> Result<()> {
        let storage_key = Self::make_key(scope, name);

        // Encrypt the secret value
        let encrypted = self.key.encrypt(value.expose().as_bytes())?;

        let now = Self::now_iso8601();

        // Check if secret exists to determine version
        let existing = sqlx::query("SELECT version, created_at FROM secrets WHERE storage_key = ?")
            .bind(&storage_key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| SecretsError::Storage(format!("Failed to check existing: {e}")))?;

        if let Some(row) = existing {
            // Update existing secret
            let version: i64 = row.try_get("version").unwrap_or(1);
            let new_version = version + 1;

            sqlx::query(
                "UPDATE secrets SET encrypted_value = ?, version = ?, updated_at = ? WHERE storage_key = ?",
            )
            .bind(&encrypted)
            .bind(new_version)
            .bind(&now)
            .bind(&storage_key)
            .execute(&self.pool)
            .await
            .map_err(|e| SecretsError::Storage(format!("Failed to update secret: {e}")))?;

            debug!("Updated secret: {} (version {})", storage_key, new_version);
        } else {
            // Insert new secret
            sqlx::query(
                "INSERT INTO secrets (storage_key, encrypted_value, name, version, created_at, updated_at) VALUES (?, ?, ?, 1, ?, ?)",
            )
            .bind(&storage_key)
            .bind(&encrypted)
            .bind(name)
            .bind(&now)
            .bind(&now)
            .execute(&self.pool)
            .await
            .map_err(|e| SecretsError::Storage(format!("Failed to insert secret: {e}")))?;

            debug!("Stored secret: {} (version 1)", storage_key);
        }

        Ok(())
    }

    async fn delete_secret(&self, scope: &str, name: &str) -> Result<()> {
        let storage_key = Self::make_key(scope, name);

        let result = sqlx::query("DELETE FROM secrets WHERE storage_key = ?")
            .bind(&storage_key)
            .execute(&self.pool)
            .await
            .map_err(|e| SecretsError::Storage(format!("Failed to delete secret: {e}")))?;

        if result.rows_affected() == 0 {
            return Err(SecretsError::NotFound {
                name: name.to_string(),
            });
        }

        debug!("Deleted secret: {}", storage_key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_store() -> (PersistentSecretsStore, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_secrets.sqlite");
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
        let db_path = temp_dir.path().join("persist_test.sqlite");

        // Use fixed key for persistence test
        let key_bytes = [42u8; 32];
        let key = EncryptionKey::from_bytes(&key_bytes).unwrap();

        // Write data
        {
            let store = PersistentSecretsStore::open(&db_path, key.clone()).await.unwrap();
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
        let db_path = temp_dir.path().join("wrong_key_test.sqlite");

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
        let store = PersistentSecretsStore::open(temp_dir.path(), key).await.unwrap();

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
