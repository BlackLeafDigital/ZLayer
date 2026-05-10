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

#[cfg(unix)]
use tracing::warn;
use tracing::{debug, info};

use crate::sealed::{RecipientPrivateKey, RecipientPublicKey};
use crate::{EncryptionKey, Result, SecretsError};

/// File name used for the on-disk node X25519 keypair (raw 32-byte private key).
const NODE_SECRETS_KEY_FILE: &str = "node_secrets.key";

/// Path of the on-disk node X25519 keypair (raw 32-byte private key bytes,
/// Unix mode 0600).
#[must_use]
pub fn node_secrets_key_path(base_dir: &Path) -> PathBuf {
    base_dir.join(NODE_SECRETS_KEY_FILE)
}

/// Load the existing node keypair from `{base_dir}/node_secrets.key`, or
/// generate a new one and persist it (Unix mode 0600) if the file does
/// not exist yet.
///
/// Returns `(private, public)`. The private key is held in [`RecipientPrivateKey`]
/// which zeroes itself on drop.
///
/// # Errors
/// - [`SecretsError::Storage`] if the directory cannot be created, the file
///   cannot be read/written, or the on-disk content is the wrong length
///   (must be exactly 32 bytes).
/// - [`SecretsError::Storage`] if file permissions cannot be set on Unix.
pub fn load_or_generate_node_keypair(
    base_dir: &Path,
) -> std::result::Result<(RecipientPrivateKey, RecipientPublicKey), SecretsError> {
    // 1. Ensure base_dir exists (mkdir -p semantics).
    fs::create_dir_all(base_dir).map_err(|e| {
        SecretsError::Storage(format!(
            "Failed to create node key directory {}: {e}",
            base_dir.display()
        ))
    })?;

    let path = node_secrets_key_path(base_dir);

    // 2. If the file exists, load it.
    if path.exists() {
        debug!("Loading node X25519 keypair from {}", path.display());
        let buf = fs::read(&path).map_err(|e| {
            SecretsError::Storage(format!(
                "Failed to read node key file {}: {e}",
                path.display()
            ))
        })?;

        if buf.len() != 32 {
            return Err(SecretsError::Storage(format!(
                "node_secrets.key has wrong length: expected 32, got {}",
                buf.len()
            )));
        }

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&buf);
        let private = RecipientPrivateKey::from_bytes(bytes);
        let public = private.public_key();
        return Ok((private, public));
    }

    // 3. Otherwise, generate a fresh keypair and persist it.
    info!("Generating new node X25519 keypair at {}", path.display());
    let (private, public) = RecipientPrivateKey::generate();

    write_node_key_file(&path, &private)?;

    Ok((private, public))
}

/// Persist the raw 32-byte private key to `path`, setting Unix mode 0600
/// before any data is written so the key is never world-readable on disk.
fn write_node_key_file(
    path: &Path,
    private: &RecipientPrivateKey,
) -> std::result::Result<(), SecretsError> {
    // RecipientPrivateKey holds raw 32 bytes internally but does not expose
    // a direct byte accessor (intentional: the type is zeroize-on-drop and
    // we want any disclosure to be explicit). Round-trip through its
    // `to_base64` representation to recover the raw bytes for on-disk storage.
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;
    let raw = B64
        .decode(private.to_base64())
        .map_err(|e| SecretsError::Storage(format!("Failed to encode node private key: {e}")))?;
    debug_assert_eq!(raw.len(), 32);

    // On Unix, create the file with mode 0600 from the start so the key
    // bytes are never written under a more permissive mode.
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| {
                SecretsError::Storage(format!(
                    "Failed to create node key file {}: {e}",
                    path.display()
                ))
            })?;

        file.write_all(&raw).map_err(|e| {
            SecretsError::Storage(format!(
                "Failed to write node key file {}: {e}",
                path.display()
            ))
        })?;

        // Belt-and-suspenders: explicitly set perms in case the umask or
        // a pre-existing file overrode the open mode.
        let permissions = fs::Permissions::from_mode(0o600);
        if let Err(e) = fs::set_permissions(path, permissions) {
            warn!(
                "Failed to set permissions on node key file {}: {e}",
                path.display()
            );
            return Err(SecretsError::Storage(format!(
                "Failed to set permissions on node key file {}: {e}",
                path.display()
            )));
        }
    }

    #[cfg(not(unix))]
    {
        fs::write(path, &raw).map_err(|e| {
            SecretsError::Storage(format!(
                "Failed to write node key file {}: {e}",
                path.display()
            ))
        })?;
    }

    Ok(())
}

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
    /// The default directory is determined by [`zlayer_paths::ZLayerDirs::system_default()`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            base_dir: zlayer_paths::ZLayerDirs::system_default().secrets(),
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
        let expected = zlayer_paths::ZLayerDirs::system_default().secrets();
        assert_eq!(manager.base_dir, expected);
    }

    #[test]
    fn test_with_base_dir() {
        let manager = KeyManager::with_base_dir("/custom/path");
        assert_eq!(manager.base_dir, PathBuf::from("/custom/path"));
    }

    #[test]
    fn test_key_file_path() {
        let dirs = zlayer_paths::ZLayerDirs::system_default();
        let manager = KeyManager::with_base_dir(dirs.secrets());
        let path = manager.key_file_path("production");
        assert_eq!(path, dirs.secrets().join("secrets_production.key"));
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

    // -------------------------------------------------------------------
    // Node X25519 keypair persistence tests.
    // -------------------------------------------------------------------

    #[test]
    fn node_keypair_round_trip_generate_then_load() {
        let temp = TempDir::new().unwrap();

        let (_priv1, pub1) = load_or_generate_node_keypair(temp.path()).unwrap();
        let (_priv2, pub2) = load_or_generate_node_keypair(temp.path()).unwrap();

        // Second call must load the same key from disk, not generate a new one.
        assert_eq!(pub1, pub2);

        // And the file must exist at the documented path.
        assert!(node_secrets_key_path(temp.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn node_keypair_perms_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let _ = load_or_generate_node_keypair(temp.path()).unwrap();

        let path = node_secrets_key_path(temp.path());
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "expected mode 0600, got {mode:o}");
    }

    #[test]
    fn node_keypair_rejects_wrong_length() {
        let temp = TempDir::new().unwrap();
        let path = node_secrets_key_path(temp.path());

        // Pre-create a 16-byte file at the keypair path.
        fs::create_dir_all(temp.path()).unwrap();
        fs::write(&path, [0u8; 16]).unwrap();

        // `RecipientPrivateKey` does not implement `Debug` (intentional —
        // see `sealed.rs`), so we can't `.unwrap_err()` on the tuple result.
        // Match it manually instead.
        let result = load_or_generate_node_keypair(temp.path());
        match result {
            Ok(_) => panic!("expected SecretsError::Storage, got Ok(_)"),
            Err(SecretsError::Storage(msg)) => {
                assert!(
                    msg.contains("length") || msg.contains("expected 32"),
                    "expected length error message, got: {msg}"
                );
            }
            Err(other) => panic!("expected SecretsError::Storage, got {other:?}"),
        }
    }

    #[test]
    fn node_keypair_pubkey_matches_private() {
        let temp = TempDir::new().unwrap();
        let (private, public) = load_or_generate_node_keypair(temp.path()).unwrap();

        // Re-derive the public key from the private key and confirm it
        // matches the public key returned by `load_or_generate_node_keypair`.
        let derived = private.public_key();
        assert_eq!(derived, public);
    }
}
