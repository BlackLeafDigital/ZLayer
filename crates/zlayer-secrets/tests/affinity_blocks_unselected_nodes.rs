//! Phase-1 integration test (Task #19) for cluster-replicated secrets:
//! `node_affinity` must restrict which nodes can read a secret.
//!
//! # What this exercises
//!
//! Three [`RaftSecretsStore`] instances, each with a distinct `node_id` and
//! distinct X25519 keypair, sharing one [`SecretsState`] behind a single
//! `Arc<RwLock<_>>`. Writes the leader (node A) makes are observed by
//! followers B and C immediately because they all read the same state — i.e.
//! we are testing the [`RaftSecretsStore::read_inner`] affinity gate, not
//! the openraft replication path. (The replication path lives in
//! `zlayer-scheduler` and is exercised separately.)
//!
//! The contract under test (from `RaftSecretsStore::read_inner` /
//! `list_secrets`):
//!
//! - A secret with `node_affinity = Some(Nodes { [A] })` is `Ok(Some(_))`
//!   from A and `Ok(None)` from B/C — existence is intentionally **not**
//!   leaked. The trait-level `get_secret` wrapper turns that `None` into
//!   `Err(NotFound)`, but the listing / `exists` paths surface the `None`
//!   directly.
//! - The store layer enforces affinity unconditionally — there is no
//!   admin-bypass at this layer (admin role short-circuits *RBAC*, but the
//!   node-affinity gate is checked at the store regardless of caller role).
//! - `NodeAffinity::Labels { .. }` is conservatively allow-all in Phase 1
//!   because the SM does not yet carry node labels (see the `node_allowed`
//!   TODO). Phase 1.5 will wire real label matching.
//! - `node_affinity = None` (the default) is unrestricted: every node reads.
//! - Listings (`list_secrets`) are filtered per-node by the same gate, so
//!   different nodes see different sets without any extra plumbing in the
//!   API layer.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;

use zlayer_secrets::{
    ClusterDek, RaftSecretsHandle, RaftSecretsStore, RecipientPrivateKey, RecipientPublicKey,
    Result as SecretsResult, Secret, SecretsError, SecretsProvider, SecretsState, SecretsStore,
};
use zlayer_types::api::internal::SecretsRaftOp;
use zlayer_types::storage::{NodeAffinity, NodeIdentity, ReplicatedSecret};

/// In-memory mock that *all three* node stores share. Mutations the leader
/// proposes are visible to followers' reads on the next call because they
/// all observe the same `Arc<RwLock<SecretsState>>`.
///
/// In a real cluster this would be the openraft replication boundary — for
/// the affinity test we don't need real consensus; we just need every store
/// to read the same state map, which is exactly what a successful Raft apply
/// would yield on every replica.
struct SharedRaftHandle {
    state: Arc<RwLock<SecretsState>>,
}

impl SharedRaftHandle {
    fn new(state: Arc<RwLock<SecretsState>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl RaftSecretsHandle for SharedRaftHandle {
    async fn secrets_state(&self) -> SecretsState {
        self.state.read().await.clone()
    }

    async fn propose_put_secret(&self, secret: ReplicatedSecret) -> SecretsResult<()> {
        let mut guard = self.state.write().await;
        guard
            .apply(SecretsRaftOp::PutSecret { secret })
            .map_err(|e| SecretsError::Provider(format!("apply PutSecret failed: {e}")))
    }

    async fn propose_delete_secret(&self, storage_key: &str) -> SecretsResult<()> {
        let mut guard = self.state.write().await;
        guard
            .apply(SecretsRaftOp::DeleteSecret {
                storage_key: storage_key.to_string(),
            })
            .map_err(|e| SecretsError::Provider(format!("apply DeleteSecret failed: {e}")))
    }
}

/// Three-node fixture. Each node has its own `node_id`, its own X25519
/// keypair (so each can unwrap its own copy of the cluster DEK), and its
/// own [`RaftSecretsStore`]. All three stores read from one shared
/// [`SecretsState`], pre-populated with all three node identities and a
/// generation-1 DEK envelope wrapped to all three pubkeys.
struct ThreeNodeCluster {
    a_id: String,
    b_id: String,
    /// Kept for symmetry with `a_id` / `b_id`; the current tests never
    /// reach for it directly because no test secret has affinity that
    /// happens to *exclude* C. Suppress the dead-code warning rather than
    /// renaming — future tests will want all three IDs.
    #[allow(dead_code)]
    c_id: String,
    store_a: RaftSecretsStore,
    store_b: RaftSecretsStore,
    store_c: RaftSecretsStore,
    /// Held so test bodies can apply ad-hoc ops if they need to (we don't,
    /// but keeping it around documents the shape).
    #[allow(dead_code)]
    state: Arc<RwLock<SecretsState>>,
}

fn setup_three_node_cluster() -> ThreeNodeCluster {
    let a_id = "node-a".to_string();
    let b_id = "node-b".to_string();
    let c_id = "node-c".to_string();

    // Distinct keypair per node — affinity isn't about who can decrypt the
    // DEK, but every node still needs its own wrap so the read-path crypto
    // works for any node that *is* allowed.
    let (sk_a, pk_a) = RecipientPrivateKey::generate();
    let (sk_b, pk_b) = RecipientPrivateKey::generate();
    let (sk_c, pk_c) = RecipientPrivateKey::generate();

    let now = Utc::now();
    let identity_a = NodeIdentity {
        node_id: a_id.clone(),
        secrets_pubkey: *pk_a.as_bytes(),
        wg_pubkey: "wg-a".to_string(),
        joined_at: now,
        revoked_at: None,
    };
    let identity_b = NodeIdentity {
        node_id: b_id.clone(),
        secrets_pubkey: *pk_b.as_bytes(),
        wg_pubkey: "wg-b".to_string(),
        joined_at: now,
        revoked_at: None,
    };
    let identity_c = NodeIdentity {
        node_id: c_id.clone(),
        secrets_pubkey: *pk_c.as_bytes(),
        wg_pubkey: "wg-c".to_string(),
        joined_at: now,
        revoked_at: None,
    };

    // Generate the cluster DEK and wrap it for all three nodes at gen 1.
    let dek = ClusterDek::generate();
    let mut recipients: HashMap<String, RecipientPublicKey> = HashMap::new();
    recipients.insert(
        a_id.clone(),
        RecipientPublicKey::from_bytes(*pk_a.as_bytes()),
    );
    recipients.insert(
        b_id.clone(),
        RecipientPublicKey::from_bytes(*pk_b.as_bytes()),
    );
    recipients.insert(
        c_id.clone(),
        RecipientPublicKey::from_bytes(*pk_c.as_bytes()),
    );
    let envelope = dek
        .rewrap_for_set(&recipients, 1)
        .expect("rewrap DEK for the three-node set");

    // One state, shared. Pre-seed identities + DEK envelope synchronously
    // before any store gets a handle, so the first read on any node is
    // guaranteed to see the wraps for its own node_id.
    let mut initial = SecretsState::default();
    initial
        .apply(SecretsRaftOp::RegisterNode {
            identity: identity_a,
        })
        .expect("seed register node-a");
    initial
        .apply(SecretsRaftOp::RegisterNode {
            identity: identity_b,
        })
        .expect("seed register node-b");
    initial
        .apply(SecretsRaftOp::RegisterNode {
            identity: identity_c,
        })
        .expect("seed register node-c");
    initial
        .apply(SecretsRaftOp::RotateDek {
            new_wraps: envelope,
        })
        .expect("seed rotate DEK");
    let state = Arc::new(RwLock::new(initial));

    let handle_a: Arc<dyn RaftSecretsHandle> = Arc::new(SharedRaftHandle::new(state.clone()));
    let handle_b: Arc<dyn RaftSecretsHandle> = Arc::new(SharedRaftHandle::new(state.clone()));
    let handle_c: Arc<dyn RaftSecretsHandle> = Arc::new(SharedRaftHandle::new(state.clone()));

    let store_a = RaftSecretsStore::new(sk_a, a_id.clone(), handle_a);
    let store_b = RaftSecretsStore::new(sk_b, b_id.clone(), handle_b);
    let store_c = RaftSecretsStore::new(sk_c, c_id.clone(), handle_c);

    ThreeNodeCluster {
        a_id,
        b_id,
        c_id,
        store_a,
        store_b,
        store_c,
        state,
    }
}

/// Helper: a follower's `get_secret` returning `Err(NotFound)` is the
/// trait-wrapper translation of the inner `Ok(None)`. We assert the wrapper
/// shape because the public `SecretsProvider::get_secret` is what callers
/// use; the contract is "doesn't leak existence to nodes outside affinity",
/// and `NotFound` is indistinguishable from "no such secret" from the
/// caller's POV — which is exactly the intent.
fn assert_not_found(result: Result<Secret, SecretsError>, label: &str) {
    match result {
        Err(SecretsError::NotFound { .. }) => {}
        Err(other) => panic!(
            "{label}: expected SecretsError::NotFound (existence must not leak), got {other:?}",
        ),
        Ok(_) => panic!("{label}: expected NotFound; got a Secret — affinity is not enforced!"),
    }
}

#[tokio::test]
async fn affinity_nodes_a_only_blocks_b_and_c_from_reading() {
    let cluster = setup_three_node_cluster();

    // Leader (A) writes a secret restricted to node A only.
    cluster
        .store_a
        .set_secret_with_affinity(
            "default",
            "alpha",
            &Secret::new("only-a-may-read"),
            Some(&NodeAffinity::Nodes {
                node_ids: vec![cluster.a_id.clone()],
            }),
        )
        .await
        .expect("write secret with affinity=[A]");

    // A: full plaintext read.
    let got_a = cluster
        .store_a
        .get_secret("default", "alpha")
        .await
        .expect("A is in affinity set; must read plaintext");
    assert_eq!(got_a.expose(), "only-a-may-read");

    // B and C: must not learn the secret exists. The trait wrapper turns
    // the inner `Ok(None)` into `Err(NotFound)` — the indistinguishability
    // from "no such secret" is the whole point of returning `None` instead
    // of `Forbidden`.
    assert_not_found(
        cluster.store_b.get_secret("default", "alpha").await,
        "node-b read of A-affinity secret",
    );
    assert_not_found(
        cluster.store_c.get_secret("default", "alpha").await,
        "node-c read of A-affinity secret",
    );

    // Belt-and-braces: `exists` must not leak the secret to B or C
    // either. (A *should* see it; verify that side too so we catch a
    // regression that over-filters.)
    let a_exists = cluster
        .store_a
        .exists("default", "alpha")
        .await
        .expect("exists ok");
    assert!(
        a_exists,
        "node-a is in the affinity set; exists must return true",
    );
    let b_exists = cluster
        .store_b
        .exists("default", "alpha")
        .await
        .expect("exists ok");
    assert!(
        !b_exists,
        "node-b must not be told the affinity-A secret exists (exists returned true)",
    );
    let c_exists = cluster
        .store_c
        .exists("default", "alpha")
        .await
        .expect("exists ok");
    assert!(
        !c_exists,
        "node-c must not be told the affinity-A secret exists (exists returned true)",
    );
}

#[tokio::test]
async fn affinity_admin_token_does_not_bypass() {
    // The admin role short-circuits *RBAC* checks at the API layer, but the
    // node-affinity gate is enforced at the *store* layer regardless of
    // caller role. The store has no concept of caller identity at all — it
    // only knows its own `node_id` — so there is no API-layer admin bypass
    // that could ever bleed through to it.
    //
    // The test verifies this by reading directly through node B's
    // `RaftSecretsStore` (no auth context exists at this layer). A
    // hypothetical "admin bypass" would have to live here, in the store; it
    // doesn't, and B's read must still return the no-leak shape.
    //
    // (If a future change adds an admin-bypass at the store layer,
    // re-evaluate this test. The plan defaulted to "affinity blocks even
    // admin"; matching that is preferred.)
    let cluster = setup_three_node_cluster();

    cluster
        .store_a
        .set_secret_with_affinity(
            "default",
            "alpha",
            &Secret::new("only-a-even-for-admin"),
            Some(&NodeAffinity::Nodes {
                node_ids: vec![cluster.a_id.clone()],
            }),
        )
        .await
        .expect("write A-only secret");

    assert_not_found(
        cluster.store_b.get_secret("default", "alpha").await,
        "node-b read with implicit-admin context",
    );

    // And the listing path (used by the "admin sees everything" UI) must
    // also filter B's view down to nothing.
    let listed = cluster
        .store_b
        .list_secrets("default")
        .await
        .expect("list ok");
    assert!(
        listed.iter().all(|m| m.name != "alpha"),
        "node-b list must not include the A-affinity secret (got: {:?})",
        listed.iter().map(|m| &m.name).collect::<Vec<_>>(),
    );
}

#[tokio::test]
async fn affinity_label_selector_allow_all_until_phase_1_5() {
    // Phase-1 contract from `RaftSecretsStore::node_allowed`:
    //
    //     // TODO Phase 1.5: label matching. The SM has no node-label
    //     // table yet; until labels are wired in, conservatively allow
    //     // for both `None` and `Labels` and let the API gate enforce.
    //
    // i.e. a `Labels { .. }` selector is treated as allow-all at the store
    // layer in Phase 1 — the API gate is the authoritative enforcement
    // point until Phase 1.5 wires up real label matching. This test pins
    // that intentional behavior so any change to it is forced to update
    // the test (and presumably bump the phase).
    let cluster = setup_three_node_cluster();

    let mut labels = HashMap::new();
    labels.insert("tier".to_string(), "prod".to_string());

    cluster
        .store_a
        .set_secret_with_affinity(
            "default",
            "labelled",
            &Secret::new("phase-1-allow-all"),
            Some(&NodeAffinity::Labels { labels }),
        )
        .await
        .expect("write secret with label selector");

    for (label, store) in [
        ("A", &cluster.store_a),
        ("B", &cluster.store_b),
        ("C", &cluster.store_c),
    ] {
        let got = store
            .get_secret("default", "labelled")
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "node-{label}: Phase-1 label selectors are conservatively \
                     allow-all at the store layer; got error {e:?}",
                );
            });
        assert_eq!(
            got.expose(),
            "phase-1-allow-all",
            "node-{label} should read the labelled secret in Phase 1",
        );
    }
}

#[tokio::test]
async fn affinity_none_allows_all_nodes() {
    // Sanity: the default (`node_affinity = None`) is unrestricted. This
    // guards the most common path against an over-eager affinity check
    // landing in `node_allowed` in the future.
    let cluster = setup_three_node_cluster();

    cluster
        .store_a
        .set_secret("default", "open", &Secret::new("anyone-may-read"))
        .await
        .expect("write unrestricted secret");

    for (label, store) in [
        ("A", &cluster.store_a),
        ("B", &cluster.store_b),
        ("C", &cluster.store_c),
    ] {
        let got = store
            .get_secret("default", "open")
            .await
            .unwrap_or_else(|e| panic!("node-{label}: unrestricted read failed: {e:?}"));
        assert_eq!(got.expose(), "anyone-may-read", "node-{label} read");
    }
}

#[tokio::test]
async fn affinity_listing_filters_secrets_per_node() {
    // Three secrets with three different affinity shapes. Each node should
    // see only the subset its `node_id` is allowed to host. The filtering
    // happens in `RaftSecretsStore::list_secrets` itself (it walks the
    // shared state and skips entries where `node_allowed` returns false),
    // so this exercises the same gate as the read path on a different code
    // path.
    let cluster = setup_three_node_cluster();

    // alpha: A-only.
    cluster
        .store_a
        .set_secret_with_affinity(
            "default",
            "alpha",
            &Secret::new("alpha-value"),
            Some(&NodeAffinity::Nodes {
                node_ids: vec![cluster.a_id.clone()],
            }),
        )
        .await
        .expect("write alpha");

    // beta: unrestricted.
    cluster
        .store_a
        .set_secret_with_affinity("default", "beta", &Secret::new("beta-value"), None)
        .await
        .expect("write beta");

    // gamma: A and B (C is excluded).
    cluster
        .store_a
        .set_secret_with_affinity(
            "default",
            "gamma",
            &Secret::new("gamma-value"),
            Some(&NodeAffinity::Nodes {
                node_ids: vec![cluster.a_id.clone(), cluster.b_id.clone()],
            }),
        )
        .await
        .expect("write gamma");

    let names_for = |list: Vec<zlayer_secrets::SecretMetadata>| -> Vec<String> {
        let mut names: Vec<String> = list.into_iter().map(|m| m.name).collect();
        names.sort();
        names
    };

    let list_a = cluster
        .store_a
        .list_secrets("default")
        .await
        .expect("list A");
    assert_eq!(
        names_for(list_a),
        vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
        "node-a should see all three secrets (alpha, beta, gamma)",
    );

    let list_b = cluster
        .store_b
        .list_secrets("default")
        .await
        .expect("list B");
    assert_eq!(
        names_for(list_b),
        vec!["beta".to_string(), "gamma".to_string()],
        "node-b should see beta and gamma (alpha is A-only)",
    );

    let list_c = cluster
        .store_c
        .list_secrets("default")
        .await
        .expect("list C");
    assert_eq!(
        names_for(list_c),
        vec!["beta".to_string()],
        "node-c should see only beta (alpha is A-only, gamma is A+B)",
    );

    // Cross-check that the value-read path agrees with the listing path —
    // C can read beta, but cannot read alpha or gamma.
    let beta_on_c = cluster
        .store_c
        .get_secret("default", "beta")
        .await
        .expect("C reads unrestricted beta");
    assert_eq!(beta_on_c.expose(), "beta-value");
    assert_not_found(
        cluster.store_c.get_secret("default", "alpha").await,
        "node-c read of alpha (A-only)",
    );
    assert_not_found(
        cluster.store_c.get_secret("default", "gamma").await,
        "node-c read of gamma (A+B-only)",
    );
}
