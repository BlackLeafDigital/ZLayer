//! Worker-tier bootstrap tokens.
//!
//! Tokens are short-lived, cluster-CA-signed credentials a worker presents
//! during gRPC `Register`. The cluster signer (Ed25519) already used for join
//! tokens signs these too — reuse, don't duplicate identity material.
//!
//! Token format (postcard2-encoded, then URL-safe-base64 for the CLI flag):
//!
//! ```text
//! WorkerBootstrapToken {
//!     claims: WorkerBootstrapClaims {
//!         domain_tag: "zlayer-worker-bootstrap-v1",
//!         cluster_id: String,
//!         jti: Uuid (as String),
//!         issued_at_unix: i64,
//!         expires_at_unix: i64,
//!         max_uses: u32,
//!         permitted_labels: Vec<(String, String)>,
//!     },
//!     signer_kid: String,
//!     signature_b64: String,
//! }
//! ```
//!
//! The signed payload covers every claim (postcard2-encoded
//! [`WorkerBootstrapClaims`]).
//!
//! Usage counting is the caller's responsibility — typically a
//! `SecretsRaftOp` that records `jti → uses` in the FSM. Verification only
//! checks signature + expiry; the caller passes the current usage count and
//! compares against `max_uses`.
//!
//! Multi-key (rotation/grace) verification: the caller is responsible for
//! looking up the right signer by `token.signer_kid` via
//! [`crate::load_signer_for_kid`] before calling
//! [`verify_worker_bootstrap_token`]. This module verifies against a single
//! [`ClusterSigner`] whose `key_id()` must equal the token's `signer_kid`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ed25519_dalek::Verifier;
use serde::{Deserialize, Serialize};

use crate::{ClusterSigner, Result, SecretsError};

/// Tag string written into the signed payload so a verifier can't confuse
/// a worker bootstrap token with some other Ed25519-signed blob.
const DOMAIN_TAG: &str = "zlayer-worker-bootstrap-v1";

/// Token claims (the signed portion).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerBootstrapClaims {
    /// `domain_tag` — equals the module-internal domain constant. Reject if
    /// mismatched.
    pub domain_tag: String,
    /// Cluster ID this token belongs to (random UUID issued at bootstrap).
    pub cluster_id: String,
    /// Unique token ID — used by the caller's usage-tracking layer.
    pub jti: String,
    /// Unix-seconds when the token was issued.
    pub issued_at_unix: i64,
    /// Unix-seconds when the token expires.
    pub expires_at_unix: i64,
    /// Maximum number of times this token may be redeemed. 0 means unlimited
    /// (not recommended outside dev).
    pub max_uses: u32,
    /// Optional label whitelist — when non-empty, the worker's profile must
    /// declare each of these labels (any extra labels are ignored).
    #[serde(default)]
    pub permitted_labels: Vec<(String, String)>,
}

/// Full signed token (claims + signer kid + signature).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerBootstrapToken {
    pub claims: WorkerBootstrapClaims,
    /// Key id of the signing key (matches [`ClusterSigner::key_id`]).
    pub signer_kid: String,
    /// Ed25519 signature over postcard2-encoded claims, base64-url-no-pad.
    pub signature_b64: String,
}

impl WorkerBootstrapToken {
    /// Encode the token as a single URL-safe-base64 string suitable for CLI
    /// flags / files.
    ///
    /// # Errors
    ///
    /// Returns [`SecretsError::Encryption`] on postcard2 serialization
    /// failure.
    pub fn to_cli_string(&self) -> Result<String> {
        let bytes = postcard2::to_vec(self)
            .map_err(|e| SecretsError::Encryption(format!("encode worker token: {e}")))?;
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Decode a token previously produced by [`Self::to_cli_string`].
    ///
    /// # Errors
    ///
    /// Returns [`SecretsError::Encryption`] on base64 / postcard2 failure.
    pub fn from_cli_string(s: &str) -> Result<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|e| SecretsError::Encryption(format!("decode worker token base64: {e}")))?;
        postcard2::from_bytes(&bytes)
            .map_err(|e| SecretsError::Encryption(format!("decode worker token postcard2: {e}")))
    }
}

/// Issue a fresh bootstrap token signed by the supplied [`ClusterSigner`].
///
/// # Errors
///
/// Returns [`SecretsError::Encryption`] on encoding failure.
pub fn issue_worker_bootstrap_token(
    signer: &ClusterSigner,
    cluster_id: impl Into<String>,
    valid_for_secs: i64,
    max_uses: u32,
    permitted_labels: Vec<(String, String)>,
) -> Result<WorkerBootstrapToken> {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let jti = uuid::Uuid::new_v4().to_string();

    let claims = WorkerBootstrapClaims {
        domain_tag: DOMAIN_TAG.into(),
        cluster_id: cluster_id.into(),
        jti,
        issued_at_unix: now,
        expires_at_unix: now + valid_for_secs,
        max_uses,
        permitted_labels,
    };

    let payload = postcard2::to_vec(&claims)
        .map_err(|e| SecretsError::Encryption(format!("encode bootstrap claims: {e}")))?;

    let sig_bytes = signer.sign(&payload);

    Ok(WorkerBootstrapToken {
        claims,
        signer_kid: signer.key_id(),
        signature_b64: URL_SAFE_NO_PAD.encode(sig_bytes),
    })
}

/// Verify a token's signature, domain tag, and expiry. The caller is
/// responsible for `max_uses` tracking (typically via the Raft FSM).
///
/// `signer` must be the [`ClusterSigner`] whose [`ClusterSigner::key_id`]
/// equals `token.signer_kid` — for in-grace keys, the caller should look up
/// the right signer via [`crate::load_signer_for_kid`] before calling this.
///
/// Returns the claims on success — caller checks `jti`/`max_uses` against
/// the usage counter.
///
/// # Errors
///
/// Returns [`SecretsError::Encryption`] with a human-readable reason on any
/// validation failure.
pub fn verify_worker_bootstrap_token(
    signer: &ClusterSigner,
    token: &WorkerBootstrapToken,
) -> Result<WorkerBootstrapClaims> {
    if token.claims.domain_tag != DOMAIN_TAG {
        return Err(SecretsError::Encryption(format!(
            "wrong token domain: expected {DOMAIN_TAG}, got {}",
            token.claims.domain_tag
        )));
    }

    if signer.key_id() != token.signer_kid {
        return Err(SecretsError::Encryption(format!(
            "signer kid mismatch: signer has {}, token claims {}",
            signer.key_id(),
            token.signer_kid
        )));
    }

    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if now >= token.claims.expires_at_unix {
        return Err(SecretsError::Encryption("token expired".into()));
    }
    if token.claims.issued_at_unix > now + 60 {
        return Err(SecretsError::Encryption(
            "token issued more than 60s in the future".into(),
        ));
    }

    let sig_bytes = URL_SAFE_NO_PAD
        .decode(&token.signature_b64)
        .map_err(|e| SecretsError::Encryption(format!("decode token signature: {e}")))?;
    let sig_array: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| SecretsError::Encryption("token signature wrong length".into()))?;
    let signature = ed25519_dalek::Signature::from_bytes(&sig_array);

    let payload = postcard2::to_vec(&token.claims)
        .map_err(|e| SecretsError::Encryption(format!("re-encode claims: {e}")))?;
    signer
        .verifying_key()
        .verify(&payload, &signature)
        .map_err(|e| SecretsError::Encryption(format!("token signature invalid: {e}")))?;

    Ok(token.claims.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn make_signer() -> (ClusterSigner, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("cluster_signer.json");
        let signer = ClusterSigner::load_or_generate(&path)
            .await
            .expect("load_or_generate");
        (signer, dir)
    }

    #[tokio::test]
    async fn issue_and_verify_round_trip() {
        let (signer, _dir) = make_signer().await;
        let token = issue_worker_bootstrap_token(
            &signer,
            "cluster-abc",
            3600,
            1,
            vec![("region".into(), "us-east".into())],
        )
        .expect("issue");

        let s = token.to_cli_string().expect("encode");
        let parsed = WorkerBootstrapToken::from_cli_string(&s).expect("decode");
        assert_eq!(token, parsed);

        let claims = verify_worker_bootstrap_token(&signer, &parsed).expect("verify");
        assert_eq!(claims.cluster_id, "cluster-abc");
        assert_eq!(claims.max_uses, 1);
        assert_eq!(
            claims.permitted_labels,
            vec![("region".into(), "us-east".into())]
        );
    }

    #[tokio::test]
    async fn expired_token_rejected() {
        let (signer, _dir) = make_signer().await;
        let mut token = issue_worker_bootstrap_token(&signer, "c", 3600, 1, vec![]).expect("issue");
        token.claims.expires_at_unix = 0; // far past

        // Re-sign so it's a "valid signature on a stale claims" token (the
        // attacker case: tampering with expiry).
        let payload = postcard2::to_vec(&token.claims).unwrap();
        let sig = signer.sign(&payload);
        token.signer_kid = signer.key_id();
        token.signature_b64 = URL_SAFE_NO_PAD.encode(sig);

        let err = verify_worker_bootstrap_token(&signer, &token).unwrap_err();
        assert!(format!("{err}").contains("expired"));
    }

    #[tokio::test]
    async fn tampered_signature_rejected() {
        let (signer, _dir) = make_signer().await;
        let token = issue_worker_bootstrap_token(&signer, "c", 3600, 1, vec![]).expect("issue");

        let mut bad = token.clone();
        bad.claims.cluster_id = "different-cluster".into();
        // Don't re-sign — the original signature is over the original claims,
        // so the modified token's payload won't verify.

        let err = verify_worker_bootstrap_token(&signer, &bad).unwrap_err();
        assert!(format!("{err}").contains("signature invalid"));
    }

    #[tokio::test]
    async fn wrong_domain_tag_rejected() {
        let (signer, _dir) = make_signer().await;
        let mut token = issue_worker_bootstrap_token(&signer, "c", 3600, 1, vec![]).expect("issue");
        token.claims.domain_tag = "other-domain".into();
        let err = verify_worker_bootstrap_token(&signer, &token).unwrap_err();
        assert!(format!("{err}").contains("domain"));
    }
}
