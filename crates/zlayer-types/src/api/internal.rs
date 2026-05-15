//! Internal API DTOs for scheduler-to-agent communication.
//!
//! These types describe the request/response payloads for the internal
//! endpoints used by the distributed scheduler to trigger operations on
//! agents. They use a shared secret for authentication rather than JWT
//! tokens.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request to scale a service
#[derive(Debug, Deserialize, ToSchema)]
pub struct InternalScaleRequest {
    /// Service name to scale
    pub service: String,
    /// Target replica count
    pub replicas: u32,
}

/// Response from internal scale operation
#[derive(Debug, Serialize, ToSchema)]
pub struct InternalScaleResponse {
    /// Whether the operation succeeded
    pub success: bool,
    /// Service name that was scaled
    pub service: String,
    /// New replica count
    pub replicas: u32,
    /// Optional message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// When set, this agent refused the scale because it cannot run the
    /// workload's OS (H-7 `RouteToPeer` policy). The value is the OCI-canonical
    /// OS string the workload requires (`linux` / `windows` / `darwin`). The
    /// scheduler catches this and re-dispatches to a cluster peer whose
    /// `NodeState.os` matches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reroute_to_os: Option<String>,
}

/// Request to add a `WireGuard` peer to the local overlay transport.
///
/// Sent by the leader to existing nodes when a new node joins the cluster,
/// so that all nodes learn about the new peer without waiting for periodic
/// reconciliation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InternalAddPeerRequest {
    /// New peer's `WireGuard` public key (base64)
    pub wg_public_key: String,
    /// New peer's overlay IP (e.g. "10.200.0.3")
    pub overlay_ip: String,
    /// New peer's `WireGuard` endpoint (e.g. "203.0.113.5:51820")
    pub endpoint: String,
}

/// Response from internal add-peer operation
#[derive(Debug, Serialize, ToSchema)]
pub struct InternalAddPeerResponse {
    /// Whether the operation succeeded
    pub success: bool,
    /// Optional message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Op type for the secrets Raft state machine.
///
/// Replicated through openraft alongside the existing scheduler ops.
/// `zlayer-consensus` carries the bytes; `zlayer-secrets`'s `raft_sm.rs`
/// applies them. The variants intentionally mirror the structure of
/// [`crate::storage::NodeIdentity`], [`crate::storage::WrappedDek`], and
/// [`crate::storage::ReplicatedSecret`] so the wire shape is identical to
/// the stored shape.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SecretsRaftOp {
    /// Register a new node. Triggers an automatic re-wrap of the current
    /// DEK so the new node can decrypt secrets going forward.
    RegisterNode {
        /// Identity payload (uuid, X25519 pubkey, WG pubkey, `joined_at`).
        identity: crate::storage::NodeIdentity,
    },

    /// Soft-revoke a node. Followers stop including it in DEK wraps; the
    /// next `RotateDek` excludes it permanently.
    RevokeNode {
        /// Cluster-wide node UUID being revoked.
        node_id: String,
    },

    /// Rotate the cluster DEK. The leader proposes a new generation with
    /// fresh per-node wraps; followers re-encrypt every `ReplicatedSecret`
    /// from the previous generation to the new one.
    RotateDek {
        /// New wrapped-DEK envelope (generation + per-node wraps).
        new_wraps: crate::storage::WrappedDek,
    },

    /// Insert or update a secret. The ciphertext is encrypted under the
    /// `dek_generation` recorded inside the payload.
    PutSecret {
        /// The full replicated secret record.
        secret: crate::storage::ReplicatedSecret,
    },

    /// Remove a secret entirely. Hard delete — re-encryption skips it.
    DeleteSecret {
        /// `"{scope}:{name}"` storage key, same shape as elsewhere.
        storage_key: String,
    },

    /// Revoke a specific issued join token (cannot be unrevoked).
    ///
    /// The token is identified by `token_hash`, which is the lowercase
    /// hex SHA-256 of the full token envelope b64 string (same hash form
    /// regardless of token format — Ed25519-signed envelope, HS256-JWT,
    /// or future EdDSA-JWT). The entry auto-expires at `expires_at` so
    /// the revocation table stays bounded by the un-expired token horizon.
    RevokeToken {
        /// Lowercase hex SHA-256 of the full token b64 envelope string.
        token_hash: String,
        /// Wall-clock instant at which the revocation entry may be pruned.
        /// Should match the token's own `exp` claim so the entry is no
        /// longer needed once the token would have expired anyway.
        #[schema(value_type = String, format = "date-time")]
        expires_at: chrono::DateTime<chrono::Utc>,
    },

    /// Import a foreign cluster's trust bundle so its tokens can be
    /// accepted by validators on this cluster.
    ///
    /// Idempotent: re-importing the same `cluster_domain` overwrites
    /// the previous entry. Keyed by `cluster_domain` to enforce one
    /// trust relationship per foreign cluster.
    ImportTrustBundle {
        /// The bundle to record in `SecretsState::trusted_bundles`.
        bundle: crate::api::cluster::TrustBundle,
    },

    /// Remove a previously-imported trust bundle.
    ///
    /// No-op if `cluster_domain` was not present. Used by the operator
    /// when revoking trust in a federated cluster.
    RemoveTrustBundle {
        /// Cluster domain of the bundle to remove.
        cluster_domain: String,
    },
}
