//! Phase-1 integration test (Task #19) for cluster-replicated secrets:
//! a secret written through the leader is observable on every node, and
//! the API-layer RBAC gate (`zlayer_api::require_secret_perm`) reaches
//! the same allow/deny decision on each node.
//!
//! # Approach: shared `SecretsState`, not real openraft
//!
//! Per the Task #19 guidance, this test takes the "shared `SecretsState`"
//! approach instead of bringing up three openraft instances over real
//! network sockets. Each "node" gets:
//!
//!   * its own `node_id`,
//!   * its own X25519 keypair (so each node unwraps its own copy of the
//!     cluster DEK from the per-generation envelope), and
//!   * its own `RaftSecretsStore`.
//!
//! All three stores read from one `Arc<RwLock<SecretsState>>`. The
//! "leader" is whichever store the test invokes `set_secret` /
//! `delete_secret` on; its `propose_*` calls apply the op to the shared
//! state directly, mirroring what every replica would observe after a
//! successful Raft commit. This loses asynchronous-replication realism
//! (election, log replication, snapshot install) but exercises the right
//! code paths in `RaftSecretsStore` (envelope unwrap, per-node DEK
//! cache, encrypt/decrypt, affinity gating, list filtering) — which is
//! the Phase-1 contract under test. Real Raft replication is covered by
//! `zlayer-scheduler`/`zlayer-consensus` tests.
//!
//! # What we cover
//!
//! For each `#[tokio::test]`:
//!
//!   * `secret_written_on_leader_is_readable_on_all_nodes`: the value
//!     written via store A (the "leader") decrypts to the same plaintext
//!     when fetched via stores B and C. Mirrors the admin-token API
//!     path: at the store layer there is no auth context, so admin
//!     access is "if the bytes get to you, you decrypt them".
//!
//!   * `secret_readable_on_all_nodes_with_user_grant`: admin path plus
//!     an `AuthActor` carrying a non-admin role and a direct
//!     `(secret, "default:foo", Read)` grant. We call
//!     `require_secret_perm` once *per node's* permission store
//!     (intentionally — a real cluster replicates RBAC too, but in
//!     Phase 1 each node has its own permission storage; we model that
//!     with three independent `InMemoryPermissionStore`s seeded with
//!     the same grant). All three must allow.
//!
//!   * `non_admin_without_grant_is_403_on_all_nodes`: the `require_secret_perm`
//!     gate must return `Err(ApiError::Forbidden(_))` against every
//!     node's permission store when the actor has no grants and no
//!     env-fallback applies.
//!
//!   * `delete_on_leader_propagates_to_all_nodes`: a `delete_secret` on
//!     A yields `NotFound` on B's and C's subsequent reads.
//!
//!   * `rotate_increments_version_visible_on_all_nodes`: writing then
//!     rotating bumps the version; the metadata listing on every node
//!     reports `version == 2`.

#![allow(clippy::expect_used)] // standard practice in test code

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;

use zlayer_api::handlers::users::AuthActor;
use zlayer_api::storage::{
    InMemoryEnvironmentStore, InMemoryGroupStore, InMemoryPermissionStore, PermissionLevel,
    PermissionStorage, StoredPermission, SubjectKind,
};
use zlayer_api::{require_secret_perm, ApiError};
use zlayer_secrets::{
    ClusterDek, RaftSecretsHandle, RaftSecretsStore, RecipientPrivateKey, RecipientPublicKey,
    Result as SecretsResult, Secret, SecretsError, SecretsProvider, SecretsState, SecretsStore,
};
use zlayer_types::api::internal::SecretsRaftOp;
use zlayer_types::storage::{NodeIdentity, ReplicatedSecret};

// ---------------------------------------------------------------------------
// Shared-state Raft handle
// ---------------------------------------------------------------------------

/// In-memory mock that all three node stores share. Mutations the
/// leader proposes are visible to followers' next reads because every
/// store observes the same `Arc<RwLock<SecretsState>>` — i.e. we model
/// the post-apply view every replica would have in a real Raft cluster.
///
/// `propose_*` bypasses any leader check: the fixture's caller is the
/// authority on which store is "leader" for a given operation. The trait
/// contract for Raft errors (leader redirect strings, propose timeouts)
/// is exercised in unit tests for the real `RaftCoordinator` impl in
/// `zlayer-scheduler`; here we focus on the read/write/decrypt flow.
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

// ---------------------------------------------------------------------------
// Three-node fixture
// ---------------------------------------------------------------------------

/// Three nodes (A, B, C). Each has its own `node_id`, its own X25519
/// keypair (so each node can unwrap its own copy of the cluster DEK),
/// its own `RaftSecretsStore`, and its own `InMemoryPermissionStore` /
/// `InMemoryGroupStore` / `InMemoryEnvironmentStore` so we can drive
/// `require_secret_perm` independently against each node.
///
/// All three secrets stores read/write through the same shared
/// `SecretsState`. The permission stores are deliberately independent
/// — Phase 1 RBAC is per-node (the user/permission store is local
/// SQLite/in-memory, not Raft-replicated), and the test seeds the same
/// grants into all three to model "the operator configured the same
/// RBAC on every node".
struct ThreeNodeCluster {
    // Kept for symmetry / documentation; current tests address nodes by
    // store handle directly. Future tests that exercise affinity or
    // node-specific delete will want the IDs.
    #[allow(dead_code)]
    a_id: String,
    #[allow(dead_code)]
    b_id: String,
    #[allow(dead_code)]
    c_id: String,
    store_a: RaftSecretsStore,
    store_b: RaftSecretsStore,
    store_c: RaftSecretsStore,
    perms_a: InMemoryPermissionStore,
    perms_b: InMemoryPermissionStore,
    perms_c: InMemoryPermissionStore,
    groups_a: InMemoryGroupStore,
    groups_b: InMemoryGroupStore,
    groups_c: InMemoryGroupStore,
    envs_a: InMemoryEnvironmentStore,
    envs_b: InMemoryEnvironmentStore,
    envs_c: InMemoryEnvironmentStore,
    /// Held so test bodies can apply ad-hoc ops if they need to.
    /// Currently unused but kept for future tests that want to
    /// simulate, e.g. a partial-revocation rotation.
    #[allow(dead_code)]
    state: Arc<RwLock<SecretsState>>,
}

impl ThreeNodeCluster {
    /// Iterate over `(label, perm_store, group_store, env_store)` for
    /// each node. Used by the RBAC tests so the same grant check is
    /// exercised against every node's permission storage in turn.
    fn per_node_perm_stores(
        &self,
    ) -> [(
        &'static str,
        &InMemoryPermissionStore,
        &InMemoryGroupStore,
        &InMemoryEnvironmentStore,
    ); 3] {
        [
            ("A", &self.perms_a, &self.groups_a, &self.envs_a),
            ("B", &self.perms_b, &self.groups_b, &self.envs_b),
            ("C", &self.perms_c, &self.groups_c, &self.envs_c),
        ]
    }

    /// Iterate over `(label, store)` for each node's secrets store.
    /// Tests use this to read the same secret from every node and
    /// assert the values match.
    fn per_node_secret_stores(&self) -> [(&'static str, &RaftSecretsStore); 3] {
        [
            ("A", &self.store_a),
            ("B", &self.store_b),
            ("C", &self.store_c),
        ]
    }
}

fn setup_three_node_cluster() -> ThreeNodeCluster {
    let a_id = "node-a".to_string();
    let b_id = "node-b".to_string();
    let c_id = "node-c".to_string();

    // Distinct X25519 keypair per node. Each node will unwrap its own
    // copy of the cluster DEK from the per-generation envelope.
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

    // Generate the cluster DEK and wrap to all three pubkeys at gen 1.
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

    // Pre-seed the shared state with all three identities and the
    // generation-1 envelope before any store sees it, so the very first
    // read on any node observes its own wrap.
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
        perms_a: InMemoryPermissionStore::new(),
        perms_b: InMemoryPermissionStore::new(),
        perms_c: InMemoryPermissionStore::new(),
        groups_a: InMemoryGroupStore::new(),
        groups_b: InMemoryGroupStore::new(),
        groups_c: InMemoryGroupStore::new(),
        envs_a: InMemoryEnvironmentStore::new(),
        envs_b: InMemoryEnvironmentStore::new(),
        envs_c: InMemoryEnvironmentStore::new(),
        state,
    }
}

/// Build an admin-role actor. The admin shortcut in `require_secret_perm`
/// returns `Ok(())` regardless of grants, so this models "operator with
/// the admin token reads from any node".
fn admin_actor(id: &str) -> AuthActor {
    AuthActor {
        user_id: id.to_string(),
        roles: vec!["admin".to_string()],
        email: None,
    }
}

/// Build a non-admin actor that holds only the `user` role. Pair with
/// targeted grants for the `secret` resource kind to drive the
/// direct-grant branch in `require_secret_perm`.
fn user_actor(id: &str) -> AuthActor {
    AuthActor {
        user_id: id.to_string(),
        roles: vec!["user".to_string()],
        email: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A secret written via store A is decrypted by every node's store, and
/// the API-layer admin shortcut allows the read on every node's
/// permission store. This is the "operator with the bootstrap admin
/// token writes a secret on the leader and reads it back from any node"
/// path.
#[tokio::test]
async fn secret_written_on_leader_is_readable_on_all_nodes_with_admin_token() {
    let cluster = setup_three_node_cluster();

    // Leader (A) writes.
    cluster
        .store_a
        .set_secret("default", "foo", &Secret::new("bar"))
        .await
        .expect("leader (A) set_secret");

    // Every node's `RaftSecretsStore` decrypts to the same plaintext.
    for (label, store) in cluster.per_node_secret_stores() {
        let got = store
            .get_secret("default", "foo")
            .await
            .unwrap_or_else(|e| panic!("node-{label} get_secret failed: {e:?}"));
        assert_eq!(got.expose(), "bar", "node-{label} read mismatch");
    }

    // Admin shortcut: every node's `require_secret_perm` allows.
    let admin = admin_actor("admin-1");
    for (label, perms, groups, envs) in cluster.per_node_perm_stores() {
        require_secret_perm(
            &admin,
            perms,
            groups,
            Some(envs),
            "default",
            "foo",
            PermissionLevel::Read,
        )
        .await
        .unwrap_or_else(|e| {
            panic!("node-{label}: admin shortcut should allow Read on default:foo, got {e:?}")
        });
    }
}

/// A non-admin user with a direct `(secret, "default:foo", Read)` grant
/// passes the API-layer gate on every node, and the value reads
/// correctly through every node's `RaftSecretsStore`.
///
/// Models the operator-side workflow: the admin grants `userA` Read on a
/// specific secret, and `userA` is then served by whichever node their
/// API call lands on.
#[tokio::test]
async fn secret_written_on_leader_is_readable_on_all_nodes_with_user_grant() {
    let cluster = setup_three_node_cluster();

    // Leader (A) writes.
    cluster
        .store_a
        .set_secret("default", "foo", &Secret::new("bar"))
        .await
        .expect("leader (A) set_secret");

    // Seed the same direct grant into every node's permission store.
    // We rebuild a fresh `StoredPermission` per insert because the
    // struct's `id` is a UUID generated in `new` — sharing the same
    // grant id across stores would still be valid, but separate ids
    // model the realistic "each node generated its own row".
    let user_id = "u1";
    for (label, perms, _, _) in cluster.per_node_perm_stores() {
        let grant = StoredPermission::new(
            SubjectKind::User,
            user_id,
            "secret",
            Some("default:foo".to_string()),
            PermissionLevel::Read,
        );
        <InMemoryPermissionStore as PermissionStorage>::grant(perms, &grant)
            .await
            .unwrap_or_else(|e| panic!("node-{label}: seeding grant failed: {e:?}"));
    }

    // RBAC: every node's gate must allow.
    let actor = user_actor(user_id);
    for (label, perms, groups, envs) in cluster.per_node_perm_stores() {
        require_secret_perm(
            &actor,
            perms,
            groups,
            Some(envs),
            "default",
            "foo",
            PermissionLevel::Read,
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "node-{label}: user {user_id} with direct Read grant should be allowed, got {e:?}",
            )
        });
    }

    // Value read: every node decrypts to the same plaintext.
    for (label, store) in cluster.per_node_secret_stores() {
        let got = store
            .get_secret("default", "foo")
            .await
            .unwrap_or_else(|e| panic!("node-{label} get_secret failed: {e:?}"));
        assert_eq!(got.expose(), "bar", "node-{label} read mismatch");
    }
}

/// A non-admin actor with no grants is rejected by `require_secret_perm`
/// on every node. The scope is intentionally non-env-shaped so the
/// env-fallback branch never fires; the only paths that could grant
/// access are the direct/wildcard secret grant and the group-membership
/// grant, neither of which exists.
#[tokio::test]
async fn non_admin_without_grant_403_on_all_nodes() {
    let cluster = setup_three_node_cluster();

    // The secret being present or absent must not change the gate's
    // verdict — the gate is auth, not visibility — but seed one
    // anyway so the test reflects the realistic "write happened, read
    // is being attempted by the wrong user" shape.
    cluster
        .store_a
        .set_secret("default", "foo", &Secret::new("bar"))
        .await
        .expect("leader (A) set_secret");

    let actor = user_actor("nobody");
    for (label, perms, groups, envs) in cluster.per_node_perm_stores() {
        let err = require_secret_perm(
            &actor,
            perms,
            groups,
            Some(envs),
            "default",
            "foo",
            PermissionLevel::Read,
        )
        .await
        .err()
        .unwrap_or_else(|| {
            panic!(
                "node-{label}: a user with no grants must be Forbidden on default:foo, \
                 but the call returned Ok(())",
            )
        });

        match err {
            ApiError::Forbidden(_) => {}
            other => panic!(
                "node-{label}: expected ApiError::Forbidden for ungranted Read, got {other:?}",
            ),
        }
    }
}

/// Delete on the leader is visible to every node: subsequent reads on
/// B and C return `NotFound` (the trait wrapper for the inner
/// `Ok(None)` after `state.secrets` no longer holds the key).
#[tokio::test]
async fn delete_on_leader_propagates_to_all_nodes() {
    let cluster = setup_three_node_cluster();

    // Set then delete on the leader.
    cluster
        .store_a
        .set_secret("default", "foo", &Secret::new("bar"))
        .await
        .expect("leader (A) set_secret");

    // Sanity: readable on all three before delete.
    for (label, store) in cluster.per_node_secret_stores() {
        let got = store
            .get_secret("default", "foo")
            .await
            .unwrap_or_else(|e| panic!("pre-delete: node-{label} get_secret failed: {e:?}"));
        assert_eq!(got.expose(), "bar");
    }

    cluster
        .store_a
        .delete_secret("default", "foo")
        .await
        .expect("leader (A) delete_secret");

    // After delete: every node returns NotFound.
    for (label, store) in cluster.per_node_secret_stores() {
        match store.get_secret("default", "foo").await {
            Err(SecretsError::NotFound { .. }) => {}
            Err(other) => panic!("node-{label}: expected NotFound after delete, got {other:?}",),
            Ok(_) => {
                panic!("node-{label}: expected NotFound after delete, but the secret was returned",)
            }
        }
    }
}

/// Rotation bumps the metadata `version` from 1 to 2, and every node
/// observes the new version through `list_secrets` (which is the
/// `SecretsProvider::list_secrets` trait method — the production
/// metadata listing path used by the API).
#[tokio::test]
async fn rotate_increments_version_visible_on_all_nodes() {
    let cluster = setup_three_node_cluster();

    // Initial set: version 1.
    cluster
        .store_a
        .set_secret("default", "foo", &Secret::new("v1"))
        .await
        .expect("leader (A) set_secret v1");

    // Rotate: version should become 2 on the returned `RotationResult`
    // and on every node's metadata listing.
    let rotation = cluster
        .store_a
        .rotate_secret("default", "foo", &Secret::new("v2"))
        .await
        .expect("leader (A) rotate_secret");
    assert_eq!(
        rotation.previous_version,
        Some(1),
        "previous_version should be 1 after first rotation",
    );
    assert_eq!(rotation.new_version, 2, "new_version should be 2");

    // Every node sees the rotated version in its metadata listing.
    for (label, store) in cluster.per_node_secret_stores() {
        let listed = store
            .list_secrets("default")
            .await
            .unwrap_or_else(|e| panic!("node-{label}: list_secrets failed: {e:?}"));
        let entry = listed
            .iter()
            .find(|m| m.name == "foo")
            .unwrap_or_else(|| panic!("node-{label}: rotated secret missing from listing"));
        assert_eq!(
            entry.version, 2,
            "node-{label}: version should be 2 after rotation, got {}",
            entry.version,
        );
    }

    // Belt-and-braces: the value-read path returns the rotated plaintext
    // on every node. Catches any regression where rotation updates
    // metadata but not ciphertext (or vice versa).
    for (label, store) in cluster.per_node_secret_stores() {
        let got = store
            .get_secret("default", "foo")
            .await
            .unwrap_or_else(|e| panic!("node-{label}: get_secret post-rotate failed: {e:?}"));
        assert_eq!(got.expose(), "v2", "node-{label}: post-rotate value");
    }
}
