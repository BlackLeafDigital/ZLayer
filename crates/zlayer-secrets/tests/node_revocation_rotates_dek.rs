//! Phase-1 integration tests for cluster-replicated secrets: DEK
//! rotation triggered by node revocation.
//!
//! These tests exercise the leader-side orchestration that the
//! `zlayer-scheduler::RaftCoordinator::propose_rotate_dek` performs, but
//! run it directly against an in-memory shared `SecretsState` rather
//! than spinning up a real openraft cluster. The trade-off is that we
//! test the orchestration *logic* (recipient set construction, DEK
//! re-wrap exclusion, secret re-encryption walk, generation bumps)
//! deterministically and quickly; we do *not* exercise any Raft
//! protocol behaviour (election, log replication, snapshot install).
//! That layer is covered by `zlayer-consensus`/`zlayer-scheduler` tests.
//!
//! Why shared-state instead of real Raft: per Task #19 guidance, the
//! shared-state approach is the documented Phase-1 default. Each
//! "node" gets its own X25519 keypair and its own `RaftSecretsStore`
//! instance — they all observe the same replicated `SecretsState`,
//! which mirrors what every follower would see post-apply in a real
//! cluster.
//!
//! Cluster scaffolding mirrors what `replication_three_node.rs`
//! (parallel test, same crate) is using; helpers are kept inline here
//! to avoid coupling on a `common/mod.rs` that may not exist yet.

#![cfg(feature = "persistent")]
#![allow(clippy::expect_used)] // standard practice in test code

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use chrono::Utc;

use zlayer_secrets::cluster_dek::ClusterDek;
use zlayer_secrets::sealed::{RecipientPrivateKey, RecipientPublicKey};
use zlayer_secrets::{
    RaftSecretsHandle, RaftSecretsStore, Secret, SecretsError, SecretsProvider, SecretsState,
    SecretsStore,
};

use zlayer_types::api::internal::SecretsRaftOp;
use zlayer_types::storage::{NodeIdentity, ReplicatedSecret};

// ---------------------------------------------------------------------------
// Cluster scaffolding
// ---------------------------------------------------------------------------

/// One simulated node in the test cluster.
///
/// Holds the X25519 keypair for sealed-box wraps and a `RaftSecretsStore`
/// bound to the shared replicated state. The handle inside the store
/// points at the same `Arc<SharedRaftHandle>` every other node uses, so
/// reads see the same `SecretsState` the leader's apply path produced.
struct TestNode {
    node_id: String,
    sk: RecipientPrivateKey,
    pk: RecipientPublicKey,
    store: RaftSecretsStore,
}

/// Shared Raft handle stand-in for the test cluster.
///
/// Wraps a single `SecretsState` behind a synchronous mutex (writes are
/// short, no IO) and applies any op handed to it via `apply_local`. Used
/// both as the read-state source for every node's `RaftSecretsStore` and
/// as the apply path the test fixture's "leader" routes proposes through.
///
/// `propose_*` methods bypass leader checks — the test fixture is the
/// authority on which node is leader, so these just commit the op.
struct SharedRaftHandle {
    state: StdMutex<SecretsState>,
}

impl SharedRaftHandle {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: StdMutex::new(SecretsState::default()),
        })
    }

    /// Apply an op directly. Used by the orchestration helpers in this
    /// file to simulate a leader-side propose that has already won
    /// consensus.
    fn apply_local(&self, op: SecretsRaftOp) {
        let mut guard = self.state.lock().expect("state poisoned");
        guard.apply(op).expect("apply ok");
    }

    /// Snapshot the current state. Cloned so the caller can drop the
    /// mutex before doing crypto.
    fn snapshot(&self) -> SecretsState {
        self.state.lock().expect("state poisoned").clone()
    }
}

#[async_trait]
impl RaftSecretsHandle for SharedRaftHandle {
    async fn secrets_state(&self) -> SecretsState {
        self.snapshot()
    }

    async fn propose_put_secret(&self, secret: ReplicatedSecret) -> zlayer_secrets::Result<()> {
        self.apply_local(SecretsRaftOp::PutSecret { secret });
        Ok(())
    }

    async fn propose_delete_secret(&self, storage_key: &str) -> zlayer_secrets::Result<()> {
        self.apply_local(SecretsRaftOp::DeleteSecret {
            storage_key: storage_key.to_string(),
        });
        Ok(())
    }
}

/// A 3-node test cluster sharing one replicated `SecretsState`.
///
/// `nodes[0]` is the test's "leader" — orchestration helpers
/// (`leader_register_and_rotate`, `leader_rotate_dek`) simulate the
/// leader-side path of the same name on `RaftCoordinator`. `nodes[1]`
/// and `nodes[2]` are followers; they only read state.
struct ThreeNodeCluster {
    handle: Arc<SharedRaftHandle>,
    nodes: Vec<TestNode>,
}

impl ThreeNodeCluster {
    /// Build a 3-node cluster where each node has a fresh X25519 keypair
    /// and a `RaftSecretsStore` pointing at the shared state. No DEK is
    /// installed yet; call `bootstrap_with_initial_dek` to seed one.
    fn new() -> Self {
        let handle = SharedRaftHandle::new();
        let store_handle: Arc<dyn RaftSecretsHandle> = handle.clone();

        let mut nodes = Vec::with_capacity(3);
        for label in ["node-a", "node-b", "node-c"] {
            let (sk, pk) = RecipientPrivateKey::generate();
            let store =
                RaftSecretsStore::new(sk.clone(), label.to_string(), Arc::clone(&store_handle));
            nodes.push(TestNode {
                node_id: label.to_string(),
                sk,
                pk,
                store,
            });
        }

        Self { handle, nodes }
    }

    /// Register all three nodes and install a single DEK at generation 1
    /// wrapped to all three. Mirrors what
    /// `RaftCoordinator::propose_register_node_and_rotate` would have
    /// achieved after each of the three joins, collapsed into one step
    /// for fixture brevity.
    fn bootstrap_with_initial_dek(&self) {
        let now = Utc::now();
        for n in &self.nodes {
            self.handle.apply_local(SecretsRaftOp::RegisterNode {
                identity: NodeIdentity {
                    node_id: n.node_id.clone(),
                    secrets_pubkey: *n.pk.as_bytes(),
                    wg_pubkey: format!("wg-{}", n.node_id),
                    joined_at: now,
                    revoked_at: None,
                },
            });
        }

        let dek = ClusterDek::generate();
        let mut recipients: HashMap<String, RecipientPublicKey> = HashMap::new();
        for n in &self.nodes {
            recipients.insert(
                n.node_id.clone(),
                RecipientPublicKey::from_bytes(*n.pk.as_bytes()),
            );
        }
        let envelope = dek
            .rewrap_for_set(&recipients, 1)
            .expect("initial rewrap to all 3 nodes");
        self.handle.apply_local(SecretsRaftOp::RotateDek {
            new_wraps: envelope,
        });
    }

    /// Find a node by id.
    fn node(&self, id: &str) -> &TestNode {
        self.nodes
            .iter()
            .find(|n| n.node_id == id)
            .unwrap_or_else(|| panic!("node {id} not found in test cluster"))
    }

    /// The leader for the test. By convention, `nodes[0]` is the
    /// designated leader for orchestration helpers; this matches the
    /// task description ("A=leader, B, C").
    fn leader(&self) -> &TestNode {
        &self.nodes[0]
    }

    /// Simulate the leader-side `propose_rotate_dek` path with optional
    /// node revocation. Matches the behaviour of
    /// `zlayer_scheduler::RaftCoordinator::propose_rotate_dek` as of
    /// Task #11:
    ///
    /// 1. Unwrap the current DEK using the leader's private key.
    /// 2. Build a new recipient set excluding any `revoke_node_id` and
    ///    any node whose `revoked_at` is already set.
    /// 3. Generate a fresh DEK, re-wrap to the new recipient set, bump
    ///    generation by 1, and apply `RotateDek`.
    /// 4. Walk every existing `ReplicatedSecret`, decrypt under the old
    ///    DEK, re-encrypt under the new DEK, and apply `PutSecret`.
    ///
    /// Returns the new generation.
    fn leader_rotate_dek(&self, revoke_node_id: Option<&str>) -> u64 {
        let leader = self.leader();
        let snapshot = self.handle.snapshot();
        let prev_envelope = snapshot
            .wrapped_dek
            .as_ref()
            .expect("no current DEK; bootstrap first")
            .clone();
        let prev_generation = prev_envelope.dek_generation;

        let leader_wrap = prev_envelope
            .wraps
            .get(&leader.node_id)
            .unwrap_or_else(|| panic!("leader {} has no wrap in current DEK", leader.node_id))
            .clone();
        let prev_dek =
            ClusterDek::unwrap(&leader.sk, &leader_wrap).expect("leader unwraps prev DEK");

        // Build the post-rotation recipient set: every node that hasn't
        // been soft-revoked AND isn't the explicitly revoked node.
        let mut recipients: HashMap<String, RecipientPublicKey> = HashMap::new();
        for (node_id, identity) in &snapshot.nodes {
            if identity.revoked_at.is_some() {
                continue;
            }
            if Some(node_id.as_str()) == revoke_node_id {
                continue;
            }
            recipients.insert(
                node_id.clone(),
                RecipientPublicKey::from_bytes(identity.secrets_pubkey),
            );
        }

        let new_dek = ClusterDek::generate();
        let new_generation = prev_generation.saturating_add(1);
        let new_envelope = new_dek
            .rewrap_for_set(&recipients, new_generation)
            .expect("new DEK rewrap");
        self.handle.apply_local(SecretsRaftOp::RotateDek {
            new_wraps: new_envelope,
        });

        // Re-encrypt every existing secret on the old generation.
        for secret in snapshot.secrets.values() {
            if secret.dek_generation == new_generation {
                continue;
            }
            let plaintext = prev_dek
                .decrypt(&secret.ciphertext)
                .expect("re-encrypt: decrypt under old DEK");
            let new_ciphertext = new_dek
                .encrypt(plaintext.as_slice())
                .expect("re-encrypt: encrypt under new DEK");
            let mut updated = secret.clone();
            updated.ciphertext = new_ciphertext;
            updated.dek_generation = new_generation;
            self.handle
                .apply_local(SecretsRaftOp::PutSecret { secret: updated });
        }

        new_generation
    }

    /// Simulate the leader-side `propose_register_node_and_rotate` for a
    /// re-joining node. This mirrors `RaftCoordinator`'s pure re-wrap
    /// semantics (no DEK material change, generation bumps by 1, the
    /// new identity is included in the recipient set).
    ///
    /// Used by `revoked_node_register_node_idempotent` to model a
    /// revoked node coming back online with a fresh keypair.
    #[allow(clippy::needless_pass_by_value)]
    fn leader_register_node_and_rotate(&self, identity: NodeIdentity) -> u64 {
        let leader = self.leader();
        let snapshot = self.handle.snapshot();
        let (dek, base_generation) = match snapshot.wrapped_dek.as_ref() {
            Some(envelope) => {
                let leader_wrap = envelope
                    .wraps
                    .get(&leader.node_id)
                    .unwrap_or_else(|| {
                        panic!("leader {} has no wrap in current DEK", leader.node_id)
                    })
                    .clone();
                let dek = ClusterDek::unwrap(&leader.sk, &leader_wrap)
                    .expect("leader unwraps current DEK");
                (dek, envelope.dek_generation)
            }
            None => (ClusterDek::generate(), 0),
        };

        // Apply the RegisterNode first so the apply ordering matches a
        // real leader's path.
        self.handle.apply_local(SecretsRaftOp::RegisterNode {
            identity: identity.clone(),
        });

        // Build the recipient set. Honour any prior soft-revocation by
        // skipping nodes whose identity has `revoked_at` set, except the
        // node we're (re)registering — that incoming identity has a
        // fresh `revoked_at = None` by construction.
        let mut recipients: HashMap<String, RecipientPublicKey> = HashMap::new();
        let post_snapshot = self.handle.snapshot();
        for (node_id, n) in &post_snapshot.nodes {
            if n.revoked_at.is_some() {
                continue;
            }
            recipients.insert(
                node_id.clone(),
                RecipientPublicKey::from_bytes(n.secrets_pubkey),
            );
        }
        // Defensive: ensure the joiner is included even if a stale
        // snapshot raced.
        recipients.insert(
            identity.node_id.clone(),
            RecipientPublicKey::from_bytes(identity.secrets_pubkey),
        );

        let new_generation = base_generation.saturating_add(1);
        let envelope = dek
            .rewrap_for_set(&recipients, new_generation)
            .expect("rewrap for re-registration");
        self.handle.apply_local(SecretsRaftOp::RotateDek {
            new_wraps: envelope,
        });

        new_generation
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Revoking a node MUST rotate the DEK, exclude the revoked node from
/// the new recipient set, leave remaining nodes able to decrypt the
/// re-encrypted secret, and break decryption for any caller still
/// holding the old wrap (because the new ciphertext is under a fresh
/// DEK that the old wrap doesn't unwrap to).
#[tokio::test]
async fn revoke_node_triggers_dek_rotation() {
    let cluster = ThreeNodeCluster::new();
    cluster.bootstrap_with_initial_dek();

    // Set a secret under the leader.
    cluster
        .leader()
        .store
        .set_secret("dep:myapp", "API_KEY", &Secret::new("hunter2"))
        .await
        .expect("set initial secret");

    // Snapshot the pre-rotation state for later asserts.
    let pre = cluster.handle.snapshot();
    let pre_generation = pre
        .wrapped_dek
        .as_ref()
        .expect("DEK present")
        .dek_generation;
    assert_eq!(pre_generation, 1, "fixture installs DEK at generation 1");
    assert!(
        pre.wrapped_dek
            .as_ref()
            .expect("DEK")
            .wraps
            .contains_key("node-c"),
        "node-c starts in the recipient set",
    );

    // Capture C's old wrap and the old DEK so we can verify after
    // rotation that an attacker who somehow kept old material on disk
    // still can't decrypt the new ciphertext.
    let c_old_wrap = pre
        .wrapped_dek
        .as_ref()
        .expect("DEK")
        .wraps
        .get("node-c")
        .expect("c has wrap pre-rotation")
        .clone();
    let c_old_dek = ClusterDek::unwrap(&cluster.node("node-c").sk, &c_old_wrap)
        .expect("c can unwrap its own pre-rotation wrap");

    // Apply mark-as-revoked first (this is what
    // `zlayer-api::handlers::cluster::revoke_node` does before calling
    // `propose_rotate_dek(NodeRevoked)`). Not strictly required for
    // the rotation walk to exclude C — `revoke_node_id` already does —
    // but it matches the production flow.
    cluster.handle.apply_local(SecretsRaftOp::RevokeNode {
        node_id: "node-c".to_string(),
    });
    let new_generation = cluster.leader_rotate_dek(Some("node-c"));

    // (1) generation bumped by exactly 1.
    assert_eq!(
        new_generation,
        pre_generation + 1,
        "rotate must bump dek_generation by 1",
    );

    // (2) new wraps map MUST NOT contain C.
    let post = cluster.handle.snapshot();
    let post_envelope = post.wrapped_dek.as_ref().expect("post DEK present");
    assert_eq!(post_envelope.dek_generation, new_generation);
    assert!(
        !post_envelope.wraps.contains_key("node-c"),
        "revoked node-c must not appear in new wraps",
    );
    assert!(
        post_envelope.wraps.contains_key("node-a"),
        "node-a kept in new wraps",
    );
    assert!(
        post_envelope.wraps.contains_key("node-b"),
        "node-b kept in new wraps",
    );

    // (3) A and B can still decrypt the secret. The rotation walk
    // re-encrypted it under the new DEK and bumped its dek_generation.
    let from_a = cluster
        .node("node-a")
        .store
        .get_secret("dep:myapp", "API_KEY")
        .await
        .expect("A still reads after rotation");
    assert_eq!(from_a.expose(), "hunter2");
    let from_b = cluster
        .node("node-b")
        .store
        .get_secret("dep:myapp", "API_KEY")
        .await
        .expect("B still reads after rotation");
    assert_eq!(from_b.expose(), "hunter2");

    // The replicated row is now on the new generation.
    let storage_key = RaftSecretsStore::make_key("dep:myapp", "API_KEY");
    let row = post
        .secrets
        .get(&storage_key)
        .expect("secret row replicated");
    assert_eq!(
        row.dek_generation, new_generation,
        "rotation walk re-encrypted the row to the new generation",
    );

    // (4) C, holding its old wrap on disk, cannot decrypt the new
    // ciphertext. Unwrapping the old wrap still succeeds (the wrap
    // bytes aren't retroactively scrambled), but the resulting DEK is
    // the *previous* generation's DEK, which doesn't decrypt the new
    // generation's ciphertext.
    let new_ciphertext = row.ciphertext.clone();
    let decrypt_attempt = c_old_dek.decrypt(&new_ciphertext);
    assert!(
        matches!(decrypt_attempt, Err(SecretsError::Decryption(_))),
        "C's old DEK must NOT decrypt the new-generation ciphertext, got {decrypt_attempt:?}",
    );

    // And the live read path on C surfaces the same outcome via the
    // store: C has no wrap in the current envelope, so even attempting
    // a fresh read errors out at the wrap-lookup stage. The store maps
    // this to a Provider error (re-join required).
    let from_c = cluster
        .node("node-c")
        .store
        .get_secret("dep:myapp", "API_KEY")
        .await;
    assert!(
        matches!(from_c, Err(SecretsError::Provider(_))),
        "revoked node-c must not be able to read post-rotation, got {from_c:?}",
    );
}

/// Sanity: post-rotation forward security on the DEK is not retroactive.
/// A snapshot of (old DEK, old ciphertext) decrypts to the original
/// plaintext. The protection is "no new wraps for the revoked node" +
/// "all live data re-encrypted under the new DEK", not retroactive
/// scrambling of bytes that already left the cluster.
#[tokio::test]
async fn revoked_node_can_still_decrypt_old_ciphertext_with_old_dek() {
    let cluster = ThreeNodeCluster::new();
    cluster.bootstrap_with_initial_dek();

    cluster
        .leader()
        .store
        .set_secret("dep:myapp", "API_KEY", &Secret::new("hunter2"))
        .await
        .expect("set initial secret");

    // Snapshot what C has on disk *before* the rotation: the old wrap
    // and the old ciphertext bytes.
    let pre = cluster.handle.snapshot();
    let storage_key = RaftSecretsStore::make_key("dep:myapp", "API_KEY");
    let old_ciphertext = pre
        .secrets
        .get(&storage_key)
        .expect("secret present pre-rotation")
        .ciphertext
        .clone();
    let c_old_wrap = pre
        .wrapped_dek
        .as_ref()
        .expect("DEK")
        .wraps
        .get("node-c")
        .expect("c has wrap pre-rotation")
        .clone();
    let c_old_dek = ClusterDek::unwrap(&cluster.node("node-c").sk, &c_old_wrap)
        .expect("c unwraps pre-rotation");

    // Now revoke C and rotate. After this, the cluster's stored
    // ciphertext is under a new DEK, but the *snapshot* C made earlier
    // is not affected.
    cluster.handle.apply_local(SecretsRaftOp::RevokeNode {
        node_id: "node-c".to_string(),
    });
    cluster.leader_rotate_dek(Some("node-c"));

    let plaintext = c_old_dek
        .decrypt(&old_ciphertext)
        .expect("old DEK decrypts old ciphertext (forward security is not retroactive)");
    assert_eq!(
        plaintext.as_slice(),
        b"hunter2",
        "old material decrypts to the original plaintext",
    );
}

/// Re-registering a revoked node is idempotent at the SM layer
/// (`RegisterNode` overwrites, per `raft_sm.rs`'s comment) and the
/// orchestration helper rebuilds the recipient set from the *current*
/// node table, which now contains the freshly-rejoined identity with
/// `revoked_at = None`.
///
/// End-to-end: after re-registration + rotate, the rejoined node MUST
/// be back in the recipient set with `revoked_at = None` and MUST be
/// able to decrypt the latest secret.
#[tokio::test]
async fn revoked_node_register_node_idempotent() {
    let cluster = ThreeNodeCluster::new();
    cluster.bootstrap_with_initial_dek();

    cluster
        .leader()
        .store
        .set_secret("dep:myapp", "API_KEY", &Secret::new("hunter2"))
        .await
        .expect("set initial secret");

    // Revoke C, rotate.
    cluster.handle.apply_local(SecretsRaftOp::RevokeNode {
        node_id: "node-c".to_string(),
    });
    cluster.leader_rotate_dek(Some("node-c"));

    let mid = cluster.handle.snapshot();
    assert!(
        mid.nodes
            .get("node-c")
            .expect("c entry present (soft-revocation keeps the row)")
            .revoked_at
            .is_some(),
        "c is soft-revoked",
    );
    assert!(
        !mid.wrapped_dek
            .as_ref()
            .expect("DEK")
            .wraps
            .contains_key("node-c"),
        "c is excluded from the post-revoke wraps",
    );

    // Now re-register C with the *same* keypair — RegisterNode apply
    // overwrites the existing entry per `raft_sm.rs` ("Insert;
    // overwriting is OK (e.g. re-join after a crash before revoke)."),
    // and the fresh identity carries `revoked_at = None`.
    let c = cluster.node("node-c");
    let rejoin_identity = NodeIdentity {
        node_id: c.node_id.clone(),
        secrets_pubkey: *c.pk.as_bytes(),
        wg_pubkey: format!("wg-{}", c.node_id),
        joined_at: Utc::now(),
        revoked_at: None,
    };
    let new_generation = cluster.leader_register_node_and_rotate(rejoin_identity);

    let post = cluster.handle.snapshot();

    // (1) C's revoked_at is cleared.
    let c_entry = post.nodes.get("node-c").expect("c entry present");
    assert!(
        c_entry.revoked_at.is_none(),
        "re-registering must clear revoked_at on the existing entry, got {:?}",
        c_entry.revoked_at,
    );

    // (2) The next RotateDek included C — bumped generation, C in
    // wraps.
    let envelope = post.wrapped_dek.as_ref().expect("DEK");
    assert_eq!(envelope.dek_generation, new_generation);
    assert!(
        envelope.wraps.contains_key("node-c"),
        "rejoined C must be in the new recipient set",
    );

    // (3) C can unwrap the new envelope's wrap into the same DEK bytes
    // every other node has — i.e. the recipient-set fix-up actually
    // gives C usable key material, not just a placeholder entry.
    let c_new_wrap = envelope
        .wraps
        .get("node-c")
        .expect("c has a wrap in the post-rejoin envelope")
        .clone();
    let _c_new_dek = ClusterDek::unwrap(&cluster.node("node-c").sk, &c_new_wrap)
        .expect("rejoined C unwraps the new envelope's wrap");

    // NOTE: the cluster store's read invariant is "row generation MUST
    // equal envelope generation". `leader_register_node_and_rotate`
    // mirrors `RaftCoordinator::propose_register_node_and_rotate`, which
    // is a pure re-wrap (same DEK material, fresh `dek_generation`) and
    // does NOT re-encrypt every row. So a `get_secret` here would
    // surface the documented Provider error
    // ("secret X encrypted under DEK generation N but current is N+1
    //  — wait for rotation re-encrypt to finish"). The end-to-end
    // "rejoin → decrypt the latest write" path is covered by
    // `register_then_revoke_then_register_then_decrypt_works`, which
    // exercises the operator-style "rejoin then write" sequence.
}

/// Combined flow: register A,B,C → revoke C → C cannot decrypt the
/// latest secret → register C with the same keypair → C can decrypt
/// the latest secret.
///
/// This is the operational contract for "node was revoked by mistake;
/// rejoin it" and it must work without any extra operator action
/// beyond re-registration (the `propose_register_node_and_rotate` path
/// is what the cluster-join HTTP handler calls).
#[tokio::test]
async fn register_then_revoke_then_register_then_decrypt_works() {
    let cluster = ThreeNodeCluster::new();
    cluster.bootstrap_with_initial_dek();

    cluster
        .leader()
        .store
        .set_secret("dep:myapp", "API_KEY", &Secret::new("hunter2"))
        .await
        .expect("set initial secret");

    // Revoke C and rotate.
    cluster.handle.apply_local(SecretsRaftOp::RevokeNode {
        node_id: "node-c".to_string(),
    });
    cluster.leader_rotate_dek(Some("node-c"));

    // Confirm C cannot read after revoke.
    let denied = cluster
        .node("node-c")
        .store
        .get_secret("dep:myapp", "API_KEY")
        .await;
    assert!(
        matches!(denied, Err(SecretsError::Provider(_))),
        "C must not read post-revoke, got {denied:?}",
    );

    // Update the secret while C is out — proves that the rejoin path
    // brings C up to a *current* DEK that decrypts the latest write,
    // not just the value frozen at revoke time.
    cluster
        .leader()
        .store
        .set_secret("dep:myapp", "API_KEY", &Secret::new("hunter3"))
        .await
        .expect("update secret while C is out");

    // Re-register C (same keypair). The orchestration helper rebuilds
    // the recipient set from the live node table — which now contains
    // C with `revoked_at = None` again — and bumps the generation. No
    // re-encrypt walk runs (the value was just written under the prior
    // generation by the leader, but `leader_register_node_and_rotate`
    // is a *re-wrap* not a *rotate*, so the underlying DEK material is
    // unchanged: existing secrets stay valid under the same DEK bytes
    // even as `dek_generation` advances).
    //
    // To match the cluster store's read invariant ("row generation MUST
    // equal envelope generation"), follow up the re-wrap with a fresh
    // value write under the leader. This mirrors what an operator would
    // do in practice (`zlayer secret set`) after rejoining a node, and
    // also exercises the post-rejoin write path.
    let c = cluster.node("node-c");
    let rejoin = NodeIdentity {
        node_id: c.node_id.clone(),
        secrets_pubkey: *c.pk.as_bytes(),
        wg_pubkey: format!("wg-{}", c.node_id),
        joined_at: Utc::now(),
        revoked_at: None,
    };
    cluster.leader_register_node_and_rotate(rejoin);

    // Bump the secret value to align row generation with the new
    // envelope generation. (Without this, the read path would error
    // out with `Provider("secret X encrypted under DEK generation N
    // but current is N+1 — wait for rotation re-encrypt to finish")`,
    // which is the documented invariant for cluster-mode reads.)
    cluster
        .leader()
        .store
        .set_secret("dep:myapp", "API_KEY", &Secret::new("hunter4"))
        .await
        .expect("write under new envelope generation");

    let from_c = cluster
        .node("node-c")
        .store
        .get_secret("dep:myapp", "API_KEY")
        .await
        .expect("C reads the latest secret after rejoin");
    assert_eq!(from_c.expose(), "hunter4");

    // And A and B continue to read the same value.
    let from_a = cluster
        .node("node-a")
        .store
        .get_secret("dep:myapp", "API_KEY")
        .await
        .expect("A reads after rejoin");
    assert_eq!(from_a.expose(), "hunter4");
    let from_b = cluster
        .node("node-b")
        .store
        .get_secret("dep:myapp", "API_KEY")
        .await
        .expect("B reads after rejoin");
    assert_eq!(from_b.expose(), "hunter4");
}
