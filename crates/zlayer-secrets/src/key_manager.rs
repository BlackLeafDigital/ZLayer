//! Encryption key management for `ZLayer` secrets.
//!
//! Provides automatic key discovery, generation, and persistence with
//! support for environment-based configuration, password derivation,
//! and file-based key storage.
//!
//! ## Key Resolution Priority
//!
//! Keys are resolved in the following order:
//! 1. `ZLAYER_SECRETS_KEY` environment variable (hex-encoded 32-byte key)
//! 2. `ZLAYER_SECRETS_PASSWORD` environment variable (derived with Argon2id)
//! 3. Key file at `{base_dir}/secrets_{deployment}.key`
//! 4. Auto-generated key (saved to key file for future use)

use std::fs;
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use crate::{EncryptionKey, Result, SecretsError};

/// Default directory for storing encryption key files.
pub const SECRETS_DIR: &str = "/var/lib/zlayer/secrets";

/// Environment variable name for hex-encoded encryption key.
const ENV_KEY: &str = "ZLAYER_SECRETS_KEY";

/// Environment variable name for password-based key derivation.
const ENV_PASSWORD: &str = "ZLAYER_SECRETS_PASSWORD";

/// Manages encryption keys for secret storage.
///
/// The `KeyManager` handles key discovery, generation, and persistence
/// with automatic fallback through multiple sources.
///
/// # Example
///
/// ```no_run
/// use zlayer_secrets::KeyManager;
///
/// let manager = KeyManager::new();
/// let key = manager.get_or_create_key("production").unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct KeyManager {
    base_dir: PathBuf,
}

impl Default for KeyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyManager {
    /// Creates a new `KeyManager` with the default secrets directory.
    ///
    /// The default directory is `/var/lib/zlayer/secrets`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            base_dir: PathBuf::from(SECRETS_DIR),
        }
    }

    /// Creates a new `KeyManager` with a custom base directory.
    ///
    /// # Arguments
    /// * `base_dir` - Path to the directory for storing key files
    #[must_use]
    pub fn with_base_dir(base_dir: impl AsRef<Path>) -> Self {
        Self {
            base_dir: base_dir.as_ref().to_path_buf(),
        }
    }

    /// Returns the path to the key file for a given deployment.
    fn key_file_path(&self, deployment: &str) -> PathBuf {
        self.base_dir.join(format!("secrets_{deployment}.key"))
    }

    /// Gets or creates an encryption key for the specified deployment.
    ///
    /// Attempts to obtain a key through the following priority chain:
    ///
    /// 1. **Environment key**: If `ZLAYER_SECRETS_KEY` is set, decodes the
    ///    hex-encoded 32-byte key directly.
    ///
    /// 2. **Password derivation**: If `ZLAYER_SECRETS_PASSWORD` is set,
    ///    derives a key using Argon2id with the deployment name as salt.
    ///
    /// 3. **File-based key**: Loads the key from the deployment's key file
    ///    if it exists at `{base_dir}/secrets_{deployment}.key`.
    ///
    /// 4. **Auto-generation**: Generates a new random key and saves it to
    ///    the key file with restricted permissions (0600 on Unix).
    ///
    /// # Arguments
    /// * `deployment` - The deployment name used for key file naming and
    ///   password salt derivation
    ///
    /// # Errors
    ///
    /// Returns `SecretsError::Encryption` if:
    /// - The hex-encoded key in `ZLAYER_SECRETS_KEY` is invalid
    /// - Key file I/O operations fail
    /// - Password derivation fails
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zlayer_secrets::KeyManager;
    ///
    /// let manager = KeyManager::new();
    ///
    /// // First call: generates and saves key
    /// let key = manager.get_or_create_key("production").unwrap();
    ///
    /// // Subsequent calls: loads from file
    /// let same_key = manager.get_or_create_key("production").unwrap();
    /// assert_eq!(key.as_bytes(), same_key.as_bytes());
    /// ```
    pub fn get_or_create_key(&self, deployment: &str) -> Result<EncryptionKey> {
        // Priority 1: Check ZLAYER_SECRETS_KEY env var (hex-encoded)
        if let Ok(hex_key) = std::env::var(ENV_KEY) {
            debug!("Using encryption key from {ENV_KEY} environment variable");
            return Self::key_from_hex(&hex_key);
        }

        // Priority 2: Check ZLAYER_SECRETS_PASSWORD env var, derive with Argon2
        if let Ok(password) = std::env::var(ENV_PASSWORD) {
            debug!("Deriving encryption key from {ENV_PASSWORD} environment variable");
            return Self::key_from_password(&password, deployment);
        }

        // Priority 3: Load from file if exists
        let key_path = self.key_file_path(deployment);
        if key_path.exists() {
            debug!("Loading encryption key from file: {}", key_path.display());
            return Self::load_key_from_file(&key_path);
        }

        // Priority 4: Auto-generate and save to file
        info!(
            "Generating new encryption key for deployment '{}' at {}",
            deployment,
            key_path.display()
        );
        Self::generate_and_save_key(&key_path)
    }

    /// Decodes a hex-encoded key string into an [`EncryptionKey`].
    fn key_from_hex(hex_key: &str) -> Result<EncryptionKey> {
        let key_bytes = hex::decode(hex_key.trim()).map_err(|e| {
            SecretsError::Encryption(format!("Invalid hex-encoded key in {ENV_KEY}: {e}"))
        })?;

        EncryptionKey::from_bytes(&key_bytes)
    }

    /// Derives an encryption key from a password using the deployment as salt.
    fn key_from_password(password: &str, deployment: &str) -> Result<EncryptionKey> {
        // Use deployment name as salt - this ensures different deployments
        // with the same password get different keys
        EncryptionKey::derive_from_password(password, deployment.as_bytes())
    }

    /// Loads an encryption key from a file containing raw key bytes.
    fn load_key_from_file(path: &Path) -> Result<EncryptionKey> {
        let key_bytes = fs::read(path).map_err(|e| {
            SecretsError::Encryption(format!("Failed to read key file {}: {e}", path.display()))
        })?;

        EncryptionKey::from_bytes(&key_bytes)
    }

    /// Generates a new encryption key and saves it to a file.
    ///
    /// On Unix systems, sets file permissions to 0600 (owner read/write only).
    fn generate_and_save_key(path: &Path) -> Result<EncryptionKey> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                SecretsError::Encryption(format!(
                    "Failed to create key directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        // Generate new key
        let key = EncryptionKey::generate();

        // Write key bytes to file
        fs::write(path, key.as_bytes()).map_err(|e| {
            SecretsError::Encryption(format!("Failed to write key file {}: {e}", path.display()))
        })?;

        // Set restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = fs::Permissions::from_mode(0o600);
            if let Err(e) = fs::set_permissions(path, permissions) {
                warn!(
                    "Failed to set permissions on key file {}: {e}",
                    path.display()
                );
            }
        }

        info!("Created new encryption key at {}", path.display());
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    /// Guard that clears environment variables on drop to ensure test isolation.
    struct EnvGuard;

    impl EnvGuard {
        fn new() -> Self {
            // Clear any existing env vars at test start
            env::remove_var(ENV_KEY);
            env::remove_var(ENV_PASSWORD);
            Self
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            env::remove_var(ENV_KEY);
            env::remove_var(ENV_PASSWORD);
        }
    }

    fn setup_manager() -> (KeyManager, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let manager = KeyManager::with_base_dir(temp_dir.path());
        (manager, temp_dir)
    }

    #[test]
    fn test_new_uses_default_dir() {
        let manager = KeyManager::new();
        assert_eq!(manager.base_dir, PathBuf::from(SECRETS_DIR));
    }

    #[test]
    fn test_with_base_dir() {
        let manager = KeyManager::with_base_dir("/custom/path");
        assert_eq!(manager.base_dir, PathBuf::from("/custom/path"));
    }

    #[test]
    fn test_key_file_path() {
        let manager = KeyManager::with_base_dir("/var/lib/zlayer/secrets");
        let path = manager.key_file_path("production");
        assert_eq!(
            path,
            PathBuf::from("/var/lib/zlayer/secrets/secrets_production.key")
        );
    }

    // All tests that call get_or_create_key must be serial because that method
    // reads environment variables which are process-global state.
    #[test]
    #[serial]
    fn test_auto_generate_key() {
        let _guard = EnvGuard::new();
        let (manager, _temp) = setup_manager();

        let key = manager.get_or_create_key("test-deployment").unwrap();
        assert_eq!(key.as_bytes().len(), 32);

        // Key file should exist
        let key_path = manager.key_file_path("test-deployment");
        assert!(key_path.exists());
    }

    #[test]
    #[serial]
    fn test_load_existing_key() {
        let _guard = EnvGuard::new();
        let (manager, _temp) = setup_manager();

        // Generate key first
        let key1 = manager.get_or_create_key("test-deployment").unwrap();

        // Should load the same key
        let key2 = manager.get_or_create_key("test-deployment").unwrap();

        assert_eq!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    #[serial]
    fn test_different_deployments_get_different_keys() {
        let _guard = EnvGuard::new();
        let (manager, _temp) = setup_manager();

        let key1 = manager.get_or_create_key("deployment-a").unwrap();
        let key2 = manager.get_or_create_key("deployment-b").unwrap();

        assert_ne!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    #[serial]
    fn test_env_key_takes_priority() {
        let _guard = EnvGuard::new();
        let (manager, _temp) = setup_manager();

        // Create a known key
        let known_key = [42u8; 32];
        let hex_key = hex::encode(known_key);

        // Set env var
        env::set_var(ENV_KEY, &hex_key);

        let key = manager.get_or_create_key("any-deployment").unwrap();
        assert_eq!(key.as_bytes(), &known_key);
    }

    #[test]
    #[serial]
    fn test_env_password_takes_priority_over_file() {
        let _guard = EnvGuard::new();
        let (manager, _temp) = setup_manager();

        // First generate a file-based key
        let file_key = manager.get_or_create_key("test-deployment").unwrap();

        // Set password env var
        env::set_var(ENV_PASSWORD, "my-secret-password");

        // Should now derive from password, not load from file
        let password_key = manager.get_or_create_key("test-deployment").unwrap();
        assert_ne!(file_key.as_bytes(), password_key.as_bytes());
    }

    #[test]
    #[serial]
    fn test_password_derivation_is_deterministic() {
        let _guard = EnvGuard::new();
        let (manager, _temp) = setup_manager();

        env::set_var(ENV_PASSWORD, "test-password");

        let key1 = manager.get_or_create_key("deployment").unwrap();
        let key2 = manager.get_or_create_key("deployment").unwrap();

        assert_eq!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    #[serial]
    fn test_password_with_different_deployments() {
        let _guard = EnvGuard::new();
        let (manager, _temp) = setup_manager();

        env::set_var(ENV_PASSWORD, "same-password");

        // Same password but different deployments should produce different keys
        let key1 = manager.get_or_create_key("deployment-a").unwrap();
        let key2 = manager.get_or_create_key("deployment-b").unwrap();

        assert_ne!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    #[serial]
    fn test_invalid_hex_key_error() {
        let _guard = EnvGuard::new();
        let (manager, _temp) = setup_manager();

        env::set_var(ENV_KEY, "not-valid-hex!!");

        let result = manager.get_or_create_key("test");
        assert!(result.is_err());
    }

    #[test]
    #[serial]
    fn test_hex_key_wrong_length_error() {
        let _guard = EnvGuard::new();
        let (manager, _temp) = setup_manager();

        // Only 16 bytes (32 hex chars needed for 32 bytes)
        env::set_var(ENV_KEY, "0011223344556677889900112233445566778899");

        let result = manager.get_or_create_key("test");
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn test_key_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = EnvGuard::new();
        let (manager, _temp) = setup_manager();

        manager.get_or_create_key("secure-deployment").unwrap();

        let key_path = manager.key_file_path("secure-deployment");
        let metadata = fs::metadata(&key_path).unwrap();
        let permissions = metadata.permissions();

        // Should be 0600 (owner read/write only)
        assert_eq!(permissions.mode() & 0o777, 0o600);
    }
}
