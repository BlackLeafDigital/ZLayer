//! `XChaCha20-Poly1305` encryption for secrets.
//!
//! Provides authenticated encryption using `XChaCha20-Poly1305` with
//! Argon2id key derivation for password-based encryption.

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use rand::rngs::OsRng;
use rand::RngCore;
use zeroize::Zeroizing;

use crate::{Result, SecretsError};

/// Size of the XChaCha20-Poly1305 nonce in bytes.
pub const NONCE_SIZE: usize = 24;

/// Size of the encryption key in bytes.
pub const KEY_SIZE: usize = 32;

/// Encryption key with secure memory handling.
///
/// The key bytes are wrapped in [`Zeroizing`] to ensure they are
/// zeroed from memory when dropped.
#[derive(Clone)]
pub struct EncryptionKey {
    key: Zeroizing<[u8; KEY_SIZE]>,
}

impl EncryptionKey {
    /// Derives an encryption key from a password using Argon2id.
    ///
    /// # Arguments
    /// * `password` - The password to derive the key from
    /// * `salt` - Salt bytes (should be at least 16 bytes, randomly generated per-key)
    ///
    /// # Errors
    /// Returns `SecretsError::Encryption` if key derivation fails.
    pub fn derive_from_password(password: &str, salt: &[u8]) -> Result<Self> {
        use argon2::{Algorithm, Argon2, Params, Version};

        // Argon2id parameters - balanced security/performance
        // Using OWASP recommended minimums for interactive scenarios
        let params = Params::new(
            19 * 1024, // 19 MiB memory cost (OWASP recommendation)
            2,         // 2 iterations
            1,         // 1 degree of parallelism
            Some(KEY_SIZE),
        )
        .map_err(|e| SecretsError::Encryption(format!("Invalid Argon2 params: {e}")))?;

        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

        let mut key_bytes = Zeroizing::new([0u8; KEY_SIZE]);
        argon2
            .hash_password_into(password.as_bytes(), salt, key_bytes.as_mut())
            .map_err(|e| SecretsError::Encryption(format!("Key derivation failed: {e}")))?;

        Ok(Self { key: key_bytes })
    }

    /// Generates a random 32-byte encryption key.
    ///
    /// Uses the operating system's cryptographically secure random number generator.
    #[must_use]
    pub fn generate() -> Self {
        let mut key_bytes = Zeroizing::new([0u8; KEY_SIZE]);
        OsRng.fill_bytes(key_bytes.as_mut());
        Self { key: key_bytes }
    }

    /// Creates an encryption key from raw bytes.
    ///
    /// # Arguments
    /// * `bytes` - The key bytes (must be exactly 32 bytes)
    ///
    /// # Errors
    /// Returns `SecretsError::Encryption` if the byte slice is not exactly 32 bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != KEY_SIZE {
            return Err(SecretsError::Encryption(format!(
                "Invalid key length: expected {KEY_SIZE} bytes, got {}",
                bytes.len()
            )));
        }

        let mut key_bytes = Zeroizing::new([0u8; KEY_SIZE]);
        key_bytes.copy_from_slice(bytes);
        Ok(Self { key: key_bytes })
    }

    /// Returns the raw key bytes.
    ///
    /// Use with caution - only for persisting the key securely.
    #[inline]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.key.as_ref()
    }

    /// Encrypts plaintext using XChaCha20-Poly1305.
    ///
    /// The returned ciphertext has the 24-byte nonce prepended:
    /// `[nonce (24 bytes)][ciphertext + auth tag]`
    ///
    /// # Arguments
    /// * `plaintext` - The data to encrypt
    ///
    /// # Errors
    /// Returns `SecretsError::Encryption` if encryption fails.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let cipher = XChaCha20Poly1305::new_from_slice(self.key.as_ref())
            .map_err(|e| SecretsError::Encryption(format!("Failed to create cipher: {e}")))?;

        // Generate random nonce
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);

        // Encrypt
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| SecretsError::Encryption(format!("Encryption failed: {e}")))?;

        // Prepend nonce to ciphertext
        let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);

        Ok(result)
    }

    /// Decrypts data that was encrypted with [`Self::encrypt`].
    ///
    /// Expects the input format: `[nonce (24 bytes)][ciphertext + auth tag]`
    ///
    /// # Arguments
    /// * `data` - The encrypted data with prepended nonce
    ///
    /// # Errors
    /// Returns `SecretsError::Decryption` if:
    /// - The data is too short (less than nonce size)
    /// - Decryption or authentication fails
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < NONCE_SIZE {
            return Err(SecretsError::Decryption(format!(
                "Data too short: expected at least {NONCE_SIZE} bytes for nonce, got {}",
                data.len()
            )));
        }

        let cipher = XChaCha20Poly1305::new_from_slice(self.key.as_ref())
            .map_err(|e| SecretsError::Decryption(format!("Failed to create cipher: {e}")))?;

        // Extract nonce and ciphertext
        let (nonce_bytes, ciphertext) = data.split_at(NONCE_SIZE);
        let nonce = XNonce::from_slice(nonce_bytes);

        // Decrypt and verify
        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| SecretsError::Decryption(format!("Decryption failed: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_key() {
        let key = EncryptionKey::generate();
        assert_eq!(key.as_bytes().len(), KEY_SIZE);

        // Generated keys should be different
        let key2 = EncryptionKey::generate();
        assert_ne!(key.as_bytes(), key2.as_bytes());
    }

    #[test]
    fn test_from_bytes_valid() {
        let bytes = [42u8; KEY_SIZE];
        let key = EncryptionKey::from_bytes(&bytes).unwrap();
        assert_eq!(key.as_bytes(), &bytes);
    }

    #[test]
    fn test_from_bytes_invalid_length() {
        let bytes = [0u8; 16]; // Too short
        let result = EncryptionKey::from_bytes(&bytes);
        assert!(result.is_err());

        let bytes = [0u8; 64]; // Too long
        let result = EncryptionKey::from_bytes(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_derive_from_password() {
        let salt = b"unique_salt_1234";
        let key = EncryptionKey::derive_from_password("my_secure_password", salt).unwrap();
        assert_eq!(key.as_bytes().len(), KEY_SIZE);

        // Same password + salt should produce same key
        let key2 = EncryptionKey::derive_from_password("my_secure_password", salt).unwrap();
        assert_eq!(key.as_bytes(), key2.as_bytes());

        // Different password should produce different key
        let key3 = EncryptionKey::derive_from_password("different_password", salt).unwrap();
        assert_ne!(key.as_bytes(), key3.as_bytes());

        // Different salt should produce different key
        let key4 =
            EncryptionKey::derive_from_password("my_secure_password", b"different_salt__").unwrap();
        assert_ne!(key.as_bytes(), key4.as_bytes());
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = EncryptionKey::generate();
        let plaintext = b"Hello, World! This is a secret message.";

        let encrypted = key.encrypt(plaintext).unwrap();

        // Encrypted should be longer (nonce + auth tag)
        assert!(encrypted.len() > plaintext.len());

        let decrypted = key.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_produces_different_ciphertext() {
        let key = EncryptionKey::generate();
        let plaintext = b"Same message";

        let encrypted1 = key.encrypt(plaintext).unwrap();
        let encrypted2 = key.encrypt(plaintext).unwrap();

        // Different nonces should produce different ciphertext
        assert_ne!(encrypted1, encrypted2);

        // But both should decrypt to the same plaintext
        assert_eq!(key.decrypt(&encrypted1).unwrap(), plaintext);
        assert_eq!(key.decrypt(&encrypted2).unwrap(), plaintext);
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let key1 = EncryptionKey::generate();
        let key2 = EncryptionKey::generate();
        let plaintext = b"Secret data";

        let encrypted = key1.encrypt(plaintext).unwrap();
        let result = key2.decrypt(&encrypted);

        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_tampered_data_fails() {
        let key = EncryptionKey::generate();
        let plaintext = b"Important data";

        let mut encrypted = key.encrypt(plaintext).unwrap();

        // Tamper with the ciphertext (not the nonce)
        if let Some(byte) = encrypted.get_mut(NONCE_SIZE + 5) {
            *byte ^= 0xFF;
        }

        let result = key.decrypt(&encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_too_short_data() {
        let key = EncryptionKey::generate();
        let short_data = [0u8; 10]; // Less than NONCE_SIZE

        let result = key.decrypt(&short_data);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_empty_plaintext() {
        let key = EncryptionKey::generate();
        let plaintext = b"";

        let encrypted = key.encrypt(plaintext).unwrap();
        let decrypted = key.decrypt(&encrypted).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_password_derived_key_encrypt_decrypt() {
        let salt = b"random_salt_here";
        let key = EncryptionKey::derive_from_password("password123", salt).unwrap();
        let plaintext = b"Sensitive information";

        let encrypted = key.encrypt(plaintext).unwrap();
        let decrypted = key.decrypt(&encrypted).unwrap();

        assert_eq!(decrypted, plaintext);
    }
}
