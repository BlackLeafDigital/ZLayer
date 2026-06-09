//! Ed25519 cluster signer for signed join tokens.
//!
//! A `ClusterSigner` holds an Ed25519 keypair used by the cluster leader to
//! sign join tokens (and verify them on the receiving end). The keypair is
//! persisted to disk as a JSON keystore (Unix mode 0600) so the cluster
//! identity survives daemon restarts and supports key rotation with grace
//! periods.
//!
//! ## On-disk format (`version = 1`)
//!
//! The keystore is a JSON document like:
//!
//! ```json
//! {
//!   "version": 1,
//!   "keys": [
//!     {
//!       "id": "abc12345",
//!       "seed_b64": "...",
//!       "created_at": "2026-05-14T17:55:00Z"
//!     }
//!   ],
//!   "active": "abc12345",
//!   "retired_grace_until": {}
//! }
//! ```
//!
//! - `version: u32` — file format version (1 for now). Future bumps can
//!   migrate cleanly.
//! - `keys` — every known signing keypair (active or in grace).
//! - `active` — the kid of the currently-active key. Must match exactly one
//!   entry in `keys`.
//! - `retired_grace_until` — kids of retired keys mapped to the timestamp at
//!   which their grace period expires. After that timestamp the entry is
//!   eligible for pruning.
//!
//! ## Legacy migration
//!
//! Wave 1 persisted the seed as exactly 32 raw bytes. `load_or_generate`
//! transparently detects that format (file exists, parses as 32 bytes, and
//! is not valid keystore JSON), migrates it to the new JSON layout in place
//! (atomic write via `{path}.tmp` then rename), and continues normally. The
//! migration is idempotent: running `load_or_generate` twice on a freshly-
//! migrated keystore performs no further writes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::sync::Mutex;

use crate::SecretsError;

/// Length of the on-disk raw seed in bytes (Ed25519 `SigningKey` seed size).
const SIGNING_KEY_SEED_LEN: usize = 32;

/// Current on-disk keystore format version.
pub(crate) const KEYSTORE_VERSION: u32 = 1;

/// One entry in the keystore — a single Ed25519 signing key with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KeyEntry {
    /// Short greppable identifier (first 8 hex chars of SHA-256(public key)).
    pub(crate) id: String,
    /// URL-safe no-pad base64 encoding of the 32-byte signing seed.
    pub(crate) seed_b64: String,
    /// When this key was generated.
    pub(crate) created_at: DateTime<Utc>,
}

/// The on-disk JSON keystore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KeyStoreFile {
    /// Format version (currently `1`).
    pub(crate) version: u32,
    /// Every known signing keypair, active or in grace.
    pub(crate) keys: Vec<KeyEntry>,
    /// Kid of the currently-active signing key. Must match exactly one
    /// `keys[].id`.
    pub(crate) active: String,
    /// Kids of retired keys mapped to the timestamp at which their grace
    /// period expires.
    #[serde(default)]
    pub(crate) retired_grace_until: HashMap<String, DateTime<Utc>>,
}

/// Ed25519 keypair used to sign cluster join tokens.
///
/// Cloning is intentionally not implemented: each `ClusterSigner` owns its
/// signing material and should be passed by reference through the daemon
/// (typically `Arc<ClusterSigner>`).
pub struct ClusterSigner {
    signing: SigningKey,
    public: VerifyingKey,
}

impl std::fmt::Debug for ClusterSigner {
    /// Redacts the private key material. Only the short, public `key_id` is
    /// printed so signing keys never leak via accidental `{:?}` formatting.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClusterSigner")
            .field("key_id", &self.key_id())
            .field("signing", &"<redacted>")
            .finish()
    }
}

impl ClusterSigner {
    /// Generate a fresh keypair from the OS CSPRNG.
    ///
    /// The seed is drawn directly from the OS via `rand::rngs::OsRng`
    /// (workspace `rand` 0.9) and fed to `SigningKey::from_bytes`. We avoid
    /// `SigningKey::generate` here because `ed25519-dalek 2.x` requires
    /// `rand_core 0.6`'s `CryptoRngCore` trait, while the workspace pins
    /// `rand 0.9` (whose `OsRng` implements `rand_core 0.9`'s `TryRngCore`).
    /// Filling 32 bytes via the workspace `rand` is equivalent: an Ed25519
    /// signing key is just 32 random bytes.
    ///
    /// # Panics
    /// Panics if the OS CSPRNG fails. This matches the behavior of
    /// `SigningKey::generate(&mut OsRng)` and is appropriate because key
    /// generation cannot proceed without entropy.
    #[must_use]
    pub fn generate() -> Self {
        let mut seed = [0u8; SIGNING_KEY_SEED_LEN];
        rand::rngs::OsRng
            .try_fill_bytes(&mut seed)
            .expect("OS CSPRNG must be available to generate a cluster signer key");
        let signing = SigningKey::from_bytes(&seed);
        let public = signing.verifying_key();
        Self { signing, public }
    }

    /// Load the keypair from the on-disk keystore at `path` if present;
    /// otherwise generate a fresh one, persist it as a JSON keystore with
    /// mode 0600, and return it. Returns the `ClusterSigner` for the
    /// currently-active key.
    ///
    /// If the file exists in the legacy raw-32-byte format (Wave 1), it is
    /// transparently migrated to the JSON keystore layout in place. The
    /// migration is idempotent.
    ///
    /// The parent directory of `path` is created if it does not exist
    /// (`mkdir -p` semantics).
    ///
    /// # Errors
    /// - [`SecretsError::Storage`] if the parent directory cannot be created,
    ///   if the file cannot be read or written, or if the file exists but is
    ///   neither valid keystore JSON nor a 32-byte legacy seed.
    /// - [`SecretsError::Storage`] if Unix file permissions cannot be set.
    pub async fn load_or_generate(path: &Path) -> Result<Self, SecretsError> {
        // 1. Ensure the parent directory exists.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                SecretsError::Storage(format!(
                    "Failed to create cluster signer directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        // 2. If the file does not exist, generate fresh and persist.
        if !fs::try_exists(path).await.map_err(|e| {
            SecretsError::Storage(format!(
                "Failed to stat cluster signer key file {}: {e}",
                path.display()
            ))
        })? {
            let signer = Self::generate();
            let entry = KeyEntry {
                id: signer.key_id(),
                seed_b64: URL_SAFE_NO_PAD.encode(signer.signing.to_bytes()),
                created_at: Utc::now(),
            };
            let store = KeyStoreFile {
                version: KEYSTORE_VERSION,
                active: entry.id.clone(),
                keys: vec![entry],
                retired_grace_until: HashMap::new(),
            };
            Self::write_keystore(path, &store).await?;
            return Ok(signer);
        }

        // 3. File exists — read it. `read_keystore` handles both the JSON
        //    format and the legacy 32-byte format, migrating the latter in
        //    place.
        let store = Self::read_keystore(path).await?;
        Self::from_keystore(&store, path)
    }

    /// Construct a `ClusterSigner` for the active key in `store`.
    fn from_keystore(store: &KeyStoreFile, path: &Path) -> Result<Self, SecretsError> {
        let active = store
            .keys
            .iter()
            .find(|k| k.id == store.active)
            .ok_or_else(|| {
                SecretsError::Storage(format!(
                    "cluster signer keystore {} declares active kid {:?} but no matching key entry exists",
                    path.display(),
                    store.active
                ))
            })?;

        let seed_bytes = URL_SAFE_NO_PAD.decode(&active.seed_b64).map_err(|e| {
            SecretsError::Storage(format!(
                "cluster signer keystore {} has invalid base64 seed for kid {:?}: {e}",
                path.display(),
                active.id
            ))
        })?;

        if seed_bytes.len() != SIGNING_KEY_SEED_LEN {
            return Err(SecretsError::Storage(format!(
                "cluster signer keystore {} has wrong seed length for kid {:?}: expected {}, got {}",
                path.display(),
                active.id,
                SIGNING_KEY_SEED_LEN,
                seed_bytes.len()
            )));
        }

        let mut seed = [0u8; SIGNING_KEY_SEED_LEN];
        seed.copy_from_slice(&seed_bytes);
        let signing = SigningKey::from_bytes(&seed);
        let public = signing.verifying_key();
        Ok(Self { signing, public })
    }

    /// Read the on-disk keystore at `path`. If the file is in the legacy
    /// raw-32-byte format from Wave 1, migrate it to the new JSON layout in
    /// place and return the migrated structure.
    ///
    /// This helper is `pub(crate)` so subsequent Wave 5A agents (rotate,
    /// multi-key validate, background prune) can reuse it without re-doing
    /// the migration logic.
    ///
    /// # Errors
    /// - [`SecretsError::Storage`] if the file cannot be read.
    /// - [`SecretsError::Storage`] if the file is neither valid keystore JSON
    ///   nor exactly 32 bytes (legacy format).
    /// - [`SecretsError::Storage`] if the legacy 32-byte file fails to
    ///   migrate (e.g., the rewrite step errors).
    pub(crate) async fn read_keystore(path: &Path) -> Result<KeyStoreFile, SecretsError> {
        let buf = fs::read(path).await.map_err(|e| {
            SecretsError::Storage(format!(
                "Failed to read cluster signer key file {}: {e}",
                path.display()
            ))
        })?;

        // Try JSON keystore first.
        match serde_json::from_slice::<KeyStoreFile>(&buf) {
            Ok(store) => Ok(store),
            Err(json_err) => {
                // Maybe legacy raw-seed format.
                if buf.len() == SIGNING_KEY_SEED_LEN {
                    let mut seed = [0u8; SIGNING_KEY_SEED_LEN];
                    seed.copy_from_slice(&buf);
                    let signing = SigningKey::from_bytes(&seed);
                    let public = signing.verifying_key();
                    let digest = Sha256::digest(public.as_bytes());
                    let kid = hex_short(&digest);
                    let entry = KeyEntry {
                        id: kid.clone(),
                        seed_b64: URL_SAFE_NO_PAD.encode(seed),
                        created_at: Utc::now(),
                    };
                    let store = KeyStoreFile {
                        version: KEYSTORE_VERSION,
                        active: kid,
                        keys: vec![entry],
                        retired_grace_until: HashMap::new(),
                    };
                    Self::write_keystore(path, &store).await?;
                    Ok(store)
                } else {
                    Err(SecretsError::Storage(format!(
                        "cluster signer key file {} has unexpected format: not valid keystore JSON ({json_err}) and not a {SIGNING_KEY_SEED_LEN}-byte legacy seed (got {} bytes)",
                        path.display(),
                        buf.len()
                    )))
                }
            }
        }
    }

    /// Persist `store` to `path` atomically with Unix mode 0600.
    ///
    /// Writes to `{path}.tmp` first (with mode 0600 from the start on Unix,
    /// so the seed bytes are never on disk under a more permissive mode),
    /// then renames over `path`. On Unix the rename is atomic within the
    /// same filesystem, so a crash mid-write cannot leave a half-written
    /// keystore at `path`.
    ///
    /// This helper is `pub(crate)` so subsequent Wave 5A agents can reuse
    /// it without re-implementing the atomic-write + 0600 dance.
    ///
    /// # Errors
    /// - [`SecretsError::Storage`] if serialization fails (should be
    ///   impossible for our types but propagated defensively).
    /// - [`SecretsError::Storage`] if the temp file cannot be created,
    ///   written, flushed, permissioned, or renamed.
    pub(crate) async fn write_keystore(
        path: &Path,
        store: &KeyStoreFile,
    ) -> Result<(), SecretsError> {
        let json = serde_json::to_vec_pretty(store).map_err(|e| {
            SecretsError::Storage(format!(
                "Failed to serialize cluster signer keystore for {}: {e}",
                path.display()
            ))
        })?;

        let tmp = tmp_path_for(path);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            use tokio::fs::OpenOptions;
            use tokio::io::AsyncWriteExt as _;

            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .await
                .map_err(|e| {
                    SecretsError::Storage(format!(
                        "Failed to create cluster signer keystore temp file {}: {e}",
                        tmp.display()
                    ))
                })?;

            file.write_all(&json).await.map_err(|e| {
                SecretsError::Storage(format!(
                    "Failed to write cluster signer keystore temp file {}: {e}",
                    tmp.display()
                ))
            })?;
            file.flush().await.map_err(|e| {
                SecretsError::Storage(format!(
                    "Failed to flush cluster signer keystore temp file {}: {e}",
                    tmp.display()
                ))
            })?;

            // Belt-and-suspenders: re-set 0600 on the temp file in case the
            // FS/umask gave us anything broader.
            let permissions = std::fs::Permissions::from_mode(0o600);
            fs::set_permissions(&tmp, permissions).await.map_err(|e| {
                SecretsError::Storage(format!(
                    "Failed to set permissions on cluster signer keystore temp file {}: {e}",
                    tmp.display()
                ))
            })?;
        }

        #[cfg(not(unix))]
        {
            fs::write(&tmp, &json).await.map_err(|e| {
                SecretsError::Storage(format!(
                    "Failed to write cluster signer keystore temp file {}: {e}",
                    tmp.display()
                ))
            })?;
        }

        // Atomic rename over the destination.
        fs::rename(&tmp, path).await.map_err(|e| {
            SecretsError::Storage(format!(
                "Failed to rename cluster signer keystore {} -> {}: {e}",
                tmp.display(),
                path.display()
            ))
        })?;

        Ok(())
    }

    /// The public verifying key.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.public
    }

    /// URL-safe no-pad base64 of the 32-byte verifying key.
    #[must_use]
    pub fn public_key_b64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.public.as_bytes())
    }

    /// First 8 hex chars of SHA-256(verifying_key bytes). Short, greppable
    /// identifier for log lines and token headers.
    #[must_use]
    pub fn key_id(&self) -> String {
        let digest = Sha256::digest(self.public.as_bytes());
        hex_short(&digest)
    }

    /// Sign a message. Returns the raw 64-byte Ed25519 signature.
    #[must_use]
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        let sig: Signature = self.signing.sign(msg);
        sig.to_bytes()
    }
}

/// Lowercase hex of the first 4 bytes of `digest`. The result is 8 hex
/// chars — the short, greppable `kid` form used in log lines, token
/// headers, and `CaCert::active_kid`.
fn hex_short(digest: &[u8]) -> String {
    hex::encode(&digest[..4])
}

/// Build the temp-file path used during atomic keystore writes.
///
/// We append a `.tmp` suffix to the filename rather than a sibling random
/// name so the temp lives in the same directory as the destination — that
/// directory is already guaranteed to be on the same filesystem, making the
/// subsequent `rename` atomic.
fn tmp_path_for(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(".tmp");
    PathBuf::from(os)
}

/// Process-local mutex serializing read-modify-write operations on the
/// keystore.
///
/// Rotations and grace-pruning are operator-driven (CLI / API) and rare,
/// so a single global mutex is sufficient. The on-disk file write itself
/// is already atomic via `rename`, but two concurrent rotations on the
/// same process could still race when computing the new state (one reads
/// the keystore before the other has written its update, then overwrites
/// the other's work). The mutex prevents that lost-update window.
///
/// We do not key the mutex by path because (a) only one keystore is ever
/// used per process in practice, (b) the operations are sub-millisecond,
/// and (c) keying introduces a per-path registry that complicates
/// lifetime management for no real benefit.
fn keystore_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Status of a key returned by [`list_valid_pubkeys`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PubkeyStatus {
    /// The currently-active signing key (used for newly-issued tokens).
    Active,
    /// A retired key still within its grace window, accepted for
    /// verification only.
    Grace,
}

/// One entry in the result of [`list_valid_pubkeys`].
///
/// `valid_until` is `None` for [`PubkeyStatus::Active`] (the active key has
/// no scheduled expiration — it remains valid until the next rotation),
/// and `Some(_)` for [`PubkeyStatus::Grace`] indicating when the key will
/// be pruned and stop being accepted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PubkeyInfo {
    /// The key's short identifier (first 8 hex chars of SHA-256(pubkey)).
    pub kid: String,
    /// URL-safe no-pad base64 of the 32-byte verifying key.
    pub public_key_b64: String,
    /// Whether this key is the active signer or in grace.
    pub status: PubkeyStatus,
    /// For [`PubkeyStatus::Grace`], the timestamp this key stops being
    /// accepted. `None` for [`PubkeyStatus::Active`].
    pub valid_until: Option<DateTime<Utc>>,
    /// When this key was generated.
    pub created_at: DateTime<Utc>,
}

/// The outcome of a [`rotate_keystore`] call.
///
/// Named `KeystoreRotationResult` to disambiguate from the existing
/// `crate::RotationResult` which describes generic secret rotations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeystoreRotationResult {
    /// The kid of the new active key.
    pub new_active_kid: String,
    /// URL-safe no-pad base64 of the new active key.
    pub new_active_public_key_b64: String,
    /// The kid that was previously active and is now in grace.
    pub previous_kid: String,
    /// When the previous-active key's grace period expires (RFC3339).
    /// After this, the key is purged on the next [`prune_expired_grace`].
    pub previous_grace_until: DateTime<Utc>,
}

/// Rotate the cluster signing keystore at `path`:
/// generate a fresh keypair, set it as active, and move the previous
/// active key into the grace map with expiration `now + grace`.
///
/// The new key is the only signer used for future tokens; the previous
/// key remains valid for token verification until `now + grace`, after
/// which [`prune_expired_grace`] removes it.
///
/// # Concurrency
/// This function takes a process-local mutex around the read-modify-write
/// cycle so two concurrent rotations in the same process cannot lose each
/// other's update. The mutex does NOT protect against cross-process races
/// — but `ZLayer` runs a single daemon per host, and rotations are
/// operator-driven (one CLI/API call at a time), so that scenario is
/// out of scope.
///
/// # Errors
/// - [`SecretsError::Storage`] if the keystore cannot be read or written.
/// - [`SecretsError::Storage`] (`"kid collision; retry rotation"`) on the
///   astronomically-unlikely event that the new kid collides with an
///   existing entry. Callers should retry on this error.
pub async fn rotate_keystore(
    path: &Path,
    grace: std::time::Duration,
) -> Result<KeystoreRotationResult, SecretsError> {
    let _guard = keystore_lock().lock().await;

    // 1. Read existing keystore.
    let mut store = ClusterSigner::read_keystore(path).await?;
    let old_active = store.active.clone();

    // 2. Generate fresh seed via OS CSPRNG.
    let mut seed = [0u8; SIGNING_KEY_SEED_LEN];
    rand::rngs::OsRng
        .try_fill_bytes(&mut seed)
        .map_err(|e| SecretsError::Storage(format!("OS CSPRNG failed during rotation: {e}")))?;
    let signing = SigningKey::from_bytes(&seed);
    let public = signing.verifying_key();
    let new_kid = {
        let digest = Sha256::digest(public.as_bytes());
        hex_short(&digest)
    };
    let new_pub_b64 = URL_SAFE_NO_PAD.encode(public.as_bytes());

    // 3. Refuse to overwrite an existing kid — even though the chance is
    //    ~1 in 2^32, a real collision would silently invalidate the
    //    previous key. Bail and let the caller retry.
    if store.keys.iter().any(|k| k.id == new_kid) {
        return Err(SecretsError::Storage(format!(
            "kid collision; retry rotation (new_kid={new_kid} already in keystore)"
        )));
    }

    // 4. Compute grace expiry. `chrono::Duration::from_std` only fails on
    //    durations exceeding i64::MAX milliseconds — clamp defensively.
    let now = Utc::now();
    let grace_chrono = chrono::Duration::from_std(grace).unwrap_or(chrono::Duration::MAX);
    let previous_grace_until = now
        .checked_add_signed(grace_chrono)
        .unwrap_or(DateTime::<Utc>::MAX_UTC);

    // 5. Append new entry, retire old active, swap.
    store.keys.push(KeyEntry {
        id: new_kid.clone(),
        seed_b64: URL_SAFE_NO_PAD.encode(seed),
        created_at: now,
    });
    store
        .retired_grace_until
        .insert(old_active.clone(), previous_grace_until);
    store.active.clone_from(&new_kid);

    // 6. Persist atomically.
    ClusterSigner::write_keystore(path, &store).await?;

    Ok(KeystoreRotationResult {
        new_active_kid: new_kid,
        new_active_public_key_b64: new_pub_b64,
        previous_kid: old_active,
        previous_grace_until,
    })
}

/// Load a [`ClusterSigner`] for a specific `kid` from the keystore at
/// `path` if and only if that kid is currently trusted.
///
/// A kid is currently trusted when:
/// - it is the active signing key (`store.active == kid`), OR
/// - it is in `store.retired_grace_until` AND the recorded expiration is
///   still in the future.
///
/// Returns `Ok(None)` if the kid is not in the keystore at all, or if it
/// is in the keystore but its grace window has already elapsed (which
/// shouldn't normally happen because [`prune_expired_grace`] removes such
/// entries on the daemon's hourly tick).
///
/// This is the verify-side counterpart to [`rotate_keystore`]: token
/// validation looks up the signer matching the token's `kid` header and
/// returns `Some(_)` only when the key is currently valid.
///
/// # Errors
/// - [`SecretsError::Storage`] if the keystore cannot be read or contains
///   a malformed seed.
pub async fn load_signer_for_kid(
    path: &Path,
    kid: &str,
) -> Result<Option<ClusterSigner>, SecretsError> {
    let store = ClusterSigner::read_keystore(path).await?;

    let Some(entry) = store.keys.iter().find(|k| k.id == kid) else {
        return Ok(None);
    };

    // Decide whether this kid is currently valid.
    let valid = if store.active == kid {
        true
    } else if let Some(expires_at) = store.retired_grace_until.get(kid) {
        Utc::now() < *expires_at
    } else {
        // Present in `keys` but neither active nor in grace — leftover
        // entry that should have been pruned. Treat as invalid.
        false
    };

    if !valid {
        return Ok(None);
    }

    let seed_bytes = URL_SAFE_NO_PAD.decode(&entry.seed_b64).map_err(|e| {
        SecretsError::Storage(format!(
            "cluster signer keystore {} has invalid base64 seed for kid {:?}: {e}",
            path.display(),
            entry.id
        ))
    })?;
    if seed_bytes.len() != SIGNING_KEY_SEED_LEN {
        return Err(SecretsError::Storage(format!(
            "cluster signer keystore {} has wrong seed length for kid {:?}: expected {}, got {}",
            path.display(),
            entry.id,
            SIGNING_KEY_SEED_LEN,
            seed_bytes.len()
        )));
    }
    let mut seed = [0u8; SIGNING_KEY_SEED_LEN];
    seed.copy_from_slice(&seed_bytes);
    let signing = SigningKey::from_bytes(&seed);
    let public = signing.verifying_key();
    Ok(Some(ClusterSigner { signing, public }))
}

/// List every key in the keystore that is currently valid (active or
/// in-grace and not yet expired).
///
/// Order: the active key first, then grace entries sorted by descending
/// `valid_until` (closest-to-expiry-last, so newer grace entries come
/// before older ones). Stale entries whose `retired_grace_until` is
/// already in the past are omitted (they're pending prune).
///
/// # Errors
/// - [`SecretsError::Storage`] if the keystore cannot be read.
pub async fn list_valid_pubkeys(path: &Path) -> Result<Vec<PubkeyInfo>, SecretsError> {
    let store = ClusterSigner::read_keystore(path).await?;
    let now = Utc::now();

    let mut active_info: Option<PubkeyInfo> = None;
    let mut grace_infos: Vec<PubkeyInfo> = Vec::new();

    for entry in &store.keys {
        let seed_bytes = URL_SAFE_NO_PAD.decode(&entry.seed_b64).map_err(|e| {
            SecretsError::Storage(format!(
                "cluster signer keystore {} has invalid base64 seed for kid {:?}: {e}",
                path.display(),
                entry.id
            ))
        })?;
        if seed_bytes.len() != SIGNING_KEY_SEED_LEN {
            return Err(SecretsError::Storage(format!(
                "cluster signer keystore {} has wrong seed length for kid {:?}: expected {}, got {}",
                path.display(),
                entry.id,
                SIGNING_KEY_SEED_LEN,
                seed_bytes.len()
            )));
        }
        let mut seed = [0u8; SIGNING_KEY_SEED_LEN];
        seed.copy_from_slice(&seed_bytes);
        let public = SigningKey::from_bytes(&seed).verifying_key();
        let public_key_b64 = URL_SAFE_NO_PAD.encode(public.as_bytes());

        if entry.id == store.active {
            active_info = Some(PubkeyInfo {
                kid: entry.id.clone(),
                public_key_b64,
                status: PubkeyStatus::Active,
                valid_until: None,
                created_at: entry.created_at,
            });
        } else if let Some(expires_at) = store.retired_grace_until.get(&entry.id) {
            if now < *expires_at {
                grace_infos.push(PubkeyInfo {
                    kid: entry.id.clone(),
                    public_key_b64,
                    status: PubkeyStatus::Grace,
                    valid_until: Some(*expires_at),
                    created_at: entry.created_at,
                });
            }
            // else: expired, pending prune — skip.
        }
        // else: leftover entry with no grace record; skip.
    }

    // Sort grace by descending `valid_until` (latest expiry first).
    grace_infos.sort_by(|a, b| b.valid_until.cmp(&a.valid_until));

    let mut out = Vec::with_capacity(1 + grace_infos.len());
    if let Some(active) = active_info {
        out.push(active);
    }
    out.extend(grace_infos);
    Ok(out)
}

/// Remove every keystore entry whose grace window has expired.
///
/// Iterates `retired_grace_until`, drops every kid whose expiration is
/// `<= now`, and also removes the matching entries from `store.keys`.
/// Persists the keystore only if at least one key was pruned (so the
/// hot-loop "nothing to do" path performs zero I/O writes).
///
/// Wave 5A.5 wires a background daemon task to call this hourly.
///
/// # Errors
/// - [`SecretsError::Storage`] if the keystore cannot be read or written.
///
/// # Returns
/// The number of keys pruned.
pub async fn prune_expired_grace(path: &Path) -> Result<usize, SecretsError> {
    let _guard = keystore_lock().lock().await;

    let mut store = ClusterSigner::read_keystore(path).await?;
    let now = Utc::now();

    // Collect kids whose grace has expired.
    let expired: Vec<String> = store
        .retired_grace_until
        .iter()
        .filter_map(|(kid, expires)| {
            if *expires <= now {
                Some(kid.clone())
            } else {
                None
            }
        })
        .collect();

    if expired.is_empty() {
        return Ok(0);
    }

    // Drop them from both maps. We never prune the active key even if
    // (somehow) it ended up in `retired_grace_until`.
    for kid in &expired {
        if *kid == store.active {
            continue;
        }
        store.retired_grace_until.remove(kid);
        store.keys.retain(|k| &k.id != kid);
    }

    let pruned = expired.iter().filter(|k| **k != store.active).count();
    if pruned == 0 {
        // We only had the active-key edge case above.
        return Ok(0);
    }

    ClusterSigner::write_keystore(path, &store).await?;
    Ok(pruned)
}

/// Long-lived cluster CA keypair.
///
/// Identifies this cluster across the entire federation. NEVER
/// rotated — rotation would invalidate every `CaCert` this cluster has
/// ever issued and break federation trust. Stored as a raw 32-byte
/// Ed25519 seed at `{data_dir}/cluster_ca.key` (0600).
///
/// The per-rotation signing keys (see [`ClusterSigner`]) live in a
/// separate JSON keystore at `cluster_signing.key`. The CA key is
/// deliberately not part of that keystore so it survives the
/// rotation-and-prune machinery.
pub struct ClusterCa {
    signing: ed25519_dalek::SigningKey,
    public: ed25519_dalek::VerifyingKey,
}

impl std::fmt::Debug for ClusterCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClusterCa")
            .field("ca_kid", &self.ca_kid())
            .finish_non_exhaustive()
    }
}

impl ClusterCa {
    /// Generate a fresh CA keypair using the OS CSPRNG.
    ///
    /// # Panics
    ///
    /// Panics if the OS CSPRNG fails — same fail-loud behavior as
    /// [`ClusterSigner::generate`].
    #[must_use]
    pub fn generate() -> Self {
        use rand::TryRngCore;
        let mut seed = [0u8; 32];
        rand::rngs::OsRng
            .try_fill_bytes(&mut seed)
            .expect("OS CSPRNG must be available to generate a cluster CA key");
        let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
        let public = signing.verifying_key();
        Self { signing, public }
    }

    /// Load the CA seed from `path`, or generate + persist a fresh one
    /// if the file does not exist.
    ///
    /// On creation the file is written atomically (tmp + rename) with
    /// mode 0600 on Unix. The 32-byte seed is the entirety of the
    /// file — no JSON, no headers; this is intentionally a simpler
    /// format than the signing keystore because the CA key never
    /// rotates.
    ///
    /// # Errors
    ///
    /// - [`SecretsError::Storage`] for any IO error reading or writing
    ///   the file, or if an existing file has the wrong length.
    pub async fn load_or_generate(path: &std::path::Path) -> Result<Self, SecretsError> {
        if tokio::fs::try_exists(path)
            .await
            .map_err(|e| SecretsError::Storage(format!("checking {}: {e}", path.display())))?
        {
            let bytes = tokio::fs::read(path)
                .await
                .map_err(|e| SecretsError::Storage(format!("reading {}: {e}", path.display())))?;
            if bytes.len() != 32 {
                return Err(SecretsError::Storage(format!(
                    "cluster_ca.key at {} has wrong length: expected 32 bytes, got {}",
                    path.display(),
                    bytes.len()
                )));
            }
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes);
            let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
            let public = signing.verifying_key();
            return Ok(Self { signing, public });
        }

        let ca = Self::generate();
        let seed = ca.signing.to_bytes();
        let tmp_path = path.with_extension("ca.tmp");
        tokio::fs::write(&tmp_path, &seed[..])
            .await
            .map_err(|e| SecretsError::Storage(format!("writing tmp ca file: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            tokio::fs::set_permissions(&tmp_path, perms)
                .await
                .map_err(|e| SecretsError::Storage(format!("chmod 0600 ca tmp: {e}")))?;
        }
        tokio::fs::rename(&tmp_path, path)
            .await
            .map_err(|e| SecretsError::Storage(format!("rename ca tmp to final: {e}")))?;
        Ok(ca)
    }

    /// CA verifying key as URL-safe no-pad base64.
    #[must_use]
    pub fn ca_public_key_b64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.public.as_bytes())
    }

    /// Short CA key id: first 8 hex chars of SHA-256(CA verifying key bytes).
    #[must_use]
    pub fn ca_kid(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.public.as_bytes());
        let digest = hasher.finalize();
        hex_short(&digest)
    }

    /// CA verifying key for verification (not exported through trait).
    #[must_use]
    pub fn verifying_key(&self) -> ed25519_dalek::VerifyingKey {
        self.public
    }

    /// Build a [`CaCert`] binding `active_kid` (whose verifying-key
    /// base64 is `active_pubkey_b64`) to `cluster_domain` for the
    /// window `[now, now + grace]`. The result's `sig_by_ca` is the
    /// CA's Ed25519 signature over the canonical bytes of the body.
    ///
    /// "Canonical bytes" = `serde_json::to_vec` of a `CaCert` with the
    /// signature field cleared (`""`). Verifiers must do the same
    /// transformation before calling `verify_strict`.
    ///
    /// # Errors
    ///
    /// - [`SecretsError::Provider`] if serializing the canonical body
    ///   fails (should not happen with the well-formed `CaCert` struct).
    pub fn issue_ca_cert(
        &self,
        active_kid: String,
        active_pubkey_b64: String,
        cluster_domain: String,
        grace: std::time::Duration,
    ) -> Result<zlayer_types::api::cluster::CaCert, SecretsError> {
        use ed25519_dalek::Signer;

        let now = chrono::Utc::now();
        let issued_at = now.to_rfc3339();
        let expires_at = (now
            + chrono::Duration::from_std(grace)
                .map_err(|e| SecretsError::Provider(format!("grace out of range: {e}")))?)
        .to_rfc3339();

        let mut cert = zlayer_types::api::cluster::CaCert {
            v: zlayer_types::api::cluster::CA_CERT_FORMAT_VERSION,
            active_kid,
            active_pubkey_b64,
            issued_at,
            expires_at,
            cluster_domain,
            sig_by_ca: String::new(),
        };
        let body_bytes = serde_json::to_vec(&cert).map_err(|e| {
            SecretsError::Provider(format!("serializing CaCert body for signing: {e}"))
        })?;
        let sig = self.signing.sign(&body_bytes);
        cert.sig_by_ca = URL_SAFE_NO_PAD.encode(sig.to_bytes());
        Ok(cert)
    }

    /// Verify a `CaCert` against a CA public key.
    ///
    /// The CA public key is the bytes that an importer obtained
    /// out-of-band from a [`crate::TrustBundle`]. Verification:
    /// 1. Decodes the `sig_by_ca` base64.
    /// 2. Recomputes the canonical body bytes (`sig_by_ca` cleared).
    /// 3. Calls `verify_strict` on the CA pubkey.
    /// 4. Checks `expires_at > now` so an expired cert is rejected.
    ///
    /// Does NOT check `cluster_domain` — that's the caller's job
    /// (typically: "does the cert's `cluster_domain` match the
    /// `TrustBundle` we looked up by domain?").
    ///
    /// # Errors
    ///
    /// - [`SecretsError::Provider`] for any decode/verification/expiry
    ///   failure with an actionable message.
    pub fn verify_ca_cert(
        ca_pubkey_b64: &str,
        cert: &zlayer_types::api::cluster::CaCert,
    ) -> Result<(), SecretsError> {
        let ca_pubkey_bytes = URL_SAFE_NO_PAD
            .decode(ca_pubkey_b64.as_bytes())
            .map_err(|e| SecretsError::Provider(format!("CA pubkey base64 decode: {e}")))?;
        let ca_pubkey_arr: [u8; 32] = ca_pubkey_bytes.as_slice().try_into().map_err(|_| {
            SecretsError::Provider(format!("CA pubkey wrong length: {}", ca_pubkey_bytes.len()))
        })?;
        let ca_pubkey = ed25519_dalek::VerifyingKey::from_bytes(&ca_pubkey_arr)
            .map_err(|e| SecretsError::Provider(format!("invalid CA pubkey: {e}")))?;

        let sig_bytes = URL_SAFE_NO_PAD
            .decode(cert.sig_by_ca.as_bytes())
            .map_err(|e| SecretsError::Provider(format!("sig_by_ca base64 decode: {e}")))?;
        let sig_arr: [u8; 64] = sig_bytes.as_slice().try_into().map_err(|_| {
            SecretsError::Provider(format!("sig_by_ca wrong length: {}", sig_bytes.len()))
        })?;
        let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);

        let mut body = cert.clone();
        body.sig_by_ca = String::new();
        let body_bytes = serde_json::to_vec(&body).map_err(|e| {
            SecretsError::Provider(format!("recomputing CaCert canonical body: {e}"))
        })?;
        ca_pubkey.verify_strict(&body_bytes, &sig).map_err(|e| {
            SecretsError::Provider(format!("CA signature verification failed: {e}"))
        })?;

        let exp = chrono::DateTime::parse_from_rfc3339(&cert.expires_at)
            .map_err(|e| SecretsError::Provider(format!("CaCert expires_at parse: {e}")))?
            .with_timezone(&chrono::Utc);
        if chrono::Utc::now() >= exp {
            return Err(SecretsError::Provider(format!(
                "CaCert expired at {}",
                cert.expires_at
            )));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SigningBackend trait (Wave 8 — minimal scope)
//
// The trait abstracts where the keystore lives so future hardware-backed
// implementations (TPM 2.0, YubiHSM, cloud KMS) can swap in without
// touching call sites. The current shipping implementation is `FileBackend`,
// which delegates to the JSON keystore on disk via the existing free
// functions. TPM/YubiHSM impls are deliberately out of scope for this
// wave — the trait is the extension point only.
// ---------------------------------------------------------------------------

/// Abstract interface over a cluster-signing-key store.
///
/// Implementations may live on local disk (`FileBackend`), in a TPM,
/// in an HSM, or in a cloud KMS — the trait keeps the call sites
/// agnostic. The default and only shipped implementation today is
/// [`FileBackend`].
///
/// All methods are async because hardware backends inevitably involve
/// IO (TPM sessions, network round-trips to KMS, etc.). The file
/// backend's IO is the existing tokio-fs path.
#[async_trait::async_trait]
pub trait SigningBackend: Send + Sync + std::fmt::Debug {
    /// Returns a human-readable name (`"file"`, `"tpm"`, …) for log
    /// lines and `--key-store-backend` debug output.
    fn name(&self) -> &'static str;

    /// Returns `true` if private key material lives in tamper-resistant
    /// hardware. Pure-software backends return `false`. Used for
    /// startup logging and the "key-store-backend: tpm (hw-backed)"
    /// banner.
    fn is_hardware_backed(&self) -> bool;

    /// Lowercase hex first-8-chars-of-SHA256 of the currently-active
    /// verifying key. Stable across processes for the same key.
    async fn active_key_id(&self) -> Result<String, SecretsError>;

    /// URL-safe no-pad base64 of the verifying key bytes for the given
    /// `kid`. Returns `Ok(None)` if the kid is unknown or its grace
    /// window has expired.
    async fn public_key_b64(&self, kid: &str) -> Result<Option<String>, SecretsError>;

    /// All currently-valid (active OR not-yet-expired grace) keys in
    /// the store with their statuses.
    async fn list_valid_pubkeys(&self) -> Result<Vec<PubkeyInfo>, SecretsError>;

    /// Sign `msg` with the key identified by `kid`. Fails with
    /// `SecretsError::Provider` if the kid is unknown or expired.
    /// Note that signing with a grace-window key is unusual (callers
    /// typically only sign with the active key) but supported for
    /// recovery scenarios.
    async fn sign(&self, kid: &str, msg: &[u8]) -> Result<[u8; 64], SecretsError>;

    /// Rotate the keystore: generate a new active key, move the
    /// previous active into the grace window for `grace`, and return
    /// the new active kid + public key. Idempotent only in the sense
    /// that calling twice produces two rotations.
    async fn rotate(
        &self,
        grace: std::time::Duration,
    ) -> Result<KeystoreRotationResult, SecretsError>;

    /// Prune any grace-window entries whose retention has elapsed.
    /// Returns the count of pruned entries. Called periodically by the
    /// daemon's keystore sweep task.
    async fn prune_expired_grace(&self) -> Result<usize, SecretsError>;
}

/// File-backed `SigningBackend` implementation.
///
/// Thin adapter over the existing JSON-keystore free functions
/// (`load_signer_for_kid`, `rotate_keystore`, `list_valid_pubkeys`,
/// `prune_expired_grace`). Each call opens the file fresh so external
/// edits (e.g., the daemon's hourly sweep) are picked up without
/// needing a cache-invalidation hook.
///
/// Private key material lives on disk encrypted only by filesystem
/// permissions (0600 owner-only). For tamper-resistant storage use a
/// future TPM or `YubiHSM` backend.
#[derive(Debug, Clone)]
pub struct FileBackend {
    path: std::path::PathBuf,
}

impl FileBackend {
    /// Create a new `FileBackend` rooted at `path`. The path is the
    /// JSON keystore file; if it does not yet exist, the first
    /// operation that requires it (any of the trait methods) will
    /// create it via `load_or_generate`.
    #[must_use]
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }

    /// Path to the JSON keystore this backend wraps. Useful for
    /// debugging / migration tooling.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait::async_trait]
impl SigningBackend for FileBackend {
    fn name(&self) -> &'static str {
        "file"
    }

    fn is_hardware_backed(&self) -> bool {
        false
    }

    async fn active_key_id(&self) -> Result<String, SecretsError> {
        // `load_or_generate` returns the *active* signer; its `key_id`
        // is the answer. Idempotent — does nothing if the file
        // already exists with a valid active entry.
        let signer = ClusterSigner::load_or_generate(&self.path).await?;
        Ok(signer.key_id())
    }

    async fn public_key_b64(&self, kid: &str) -> Result<Option<String>, SecretsError> {
        Ok(load_signer_for_kid(&self.path, kid)
            .await?
            .map(|s| s.public_key_b64()))
    }

    async fn list_valid_pubkeys(&self) -> Result<Vec<PubkeyInfo>, SecretsError> {
        list_valid_pubkeys(&self.path).await
    }

    async fn sign(&self, kid: &str, msg: &[u8]) -> Result<[u8; 64], SecretsError> {
        let signer = load_signer_for_kid(&self.path, kid).await?.ok_or_else(|| {
            SecretsError::Provider(format!(
                "kid {kid} not in keystore (unknown or grace expired)"
            ))
        })?;
        Ok(signer.sign(msg))
    }

    async fn rotate(
        &self,
        grace: std::time::Duration,
    ) -> Result<KeystoreRotationResult, SecretsError> {
        rotate_keystore(&self.path, grace).await
    }

    async fn prune_expired_grace(&self) -> Result<usize, SecretsError> {
        prune_expired_grace(&self.path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier};
    use tempfile::tempdir;

    #[test]
    fn generate_produces_distinct_keys() {
        let a = ClusterSigner::generate();
        let b = ClusterSigner::generate();
        assert_ne!(
            a.key_id(),
            b.key_id(),
            "two fresh generate() calls produced the same key_id"
        );
        assert_ne!(a.verifying_key().as_bytes(), b.verifying_key().as_bytes());
    }

    #[tokio::test]
    async fn round_trip_through_disk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signer.key");

        let first = ClusterSigner::load_or_generate(&path).await.unwrap();
        let first_id = first.key_id();

        // File should now exist as a JSON keystore.
        assert!(path.exists(), "key file was not persisted");
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.starts_with('{'),
            "expected JSON keystore, got: {on_disk:?}"
        );

        // Second call must load, not regenerate.
        let second = ClusterSigner::load_or_generate(&path).await.unwrap();
        assert_eq!(
            first_id,
            second.key_id(),
            "second load_or_generate regenerated instead of loading"
        );
        assert_eq!(
            first.verifying_key().as_bytes(),
            second.verifying_key().as_bytes()
        );
    }

    #[test]
    fn sign_then_verify() {
        let signer = ClusterSigner::generate();
        let msg = b"join-token-payload-v1";

        let sig_bytes = signer.sign(msg);
        let sig = Signature::from_bytes(&sig_bytes);
        signer
            .verifying_key()
            .verify(msg, &sig)
            .expect("valid signature should verify");

        // Flipping a bit in the message must invalidate the signature.
        let mut tampered = msg.to_vec();
        tampered[0] ^= 0x01;
        assert!(
            signer.verifying_key().verify(&tampered, &sig).is_err(),
            "tampered message verified against original signature"
        );

        // Flipping a bit in the signature must also fail.
        let mut bad_sig_bytes = sig_bytes;
        bad_sig_bytes[0] ^= 0x01;
        let bad_sig = Signature::from_bytes(&bad_sig_bytes);
        assert!(
            signer.verifying_key().verify(msg, &bad_sig).is_err(),
            "tampered signature verified against original message"
        );
    }

    #[test]
    fn key_id_is_8_hex_chars() {
        let signer = ClusterSigner::generate();
        let id = signer.key_id();
        assert_eq!(id.len(), 8, "key_id should be 8 hex chars, got {id:?}");
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "key_id should be hex, got {id:?}"
        );
    }

    #[test]
    fn public_key_b64_round_trips() {
        let signer = ClusterSigner::generate();
        let b64 = signer.public_key_b64();
        // URL_SAFE_NO_PAD of 32 bytes = ceil(32 * 4 / 3) = 43 chars, no padding.
        assert_eq!(b64.len(), 43);
        let decoded = URL_SAFE_NO_PAD.decode(&b64).unwrap();
        assert_eq!(decoded, signer.verifying_key().as_bytes());
    }

    /// Garbage files must be rejected. The new format is JSON, so a 16-byte
    /// blob is neither valid keystore JSON nor a 32-byte legacy seed and
    /// must produce a `SecretsError::Storage`.
    #[tokio::test]
    async fn load_or_generate_rejects_garbage_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signer.key");
        std::fs::write(&path, [0u8; 16]).unwrap();

        let err = ClusterSigner::load_or_generate(&path)
            .await
            .expect_err("should reject 16-byte garbage file");
        match err {
            SecretsError::Storage(msg) => {
                assert!(
                    msg.contains("unexpected format"),
                    "expected 'unexpected format' error, got: {msg}"
                );
            }
            other => panic!("expected SecretsError::Storage, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn persisted_file_is_mode_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signer.key");
        let _ = ClusterSigner::load_or_generate(&path).await.unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "expected mode 0600, got {mode:o}");
    }

    /// A pre-existing 32-byte raw-seed file (Wave 1 format) must be migrated
    /// in place on first load, and the resulting keystore must produce the
    /// same signing key on subsequent loads.
    #[tokio::test]
    async fn migration_from_raw_seed_file_works_once() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");

        // 1. Write a legacy 32-byte file.
        let mut legacy_seed = [0u8; 32];
        rand::rngs::OsRng.try_fill_bytes(&mut legacy_seed).unwrap();
        std::fs::write(&path, legacy_seed).unwrap();

        // 2. First load_or_generate triggers migration.
        let signer = ClusterSigner::load_or_generate(&path).await.unwrap();
        let migrated_kid = signer.key_id();

        // 3. File should now be JSON.
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.starts_with('{'),
            "expected JSON keystore after migration, got: {content:?}"
        );
        assert!(content.contains("\"version\":"));
        assert!(content.contains("\"active\":"));

        // 4. Re-loading gives the same key.
        let again = ClusterSigner::load_or_generate(&path).await.unwrap();
        assert_eq!(again.key_id(), migrated_kid);

        // 5. Public key bytes match — confirms the seed survived migration.
        assert_eq!(
            signer.verifying_key().as_bytes(),
            again.verifying_key().as_bytes()
        );
    }

    #[tokio::test]
    async fn fresh_load_or_generate_produces_json_keystore() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");
        let _ = ClusterSigner::load_or_generate(&path).await.unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"version\":"));
        assert!(content.contains("\"active\":"));
        assert!(content.contains("\"keys\":"));
    }

    #[tokio::test]
    async fn load_or_generate_idempotent_on_keystore() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");

        let first = ClusterSigner::load_or_generate(&path).await.unwrap();
        let json1 = std::fs::read_to_string(&path).unwrap();

        let second = ClusterSigner::load_or_generate(&path).await.unwrap();
        let json2 = std::fs::read_to_string(&path).unwrap();

        assert_eq!(first.key_id(), second.key_id());
        assert_eq!(
            json1, json2,
            "keystore should not be rewritten on a no-op load"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn keystore_file_is_mode_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");
        let _ = ClusterSigner::load_or_generate(&path).await.unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // -------------------------------------------------------------------
    // Wave 5A.2 — rotation, multi-kid lookup, and grace pruning.
    // -------------------------------------------------------------------

    /// Helper: read the keystore file directly off disk for assertions.
    async fn read_store(path: &Path) -> KeyStoreFile {
        ClusterSigner::read_keystore(path).await.unwrap()
    }

    #[tokio::test]
    async fn rotation_flips_active_and_old_keeps_grace() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");

        let original = ClusterSigner::load_or_generate(&path).await.unwrap();
        let original_kid = original.key_id();

        let result = rotate_keystore(&path, std::time::Duration::from_secs(3600))
            .await
            .unwrap();
        assert_ne!(result.new_active_kid, result.previous_kid);
        assert_eq!(result.previous_kid, original_kid);

        let store = read_store(&path).await;
        assert_eq!(store.active, result.new_active_kid);
        assert_eq!(store.keys.len(), 2);
        assert!(store.retired_grace_until.contains_key(&result.previous_kid));
        // The new active key must NOT have a grace entry.
        assert!(!store
            .retired_grace_until
            .contains_key(&result.new_active_kid));
    }

    #[tokio::test]
    async fn rotation_returns_correct_grace_expiry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");
        let _ = ClusterSigner::load_or_generate(&path).await.unwrap();

        let before = Utc::now();
        let result = rotate_keystore(&path, std::time::Duration::from_secs(7200))
            .await
            .unwrap();
        let after = Utc::now();

        // grace_until should be roughly `before + 2h` ..= `after + 2h`.
        let lower = before + chrono::Duration::seconds(7200);
        let upper = after + chrono::Duration::seconds(7200);
        assert!(
            result.previous_grace_until >= lower && result.previous_grace_until <= upper,
            "grace_until {:?} not within expected window [{:?}, {:?}]",
            result.previous_grace_until,
            lower,
            upper,
        );

        // Persisted value must match the returned value.
        let store = read_store(&path).await;
        assert_eq!(
            store.retired_grace_until.get(&result.previous_kid).copied(),
            Some(result.previous_grace_until)
        );
    }

    #[tokio::test]
    async fn load_signer_for_kid_returns_active() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");
        let active = ClusterSigner::load_or_generate(&path).await.unwrap();

        let loaded = load_signer_for_kid(&path, &active.key_id())
            .await
            .unwrap()
            .expect("active kid should load");
        assert_eq!(loaded.key_id(), active.key_id());
        assert_eq!(
            loaded.verifying_key().as_bytes(),
            active.verifying_key().as_bytes()
        );
    }

    #[tokio::test]
    async fn load_signer_for_kid_returns_grace() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");
        let original = ClusterSigner::load_or_generate(&path).await.unwrap();
        let original_kid = original.key_id();

        let _result = rotate_keystore(&path, std::time::Duration::from_secs(3600))
            .await
            .unwrap();

        let loaded = load_signer_for_kid(&path, &original_kid)
            .await
            .unwrap()
            .expect("grace-period kid should still load");
        assert_eq!(loaded.key_id(), original_kid);
        // It must produce the same verifying key as the original instance.
        assert_eq!(
            loaded.verifying_key().as_bytes(),
            original.verifying_key().as_bytes()
        );
    }

    #[tokio::test]
    async fn load_signer_for_kid_returns_none_for_unknown() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");
        let _ = ClusterSigner::load_or_generate(&path).await.unwrap();

        let loaded = load_signer_for_kid(&path, "deadbeef").await.unwrap();
        assert!(loaded.is_none(), "unknown kid should return None");
    }

    #[tokio::test]
    async fn load_signer_for_kid_returns_none_for_expired_grace() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");
        let _ = ClusterSigner::load_or_generate(&path).await.unwrap();

        // Rotate to populate `retired_grace_until`, then manually rewind the
        // expiration into the past.
        let result = rotate_keystore(&path, std::time::Duration::from_secs(3600))
            .await
            .unwrap();

        let mut store = read_store(&path).await;
        let past = Utc::now() - chrono::Duration::seconds(60);
        store
            .retired_grace_until
            .insert(result.previous_kid.clone(), past);
        ClusterSigner::write_keystore(&path, &store).await.unwrap();

        let loaded = load_signer_for_kid(&path, &result.previous_kid)
            .await
            .unwrap();
        assert!(
            loaded.is_none(),
            "kid with expired grace must not load via load_signer_for_kid"
        );
    }

    #[tokio::test]
    async fn list_valid_pubkeys_returns_active_first_then_grace() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");
        let original = ClusterSigner::load_or_generate(&path).await.unwrap();
        let original_kid = original.key_id();

        let result = rotate_keystore(&path, std::time::Duration::from_secs(3600))
            .await
            .unwrap();
        assert_eq!(result.previous_kid, original_kid);

        let list = list_valid_pubkeys(&path).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].status, PubkeyStatus::Active);
        assert_eq!(list[0].kid, result.new_active_kid);
        assert!(list[0].valid_until.is_none());
        assert_eq!(list[1].status, PubkeyStatus::Grace);
        assert_eq!(list[1].kid, original_kid);
        assert!(list[1].valid_until.is_some());
    }

    #[tokio::test]
    async fn list_valid_pubkeys_omits_expired_grace() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");
        let _ = ClusterSigner::load_or_generate(&path).await.unwrap();

        let result = rotate_keystore(&path, std::time::Duration::from_secs(3600))
            .await
            .unwrap();

        // Rewind grace into the past — the listing must drop it.
        let mut store = read_store(&path).await;
        store.retired_grace_until.insert(
            result.previous_kid.clone(),
            Utc::now() - chrono::Duration::seconds(1),
        );
        ClusterSigner::write_keystore(&path, &store).await.unwrap();

        let list = list_valid_pubkeys(&path).await.unwrap();
        assert_eq!(list.len(), 1, "expired-grace entry should be omitted");
        assert_eq!(list[0].kid, result.new_active_kid);
        assert_eq!(list[0].status, PubkeyStatus::Active);
    }

    #[tokio::test]
    async fn prune_expired_grace_removes_expired_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");
        let _ = ClusterSigner::load_or_generate(&path).await.unwrap();

        let result = rotate_keystore(&path, std::time::Duration::from_secs(3600))
            .await
            .unwrap();

        // Force the grace to be in the past.
        let mut store = read_store(&path).await;
        store.retired_grace_until.insert(
            result.previous_kid.clone(),
            Utc::now() - chrono::Duration::seconds(1),
        );
        ClusterSigner::write_keystore(&path, &store).await.unwrap();

        let pruned = prune_expired_grace(&path).await.unwrap();
        assert_eq!(pruned, 1);

        let after = read_store(&path).await;
        assert_eq!(after.keys.len(), 1);
        assert_eq!(after.keys[0].id, result.new_active_kid);
        assert!(after.retired_grace_until.is_empty());
    }

    #[tokio::test]
    async fn prune_expired_grace_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");
        let _ = ClusterSigner::load_or_generate(&path).await.unwrap();

        // No grace entries at all — first call is a no-op, returns 0.
        let first = prune_expired_grace(&path).await.unwrap();
        assert_eq!(first, 0);
        let bytes_after_first = std::fs::read(&path).unwrap();

        // Second call must also be 0 and must NOT rewrite the file.
        let second = prune_expired_grace(&path).await.unwrap();
        assert_eq!(second, 0);
        let bytes_after_second = std::fs::read(&path).unwrap();
        assert_eq!(
            bytes_after_first, bytes_after_second,
            "idempotent prune must not rewrite the keystore on a no-op"
        );

        // Now add an expired grace and prune.
        let result = rotate_keystore(&path, std::time::Duration::from_secs(3600))
            .await
            .unwrap();
        let mut store = read_store(&path).await;
        store.retired_grace_until.insert(
            result.previous_kid.clone(),
            Utc::now() - chrono::Duration::seconds(1),
        );
        ClusterSigner::write_keystore(&path, &store).await.unwrap();

        let count = prune_expired_grace(&path).await.unwrap();
        assert_eq!(count, 1);

        // Second prune after the cleanup must again be a no-op.
        let count_again = prune_expired_grace(&path).await.unwrap();
        assert_eq!(count_again, 0);
    }

    #[tokio::test]
    async fn rotate_then_load_signer_for_old_kid_still_works_within_grace() {
        use ed25519_dalek::Verifier;

        let dir = tempdir().unwrap();
        let path = dir.path().join("cluster_signing.key");
        let original = ClusterSigner::load_or_generate(&path).await.unwrap();
        let original_kid = original.key_id();

        // Sign a message with the original key.
        let msg = b"join-token-payload";
        let sig_bytes = original.sign(msg);

        // Rotate the keystore.
        let _result = rotate_keystore(&path, std::time::Duration::from_secs(3600))
            .await
            .unwrap();

        // The old signer should still be loadable within grace.
        let loaded = load_signer_for_kid(&path, &original_kid)
            .await
            .unwrap()
            .expect("old kid should still load while in grace");

        // And its verifying key should still verify the original signature.
        let sig = Signature::from_bytes(&sig_bytes);
        loaded
            .verifying_key()
            .verify(msg, &sig)
            .expect("signature from pre-rotation key must verify against in-grace key");
    }

    // -------------------------------------------------------------------
    // Wave 8 — SigningBackend trait + FileBackend adapter.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn file_backend_round_trips_through_trait() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ks.json");
        let backend: std::sync::Arc<dyn SigningBackend> =
            std::sync::Arc::new(FileBackend::new(path.clone()));

        let active = backend.active_key_id().await.unwrap();
        assert_eq!(active.len(), 8, "kid is 8 hex chars");

        let pubkey = backend.public_key_b64(&active).await.unwrap();
        assert!(pubkey.is_some(), "active key must resolve via the trait");

        // Sign-then-verify-by-hand round trip.
        let msg = b"hello signing backend";
        let sig = backend.sign(&active, msg).await.unwrap();
        let pubkey_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(pubkey.unwrap())
            .unwrap();
        let verifying =
            ed25519_dalek::VerifyingKey::from_bytes(&pubkey_bytes.try_into().unwrap()).unwrap();
        let signature = ed25519_dalek::Signature::from_bytes(&sig);
        verifying
            .verify_strict(msg, &signature)
            .expect("file-backend signature must verify against its own pubkey");
    }

    #[tokio::test]
    async fn file_backend_reports_software_only() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FileBackend::new(dir.path().join("ks.json"));
        assert_eq!(backend.name(), "file");
        assert!(
            !backend.is_hardware_backed(),
            "file backend must NOT report hardware-backed"
        );
    }

    #[tokio::test]
    async fn file_backend_rotate_through_trait_produces_grace_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ks.json");
        let backend = FileBackend::new(path);

        // Seed the keystore by reading the initial active key.
        let original_kid = backend.active_key_id().await.unwrap();

        let result = backend
            .rotate(std::time::Duration::from_secs(3600))
            .await
            .unwrap();
        assert_ne!(result.new_active_kid, original_kid);
        assert_eq!(result.previous_kid, original_kid);

        // Both keys should now appear in the listing.
        let infos = backend.list_valid_pubkeys().await.unwrap();
        assert_eq!(infos.len(), 2, "active + 1 grace entry after rotation");
    }

    #[tokio::test]
    async fn file_backend_unknown_kid_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FileBackend::new(dir.path().join("ks.json"));
        let _ = backend.active_key_id().await.unwrap(); // seed
        let unknown = backend.public_key_b64("deadbeef").await.unwrap();
        assert!(unknown.is_none(), "unknown kid must resolve to None");
    }

    // -------------------------------------------------------------------
    // Wave 9B — long-lived ClusterCa + CaCert issue/verify.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn cluster_ca_load_or_generate_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cluster_ca.key");

        // First load creates the file.
        let ca1 = ClusterCa::load_or_generate(&path).await.unwrap();
        let kid1 = ca1.ca_kid();
        let pubkey1 = ca1.ca_public_key_b64();
        assert_eq!(kid1.len(), 8);
        assert!(!pubkey1.is_empty());

        // Second load reads back the same key — kid and pubkey are stable.
        let ca2 = ClusterCa::load_or_generate(&path).await.unwrap();
        assert_eq!(ca2.ca_kid(), kid1);
        assert_eq!(ca2.ca_public_key_b64(), pubkey1);

        // File is 32 bytes exactly.
        let bytes = tokio::fs::read(&path).await.unwrap();
        assert_eq!(bytes.len(), 32);
    }

    #[tokio::test]
    async fn cluster_ca_issues_and_verifies_ca_cert() {
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let ca_path = dir.path().join("cluster_ca.key");
        let ca = ClusterCa::load_or_generate(&ca_path).await.unwrap();

        let active_kid = "deadbeef".to_string();
        let active_pubkey_b64 = "Y29udGVudG9mYV9ub25fcGtfYi02NF9zaWduZWRfa2V5Xw".to_string();
        let cluster_domain = "test-cluster".to_string();

        let cert = ca
            .issue_ca_cert(
                active_kid.clone(),
                active_pubkey_b64.clone(),
                cluster_domain.clone(),
                Duration::from_secs(3600),
            )
            .unwrap();
        assert_eq!(cert.active_kid, active_kid);
        assert_eq!(cert.cluster_domain, cluster_domain);
        assert_eq!(cert.v, zlayer_types::api::cluster::CA_CERT_FORMAT_VERSION);
        assert!(!cert.sig_by_ca.is_empty());

        // Verification round-trips against the CA's own pubkey.
        ClusterCa::verify_ca_cert(&ca.ca_public_key_b64(), &cert).unwrap();
    }

    #[tokio::test]
    async fn cluster_ca_cert_verification_fails_under_wrong_pubkey() {
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let ca = ClusterCa::load_or_generate(&dir.path().join("ca1.key"))
            .await
            .unwrap();
        let other = ClusterCa::load_or_generate(&dir.path().join("ca2.key"))
            .await
            .unwrap();

        let cert = ca
            .issue_ca_cert(
                "abcd1234".into(),
                "ignored-for-this-test-xx".into(),
                "test-cluster".into(),
                Duration::from_secs(3600),
            )
            .unwrap();

        // Wrong CA pubkey must reject.
        let err = ClusterCa::verify_ca_cert(&other.ca_public_key_b64(), &cert).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("verification failed") || msg.contains("signature"),
            "expected sig-verification error; got {msg}"
        );
    }

    #[tokio::test]
    async fn cluster_ca_cert_verification_fails_when_expired() {
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let ca = ClusterCa::load_or_generate(&dir.path().join("ca.key"))
            .await
            .unwrap();

        // Issue a cert with negligible grace; it expires before we can verify.
        let cert = ca
            .issue_ca_cert(
                "abcd1234".into(),
                "ignored-for-this-test-xx".into(),
                "test".into(),
                Duration::from_millis(1),
            )
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let err = ClusterCa::verify_ca_cert(&ca.ca_public_key_b64(), &cert).unwrap_err();
        assert!(
            err.to_string().contains("expired"),
            "expected expired-cert error; got {err}"
        );
    }
}
