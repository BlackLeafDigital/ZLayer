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
    /// Server-side advisory warnings to surface to the operator/CLI.
    ///
    /// Examples: "your token format is deprecated and will be removed in
    /// release X.Y", "consider rotating the signing key, last rotated N
    /// days ago". Present-but-empty means "no warnings"; serialized as
    /// `null` (skip-if-none) when there are none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
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

/// Claims carried inside a signed cluster join token.
///
/// **Field declaration order is the canonical signing order.** Do NOT
/// reorder these fields without bumping the envelope's `v` and writing
/// a migration — Wave 3.2's `mint_signed_cluster_join_token` signs
/// `serde_json::to_vec(&claims)` directly, which depends on the
/// declaration order being stable.
///
/// Timestamps are RFC3339 strings (not Unix epoch) so a token printed
/// to a wiki or chat log is human-readable. `chrono::DateTime<Utc>` would
/// also work; we chose `String` to keep the wire format trivially
/// inspectable with `base64 -d | jq .`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ClusterJoinClaims {
    /// Public API endpoint of the issuing leader (e.g. `https://leader.prod:3669`).
    pub api_endpoint: String,
    /// Raft endpoint of the issuing leader (e.g. `10.0.0.1:3670`).
    pub raft_endpoint: String,
    /// `WireGuard` public key of the issuing leader (base64 standard, no-pad).
    pub leader_wg_pubkey: String,
    /// Overlay CIDR the cluster owns (e.g. `10.42.0.0/16`).
    pub overlay_cidr: String,
    /// Expiration as RFC3339, e.g. `2026-05-15T17:55:00Z`.
    pub exp: String,
    /// Issued-at as RFC3339.
    pub iat: String,
    /// Issuing leader node identity. In Wave 3 this is the raw node UUID;
    /// Wave 9 will switch this to a `spiffe://<cluster_domain>/<node_id>` URI
    /// (token format version bump). Verifiers in Wave 3 treat `iss` as opaque
    /// metadata — no parsing required.
    pub iss: String,
}

/// Envelope around `ClusterJoinClaims` carrying the Ed25519 signature.
///
/// On the wire, this struct is serialized as JSON and then base64
/// url-safe-no-pad encoded. The Wave 3.2 mint function produces that
/// outer base64; the parser reverses it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SignedClusterJoinToken {
    /// Format version. `1` in Wave 3. Wave 9 will introduce `v=2` (adds
    /// a `ca_chain` field for federated trust); parsers MUST reject any
    /// version they don't understand.
    pub v: u32,
    /// Key identifier (first 8 hex chars of SHA-256 over the verifying
    /// key bytes). Lets joining nodes pick the correct pubkey during
    /// rotation (Wave 5).
    pub kid: String,
    /// The payload that's actually signed.
    pub claims: ClusterJoinClaims,
    /// Ed25519 signature over `serde_json::to_vec(&claims)`, encoded as
    /// URL-safe no-pad base64.
    pub sig: String,
}

/// Current envelope version Wave-3 mints. Re-export so mint and verify
/// stay in lockstep without a stringly-typed constant elsewhere.
pub const SIGNED_TOKEN_V_WAVE3: u32 = 1;

/// Response body for `GET /api/v1/cluster/signing-pubkey`.
///
/// Returns the cluster's currently-active Ed25519 verifying key in URL-safe
/// no-pad base64, along with a short identifier (`kid`) derived from
/// `SHA-256(verifying_key)[..4]` (first 8 hex chars). Joining nodes use
/// `kid` to disambiguate during key rotation (Wave 5).
///
/// This endpoint is intentionally unauthenticated: the data is a public key.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SigningPubkeyResponse {
    /// URL-safe no-pad base64 of the 32-byte Ed25519 verifying key.
    pub public_key_b64: String,
    /// Short greppable key id: first 8 hex chars of SHA-256(verifying_key).
    pub kid: String,
}

/// Per-key entry returned by `GET /api/v1/cluster/signing-pubkeys`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SigningPubkeyEntry {
    /// Short key id (8 hex chars).
    pub kid: String,
    /// URL-safe no-pad base64 of the Ed25519 verifying key (32 bytes → 43 chars).
    pub public_key_b64: String,
    /// `"active"` or `"grace"`. Active = newly-issued tokens use this key.
    /// Grace = previously active; still verifies in-flight tokens until
    /// `valid_until`.
    pub status: String,
    /// RFC3339 timestamp this key stops being accepted. Only present for
    /// `status = "grace"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<String>,
    /// RFC3339 timestamp this key was created.
    pub created_at: String,
}

/// Response body for `GET /api/v1/cluster/signing-pubkeys`.
///
/// Returns every currently-trusted Ed25519 verifying key. The first entry
/// is the active key (the one new tokens are minted under); subsequent
/// entries are grace-period keys that still accept verification but not
/// minting. Joining nodes use this when fetching keys for a token whose
/// `kid` is a previous-active (rotated-out) key.
///
/// Unauthenticated by design (matches `signing-pubkey` singular).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SigningPubkeysResponse {
    pub keys: Vec<SigningPubkeyEntry>,
}

/// Request body for `POST /api/v1/cluster/rotate-signing-key`.
///
/// Triggers a rotation of the cluster's Ed25519 signing keystore: a fresh
/// keypair is generated, set as the new active key, and the previously
/// active key is moved into the grace map. Grace-period keys continue to
/// verify in-flight join tokens until their `valid_until` timestamp.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct RotateSigningKeyRequest {
    /// How long the previous-active key should remain valid for verifying
    /// in-flight tokens after rotation. Humantime syntax (`24h`, `7d`).
    /// Defaults to `7d` if omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace: Option<String>,
}

/// Response body for `POST /api/v1/cluster/rotate-signing-key`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RotateSigningKeyResponse {
    /// New active key id (8 hex chars).
    pub kid: String,
    /// URL-safe no-pad base64 of the new active verifying key.
    pub public_key_b64: String,
    /// Previous active kid, now in grace.
    pub previous_kid: String,
    /// RFC3339 timestamp when the previous key's grace expires.
    pub previous_grace_until: String,
}

/// Request body for `POST /api/v1/cluster/revoke-token`.
///
/// The operator supplies EITHER the raw token (the same b64 envelope
/// string `zlayer node generate-join-token` printed) OR the lowercase
/// hex SHA-256 of that string. The server normalises to the hash form
/// before proposing the Raft op so the actual token never enters
/// replicated state. A `reason` may be attached for audit.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevokeTokenRequest {
    /// Either the raw token envelope (b64 string) or its lowercase hex
    /// SHA-256. The handler auto-detects which: 64 lowercase hex chars
    /// is treated as a hash; anything else is hashed before insertion.
    pub token_or_hash: String,
    /// Optional human-readable reason recorded in the audit log
    /// (NOT replicated — local to the leader that processed the request).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Response body for `POST /api/v1/cluster/revoke-token`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevokeTokenResponse {
    /// The canonical hash form of the revoked token (lowercase hex SHA-256).
    pub token_hash: String,
    /// RFC3339 timestamp when the revocation entry will be pruned. Matches
    /// the token's own `exp` claim if the server could parse the envelope,
    /// or `now() + 24h` as a safe fallback if only a hash was supplied.
    pub expires_at: String,
}

/// One entry in the cluster-wide token revocation list.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevocationEntry {
    /// Lowercase hex SHA-256 of the revoked token b64 envelope.
    pub token_hash: String,
    /// RFC3339 timestamp when this entry will be pruned.
    pub expires_at: String,
}

/// Response body for `GET /api/v1/cluster/revocations`.
///
/// Returns all currently-active (un-expired) revocations replicated
/// through Raft. Entries auto-prune at apply time; this listing is a
/// point-in-time view of the local state machine.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct RevocationListResponse {
    /// All currently-revoked tokens. Sorted by `expires_at` ascending so
    /// the soonest-to-be-pruned entries come first.
    pub revocations: Vec<RevocationEntry>,
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
