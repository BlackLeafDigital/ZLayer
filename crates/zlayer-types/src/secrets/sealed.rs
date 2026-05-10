//! Sealed-box wire types and errors (recipient pubkey + ciphertext envelope).
//!
//! Lifted into `zlayer-types` so cross-crate consumers can name the wire
//! shape without depending on `zlayer-secrets`. The actual sealed-box
//! crypto (sealing/opening, `RecipientPrivateKey`, key-material zeroization)
//! stays in `zlayer-secrets` and consumes these wire types from here.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

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
