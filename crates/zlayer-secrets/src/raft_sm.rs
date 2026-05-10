//! In-memory state and apply logic for the cluster secrets state machine.
//!
//! Pure synchronous logic — no IO, no crypto, no async. The leader-side
//! orchestration that decides *when* to propose [`SecretsRaftOp`] variants
//! lives in `zlayer-scheduler`'s Raft integration; the actual crypto for
//! wrapping/encrypting lives in `crate::cluster_dek` (added in a sibling
//! task). This module just takes ops off the Raft log and updates local
//! state deterministically so every replica converges on the same view.

use std::collections::HashMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use zlayer_types::api::internal::SecretsRaftOp;
use zlayer_types::storage::{NodeIdentity, ReplicatedSecret, WrappedDek};

use crate::SecretsError;

/// Snapshot of the cluster secrets state on this node.
///
/// Followers and the leader hold identical content. Snapshots
/// (de)serialize through serde for openraft.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SecretsState {
    /// Every node ever registered, keyed by `node_id`. Soft-revocation
    /// is recorded inline (`NodeIdentity::revoked_at`); the entry is
    /// kept so historical wraps in old `WrappedDek` generations can still
    /// be referenced for audit.
    pub nodes: HashMap<String, NodeIdentity>,

    /// Current cluster DEK envelope (per-node sealed-box wraps + generation).
    /// `None` until the first `RegisterNode` + `RotateDek` pair lands.
    pub wrapped_dek: Option<WrappedDek>,

    /// Replicated secrets, keyed by their `storage_key` (`"{scope}:{name}"`).
    pub secrets: HashMap<String, ReplicatedSecret>,
}

impl SecretsState {
    /// Apply a Raft op to local state.
    ///
    /// Deterministic — every replica that sees the same op sequence must
    /// end up with the same `SecretsState`. Returns an error only on
    /// genuinely impossible inputs (e.g. `DeleteSecret` for an unknown
    /// key); the leader's orchestration is expected to ensure the inputs
    /// are well-formed before proposing.
    ///
    /// # Errors
    /// - [`SecretsError::Provider`] if the op references state that
    ///   doesn't exist (revoke unknown node, delete unknown secret).
    pub fn apply(&mut self, op: SecretsRaftOp) -> Result<(), SecretsError> {
        match op {
            SecretsRaftOp::RegisterNode { identity } => {
                // Insert; overwriting is OK (e.g. re-join after a crash before revoke).
                self.nodes.insert(identity.node_id.clone(), identity);
                Ok(())
            }
            SecretsRaftOp::RevokeNode { node_id } => {
                let entry = self.nodes.get_mut(&node_id).ok_or_else(|| {
                    SecretsError::Provider(format!("RevokeNode for unknown node_id: {node_id}"))
                })?;
                if entry.revoked_at.is_none() {
                    entry.revoked_at = Some(Utc::now());
                }
                Ok(())
            }
            SecretsRaftOp::RotateDek { new_wraps } => {
                // Replace wholesale. The leader is responsible for
                // emitting a sequence of `PutSecret` re-encrypts after
                // the rotation; followers just store the new envelope
                // and apply re-encrypts as they arrive.
                self.wrapped_dek = Some(new_wraps);
                Ok(())
            }
            SecretsRaftOp::PutSecret { secret } => {
                self.secrets.insert(secret.storage_key.clone(), secret);
                Ok(())
            }
            SecretsRaftOp::DeleteSecret { storage_key } => {
                self.secrets.remove(&storage_key).ok_or_else(|| {
                    SecretsError::Provider(format!(
                        "DeleteSecret for unknown storage_key: {storage_key}"
                    ))
                })?;
                Ok(())
            }
        }
    }

    /// Serialize the state for an openraft snapshot. Uses JSON for now;
    /// the consensus wire-up task may swap this for a more compact codec
    /// once it audits whatever the scheduler SM uses.
    ///
    /// # Errors
    /// - [`SecretsError::Storage`] if serialization fails.
    pub fn snapshot(&self) -> Result<Vec<u8>, SecretsError> {
        serde_json::to_vec(self).map_err(|e| SecretsError::Storage(format!("snapshot: {e}")))
    }

    /// Restore from a snapshot blob produced by [`Self::snapshot`].
    ///
    /// # Errors
    /// - [`SecretsError::Storage`] if deserialization fails.
    pub fn restore(bytes: &[u8]) -> Result<Self, SecretsError> {
        serde_json::from_slice(bytes).map_err(|e| SecretsError::Storage(format!("restore: {e}")))
    }

    /// Convenience: is this node currently in the active recipient set
    /// for the current DEK generation?
    #[must_use]
    pub fn node_can_decrypt(&self, node_id: &str) -> bool {
        self.wrapped_dek
            .as_ref()
            .is_some_and(|w| w.wraps.contains_key(node_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use zlayer_types::secrets::SecretMetadata;

    fn make_identity(node_id: &str) -> NodeIdentity {
        NodeIdentity {
            node_id: node_id.to_string(),
            secrets_pubkey: [0u8; 32],
            wg_pubkey: format!("wg-{node_id}"),
            joined_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            revoked_at: None,
        }
    }

    fn make_wrapped_dek(generation: u64, node_ids: &[&str]) -> WrappedDek {
        let mut wraps = HashMap::new();
        for nid in node_ids {
            wraps.insert((*nid).to_string(), vec![0xAB, 0xCD]);
        }
        WrappedDek {
            dek_generation: generation,
            wraps,
        }
    }

    fn make_secret(name: &str, generation: u64) -> ReplicatedSecret {
        ReplicatedSecret {
            storage_key: format!("dep:{name}"),
            ciphertext: vec![1, 2, 3, 4],
            dek_generation: generation,
            metadata: SecretMetadata::new(name),
            node_affinity: None,
        }
    }

    #[test]
    fn apply_register_node_inserts() {
        let mut state = SecretsState::default();
        state
            .apply(SecretsRaftOp::RegisterNode {
                identity: make_identity("node-a"),
            })
            .expect("register should succeed");
        assert_eq!(state.nodes.len(), 1);
        assert!(state.nodes.contains_key("node-a"));
    }

    #[test]
    fn apply_register_node_overwrites_existing() {
        let mut state = SecretsState::default();
        let mut first = make_identity("node-a");
        first.wg_pubkey = "wg-original".to_string();
        state
            .apply(SecretsRaftOp::RegisterNode { identity: first })
            .expect("first register");

        let mut second = make_identity("node-a");
        second.wg_pubkey = "wg-replaced".to_string();
        state
            .apply(SecretsRaftOp::RegisterNode { identity: second })
            .expect("second register should not error");

        assert_eq!(state.nodes.len(), 1);
        assert_eq!(state.nodes["node-a"].wg_pubkey, "wg-replaced");
    }

    #[test]
    fn apply_revoke_node_marks_revoked_at() {
        let mut state = SecretsState::default();
        state
            .apply(SecretsRaftOp::RegisterNode {
                identity: make_identity("node-a"),
            })
            .expect("register");
        state
            .apply(SecretsRaftOp::RevokeNode {
                node_id: "node-a".to_string(),
            })
            .expect("revoke");
        assert!(state.nodes["node-a"].revoked_at.is_some());

        // Idempotent: revoking again should not error and should not
        // overwrite the original revocation timestamp.
        let original_ts = state.nodes["node-a"].revoked_at;
        state
            .apply(SecretsRaftOp::RevokeNode {
                node_id: "node-a".to_string(),
            })
            .expect("revoke again");
        assert_eq!(state.nodes["node-a"].revoked_at, original_ts);
    }

    #[test]
    fn apply_revoke_unknown_node_errors() {
        let mut state = SecretsState::default();
        let err = state
            .apply(SecretsRaftOp::RevokeNode {
                node_id: "missing".to_string(),
            })
            .expect_err("revoke unknown should fail");
        assert!(matches!(err, SecretsError::Provider(_)), "got: {err:?}");
    }

    #[test]
    fn apply_rotate_dek_replaces_wraps() {
        let mut state = SecretsState::default();
        state
            .apply(SecretsRaftOp::RotateDek {
                new_wraps: make_wrapped_dek(1, &["node-a"]),
            })
            .expect("rotate 1");
        state
            .apply(SecretsRaftOp::RotateDek {
                new_wraps: make_wrapped_dek(2, &["node-a", "node-b"]),
            })
            .expect("rotate 2");
        let dek = state.wrapped_dek.as_ref().expect("dek present");
        assert_eq!(dek.dek_generation, 2);
        assert_eq!(dek.wraps.len(), 2);
        assert!(dek.wraps.contains_key("node-a"));
        assert!(dek.wraps.contains_key("node-b"));
    }

    #[test]
    fn apply_put_secret_inserts_then_overwrites() {
        let mut state = SecretsState::default();
        let mut first = make_secret("api-key", 1);
        first.ciphertext = vec![0xDE, 0xAD];
        state
            .apply(SecretsRaftOp::PutSecret {
                secret: first.clone(),
            })
            .expect("put 1");
        assert_eq!(state.secrets.len(), 1);
        assert_eq!(
            state.secrets[&first.storage_key].ciphertext,
            vec![0xDE, 0xAD]
        );

        let mut second = make_secret("api-key", 2);
        second.ciphertext = vec![0xBE, 0xEF];
        state
            .apply(SecretsRaftOp::PutSecret {
                secret: second.clone(),
            })
            .expect("put 2");
        assert_eq!(state.secrets.len(), 1);
        assert_eq!(
            state.secrets[&second.storage_key].ciphertext,
            vec![0xBE, 0xEF]
        );
        assert_eq!(state.secrets[&second.storage_key].dek_generation, 2);
    }

    #[test]
    fn apply_delete_secret_removes() {
        let mut state = SecretsState::default();
        let secret = make_secret("api-key", 1);
        let key = secret.storage_key.clone();
        state
            .apply(SecretsRaftOp::PutSecret { secret })
            .expect("put");
        state
            .apply(SecretsRaftOp::DeleteSecret {
                storage_key: key.clone(),
            })
            .expect("delete");
        assert!(state.secrets.is_empty());
    }

    #[test]
    fn apply_delete_unknown_secret_errors() {
        let mut state = SecretsState::default();
        let err = state
            .apply(SecretsRaftOp::DeleteSecret {
                storage_key: "dep:nope".to_string(),
            })
            .expect_err("delete unknown should fail");
        assert!(matches!(err, SecretsError::Provider(_)), "got: {err:?}");
    }

    #[test]
    fn snapshot_round_trip() {
        let mut state = SecretsState::default();
        state
            .apply(SecretsRaftOp::RegisterNode {
                identity: make_identity("node-a"),
            })
            .expect("register a");
        state
            .apply(SecretsRaftOp::RegisterNode {
                identity: make_identity("node-b"),
            })
            .expect("register b");
        state
            .apply(SecretsRaftOp::RotateDek {
                new_wraps: make_wrapped_dek(7, &["node-a", "node-b"]),
            })
            .expect("rotate");
        state
            .apply(SecretsRaftOp::PutSecret {
                secret: make_secret("api-key", 7),
            })
            .expect("put");
        state
            .apply(SecretsRaftOp::RevokeNode {
                node_id: "node-b".to_string(),
            })
            .expect("revoke b");

        let bytes = state.snapshot().expect("snapshot ok");
        let restored = SecretsState::restore(&bytes).expect("restore ok");

        // Storage shapes don't derive PartialEq, and `HashMap` iteration
        // order isn't stable across snapshot/restore. Compare the parsed
        // JSON values (which match by object content rather than key
        // insertion order) so the assertion isn't flaky.
        let bytes2 = restored.snapshot().expect("snapshot restored ok");
        let v1: serde_json::Value = serde_json::from_slice(&bytes).expect("parse v1");
        let v2: serde_json::Value = serde_json::from_slice(&bytes2).expect("parse v2");
        assert_eq!(v1, v2);

        // And the restored shape exposes the same surface as the original.
        assert_eq!(restored.nodes.len(), state.nodes.len());
        assert_eq!(restored.secrets.len(), state.secrets.len());
        assert_eq!(
            restored.wrapped_dek.as_ref().map(|w| w.dek_generation),
            state.wrapped_dek.as_ref().map(|w| w.dek_generation),
        );
    }

    #[test]
    fn node_can_decrypt_reflects_wraps() {
        let mut state = SecretsState::default();
        assert!(!state.node_can_decrypt("node-a"));

        state
            .apply(SecretsRaftOp::RotateDek {
                new_wraps: make_wrapped_dek(1, &["node-a"]),
            })
            .expect("rotate include");
        assert!(state.node_can_decrypt("node-a"));
        assert!(!state.node_can_decrypt("node-b"));

        state
            .apply(SecretsRaftOp::RotateDek {
                new_wraps: make_wrapped_dek(2, &["node-b"]),
            })
            .expect("rotate exclude a");
        assert!(!state.node_can_decrypt("node-a"));
        assert!(state.node_can_decrypt("node-b"));
    }
}
