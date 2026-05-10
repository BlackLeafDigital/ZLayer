//! Cluster Data Encryption Key (DEK) primitives for Phase 1
//! cluster-replicated secrets.
//!
//! The DEK itself never sits on disk — only sealed-box wraps (one per
//! node) live in Raft via [`zlayer_types::storage::WrappedDek`]. Each
//! node unwraps its own copy on startup with its node X25519 private key
//! ([`crate::sealed::RecipientPrivateKey`]), holds it in [`Zeroizing`]
//! memory, and uses it to en/decrypt [`zlayer_types::storage::ReplicatedSecret`]
//! ciphertexts via XChaCha20-Poly1305 with a random 24-byte nonce
//! prepended (matching the on-disk format used by [`crate::EncryptionKey`]).

use std::collections::HashMap;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::rngs::OsRng;
use rand::TryRngCore;
use zeroize::Zeroizing;

use zlayer_types::storage::WrappedDek;

use crate::sealed::{self, RecipientPrivateKey, RecipientPublicKey};
use crate::SecretsError;

/// Size of the DEK in bytes (XChaCha20-Poly1305 key size).
pub const DEK_SIZE: usize = 32;

/// Size of the XChaCha20-Poly1305 nonce in bytes.
pub const NONCE_SIZE: usize = 24;

/// The cluster-wide DEK. 32 bytes of key material.
///
/// Held in [`Zeroizing`] memory; zeroed on drop. The type does NOT
/// implement `Debug` / `Display` to keep accidental disclosure off
/// the table.
pub struct ClusterDek {
    bytes: Zeroizing<[u8; DEK_SIZE]>,
}

impl ClusterDek {
    /// Generate a fresh DEK from the operating system RNG.
    ///
    /// # Panics
    /// Panics if the OS random number generator fails.
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = Zeroizing::new([0u8; DEK_SIZE]);
        OsRng.try_fill_bytes(bytes.as_mut()).expect("OS RNG failed");
        Self { bytes }
    }

    /// Construct from raw bytes (e.g. after unwrapping). Bytes are copied
    /// into the zeroized buffer; the source is left untouched.
    #[must_use]
    pub fn from_bytes(bytes: [u8; DEK_SIZE]) -> Self {
        let mut buf = Zeroizing::new([0u8; DEK_SIZE]);
        buf.copy_from_slice(&bytes);
        Self { bytes: buf }
    }

    /// Sealed-box-wrap this DEK to a single recipient.
    ///
    /// Returns the raw libsodium sealed-box ciphertext bytes
    /// (`ephemeral_pubkey || box(dek)`), suitable for direct insertion
    /// into [`WrappedDek::wraps`].
    ///
    /// # Errors
    /// Returns [`SecretsError::Encryption`] if the sealed-box construction
    /// fails.
    pub fn wrap(&self, recipient: &RecipientPublicKey) -> Result<Vec<u8>, SecretsError> {
        sealed::seal_raw(self.bytes.as_ref(), recipient)
            .map_err(|e| SecretsError::Encryption(format!("DEK wrap failed: {e}")))
    }

    /// Sealed-box-wrap this DEK to every recipient in a node-id-keyed map
    /// and produce a [`WrappedDek`] envelope ready to commit through Raft.
    ///
    /// # Errors
    /// Returns [`SecretsError::Encryption`] if any per-recipient wrap fails.
    pub fn rewrap_for_set(
        &self,
        recipients: &HashMap<String, RecipientPublicKey>,
        new_generation: u64,
    ) -> Result<WrappedDek, SecretsError> {
        let mut wraps: HashMap<String, Vec<u8>> = HashMap::with_capacity(recipients.len());
        for (node_id, pubkey) in recipients {
            let wrapped = self.wrap(pubkey).map_err(|e| {
                SecretsError::Encryption(format!("DEK wrap failed for node {node_id}: {e}"))
            })?;
            wraps.insert(node_id.clone(), wrapped);
        }
        Ok(WrappedDek {
            dek_generation: new_generation,
            wraps,
        })
    }

    /// Unwrap a sealed-box-encrypted DEK using a node's X25519 private key.
    ///
    /// # Errors
    /// Returns [`SecretsError::Decryption`] if the sealed-box ciphertext
    /// fails authentication, is malformed, or does not decode to exactly
    /// 32 bytes of key material.
    pub fn unwrap(node_priv: &RecipientPrivateKey, wrapped: &[u8]) -> Result<Self, SecretsError> {
        let plaintext = sealed::open_raw(wrapped, node_priv)
            .map_err(|e| SecretsError::Decryption(format!("DEK unwrap failed: {e}")))?;
        if plaintext.len() != DEK_SIZE {
            return Err(SecretsError::Decryption(format!(
                "Unwrapped DEK has wrong length: expected {DEK_SIZE} bytes, got {}",
                plaintext.len()
            )));
        }
        // Copy into a fixed-size zeroized buffer; let the heap `Vec` drop
        // (its `Zeroize` impl below scrubs the bytes first).
        let mut buf = Zeroizing::new([0u8; DEK_SIZE]);
        buf.copy_from_slice(&plaintext);
        // Best-effort zeroize the heap-allocated unwrap buffer too.
        let mut plaintext = plaintext;
        zeroize::Zeroize::zeroize(&mut plaintext);
        Ok(Self { bytes: buf })
    }

    /// Encrypt a plaintext payload under the DEK using XChaCha20-Poly1305.
    /// On-wire format: `[24-byte nonce][ciphertext + tag]`.
    ///
    /// # Errors
    /// Returns [`SecretsError::Encryption`] if cipher construction or the
    /// AEAD encryption itself fails.
    ///
    /// # Panics
    /// Panics if the OS random number generator fails to produce nonce bytes.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, SecretsError> {
        let cipher = XChaCha20Poly1305::new_from_slice(self.bytes.as_ref())
            .map_err(|e| SecretsError::Encryption(format!("Failed to create cipher: {e}")))?;

        let mut nonce_bytes = [0u8; NONCE_SIZE];
        OsRng
            .try_fill_bytes(&mut nonce_bytes)
            .expect("OS RNG failed");
        let nonce = XNonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| SecretsError::Encryption(format!("Encryption failed: {e}")))?;

        let mut out = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypt the inverse of [`Self::encrypt`].
    ///
    /// Returns the plaintext wrapped in [`Zeroizing`] so it is scrubbed
    /// from memory when the caller drops it.
    ///
    /// # Errors
    /// Returns [`SecretsError::Decryption`] if:
    /// - `blob` is shorter than [`NONCE_SIZE`].
    /// - Cipher construction fails.
    /// - AEAD authentication or decryption fails.
    pub fn decrypt(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, SecretsError> {
        if blob.len() < NONCE_SIZE {
            return Err(SecretsError::Decryption(format!(
                "Data too short: expected at least {NONCE_SIZE} bytes for nonce, got {}",
                blob.len()
            )));
        }
        let cipher = XChaCha20Poly1305::new_from_slice(self.bytes.as_ref())
            .map_err(|e| SecretsError::Decryption(format!("Failed to create cipher: {e}")))?;

        let (nonce_bytes, ciphertext) = blob.split_at(NONCE_SIZE);
        let nonce = XNonce::from_slice(nonce_bytes);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| SecretsError::Decryption(format!("Decryption failed: {e}")))?;
        Ok(Zeroizing::new(plaintext))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dek_round_trip_wrap_unwrap() {
        let (sk, pk) = RecipientPrivateKey::generate();
        let dek = ClusterDek::generate();

        // `Zeroizing<[u8; 32]>` derefs to `[u8; 32]`, which is `Copy`.
        let original: [u8; DEK_SIZE] = *dek.bytes;

        let wrapped = dek.wrap(&pk).expect("wrap");
        let unwrapped = ClusterDek::unwrap(&sk, &wrapped).expect("unwrap");

        assert_eq!(*unwrapped.bytes, original);
    }

    #[test]
    fn dek_round_trip_encrypt_decrypt() {
        let dek = ClusterDek::generate();
        let payload = b"the rain in spain falls mainly on the plain";

        let blob = dek.encrypt(payload).expect("encrypt");
        // nonce + ciphertext + 16-byte poly1305 tag
        assert!(blob.len() > NONCE_SIZE + payload.len());

        let plaintext = dek.decrypt(&blob).expect("decrypt");
        assert_eq!(plaintext.as_slice(), payload);
    }

    #[test]
    fn dek_decrypt_tamper_detection() {
        let dek = ClusterDek::generate();
        let payload = b"important replicated secret";

        let mut blob = dek.encrypt(payload).expect("encrypt");

        // Flip a byte in the ciphertext region (after the 24-byte nonce).
        // Use a deterministic offset within the ciphertext to keep the test
        // reproducible; the tamper still triggers Poly1305 rejection.
        let target = NONCE_SIZE + 3;
        assert!(target < blob.len(), "blob too short to tamper");
        blob[target] ^= 0xA5;

        let result = dek.decrypt(&blob);
        assert!(matches!(result, Err(SecretsError::Decryption(_))));
    }

    #[test]
    fn rewrap_for_set_emits_per_node_wraps() {
        let (sk_a, pk_a) = RecipientPrivateKey::generate();
        let (sk_b, pk_b) = RecipientPrivateKey::generate();

        let mut recipients = HashMap::new();
        recipients.insert("node-a".to_string(), pk_a);
        recipients.insert("node-b".to_string(), pk_b);

        let dek = ClusterDek::generate();
        let original: [u8; DEK_SIZE] = *dek.bytes;

        let envelope = dek.rewrap_for_set(&recipients, 7).expect("rewrap");

        assert_eq!(envelope.dek_generation, 7);
        assert_eq!(envelope.wraps.len(), 2);
        assert!(envelope.wraps.contains_key("node-a"));
        assert!(envelope.wraps.contains_key("node-b"));

        // Each node can unwrap its own copy and recover the same DEK bytes.
        let unwrapped_a = ClusterDek::unwrap(&sk_a, &envelope.wraps["node-a"]).expect("unwrap a");
        let unwrapped_b = ClusterDek::unwrap(&sk_b, &envelope.wraps["node-b"]).expect("unwrap b");

        assert_eq!(*unwrapped_a.bytes, original);
        assert_eq!(*unwrapped_b.bytes, original);
    }

    #[test]
    fn unwrap_with_wrong_key_fails() {
        let (_sk_a, pk_a) = RecipientPrivateKey::generate();
        let (sk_b, _pk_b) = RecipientPrivateKey::generate();

        let dek = ClusterDek::generate();
        let wrapped = dek.wrap(&pk_a).expect("wrap to A");

        let result = ClusterDek::unwrap(&sk_b, &wrapped);
        assert!(matches!(result, Err(SecretsError::Decryption(_))));
    }
}
