//! Cluster join / membership wire DTOs.
//!
//! Lifted from `zlayer-api::handlers::cluster` so the CLI, the manager UI,
//! and any other client can describe these requests/responses without
//! depending on `zlayer-api`. The handler itself stays in `zlayer-api`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::nodes::GpuInfoSummary;
use crate::spec::{ArchKind, OsKind};

/// Request body for `POST /api/v1/cluster/join`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ClusterJoinRequest {
    /// Base64-encoded join token (contains `auth_secret` for validation)
    pub token: String,
    /// Joining node's advertise address (IP)
    pub advertise_addr: String,
    /// Joining node's overlay port (`WireGuard`)
    pub overlay_port: u16,
    /// Joining node's Raft RPC port
    pub raft_port: u16,
    /// Joining node's API server port
    #[serde(default = "default_api_port")]
    pub api_port: u16,
    /// Joining node's `WireGuard` public key
    pub wg_public_key: String,
    /// Node mode: "full" or "replicate"
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Services to replicate (only if mode == "replicate")
    pub services: Option<Vec<String>>,
    /// Total CPU cores on the joining node
    #[serde(default)]
    pub cpu_total: f64,
    /// Total memory in bytes
    #[serde(default)]
    pub memory_total: u64,
    /// Total disk in bytes
    #[serde(default)]
    pub disk_total: u64,
    /// Detected GPUs
    #[serde(default)]
    pub gpus: Vec<GpuInfoSummary>,
    /// Operating system of the joining agent. `None` = legacy client that did
    /// not report platform info.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<OsKind>,
    /// CPU architecture of the joining agent. Same legacy semantics as `os`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<ArchKind>,
    /// Joiner's 32-byte X25519 pubkey for sealed-box DEK wrapping.
    /// Present on Phase-1+ joiners; absent on legacy clients (in which
    /// case the leader treats the node as not eligible to host
    /// replicated-secret ciphertext until it re-joins with a pubkey).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets_pubkey: Option<[u8; 32]>,
}

#[must_use]
pub fn default_mode() -> String {
    "full".to_string()
}

#[must_use]
pub fn default_api_port() -> u16 {
    3669
}

/// Response body for `POST /api/v1/cluster/join`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ClusterJoinResponse {
    /// Assigned node UUID
    pub node_id: String,
    /// Assigned Raft node ID (monotonic u64)
    pub raft_node_id: u64,
    /// Assigned overlay IP for the new node
    pub overlay_ip: String,
    /// Per-node slice CIDR assigned by the leader (e.g. "10.200.42.0/28").
    /// Empty string if the leader is not slice-aware yet.
    #[serde(default)]
    pub slice_cidr: String,
    /// Existing peers in the cluster
    pub peers: Vec<ClusterPeer>,
    /// Role assigned to this node: "voter" or "learner"
    pub role: String,
    /// Node JWT minted by the leader for this joiner — `roles: ["node"]`,
    /// `node_id` set. Used to authenticate inter-node calls separately
    /// from any user identity. `None` on legacy responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_jwt: Option<String>,
    /// Sealed-box-wrapped copy of the cluster DEK addressed to the
    /// joiner's `secrets_pubkey`. The joiner unwraps with its node X25519
    /// private key and holds the DEK in zeroized memory. `None` on legacy
    /// responses or when the joiner did not provide a `secrets_pubkey`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapped_dek: Option<Vec<u8>>,
    /// Cluster DEK generation that `wrapped_dek` was sealed under. Lets
    /// the joiner detect rotation drift if it re-joins after a revocation
    /// rotated the cluster DEK. `None` when `wrapped_dek` is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dek_generation: Option<u64>,
}

/// Summary of an existing cluster peer returned in join response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ClusterPeer {
    /// UUID
    pub node_id: String,
    /// Raft node ID
    pub raft_node_id: u64,
    /// Advertise address
    pub advertise_addr: String,
    /// Overlay port
    pub overlay_port: u16,
    /// Raft port
    pub raft_port: u16,
    /// `WireGuard` public key
    pub wg_public_key: String,
    /// Overlay IP
    pub overlay_ip: String,
}

/// Summary of a cluster node for listing.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ClusterNodeSummary {
    /// UUID or Raft-level ID
    pub id: String,
    /// Network address (Raft RPC address)
    pub address: String,
    /// Advertise address (public IP)
    pub advertise_addr: String,
    /// Current status (e.g. "ready", "draining", "dead")
    pub status: String,
    /// Role in the Raft cluster: "leader", "voter", or "learner"
    pub role: String,
    /// Join mode: "full" or "replicate"
    pub mode: String,
    /// Whether this node is the Raft leader
    pub is_leader: bool,
    /// Overlay network IP assigned to this node
    pub overlay_ip: String,
    /// Total CPU cores on this node
    pub cpu_total: f64,
    /// Current CPU usage (cores)
    pub cpu_used: f64,
    /// Total memory in bytes
    pub memory_total: u64,
    /// Current memory usage in bytes
    pub memory_used: u64,
    /// When the node was registered (Unix timestamp ms)
    pub registered_at: u64,
    /// Last heartbeat timestamp (Unix timestamp ms)
    pub last_heartbeat: u64,
}
