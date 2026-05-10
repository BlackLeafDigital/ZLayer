//! Cluster-replicated secrets store backed by openraft.
//!
//! Reads from the local [`SecretsState`] (a follower view of the replicated
//! state). Writes go through the leader via the [`RaftSecretsHandle`]
//! abstraction, which the scheduler crate's `RaftCoordinator` implements.
//! Decryption uses the local node's X25519 private key to unwrap this
//! node's copy of the cluster DEK; the unwrapped DEK is cached and
//! invalidated lazily when a read notices the local
//! `wrapped_dek.dek_generation` has moved.
//!
//! This is the cluster-mode counterpart to [`crate::PersistentSecretsStore`].
//! The daemon picks one or the other at startup based on whether
//! `--cluster` is on (Task #18).
//!
//! # Why a trait instead of a direct dep on `zlayer-scheduler`?
//!
//! `zlayer-scheduler` already depends on `zlayer-secrets` for crypto
//! primitives ([`crate::cluster_dek::ClusterDek`]) and the SM
//! ([`crate::raft_sm::SecretsState`]). Adding the reverse edge would
//! create a cycle. The [`RaftSecretsHandle`] trait inverts the
//! dependency: this module names *the operations it needs* abstractly,
//! and the scheduler implements the trait against its `RaftCoordinator`.
//! This also makes the store trivially mockable for unit tests (see
//! `mod tests` below).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use zlayer_types::storage::{NodeAffinity, ReplicatedSecret, WrappedDek};

use crate::cluster_dek::ClusterDek;
use crate::raft_sm::SecretsState;
use crate::sealed::RecipientPrivateKey;
use crate::{Result, Secret, SecretMetadata, SecretsError, SecretsProvider, SecretsStore};

/// Operations [`RaftSecretsStore`] needs from a running consensus instance.
///
/// The scheduler crate's `RaftCoordinator` implements this trait; tests
/// substitute an in-memory mock that applies ops directly to a local
/// [`SecretsState`] without spinning up Raft. See
/// `tests::InMemoryRaftHandle` below.
///
/// # Why not just take `Arc<RaftCoordinator>`?
///
/// `zlayer-scheduler -> zlayer-secrets` already exists; adding the
/// reverse edge would close a dependency cycle. Inverting via a trait
/// keeps the layering one-directional.
#[async_trait]
pub trait RaftSecretsHandle: Send + Sync {
    /// Snapshot of the current cluster secrets state. Returned by clone
    /// so the caller can drop any internal locks held by the
    /// implementation before doing crypto / locking other things.
    async fn secrets_state(&self) -> SecretsState;

    /// Propose a [`SecretsRaftOp::PutSecret`]. Leader-only. Followers
    /// surface a `not leader; redirect to: <id>` error so the API layer
    /// can issue a redirect.
    ///
    /// # Errors
    /// - [`SecretsError::Provider`] when this node is not the leader, or
    ///   when the underlying Raft propose fails.
    async fn propose_put_secret(&self, secret: ReplicatedSecret) -> Result<()>;

    /// Propose a [`SecretsRaftOp::DeleteSecret`]. Leader-only with the
    /// same redirect-on-follower semantics as
    /// [`Self::propose_put_secret`].
    ///
    /// # Errors
    /// - [`SecretsError::Provider`] when this node is not the leader, or
    ///   when the underlying Raft propose fails.
    async fn propose_delete_secret(&self, storage_key: &str) -> Result<()>;
}

/// Cluster-replicated secrets store.
///
/// Reads are served from the local Raft-replicated [`SecretsState`].
/// Writes are proposed through the leader via [`RaftSecretsHandle`].
/// The unwrapped cluster DEK is cached in memory and invalidated when
/// the observed `dek_generation` changes.
pub struct RaftSecretsStore {
    /// Local node's X25519 private key, used to unwrap this node's copy
    /// of the cluster DEK from the per-generation [`WrappedDek`].
    node_priv: Arc<RecipientPrivateKey>,

    /// This node's cluster UUID — needed to look up
    /// `wrapped_dek.wraps[node_id]`.
    node_id: String,

    /// Handle to the running consensus instance. Reads pull state from
    /// it; writes propose through it.
    raft: Arc<dyn RaftSecretsHandle>,

    /// Cached unwrapped DEK + the generation it was unwrapped from.
    /// Lazily refreshed on read when the observed generation differs.
    dek_cache: RwLock<Option<CachedDek>>,
}

/// One slot in the DEK cache: the unwrapped DEK plus the generation it
/// was unwrapped from. The generation field is what `ensure_dek_current`
/// compares against to decide whether to drop and re-unwrap.
struct CachedDek {
    generation: u64,
    dek: ClusterDek,
}

impl RaftSecretsStore {
    /// Construct a new store bound to a running [`RaftSecretsHandle`].
    ///
    /// The DEK cache starts empty and is populated on first read.
    #[must_use]
    pub fn new(
        node_priv: RecipientPrivateKey,
        node_id: String,
        raft: Arc<dyn RaftSecretsHandle>,
    ) -> Self {
        Self {
            node_priv: Arc::new(node_priv),
            node_id,
            raft,
            dek_cache: RwLock::new(None),
        }
    }

    /// Construct a storage key in the same shape used by
    /// [`crate::PersistentSecretsStore`] (`"{scope}:{name}"`), so a
    /// secret written via `RaftSecretsStore` is findable via the same
    /// key under the persistent store and vice versa.
    #[inline]
    #[must_use]
    pub fn make_key(scope: &str, name: &str) -> String {
        format!("{scope}:{name}")
    }

    /// Is the local node currently entitled to host this secret's
    /// decryptable form?
    ///
    /// - `None` affinity: any node may host. Returns `true`.
    /// - [`NodeAffinity::Nodes`]: returns `true` iff `node_id` is in the
    ///   allow-list.
    /// - [`NodeAffinity::Labels`]: node-label matching is not yet
    ///   implemented in this layer (the SM doesn't carry labels). Returns
    ///   `true` so the read is permitted; the API gate is the
    ///   authoritative enforcement point until labels are wired in.
    #[must_use]
    pub fn node_allowed(node_id: &str, affinity: Option<&NodeAffinity>) -> bool {
        match affinity {
            // TODO Phase 1.5: label matching. The SM has no node-label
            // table yet; until labels are wired in, conservatively allow
            // for both `None` and `Labels` and let the API gate enforce.
            None | Some(NodeAffinity::Labels { .. }) => true,
            Some(NodeAffinity::Nodes { node_ids }) => node_ids.iter().any(|n| n == node_id),
        }
    }

    /// Refresh the cached DEK from the current `wrapped_dek` if its
    /// generation moved (or if the cache is empty). On success, the
    /// cache holds an unwrapped DEK pinned to the current generation.
    ///
    /// `current_envelope` is the envelope read from
    /// [`SecretsState::wrapped_dek`] by the caller. Passing it in
    /// (instead of reaching into `self.raft.secrets_state()` again)
    /// avoids a second clone of the entire state when the caller is
    /// already iterating over secrets.
    async fn ensure_dek_for_envelope(&self, current_envelope: &WrappedDek) -> Result<()> {
        // Fast path: cache hit on the right generation.
        {
            let guard = self.dek_cache.read().await;
            if let Some(cached) = guard.as_ref() {
                if cached.generation == current_envelope.dek_generation {
                    return Ok(());
                }
            }
        }

        // Slow path: take the wrap bytes for this node out, drop the
        // read lock on the envelope reference, and unwrap.
        let wrap = current_envelope
            .wraps
            .get(&self.node_id)
            .ok_or_else(|| {
                SecretsError::Provider(format!(
                    "node {} has no wrap in current DEK (generation {}); \
                     cannot decrypt cluster secrets — re-join the cluster \
                     so the leader can re-wrap",
                    self.node_id, current_envelope.dek_generation
                ))
            })?
            .clone();

        let dek = ClusterDek::unwrap(&self.node_priv, &wrap)?;

        let mut guard = self.dek_cache.write().await;
        *guard = Some(CachedDek {
            generation: current_envelope.dek_generation,
            dek,
        });
        Ok(())
    }

    /// Look up the secret in the local [`SecretsState`], filter by
    /// [`NodeAffinity`] (returning `None` when this node isn't allowed —
    /// existence is intentionally not leaked), then unwrap the DEK as
    /// needed and decrypt the ciphertext.
    ///
    /// Returns `Ok(None)` for "no such secret" and "not allowed for this
    /// node"; the [`SecretsProvider`] trait reserves
    /// [`SecretsError::NotFound`] for callers that want a hard "not
    /// here" signal — the cluster store prefers `None` so the API gate
    /// can decide between 404 and 403.
    async fn read_inner(&self, scope: &str, name: &str) -> Result<Option<Secret>> {
        let storage_key = Self::make_key(scope, name);
        let state = self.raft.secrets_state().await;

        // Pull out everything we need from the state snapshot, then
        // drop it before doing any crypto so we never hold a state
        // reference across an await on the DEK cache lock.
        let (ciphertext, dek_generation, envelope) = {
            let Some(replicated) = state.secrets.get(&storage_key) else {
                return Ok(None);
            };
            if !Self::node_allowed(&self.node_id, replicated.node_affinity.as_ref()) {
                return Ok(None);
            }
            let Some(envelope) = state.wrapped_dek.as_ref() else {
                return Err(SecretsError::Provider(
                    "cluster has no DEK yet; secret cannot be decrypted".to_string(),
                ));
            };
            (
                replicated.ciphertext.clone(),
                replicated.dek_generation,
                envelope.clone(),
            )
        };

        if envelope.dek_generation < dek_generation {
            // Should be impossible — secrets always reference a
            // generation <= current. Surface as a Provider error so the
            // operator can investigate.
            return Err(SecretsError::Provider(format!(
                "secret {storage_key} references DEK generation {dek_generation} \
                 but current cluster DEK is older (generation {}); state is \
                 inconsistent",
                envelope.dek_generation
            )));
        }

        // The encrypt-side puts every secret on the *current* DEK at
        // write time, and rotations re-encrypt every existing secret on
        // commit. So in steady state, `dek_generation == envelope.dek_generation`.
        // We assert that here defensively — if a row is left on an old
        // generation (mid-rotation crash before re-encrypt completes),
        // we fail the read rather than silently decrypt with the wrong
        // key. The leader's rotation walker is responsible for cleaning
        // up stragglers.
        if dek_generation != envelope.dek_generation {
            return Err(SecretsError::Provider(format!(
                "secret {storage_key} encrypted under DEK generation {dek_generation} \
                 but current is {} — wait for rotation re-encrypt to finish",
                envelope.dek_generation
            )));
        }

        self.ensure_dek_for_envelope(&envelope).await?;

        let guard = self.dek_cache.read().await;
        let cached = guard.as_ref().ok_or_else(|| {
            SecretsError::Provider("DEK cache unexpectedly empty after refresh".to_string())
        })?;

        let plaintext = cached.dek.decrypt(&ciphertext)?;
        let value = std::str::from_utf8(plaintext.as_slice())
            .map_err(|e| SecretsError::Decryption(format!("invalid UTF-8 in secret: {e}")))?;
        Ok(Some(Secret::new(value)))
    }

    /// Encrypt `plaintext` under the current cluster DEK, returning
    /// `(ciphertext, generation)`.
    async fn encrypt_under_current(&self, plaintext: &[u8]) -> Result<(Vec<u8>, u64)> {
        let state = self.raft.secrets_state().await;
        let envelope = state.wrapped_dek.clone().ok_or_else(|| {
            SecretsError::Provider(
                "cluster has no DEK yet; cannot write secret — wait for the \
                 first node to register via propose_register_node_and_rotate"
                    .to_string(),
            )
        })?;
        self.ensure_dek_for_envelope(&envelope).await?;
        let guard = self.dek_cache.read().await;
        let cached = guard.as_ref().ok_or_else(|| {
            SecretsError::Provider("DEK cache unexpectedly empty after refresh".to_string())
        })?;
        let ciphertext = cached.dek.encrypt(plaintext)?;
        Ok((ciphertext, cached.generation))
    }
}

#[async_trait]
impl SecretsProvider for RaftSecretsStore {
    async fn get_secret(&self, scope: &str, name: &str) -> Result<Secret> {
        match self.read_inner(scope, name).await? {
            Some(secret) => Ok(secret),
            None => Err(SecretsError::NotFound {
                name: name.to_string(),
            }),
        }
    }

    async fn get_secrets(&self, scope: &str, names: &[&str]) -> Result<HashMap<String, Secret>> {
        let mut out = HashMap::with_capacity(names.len());
        for name in names {
            // Per the trait docs: missing secrets are silently omitted
            // from the batch result (vs. erroring). `read_inner` already
            // returns `Ok(None)` for both "doesn't exist" and "not
            // allowed for this node", which is exactly what we want.
            if let Some(secret) = self.read_inner(scope, name).await? {
                out.insert((*name).to_string(), secret);
            }
        }
        Ok(out)
    }

    async fn list_secrets(&self, scope: &str) -> Result<Vec<SecretMetadata>> {
        let state = self.raft.secrets_state().await;
        let prefix = format!("{scope}:");
        let mut results = Vec::new();
        for replicated in state.secrets.values() {
            if !replicated.storage_key.starts_with(&prefix) {
                continue;
            }
            // Hide secrets this node isn't entitled to host. Don't leak
            // existence via the metadata listing.
            if !Self::node_allowed(&self.node_id, replicated.node_affinity.as_ref()) {
                continue;
            }
            // Strip ciphertext / dek_generation; only return the public
            // metadata block.
            results.push(replicated.metadata.clone());
        }
        results.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(results)
    }

    async fn exists(&self, scope: &str, name: &str) -> Result<bool> {
        let state = self.raft.secrets_state().await;
        let storage_key = Self::make_key(scope, name);
        let Some(replicated) = state.secrets.get(&storage_key) else {
            return Ok(false);
        };
        // Don't leak existence to nodes outside the affinity set.
        Ok(Self::node_allowed(
            &self.node_id,
            replicated.node_affinity.as_ref(),
        ))
    }
}

#[async_trait]
impl SecretsStore for RaftSecretsStore {
    async fn set_secret(&self, scope: &str, name: &str, value: &Secret) -> Result<()> {
        let storage_key = Self::make_key(scope, name);

        // Look up any existing replicated row to preserve metadata
        // (created_at, version) so set_secret semantics match
        // PersistentSecretsStore (version increments on update, stable
        // created_at, fresh updated_at).
        let existing = {
            let state = self.raft.secrets_state().await;
            state.secrets.get(&storage_key).cloned()
        };

        let metadata = match existing.as_ref() {
            Some(prev) => {
                let mut m = prev.metadata.clone();
                m.update();
                m
            }
            None => SecretMetadata::new(name),
        };

        // Preserve any previously-set node_affinity. set_secret() is the
        // value-rotation path — the affinity is configured separately via
        // a dedicated API in handlers and shouldn't be cleared by a
        // simple value update.
        let node_affinity = existing.as_ref().and_then(|p| p.node_affinity.clone());

        let (ciphertext, dek_generation) = self
            .encrypt_under_current(value.expose().as_bytes())
            .await?;

        let secret = ReplicatedSecret {
            storage_key,
            ciphertext,
            dek_generation,
            metadata,
            node_affinity,
        };

        // Surface leader-redirect / propose errors verbatim — the API
        // layer parses the "not leader; redirect to: <id>" prefix to
        // issue an HTTP redirect.
        self.raft.propose_put_secret(secret).await
    }

    async fn delete_secret(&self, scope: &str, name: &str) -> Result<()> {
        let storage_key = Self::make_key(scope, name);

        // Pre-flight existence check so the caller gets a clean
        // NotFound rather than a Raft "DeleteSecret for unknown
        // storage_key" Provider error. Race with concurrent deletes is
        // acceptable; the underlying SM apply will return Provider in
        // that case which we surface verbatim.
        let exists = {
            let state = self.raft.secrets_state().await;
            state.secrets.contains_key(&storage_key)
        };
        if !exists {
            return Err(SecretsError::NotFound {
                name: name.to_string(),
            });
        }

        self.raft.propose_delete_secret(&storage_key).await
    }

    async fn rotate_secret(
        &self,
        scope: &str,
        name: &str,
        value: &Secret,
    ) -> Result<crate::RotationResult> {
        let storage_key = Self::make_key(scope, name);

        // Look up the existing record so we can return a meaningful
        // RotationResult and bump its version cleanly. Mirrors
        // PersistentSecretsStore: rotation requires the secret to
        // already exist.
        let existing = {
            let state = self.raft.secrets_state().await;
            state.secrets.get(&storage_key).cloned()
        };
        let existing = existing.ok_or_else(|| SecretsError::NotFound {
            name: name.to_string(),
        })?;

        let previous_version = existing.metadata.version;

        let mut metadata = existing.metadata.clone();
        metadata.update();
        let new_version = metadata.version;

        let (ciphertext, dek_generation) = self
            .encrypt_under_current(value.expose().as_bytes())
            .await?;

        let secret = ReplicatedSecret {
            storage_key,
            ciphertext,
            dek_generation,
            metadata,
            node_affinity: existing.node_affinity,
        };

        self.raft.propose_put_secret(secret).await?;

        Ok(crate::RotationResult {
            previous_version: Some(previous_version),
            new_version,
        })
    }

    async fn set_secret_with_affinity(
        &self,
        scope: &str,
        name: &str,
        value: &Secret,
        node_affinity: Option<&NodeAffinity>,
    ) -> Result<()> {
        let storage_key = Self::make_key(scope, name);

        // Preserve metadata semantics with [`Self::set_secret`]: bump
        // version + updated_at on update, keep created_at stable.
        let existing = {
            let state = self.raft.secrets_state().await;
            state.secrets.get(&storage_key).cloned()
        };

        let metadata = match existing.as_ref() {
            Some(prev) => {
                let mut m = prev.metadata.clone();
                m.update();
                m
            }
            None => SecretMetadata::new(name),
        };

        // Affinity precedence:
        //   - Some(_) from caller -> overwrite (this is the explicit
        //     write/update path used by the API on create/rotate).
        //   - None from caller -> preserve any previously stored selector
        //     (matches the "leave affinity unchanged" contract on the
        //     rotate request DTO).
        let resolved_affinity = match node_affinity {
            Some(_) => node_affinity.cloned(),
            None => existing.as_ref().and_then(|p| p.node_affinity.clone()),
        };

        let (ciphertext, dek_generation) = self
            .encrypt_under_current(value.expose().as_bytes())
            .await?;

        let secret = ReplicatedSecret {
            storage_key,
            ciphertext,
            dek_generation,
            metadata,
            node_affinity: resolved_affinity,
        };

        self.raft.propose_put_secret(secret).await
    }

    async fn rotate_secret_with_affinity(
        &self,
        scope: &str,
        name: &str,
        value: &Secret,
        node_affinity: Option<&NodeAffinity>,
    ) -> Result<crate::RotationResult> {
        let storage_key = Self::make_key(scope, name);

        let existing = {
            let state = self.raft.secrets_state().await;
            state.secrets.get(&storage_key).cloned()
        };
        let existing = existing.ok_or_else(|| SecretsError::NotFound {
            name: name.to_string(),
        })?;

        let previous_version = existing.metadata.version;

        let mut metadata = existing.metadata.clone();
        metadata.update();
        let new_version = metadata.version;

        let (ciphertext, dek_generation) = self
            .encrypt_under_current(value.expose().as_bytes())
            .await?;

        // Same precedence as set_secret_with_affinity above.
        let resolved_affinity = match node_affinity {
            Some(_) => node_affinity.cloned(),
            None => existing.node_affinity,
        };

        let secret = ReplicatedSecret {
            storage_key,
            ciphertext,
            dek_generation,
            metadata,
            node_affinity: resolved_affinity,
        };

        self.raft.propose_put_secret(secret).await?;

        Ok(crate::RotationResult {
            previous_version: Some(previous_version),
            new_version,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    use chrono::Utc;
    use zlayer_types::api::internal::SecretsRaftOp;
    use zlayer_types::storage::NodeIdentity;

    use crate::sealed::RecipientPublicKey;

    /// In-memory mock that applies ops directly to a local
    /// [`SecretsState`] without spinning up a real Raft cluster.
    /// Single-node "leader of one" for unit-test purposes.
    struct InMemoryRaftHandle {
        // StdMutex (not tokio::sync::Mutex) is fine because all the
        // mutations are short, synchronous applies. Wrapping in async
        // methods keeps the trait signatures.
        state: StdMutex<SecretsState>,
    }

    impl InMemoryRaftHandle {
        fn new() -> Self {
            Self {
                state: StdMutex::new(SecretsState::default()),
            }
        }

        /// Apply an op directly (bypassing leader checks) so the test
        /// fixture can register a node + rotate the DEK without going
        /// through any propose path.
        fn apply(&self, op: SecretsRaftOp) {
            let mut guard = self.state.lock().expect("state poisoned");
            guard.apply(op).expect("apply ok");
        }
    }

    #[async_trait]
    impl RaftSecretsHandle for InMemoryRaftHandle {
        async fn secrets_state(&self) -> SecretsState {
            self.state.lock().expect("state poisoned").clone()
        }

        async fn propose_put_secret(&self, secret: ReplicatedSecret) -> Result<()> {
            self.apply(SecretsRaftOp::PutSecret { secret });
            Ok(())
        }

        async fn propose_delete_secret(&self, storage_key: &str) -> Result<()> {
            self.apply(SecretsRaftOp::DeleteSecret {
                storage_key: storage_key.to_string(),
            });
            Ok(())
        }
    }

    /// Build a fixture: in-memory handle pre-seeded with a single node
    /// (`node-a`), a DEK at generation 1 wrapped to `node-a`, and a
    /// `RaftSecretsStore` bound to that node's private key.
    fn fixture() -> (
        Arc<InMemoryRaftHandle>,
        RaftSecretsStore,
        RecipientPrivateKey,
    ) {
        let (sk, pk) = RecipientPrivateKey::generate();

        let identity = NodeIdentity {
            node_id: "node-a".to_string(),
            secrets_pubkey: *pk.as_bytes(),
            wg_pubkey: "wg-a".to_string(),
            joined_at: Utc::now(),
            revoked_at: None,
        };

        let dek = ClusterDek::generate();
        let mut recipients: HashMap<String, RecipientPublicKey> = HashMap::new();
        recipients.insert(
            "node-a".to_string(),
            RecipientPublicKey::from_bytes(*pk.as_bytes()),
        );
        let envelope = dek.rewrap_for_set(&recipients, 1).expect("rewrap");

        let handle = Arc::new(InMemoryRaftHandle::new());
        handle.apply(SecretsRaftOp::RegisterNode { identity });
        handle.apply(SecretsRaftOp::RotateDek {
            new_wraps: envelope,
        });

        let store_handle: Arc<dyn RaftSecretsHandle> = handle.clone();
        let store = RaftSecretsStore::new(sk.clone(), "node-a".to_string(), store_handle);
        (handle, store, sk)
    }

    #[tokio::test]
    async fn round_trip_set_get() {
        let (_handle, store, _sk) = fixture();
        store
            .set_secret("dep:myapp", "API_KEY", &Secret::new("hunter2"))
            .await
            .expect("set");
        let got = store.get_secret("dep:myapp", "API_KEY").await.expect("get");
        assert_eq!(got.expose(), "hunter2");
    }

    #[tokio::test]
    async fn get_unknown_returns_not_found() {
        let (_handle, store, _sk) = fixture();
        let err = store
            .get_secret("dep:myapp", "missing")
            .await
            .expect_err("should error");
        assert!(matches!(err, SecretsError::NotFound { .. }));
    }

    #[tokio::test]
    async fn affinity_excluded_node_returns_not_found_without_leaking() {
        let (handle, store, _sk) = fixture();
        // Insert a secret with affinity restricted to a *different*
        // node so the local node isn't allowed to see it.
        let dek = ClusterDek::generate(); // DEK doesn't matter; we won't decrypt.
        let cipher = dek.encrypt(b"top secret").expect("encrypt");
        let secret = ReplicatedSecret {
            storage_key: RaftSecretsStore::make_key("dep:myapp", "ALLOW_ELSEWHERE"),
            ciphertext: cipher,
            dek_generation: 1,
            metadata: SecretMetadata::new("ALLOW_ELSEWHERE"),
            node_affinity: Some(NodeAffinity::Nodes {
                node_ids: vec!["node-other".to_string()],
            }),
        };
        handle.apply(SecretsRaftOp::PutSecret { secret });

        // get_secret -> NotFound (the get_secret wrapper turns the
        // None from read_inner into NotFound).
        let err = store
            .get_secret("dep:myapp", "ALLOW_ELSEWHERE")
            .await
            .expect_err("should error");
        assert!(matches!(err, SecretsError::NotFound { .. }));

        // exists should also report false, not true.
        let present = store
            .exists("dep:myapp", "ALLOW_ELSEWHERE")
            .await
            .expect("exists");
        assert!(!present, "node-a must not learn the secret exists");

        // list_secrets should not include it either.
        let listed = store.list_secrets("dep:myapp").await.expect("list");
        assert!(
            listed.iter().all(|m| m.name != "ALLOW_ELSEWHERE"),
            "list must not leak affinity-excluded secrets",
        );
    }

    #[tokio::test]
    async fn rotate_increments_version_and_returns_correct_versions() {
        let (_handle, store, _sk) = fixture();
        store
            .set_secret("dep:myapp", "API_KEY", &Secret::new("v1"))
            .await
            .expect("set v1");

        let result = store
            .rotate_secret("dep:myapp", "API_KEY", &Secret::new("v2"))
            .await
            .expect("rotate");
        assert_eq!(result.previous_version, Some(1));
        assert_eq!(result.new_version, 2);

        let got = store.get_secret("dep:myapp", "API_KEY").await.expect("get");
        assert_eq!(got.expose(), "v2");
    }

    #[tokio::test]
    async fn rotate_unknown_returns_not_found() {
        let (_handle, store, _sk) = fixture();
        let err = store
            .rotate_secret("dep:myapp", "never-set", &Secret::new("v1"))
            .await
            .expect_err("should error");
        assert!(matches!(err, SecretsError::NotFound { .. }));
    }

    #[tokio::test]
    async fn delete_then_get_returns_not_found() {
        let (_handle, store, _sk) = fixture();
        store
            .set_secret("dep:myapp", "API_KEY", &Secret::new("v1"))
            .await
            .expect("set");
        store
            .delete_secret("dep:myapp", "API_KEY")
            .await
            .expect("delete");
        let err = store
            .get_secret("dep:myapp", "API_KEY")
            .await
            .expect_err("should error");
        assert!(matches!(err, SecretsError::NotFound { .. }));
    }

    #[tokio::test]
    async fn delete_unknown_returns_not_found() {
        let (_handle, store, _sk) = fixture();
        let err = store
            .delete_secret("dep:myapp", "missing")
            .await
            .expect_err("should error");
        assert!(matches!(err, SecretsError::NotFound { .. }));
    }

    #[tokio::test]
    async fn dek_rotation_is_picked_up_on_next_read() {
        let (handle, store, _sk) = fixture();
        store
            .set_secret("dep:myapp", "API_KEY", &Secret::new("before"))
            .await
            .expect("set before");

        // Prime the cache by reading once.
        let got = store
            .get_secret("dep:myapp", "API_KEY")
            .await
            .expect("read 1");
        assert_eq!(got.expose(), "before");

        // Simulate a rotation: generate a new DEK, re-wrap for the
        // single existing node, and replay the rotation through the
        // SM. Then re-encrypt the existing secret under the new DEK
        // and put it back (this is what the leader's rotation walker
        // does in propose_rotate_dek).
        let pk_a = {
            let s = handle.secrets_state().await;
            RecipientPublicKey::from_bytes(s.nodes["node-a"].secrets_pubkey)
        };
        let new_dek = ClusterDek::generate();
        let mut recipients: HashMap<String, RecipientPublicKey> = HashMap::new();
        recipients.insert("node-a".to_string(), pk_a);
        let envelope = new_dek.rewrap_for_set(&recipients, 2).expect("rewrap");
        handle.apply(SecretsRaftOp::RotateDek {
            new_wraps: envelope,
        });

        // Re-encrypt the existing secret under the new DEK so the read
        // path's "row generation must match envelope generation" check
        // is satisfied.
        let new_cipher = new_dek.encrypt(b"before").expect("re-encrypt");
        let updated = ReplicatedSecret {
            storage_key: RaftSecretsStore::make_key("dep:myapp", "API_KEY"),
            ciphertext: new_cipher,
            dek_generation: 2,
            metadata: SecretMetadata::new("API_KEY"),
            node_affinity: None,
        };
        handle.apply(SecretsRaftOp::PutSecret { secret: updated });

        // The cache still holds the generation-1 DEK; the next read
        // must notice the gen mismatch, refresh, and decrypt under
        // generation 2.
        let got = store
            .get_secret("dep:myapp", "API_KEY")
            .await
            .expect("read 2");
        assert_eq!(got.expose(), "before");
    }

    #[tokio::test]
    async fn list_secrets_filters_by_scope_prefix() {
        let (_handle, store, _sk) = fixture();
        store
            .set_secret("dep:app1", "A", &Secret::new("1"))
            .await
            .expect("set 1");
        store
            .set_secret("dep:app1", "B", &Secret::new("2"))
            .await
            .expect("set 2");
        store
            .set_secret("dep:app2", "C", &Secret::new("3"))
            .await
            .expect("set 3");

        let list = store.list_secrets("dep:app1").await.expect("list 1");
        assert_eq!(list.len(), 2);
        let names: Vec<_> = list.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["A", "B"]);

        let list = store.list_secrets("dep:app2").await.expect("list 2");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "C");
    }

    #[test]
    fn node_allowed_unrestricted() {
        assert!(RaftSecretsStore::node_allowed("node-a", None));
    }

    #[test]
    fn node_allowed_explicit_nodes() {
        let aff = NodeAffinity::Nodes {
            node_ids: vec!["node-a".to_string(), "node-b".to_string()],
        };
        assert!(RaftSecretsStore::node_allowed("node-a", Some(&aff)));
        assert!(RaftSecretsStore::node_allowed("node-b", Some(&aff)));
        assert!(!RaftSecretsStore::node_allowed("node-c", Some(&aff)));
    }

    #[test]
    fn node_allowed_labels_phase_15_permissive() {
        // Labels are a Phase 1.5 follow-up; until then, any node passes.
        let aff = NodeAffinity::Labels {
            labels: HashMap::new(),
        };
        assert!(RaftSecretsStore::node_allowed("node-a", Some(&aff)));
    }

    #[test]
    fn make_key_matches_persistent_shape() {
        // Same `{scope}:{name}` shape PersistentSecretsStore uses, so
        // a row written via either store is findable via the other.
        assert_eq!(RaftSecretsStore::make_key("scope", "name"), "scope:name");
        assert_eq!(
            RaftSecretsStore::make_key("dep/myapp", "secret"),
            "dep/myapp:secret"
        );
    }
}
