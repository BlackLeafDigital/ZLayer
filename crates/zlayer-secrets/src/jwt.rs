//! JWT signing-key management for the `ZLayer` API daemon.
//!
//! Mirrors [`KeyManager`](crate::KeyManager) for the JWT secret used by
//! `zlayer-api` to sign session cookies. Without persistence, every daemon
//! restart that omits `ZLAYER_JWT_SECRET` invalidates every previously
//! issued session cookie — users get logged out on every restart.
//!
//! ## Resolution Priority
//!
//! 1. `ZLAYER_JWT_SECRET` environment variable — operator opt-out, returned
//!    verbatim and never persisted.
//! 2. File at `{base_dir}/jwt_secret_{deployment}.key` — loaded as bytes.
//! 3. Auto-generated: 64 random bytes (sufficient for HMAC-SHA256), saved
//!    to the same file with mode `0600` for future runs.

use std::fs;
use std::path::{Path, PathBuf};

use rand::rngs::OsRng;
use rand::TryRngCore;
use secrecy::SecretString;
#[cfg(unix)]
use tracing::warn;
use tracing::{debug, info};

use crate::{Result, SecretsError};

/// Environment variable name for the operator-supplied JWT secret.
pub const ENV_JWT_SECRET: &str = "ZLAYER_JWT_SECRET";

/// Length in bytes of an auto-generated JWT secret. 64 is the natural ceiling
/// for HMAC-SHA256 (HS256): keys longer than the hash block size get hashed
/// before use, so 64 random bytes is the upper end of what the signer
/// actually consumes.
const GENERATED_SECRET_BYTES: usize = 64;

/// Manages the API daemon's JWT signing secret.
///
/// Use [`JwtSecretManager::with_base_dir`] alongside [`KeyManager`](crate::KeyManager)
/// during daemon startup so both keys live in the same directory.
#[derive(Debug, Clone)]
pub struct JwtSecretManager {
    base_dir: PathBuf,
}

impl JwtSecretManager {
    /// Creates a manager rooted at `base_dir`.
    #[must_use]
    pub fn with_base_dir(base_dir: impl AsRef<Path>) -> Self {
        Self {
            base_dir: base_dir.as_ref().to_path_buf(),
        }
    }

    /// Returns the path to the JWT secret file for `deployment`.
    fn secret_file_path(&self, deployment: &str) -> PathBuf {
        self.base_dir.join(format!("jwt_secret_{deployment}.key"))
    }

    /// Resolves the JWT secret, generating + persisting a fresh one when
    /// neither the env var nor a saved file is present.
    ///
    /// # Errors
    ///
    /// Returns [`SecretsError::Encryption`] if the file system cannot be
    /// read or written, or if the loaded file is empty.
    pub fn get_or_create(&self, deployment: &str) -> Result<SecretString> {
        if let Ok(secret) = std::env::var(ENV_JWT_SECRET) {
            if secret.is_empty() {
                return Err(SecretsError::Encryption(format!(
                    "{ENV_JWT_SECRET} is set but empty"
                )));
            }
            debug!("Using JWT secret from {ENV_JWT_SECRET} environment variable");
            return Ok(SecretString::from(secret));
        }

        let path = self.secret_file_path(deployment);
        if path.exists() {
            debug!("Loading JWT secret from file: {}", path.display());
            return Self::load_from_file(&path);
        }

        info!(
            "Generating new JWT secret for deployment '{}' at {}",
            deployment,
            path.display()
        );
        Self::generate_and_save(&path)
    }

    fn load_from_file(path: &Path) -> Result<SecretString> {
        let bytes = fs::read(path).map_err(|e| {
            SecretsError::Encryption(format!(
                "Failed to read JWT secret file {}: {e}",
                path.display()
            ))
        })?;
        if bytes.is_empty() {
            return Err(SecretsError::Encryption(format!(
                "JWT secret file {} is empty",
                path.display()
            )));
        }
        Ok(SecretString::from(hex::encode(bytes)))
    }

    fn generate_and_save(path: &Path) -> Result<SecretString> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                SecretsError::Encryption(format!(
                    "Failed to create JWT secret directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        let mut bytes = vec![0u8; GENERATED_SECRET_BYTES];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|e| SecretsError::Encryption(format!("OS RNG failed: {e}")))?;

        fs::write(path, &bytes).map_err(|e| {
            SecretsError::Encryption(format!(
                "Failed to write JWT secret file {}: {e}",
                path.display()
            ))
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            if let Err(e) = fs::set_permissions(path, perms) {
                warn!(
                    "Failed to set permissions on JWT secret file {}: {e}",
                    path.display()
                );
            }
        }

        info!("Created new JWT secret at {}", path.display());
        Ok(SecretString::from(hex::encode(&bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    struct EnvGuard;

    impl EnvGuard {
        fn new() -> Self {
            env::remove_var(ENV_JWT_SECRET);
            Self
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            env::remove_var(ENV_JWT_SECRET);
        }
    }

    fn setup() -> (JwtSecretManager, TempDir) {
        let dir = TempDir::new().unwrap();
        (JwtSecretManager::with_base_dir(dir.path()), dir)
    }

    #[test]
    #[serial]
    fn generates_and_persists() {
        let _g = EnvGuard::new();
        let (mgr, _t) = setup();
        let s1 = mgr.get_or_create("dev").unwrap();
        let path = mgr.secret_file_path("dev");
        assert!(path.exists(), "secret file should be persisted");
        // Hex of 64 bytes is 128 chars.
        assert_eq!(s1.expose_secret().len(), GENERATED_SECRET_BYTES * 2);
    }

    #[test]
    #[serial]
    fn stable_across_calls() {
        let _g = EnvGuard::new();
        let (mgr, _t) = setup();
        let s1 = mgr.get_or_create("dev").unwrap();
        let s2 = mgr.get_or_create("dev").unwrap();
        assert_eq!(s1.expose_secret(), s2.expose_secret());
    }

    #[test]
    #[serial]
    fn different_deployments_get_different_secrets() {
        let _g = EnvGuard::new();
        let (mgr, _t) = setup();
        let a = mgr.get_or_create("a").unwrap();
        let b = mgr.get_or_create("b").unwrap();
        assert_ne!(a.expose_secret(), b.expose_secret());
    }

    #[test]
    #[serial]
    fn env_overrides_file() {
        let _g = EnvGuard::new();
        let (mgr, _t) = setup();
        // Pre-create a file-backed secret.
        let _file_secret = mgr.get_or_create("dev").unwrap();
        env::set_var(ENV_JWT_SECRET, "operator-supplied");
        let env_secret = mgr.get_or_create("dev").unwrap();
        assert_eq!(env_secret.expose_secret(), "operator-supplied");
    }

    #[test]
    #[serial]
    fn empty_env_is_rejected() {
        let _g = EnvGuard::new();
        let (mgr, _t) = setup();
        env::set_var(ENV_JWT_SECRET, "");
        let result = mgr.get_or_create("dev");
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn file_has_restrictive_perms() {
        use std::os::unix::fs::PermissionsExt;
        let _g = EnvGuard::new();
        let (mgr, _t) = setup();
        mgr.get_or_create("dev").unwrap();
        let path = mgr.secret_file_path("dev");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
