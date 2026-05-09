//! `NaCl` sealed-box wrapper for recipient-encrypted secret reads.
//!
//! Wire format is the standard libsodium sealed box (ephemeral pubkey || box(plaintext))
//! so `PyNaCl`, `tweetnacl-js`, libsodium nacl/box, and Go nacl/box all decrypt.
//! Pure `RustCrypto` — compiles to wasm32-unknown-unknown.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use crypto_box::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A sealed secret payload — recipient-encrypted ciphertext plus identifying metadata.
///
/// The ciphertext is base64-encoded (standard alphabet, with padding) so it can be
/// transported as a string and decrypted by any libsodium-compatible client.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SealedSecret {
    /// The name of the secret this payload corresponds to.
    pub name: String,
    /// The version of the secret at the time of sealing.
    pub version: u32,
    /// Identifier (typically a fingerprint) of the recipient public key.
    pub key_id: String,
    /// Standard-base64 (with padding) libsodium sealed-box ciphertext.
    pub ciphertext_b64: String,
}

/// Errors produced by sealed-box operations.
#[derive(Debug, thiserror::Error)]
pub enum SealedError {
    /// Base64 decoding of an input string failed.
    #[error("base64 decode failed: {0}")]
    Decode(#[from] base64::DecodeError),
    /// The decoded byte slice did not match the expected length for its role.
    #[error("ciphertext invalid length: {0}")]
    InvalidLength(usize),
    /// Sealing (encryption) failed.
    #[error("encryption failed")]
    Encrypt,
    /// Opening (decryption / authentication) failed.
    #[error("decryption failed")]
    Decrypt,
}

/// A 32-byte X25519 recipient public key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecipientPublicKey([u8; 32]);

impl RecipientPublicKey {
    /// Construct a recipient public key directly from its 32 raw bytes.
    #[must_use]
    pub fn from_bytes(b: [u8; 32]) -> Self {
        Self(b)
    }

    /// Decode a recipient public key from a standard-base64 (padded) string.
    ///
    /// # Errors
    /// Returns `SealedError::Decode` if the string is not valid base64, or
    /// `SealedError::InvalidLength` if it does not decode to exactly 32 bytes.
    pub fn from_base64(s: &str) -> Result<Self, SealedError> {
        let bytes = B64.decode(s)?;
        if bytes.len() != 32 {
            return Err(SealedError::InvalidLength(bytes.len()));
        }
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&bytes);
        Ok(Self(buf))
    }

    /// Encode this public key as standard-base64 (with padding).
    #[must_use]
    pub fn to_base64(&self) -> String {
        B64.encode(self.0)
    }

    /// Borrow the raw 32-byte representation.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// A 32-byte X25519 recipient private key.
///
/// The bytes are zeroed on drop. The type intentionally does **not** implement
/// `Debug` or `Display` to prevent accidental disclosure via logging.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct RecipientPrivateKey([u8; 32]);

impl RecipientPrivateKey {
    /// Generate a fresh X25519 keypair using the operating system RNG.
    #[must_use]
    pub fn generate() -> (RecipientPrivateKey, RecipientPublicKey) {
        let sk = SecretKey::generate(&mut crypto_box::aead::OsRng);
        let pk = sk.public_key();
        let sk_bytes: [u8; 32] = sk.to_bytes();
        let pk_bytes: [u8; 32] = pk.as_bytes().to_owned();
        (Self(sk_bytes), RecipientPublicKey(pk_bytes))
    }

    /// Construct a recipient private key directly from its 32 raw bytes.
    #[must_use]
    pub fn from_bytes(b: [u8; 32]) -> Self {
        Self(b)
    }

    /// Decode a recipient private key from a standard-base64 (padded) string.
    ///
    /// # Errors
    /// Returns `SealedError::Decode` if the string is not valid base64, or
    /// `SealedError::InvalidLength` if it does not decode to exactly 32 bytes.
    pub fn from_base64(s: &str) -> Result<Self, SealedError> {
        let bytes = B64.decode(s)?;
        if bytes.len() != 32 {
            return Err(SealedError::InvalidLength(bytes.len()));
        }
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&bytes);
        Ok(Self(buf))
    }

    /// Encode this private key as standard-base64 (with padding).
    ///
    /// Use with extreme care — exposing this string is equivalent to disclosing
    /// the key. Prefer keeping the key in memory and zeroing it on drop.
    #[must_use]
    pub fn to_base64(&self) -> String {
        B64.encode(self.0)
    }

    /// Derive the matching X25519 public key for this private key.
    #[must_use]
    pub fn public_key(&self) -> RecipientPublicKey {
        let sk = SecretKey::from_bytes(self.0);
        let pk = sk.public_key();
        RecipientPublicKey(pk.as_bytes().to_owned())
    }
}

/// Seal `plaintext` to `recipient_pub` using a libsodium-compatible sealed box.
///
/// Returns the standard-base64 (padded) encoding of the sealed-box ciphertext,
/// which has the wire format `ephemeral_pubkey (32 bytes) || box(plaintext)`.
///
/// # Errors
/// Returns `SealedError::Encrypt` if the underlying sealed-box construction fails.
pub fn seal(plaintext: &[u8], recipient_pub: &RecipientPublicKey) -> Result<String, SealedError> {
    let pk = PublicKey::from(*recipient_pub.as_bytes());
    let ct = pk
        .seal(&mut crypto_box::aead::OsRng, plaintext)
        .map_err(|_| SealedError::Encrypt)?;
    Ok(B64.encode(ct))
}

/// Open a base64-encoded libsodium sealed-box ciphertext with `recipient_priv`.
///
/// # Errors
/// Returns `SealedError::Decode` for malformed base64 input, or
/// `SealedError::Decrypt` if the ciphertext fails authentication or is
/// otherwise malformed.
pub fn open(
    ciphertext_b64: &str,
    recipient_priv: &RecipientPrivateKey,
) -> Result<Vec<u8>, SealedError> {
    let bytes = B64.decode(ciphertext_b64)?;
    let sk = SecretKey::from_bytes(recipient_priv.0);
    sk.unseal(&bytes).map_err(|_| SealedError::Decrypt)
}

/// Compute a stable, short fingerprint for a recipient public key.
///
/// Returns `sha256:<first-8-bytes-hex>` (16 hex chars). Useful as a stable
/// `key_id` so a client knows which recipient key was used to seal a payload.
#[must_use]
pub fn fingerprint(public_key: &RecipientPublicKey) -> String {
    let mut hasher = Sha256::new();
    hasher.update(public_key.as_bytes());
    let digest = hasher.finalize();
    format!("sha256:{}", hex::encode(&digest[..8]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;
    use rand::TryRngCore;

    #[test]
    fn roundtrip() {
        let (sk, pk) = RecipientPrivateKey::generate();

        let mut plaintext = [0u8; 32];
        OsRng.try_fill_bytes(&mut plaintext).expect("OS RNG failed");

        let ct = seal(&plaintext, &pk).expect("seal");
        let pt = open(&ct, &sk).expect("open");
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn tamper_byte_fails() {
        let (sk, pk) = RecipientPrivateKey::generate();
        let plaintext = b"super-secret-payload";

        let ct_b64 = seal(plaintext, &pk).expect("seal");

        // Decode, flip the last byte, re-encode.
        let mut bytes = B64.decode(&ct_b64).expect("decode");
        let last = bytes.last_mut().expect("non-empty ciphertext");
        *last ^= 0xFF;
        let tampered = B64.encode(&bytes);

        let result = open(&tampered, &sk);
        assert!(matches!(result, Err(SealedError::Decrypt)));
    }

    #[test]
    fn wrong_key_fails() {
        let (_sk_a, pk_a) = RecipientPrivateKey::generate();
        let (sk_b, _pk_b) = RecipientPrivateKey::generate();

        let plaintext = b"sealed to A only";
        let ct = seal(plaintext, &pk_a).expect("seal");

        let result = open(&ct, &sk_b);
        assert!(result.is_err());
    }

    #[test]
    fn fingerprint_stable() {
        let (_sk, pk) = RecipientPrivateKey::generate();
        let f1 = fingerprint(&pk);
        let f2 = fingerprint(&pk);
        assert_eq!(f1, f2);
        assert!(f1.starts_with("sha256:"));
    }

    #[test]
    fn pubkey_base64_roundtrip() {
        let (_sk, pk) = RecipientPrivateKey::generate();
        let s = pk.to_base64();
        let pk2 = RecipientPublicKey::from_base64(&s).expect("decode");
        assert_eq!(pk, pk2);
    }
}
