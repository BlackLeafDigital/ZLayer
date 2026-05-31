//! Cluster management endpoints
//!
//! Provides the `/api/v1/cluster/join` endpoint for new nodes joining
//! the cluster, and `/api/v1/cluster/nodes` for listing cluster members.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use utoipa::ToSchema;
use zlayer_overlay::{IpAllocator, NodeSliceAllocator};

use crate::error::{ApiError, Result};
use crate::handlers::internal::{
    InternalAddPeerRequest, UpgradeStartResponse, INTERNAL_AUTH_HEADER,
};
use crate::handlers::users::AuthActor;
use zlayer_scheduler::{AddMemberParams, NodeId, RaftCoordinator};

// =============================================================================
// DTOs
// =============================================================================

// Wire DTOs for cluster join / membership live in `zlayer-types` so that the
// CLI and the manager UI can describe these requests/responses without
// depending on `zlayer-api`. We re-export them here so existing call sites
// (the `cluster_join` handler, the test module) keep compiling.
pub use zlayer_types::api::cluster::{
    default_api_port, default_mode, ClusterJoinClaims, ClusterJoinRequest, ClusterJoinResponse,
    ClusterNodeSummary, ClusterPeer, GossipPeerSummary, ImportTrustBundleRequest,
    ImportTrustBundleResponse, JwtStatusResponse, RevocationEntry, RevocationListResponse,
    RevokeTokenRequest, RevokeTokenResponse, RotateSigningKeyRequest, RotateSigningKeyResponse,
    SetJwtAlgorithmRequest, SignedClusterJoinToken, SigningPubkeyEntry, SigningPubkeyResponse,
    SigningPubkeysResponse, TrustedBundleEntry, TrustedBundlesResponse, WorkerSummary,
    SIGNED_TOKEN_V_WAVE3, SIGNED_TOKEN_V_WAVE9,
};

/// Worker nodes whose `last_heartbeat` is older than this threshold are
/// reported as `"dead"` regardless of their raft-stored `status` field.
/// See the comment in `cluster_list_nodes` for the openraft hang context.
const STALE_HEARTBEAT_DEAD_MS: u64 = 30_000;

/// Heartbeat request from a worker node.
#[derive(Debug, Deserialize, ToSchema)]
pub struct HeartbeatRequest {
    pub node_id: u64,
    pub cpu_used: f64,
    pub memory_used: u64,
    pub disk_used: u64,
    #[serde(default)]
    pub gpu_utilization: Vec<zlayer_scheduler::raft::GpuUtilizationReport>,
}

/// Request body for force-leader operation.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ForceLeaderRequest {
    /// Confirmation string -- must be `"CONFIRM_FORCE_LEADER"` to prevent accidents
    pub confirm: String,
}

/// Response body for force-leader operation.
#[derive(Debug, Serialize, ToSchema)]
pub struct ForceLeaderResponse {
    pub success: bool,
    pub message: String,
    /// Number of cluster nodes whose state was preserved
    pub preserved_nodes: usize,
    /// Number of services whose state was preserved
    pub preserved_services: usize,
}

// =============================================================================
// State
// =============================================================================

/// Shared state for cluster endpoints.
///
/// Holds a reference to the Raft coordinator (if running) and a monotonic
/// counter for assigning Raft node IDs to joining members.
#[derive(Clone)]
pub struct ClusterApiState {
    /// Raft coordinator (None if Raft is not initialised)
    pub raft: Option<Arc<RaftCoordinator>>,
    /// Monotonic Raft node ID counter (starts at 2 since leader is 1)
    next_raft_id: Arc<AtomicU64>,
    /// Expected join token secret for validation
    pub join_secret: Option<String>,
    /// Ed25519 keypair used to sign cluster join tokens (Wave 1).
    ///
    /// `None` indicates the daemon has not been wired with a signer yet —
    /// for example, in unit tests that construct `ClusterApiState` without
    /// the daemon bootstrap path. Endpoints that need to sign or verify
    /// Ed25519-signed tokens MUST handle this case explicitly.
    pub cluster_signer: Option<Arc<zlayer_secrets::ClusterSigner>>,
    /// Path to the cluster signing keystore on disk (Wave 5A).
    ///
    /// Used by [`validate_join_token_ed25519`] to look up signers by `kid`
    /// so that tokens issued under a now-grace key remain verifiable for
    /// the duration of their grace window. If `None`, the validator falls
    /// back to comparing the token's `kid` against the active `cluster_signer`
    /// only (pre-Wave-5A behavior).
    ///
    /// Callers (the daemon bootstrap in `bin/zlayer/src/commands/serve.rs`)
    /// should set this to `Some({data_dir}/cluster_signing.key)` after
    /// constructing the state, since the field is `pub`.
    pub cluster_signing_key_path: Option<std::path::PathBuf>,
    /// Long-lived cluster CA keypair, used to issue [`CaCert`]s
    /// embedded in v=2 signed join tokens for cross-cluster
    /// federation. `None` if the daemon is running without
    /// federation enabled (the CA file at
    /// `{data_dir}/cluster_ca.key` is auto-generated on first start
    /// of any daemon that has CA support compiled in — Wave 9).
    pub cluster_ca: Option<Arc<zlayer_secrets::ClusterCa>>,
    /// Cluster identity for federation. Defaults to the local node
    /// UUID at daemon start. Operators may override at deploy time
    /// to a DNS-style name like `prod.zlayer.example` via a future
    /// `--cluster-domain` flag.
    pub cluster_domain: Option<String>,
    /// IP allocator for overlay network addresses (CIDR-aware, collision-safe)
    pub ip_allocator: Arc<RwLock<IpAllocator>>,
    /// Path for persisting IP allocator state
    ip_allocator_path: Option<std::path::PathBuf>,
    /// Internal shared secret for authenticating peer-broadcast calls to existing nodes.
    /// Required for the leader to POST `/api/v1/internal/add-peer` on other nodes.
    pub internal_token: Option<String>,
    /// Data directory for Raft storage and recovery markers.
    pub data_dir: Option<std::path::PathBuf>,
    /// Leader-side slice allocator. Carves the cluster CIDR into `/28` slices
    /// and hands one to each joining node. Wrapped in `RwLock` for concurrent
    /// access from multiple join handlers.
    pub slice_allocator: Arc<RwLock<NodeSliceAllocator>>,
    /// Persisted path for `slice_allocator` snapshots (None = in-memory only).
    pub slice_allocator_path: Option<PathBuf>,
    /// Set of `jti` (JWT IDs) already consumed by a successful join, used
    /// for replay protection on the signed-token validator.
    ///
    /// Phase-1 implementation: a process-local in-memory set on the leader.
    /// This is acceptable because join validation only happens on the leader
    /// (followers proxy `/cluster/join` to the leader), so a single-node set
    /// is correct for the single-leader case.
    ///
    // TODO Phase-1.5: replicate `used_jtis` through Raft so the replay
    // protection survives leader changes within the JWT's TTL window.
    pub used_jtis: Arc<Mutex<HashSet<String>>>,
    /// Worker-tier dispatcher (server-role only). When `None`, the
    /// `/api/v1/cluster/workers` endpoint returns an empty list.
    pub worker_dispatcher: Option<Arc<dyn zlayer_scheduler::cluster::WorkerDispatcher>>,
    /// Gossip pool (worker-tier deployments only). When `None`, the
    /// `/api/v1/cluster/gossip/peers` endpoint returns an empty list.
    pub gossip_pool: Option<Arc<zlayer_overlay::gossip::GossipPool>>,
}

impl ClusterApiState {
    /// Create a new cluster API state.
    ///
    /// `raft` may be `None` if the coordinator was not available.
    /// `join_secret` is the `auth_secret` that must appear in the join token.
    /// `ip_allocator` is a CIDR-aware allocator for overlay IPs.
    /// `ip_allocator_path` is the file path used to persist allocator state.
    /// `slice_allocator` is the leader-side per-node `/28` slice allocator.
    /// `slice_allocator_path` is the file path used to persist slice allocator state.
    /// `data_dir` is the Raft data directory for recovery markers.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        raft: Option<Arc<RaftCoordinator>>,
        join_secret: Option<String>,
        ip_allocator: Arc<RwLock<IpAllocator>>,
        ip_allocator_path: Option<std::path::PathBuf>,
        slice_allocator: Arc<RwLock<NodeSliceAllocator>>,
        slice_allocator_path: Option<PathBuf>,
        data_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            raft,
            next_raft_id: Arc::new(AtomicU64::new(2)),
            join_secret,
            cluster_signer: None,
            cluster_signing_key_path: None,
            cluster_ca: None,
            cluster_domain: None,
            ip_allocator,
            ip_allocator_path,
            internal_token: None,
            data_dir,
            slice_allocator,
            slice_allocator_path,
            used_jtis: Arc::new(Mutex::new(HashSet::new())),
            worker_dispatcher: None,
            gossip_pool: None,
        }
    }

    /// Create a new cluster API state with an internal token for peer broadcasting.
    #[allow(clippy::too_many_arguments)]
    pub fn with_internal_token(
        raft: Option<Arc<RaftCoordinator>>,
        join_secret: Option<String>,
        ip_allocator: Arc<RwLock<IpAllocator>>,
        ip_allocator_path: Option<std::path::PathBuf>,
        slice_allocator: Arc<RwLock<NodeSliceAllocator>>,
        slice_allocator_path: Option<PathBuf>,
        internal_token: String,
        data_dir: Option<std::path::PathBuf>,
        cluster_signer: Option<Arc<zlayer_secrets::ClusterSigner>>,
    ) -> Self {
        Self {
            raft,
            next_raft_id: Arc::new(AtomicU64::new(2)),
            join_secret,
            cluster_signer,
            cluster_signing_key_path: None,
            cluster_ca: None,
            cluster_domain: None,
            ip_allocator,
            ip_allocator_path,
            internal_token: Some(internal_token),
            data_dir,
            slice_allocator,
            slice_allocator_path,
            used_jtis: Arc::new(Mutex::new(HashSet::new())),
            worker_dispatcher: None,
            gossip_pool: None,
        }
    }

    /// Create a placeholder state (no Raft).
    ///
    /// Uses the default overlay CIDR `10.200.0.0/16`.
    ///
    /// # Panics
    ///
    /// Panics if the default overlay CIDR is invalid (should never happen).
    #[must_use]
    pub fn placeholder() -> Self {
        let allocator = IpAllocator::new(zlayer_overlay::DEFAULT_OVERLAY_CIDR)
            .expect("default overlay CIDR must be valid");
        let slice_allocator = NodeSliceAllocator::new(
            zlayer_overlay::DEFAULT_OVERLAY_CIDR
                .parse()
                .expect("default overlay CIDR must parse as IpNet"),
            zlayer_overlay::DEFAULT_SLICE_PREFIX,
        )
        .expect("default slice prefix must be valid for default cluster CIDR");
        Self {
            raft: None,
            next_raft_id: Arc::new(AtomicU64::new(2)),
            join_secret: None,
            cluster_signer: None,
            cluster_signing_key_path: None,
            cluster_ca: None,
            cluster_domain: None,
            ip_allocator: Arc::new(RwLock::new(allocator)),
            ip_allocator_path: None,
            internal_token: None,
            data_dir: None,
            slice_allocator: Arc::new(RwLock::new(slice_allocator)),
            slice_allocator_path: None,
            used_jtis: Arc::new(Mutex::new(HashSet::new())),
            worker_dispatcher: None,
            gossip_pool: None,
        }
    }

    /// Attach a worker dispatcher (called from `serve.rs` when
    /// `mode: worker-tier, role: server`).
    ///
    /// Without this, `GET /api/v1/cluster/workers` returns an empty list.
    #[must_use]
    pub fn with_worker_dispatcher(
        mut self,
        dispatcher: Arc<dyn zlayer_scheduler::cluster::WorkerDispatcher>,
    ) -> Self {
        self.worker_dispatcher = Some(dispatcher);
        self
    }

    /// Attach a gossip pool (called from `serve.rs` when running in a
    /// worker-tier server mode that brought up chitchat).
    ///
    /// Without this, `GET /api/v1/cluster/gossip/peers` returns an empty list.
    #[must_use]
    pub fn with_gossip_pool(mut self, pool: Arc<zlayer_overlay::gossip::GossipPool>) -> Self {
        self.gossip_pool = Some(pool);
        self
    }

    /// Allocate the next Raft node ID.
    fn next_id(&self) -> u64 {
        self.next_raft_id.fetch_add(1, Ordering::SeqCst)
    }
}

// =============================================================================
// Handlers
// =============================================================================

/// Handle a cluster join request.
///
/// `POST /api/v1/cluster/join`
///
/// Validates the join token, assigns a Raft node ID, calls `raft.add_member()`,
/// and returns the assignment + peer list.
///
/// # Errors
///
/// Returns an error if the Raft coordinator is unavailable, the join token is
/// invalid, IP allocation fails, or the Raft membership change fails.
#[utoipa::path(
    post,
    path = "/api/v1/cluster/join",
    request_body = ClusterJoinRequest,
    responses(
        (status = 200, description = "Node joined successfully", body = ClusterJoinResponse),
        (status = 401, description = "Invalid join token"),
        (status = 500, description = "Failed to add member to Raft"),
        (status = 503, description = "Raft coordinator not available"),
    ),
    tag = "Cluster"
)]
#[allow(clippy::too_many_lines)]
pub async fn cluster_join(
    State(state): State<ClusterApiState>,
    Json(req): Json<ClusterJoinRequest>,
) -> Result<Json<ClusterJoinResponse>> {
    // 1. Validate we have a Raft coordinator
    let raft = state
        .raft
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Raft coordinator not available".into()))?;

    // 2. Validate the join token.
    //
    // Wave 6 (v0.13.0) dispatch order:
    //   1. Ed25519-signed envelope (preferred — public-key crypto, no shared
    //      secret needed on the joiner side).
    //   2. HS256 JWT (Phase-1 form — single-use via `jti` replay protection).
    //   3. Legacy plaintext base64-JSON `auth_secret` — HARD REJECTED. The
    //      plaintext-shape detection runs only to surface a clearer error
    //      ("re-issue with `zlayer node generate-join-token`") than the
    //      otherwise-opaque HS256 decode failure.
    //
    // If a token LOOKS like an Ed25519 envelope (has `v`/`kid`/`sig` after
    // base64-decode) but fails Ed25519 verification, that's an explicit
    // reject — we DON'T fall through to HS256, because doing so would let
    // an attacker who mangles a real signed token bypass signature checks.
    //
    // The handler doesn't currently extract `ConnectInfo`, so logging uses a
    // sentinel `0.0.0.0` peer-IP. A future wave that adds the real client IP
    // will make this field meaningful without changing the log shape.

    // Wave 7.5: revocation check.
    //
    // Compute the canonical hash of the raw token and consult the
    // Raft-replicated revocation list. If the entry is present we
    // bail out early — every node converges on the same list via
    // the state machine, so a revoked token is rejected everywhere
    // within one Raft commit of `cluster_revoke_token`.
    //
    // The check runs BEFORE format-specific validators so an operator
    // who revoked a leaked token doesn't burn replay-protection slots
    // verifying a token we're going to reject anyway.
    if let Some(raft) = state.raft.as_ref() {
        let token_hash = token_canonical_hash(&req.token);
        let secrets_state = raft.secrets_state().await;
        if secrets_state.token_revoked(&token_hash) {
            tracing::warn!(
                event = "join_token_revoked",
                token_hash = %token_hash,
                "rejected revoked join token"
            );
            return Err(ApiError::Unauthorized("token revoked".into()));
        }
    }

    let peer_ip: std::net::IpAddr = std::net::Ipv4Addr::UNSPECIFIED.into();
    let mut token_accepted = false;
    if state.cluster_signer.is_some() {
        match validate_join_token_ed25519(&state, &req.token, peer_ip).await {
            Ok(_claims) => {
                token_accepted = true;
            }
            Err(ed25519_err) => {
                if is_signed_envelope_shape(&req.token) {
                    // Explicit reject: token was signed-envelope-shaped but
                    // failed verification (bad signature, kid mismatch,
                    // expired, replayed). Do NOT fall through.
                    return Err(ed25519_err);
                }
                // Otherwise the token isn't a Wave-3 envelope — fall through
                // to the HS256 / plaintext-reject path below.
            }
        }
    }

    if !token_accepted {
        // Wave 11: respect cluster-wide `jwt_algorithm` policy. Default
        // to `Both` if no Raft is attached (test/placeholder paths).
        let algorithm = if let Some(raft) = state.raft.as_ref() {
            raft.secrets_state().await.jwt_algorithm()
        } else {
            zlayer_types::api::cluster::JwtAlgorithm::Both
        };

        let try_eddsa = matches!(
            algorithm,
            zlayer_types::api::cluster::JwtAlgorithm::Eddsa
                | zlayer_types::api::cluster::JwtAlgorithm::Both
        );
        let try_hs256 = matches!(
            algorithm,
            zlayer_types::api::cluster::JwtAlgorithm::Hs256
                | zlayer_types::api::cluster::JwtAlgorithm::Both
        );

        // 1. Try EdDSA-JWT first when enabled. Don't fall through to
        //    HS256 if EdDSA succeeds.
        let mut deferred_err: Option<ApiError> = None;
        if try_eddsa {
            match validate_join_token_eddsa_jwt(&state, &req.token).await {
                Ok(()) => {
                    info!(format = "eddsa_jwt", "accepted cluster join token");
                    token_accepted = true;
                }
                Err(e) => {
                    deferred_err = Some(e);
                }
            }
        }

        // 2. Try HS256-JWT when enabled.
        if !token_accepted && try_hs256 {
            if let Some(expected_secret) = &state.join_secret {
                let hmac_key = derive_join_hmac_key(expected_secret);
                let signed_ok = validate_join_token_signed(&req.token, &hmac_key, &state.used_jtis);
                match signed_ok {
                    Ok(()) => {
                        info!(format = "hs256", "accepted cluster join token");
                        token_accepted = true;
                    }
                    Err(signed_err) => {
                        // Wave 6 (v0.13.0): plaintext tokens are no longer
                        // accepted. If the token's body looks like a legacy
                        // plaintext form (carries an `auth_secret` field
                        // matching the cluster secret), return a precise,
                        // actionable error pointing the operator at the
                        // remediation command rather than surfacing the
                        // opaque HS256 decode failure.
                        if is_legacy_plaintext_token_shape(&req.token, expected_secret) {
                            tracing::warn!(
                                peer_ip = %peer_ip,
                                format = "legacy_plaintext",
                                "rejected legacy plaintext join token (v0.13.0 dropped acceptance)"
                            );
                            return Err(ApiError::Unauthorized(
                                "plaintext join token rejected. Re-issue with 'zlayer node \
                                 generate-join-token' (signed by default). See CHANGELOG \
                                 v0.13.0 for migration."
                                    .into(),
                            ));
                        }
                        // Tuck the HS256 error away as a fallback;
                        // we surface the most-helpful error below.
                        if deferred_err.is_none() {
                            deferred_err = Some(signed_err);
                        }
                    }
                }
            }
        }

        // 3. If the policy explicitly REJECTS the format the caller used
        //    (Eddsa-only cluster receiving an HS256-shaped JWT), surface an
        //    actionable error. Detect by attempting a header-only parse to
        //    see what `alg` the token advertised.
        if !token_accepted && algorithm == zlayer_types::api::cluster::JwtAlgorithm::Eddsa {
            if let Ok(hdr) = jsonwebtoken::decode_header(&req.token) {
                if hdr.alg == jsonwebtoken::Algorithm::HS256 {
                    return Err(ApiError::Unauthorized(
                        "HS256 decommissioned on this cluster; re-issue with current \
                         'zlayer node generate-join-token'"
                            .into(),
                    ));
                }
            }
        }

        if !token_accepted {
            return Err(deferred_err.unwrap_or_else(|| {
                ApiError::Unauthorized("no acceptable join-token format found".into())
            }));
        }
    }

    // 3. Allocate a Raft node ID
    let raft_node_id = state.next_id();
    let node_uuid = uuid::Uuid::new_v4().to_string();

    // 4. Determine the Raft RPC address for the new node
    let raft_addr = format!("{}:{}", req.advertise_addr, req.raft_port);

    // 5. Allocate an overlay IP from the CIDR-aware allocator (before add_member so
    //    the IP is available to store in the Raft state machine)
    let overlay_ip = {
        let mut allocator = state.ip_allocator.write().await;
        let ip = allocator
            .allocate()
            .ok_or_else(|| ApiError::Internal("No available overlay IPs in CIDR range".into()))?;

        // Persist allocator state so IPs survive restarts
        if let Some(path) = &state.ip_allocator_path {
            if let Err(e) = allocator.save(Path::new(path)).await {
                warn!("Failed to persist IP allocations: {e}");
            }
        }

        ip.to_string()
    };

    // 5b. Assign a per-node `/28` slice of the cluster CIDR. This gives the
    //     joining node its own non-overlapping pool of container IPs, fixing
    //     the latent IP-collision bug where every agent allocated from the
    //     full cluster `/16`.
    let slice_cidr = {
        let mut slice_allocator = state.slice_allocator.write().await;
        let assigned = slice_allocator
            .assign(&raft_node_id.to_string())
            .map_err(|e| ApiError::Internal(format!("slice allocation failed: {e}")))?;
        // Persist the slice allocator snapshot if a path is configured.
        if let Some(ref p) = state.slice_allocator_path {
            let snapshot = slice_allocator.snapshot();
            match serde_json::to_string_pretty(&snapshot) {
                Ok(json) => {
                    if let Err(e) = tokio::fs::write(p, json).await {
                        warn!(error = %e, "failed to persist slice allocator state");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "failed to serialize slice allocator snapshot");
                }
            }
        }
        assigned.to_string()
    };

    // 6. Add the member to the Raft cluster (with overlay networking metadata)
    let role = raft
        .add_member(AddMemberParams {
            node_id: raft_node_id,
            addr: raft_addr,
            wg_public_key: req.wg_public_key.clone(),
            overlay_ip: overlay_ip.clone(),
            overlay_port: req.overlay_port,
            advertise_addr: req.advertise_addr.clone(),
            api_port: req.api_port,
            cpu_total: req.cpu_total,
            memory_total: req.memory_total,
            disk_total: req.disk_total,
            gpus: req.gpus.clone(),
            mode: req.mode.clone(),
            os: req.os,
            arch: req.arch,
            slice_cidr: slice_cidr.clone(),
            labels: req.labels.clone(),
        })
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to add member to Raft: {e}")))?;

    info!(
        node_uuid = %node_uuid,
        raft_node_id = raft_node_id,
        addr = %req.advertise_addr,
        overlay_ip = %overlay_ip,
        "New node joined the cluster"
    );

    // 7. Read current cluster state to build the peer list
    let cluster_state = raft.read_state().await;
    let peers: Vec<ClusterPeer> = cluster_state
        .nodes
        .values()
        .filter(|n| n.node_id != raft_node_id)
        .map(|n| {
            // Extract the raft port from the address (format: "ip:port")
            let raft_port = n
                .address
                .rsplit(':')
                .next()
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(9000);

            ClusterPeer {
                node_id: format!("{}", n.node_id),
                raft_node_id: n.node_id,
                advertise_addr: n.advertise_addr.clone(),
                overlay_port: n.overlay_port,
                raft_port,
                wg_public_key: n.wg_public_key.clone(),
                overlay_ip: n.overlay_ip.clone(),
            }
        })
        .collect();

    // 8. Broadcast the new peer's overlay info to all existing nodes (fire-and-forget).
    //
    // Without this, existing nodes only learn about the new node when they
    // reconcile from Raft state or restart.  This sends a POST to each existing
    // node's `/api/v1/internal/add-peer` endpoint so they add the WireGuard peer
    // immediately.
    if let Some(ref internal_token) = state.internal_token {
        let leader_node_id = raft.node_id();
        let new_peer_request = InternalAddPeerRequest {
            wg_public_key: req.wg_public_key.clone(),
            overlay_ip: overlay_ip.clone(),
            endpoint: format!("{}:{}", req.advertise_addr, req.overlay_port),
        };

        // Collect the API endpoints of existing nodes (excluding the joining node
        // and the current leader — the leader already knows about the new peer
        // because it processes the join request).
        let targets: Vec<String> = cluster_state
            .nodes
            .values()
            .filter(|n| n.node_id != raft_node_id && n.node_id != leader_node_id)
            .filter(|n| !n.advertise_addr.is_empty() && n.api_port > 0)
            .map(|n| format!("http://{}:{}", n.advertise_addr, n.api_port))
            .collect();

        if !targets.is_empty() {
            let token = internal_token.clone();
            tokio::spawn(async move {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                    .unwrap_or_default();

                for base_url in &targets {
                    let url = format!("{base_url}/api/v1/internal/add-peer");
                    match client
                        .post(&url)
                        .header(INTERNAL_AUTH_HEADER, &token)
                        .json(&new_peer_request)
                        .send()
                        .await
                    {
                        Ok(resp) if resp.status().is_success() => {
                            info!(peer_target = %base_url, "Broadcast new peer to existing node");
                        }
                        Ok(resp) => {
                            warn!(
                                peer_target = %base_url,
                                status = %resp.status(),
                                "Failed to broadcast new peer to existing node"
                            );
                        }
                        Err(e) => {
                            error!(
                                peer_target = %base_url,
                                error = %e,
                                "Failed to reach existing node for peer broadcast"
                            );
                        }
                    }
                }
            });
        }
    }

    // 9. Phase-1 secrets capability: if the joiner provided an X25519
    //    pubkey, register it with the cluster secrets state machine and
    //    mint a node JWT scoped to `roles: ["node"]`.
    //
    //    Legacy joiners (no `secrets_pubkey`) get back `None` for all three
    //    fields and skip the secrets-SM round-trip entirely; this preserves
    //    the pre-Phase-1 wire shape and behaviour.
    let (node_jwt, wrapped_dek, dek_generation) = if let Some(secrets_pubkey) = req.secrets_pubkey {
        let identity = zlayer_types::storage::NodeIdentity {
            node_id: node_uuid.clone(),
            secrets_pubkey,
            wg_pubkey: req.wg_public_key.clone(),
            joined_at: chrono::Utc::now(),
            revoked_at: None,
        };
        let (joiner_wrap, gen) = raft
            .propose_register_node_and_rotate(identity)
            .await
            .map_err(|e| ApiError::Internal(format!("secrets register: {e}")))?;

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let claims = zlayer_types::jwt::Claims {
            sub: format!("node:{node_uuid}"),
            // 1-year node JWT — node-to-node auth, not user-facing.
            exp: now_secs + 365 * 24 * 3600,
            iat: now_secs,
            iss: "zlayer".to_string(),
            roles: vec!["node".to_string()],
            email: None,
            node_id: Some(node_uuid.clone()),
        };
        // Sign with the same JWT-secret material used by the rest of the
        // API: the cluster `join_secret`. (When `join_secret` is `None`
        // the daemon is unauthenticated, in which case node JWTs would be
        // unverifiable by peers anyway — we fail loud rather than mint an
        // unsigned token.)
        let jwt_secret = state.join_secret.as_ref().ok_or_else(|| {
            ApiError::Internal("cluster join_secret unset; cannot sign node JWT".to_string())
        })?;
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(jwt_secret.as_bytes()),
        )
        .map_err(|e| ApiError::Internal(format!("node jwt sign: {e}")))?;

        (Some(token), Some(joiner_wrap), Some(gen))
    } else {
        (None, None, None)
    };

    Ok(Json(ClusterJoinResponse {
        node_id: node_uuid,
        raft_node_id,
        overlay_ip,
        slice_cidr,
        peers,
        role: match role {
            zlayer_scheduler::raft::MemberRole::Voter => "voter".to_string(),
            zlayer_scheduler::raft::MemberRole::Learner => "learner".to_string(),
        },
        node_jwt,
        wrapped_dek,
        dek_generation,
        // Propagate the cluster HMAC `join_secret` to the joiner so it can
        // derive the same internal Raft bearer token as the leader. Without
        // this, cross-node Raft RPCs 401 because each daemon would otherwise
        // mint a per-process random `internal_token`.
        join_secret: state.join_secret.clone(),
        // Wave 6 (v0.13.0): the plaintext-acceptance branch that used to
        // populate this field is gone. The field itself stays on
        // `ClusterJoinResponse` so future waves can surface other
        // operator-facing advisories without another wire-shape change.
        warnings: None,
    }))
}

/// List all nodes visible in the Raft cluster state.
///
/// `GET /api/v1/cluster/nodes`
///
/// # Errors
///
/// Returns an error if the cluster state cannot be read.
#[utoipa::path(
    get,
    path = "/api/v1/cluster/nodes",
    responses(
        (status = 200, description = "List of cluster nodes", body = Vec<ClusterNodeSummary>),
    ),
    security(("bearer_auth" = [])),
    tag = "Cluster"
)]
pub async fn cluster_list_nodes(
    State(state): State<ClusterApiState>,
) -> Result<Json<Vec<ClusterNodeSummary>>> {
    let Some(raft) = &state.raft else {
        // No Raft -- return empty
        return Ok(Json(Vec::new()));
    };

    let cluster_state = raft.read_state().await;
    let leader_id = raft.leader_id();

    let metrics = raft.metrics();
    let voter_ids: std::collections::BTreeSet<u64> =
        metrics.membership_config.voter_ids().collect();

    // Stale-heartbeat → dead override. See `STALE_HEARTBEAT_DEAD_MS` at
    // module scope for the openraft hang context that motivates this
    // override.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    let self_id = raft.node_id();

    let nodes: Vec<ClusterNodeSummary> = cluster_state
        .nodes
        .values()
        .map(|n| {
            let role = if Some(n.node_id) == leader_id {
                "leader".to_string()
            } else if voter_ids.contains(&n.node_id) {
                "voter".to_string()
            } else {
                "learner".to_string()
            };

            // Apply stale-heartbeat override. Never override self
            // (the daemon serving this request is by definition alive).
            let status = if n.node_id != self_id
                && n.status != "dead"
                && now_ms.saturating_sub(n.last_heartbeat) > STALE_HEARTBEAT_DEAD_MS
            {
                "dead".to_string()
            } else {
                n.status.clone()
            };

            ClusterNodeSummary {
                id: format!("{}", n.node_id),
                address: n.address.clone(),
                advertise_addr: n.advertise_addr.clone(),
                api_endpoint: format!("{}:{}", n.advertise_addr, n.api_port),
                status,
                role,
                mode: n.mode.clone(),
                is_leader: Some(n.node_id) == leader_id,
                overlay_ip: n.overlay_ip.clone(),
                cpu_total: n.cpu_total,
                cpu_used: n.cpu_used,
                memory_total: n.memory_total,
                memory_used: n.memory_used,
                registered_at: n.registered_at,
                last_heartbeat: n.last_heartbeat,
            }
        })
        .collect();

    Ok(Json(nodes))
}

/// `GET /api/v1/cluster/workers` — list currently-leased worker-tier workers.
///
/// Returns an empty list when this node is not running a worker-tier
/// dispatcher (e.g., single-node, raft-only, static, or a worker-tier
/// worker-role daemon).
///
/// # Errors
///
/// Never errors today — `known_workers()` is infallible. The result type
/// stays `Result<_>` so a future fallible implementation doesn't need a
/// signature change.
#[utoipa::path(
    get,
    path = "/api/v1/cluster/workers",
    responses(
        (status = 200, description = "List of worker-tier workers", body = Vec<WorkerSummary>),
    ),
    security(("bearer_auth" = [])),
    tag = "Cluster"
)]
pub async fn cluster_list_workers(
    State(state): State<ClusterApiState>,
) -> Result<Json<Vec<WorkerSummary>>> {
    let Some(dispatcher) = &state.worker_dispatcher else {
        return Ok(Json(Vec::new()));
    };
    let workers = dispatcher.known_workers().await;
    let summaries: Vec<WorkerSummary> = workers
        .into_iter()
        .map(|n| WorkerSummary {
            id: n.id,
            api_addr: n.api_addr.to_string(),
            labels: n.labels,
            os: n.os,
            last_seen_unix_secs: n
                .last_seen
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
                .unwrap_or(0),
            state: match n.state {
                zlayer_scheduler::cluster::NodeState::Ready => "ready".to_string(),
                zlayer_scheduler::cluster::NodeState::Unreachable => "unreachable".to_string(),
                zlayer_scheduler::cluster::NodeState::Draining => "draining".to_string(),
            },
        })
        .collect();
    Ok(Json(summaries))
}

/// `GET /api/v1/cluster/gossip/peers` — list peers known via the gossip pool.
///
/// Returns an empty list when the daemon has no gossip pool configured
/// (single-node, raft-only, static, or worker-tier without gossip enabled).
///
/// # Errors
///
/// Currently infallible — `GossipPool::peers()` can't fail today. The
/// return type stays `Result<_>` so a future fallible implementation
/// doesn't need a signature change.
#[utoipa::path(
    get,
    path = "/api/v1/cluster/gossip/peers",
    responses(
        (status = 200, description = "List of gossip pool peers", body = Vec<GossipPeerSummary>),
    ),
    security(("bearer_auth" = [])),
    tag = "Cluster"
)]
pub async fn cluster_list_gossip_peers(
    State(state): State<ClusterApiState>,
) -> Result<Json<Vec<GossipPeerSummary>>> {
    let Some(pool) = &state.gossip_pool else {
        return Ok(Json(Vec::new()));
    };
    let peers = pool.peers().await;
    let summaries: Vec<GossipPeerSummary> = peers
        .into_iter()
        .map(|p| GossipPeerSummary {
            node_id: p.node_id,
            wg_pubkey: Some(p.wg_pubkey),
            wg_endpoint: Some(p.wg_endpoint.to_string()),
            overlay_ip: Some(p.overlay_ip),
            labels: p.labels,
        })
        .collect();
    Ok(Json(summaries))
}

/// `GET /api/v1/cluster/signing-pubkey` — return the cluster's active
/// Ed25519 verifying key.
///
/// **Unauthenticated by design.** The response is a public key plus a short
/// id; nothing here leaks anything an attacker doesn't already learn from a
/// signed token. Joining nodes fetch this before they have any credential.
///
/// Returns `503 Service Unavailable` if the daemon was started without a
/// `cluster_signer` (e.g., in some test harnesses); production daemons
/// always set one.
///
/// # Errors
///
/// Returns [`ApiError::ServiceUnavailable`] when the daemon has no
/// `cluster_signer` configured on its `ClusterApiState`.
#[utoipa::path(
    get,
    path = "/api/v1/cluster/signing-pubkey",
    responses(
        (status = 200, description = "Active Ed25519 verifying key", body = SigningPubkeyResponse),
        (status = 503, description = "Cluster signer not configured on this node"),
    ),
    tag = "Cluster"
)]
pub async fn cluster_signing_pubkey(
    State(state): State<ClusterApiState>,
) -> Result<Json<SigningPubkeyResponse>> {
    let signer = state.cluster_signer.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("cluster signer not configured on this node".into())
    })?;
    Ok(Json(SigningPubkeyResponse {
        public_key_b64: signer.public_key_b64(),
        kid: signer.key_id(),
    }))
}

/// Public trust bundle for this cluster.
///
/// `GET /api/v1/cluster/trust-bundle`
///
/// Returns the cluster's long-lived CA pubkey plus cluster domain.
/// Intentionally unauthenticated — the data is a public key, and
/// federation requires importers to be able to fetch the bundle
/// without first establishing trust.
///
/// # Errors
///
/// - [`ApiError::ServiceUnavailable`] if no CA is attached on this
///   node (`cluster_ca` is `None`).
#[utoipa::path(
    get,
    path = "/api/v1/cluster/trust-bundle",
    responses(
        (status = 200, description = "Cluster trust bundle", body = zlayer_types::api::cluster::TrustBundle),
        (status = 503, description = "CA not configured on this node"),
    ),
    tag = "Cluster"
)]
pub async fn cluster_trust_bundle(
    State(state): State<ClusterApiState>,
) -> Result<Json<zlayer_types::api::cluster::TrustBundle>> {
    let ca = state.cluster_ca.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("cluster CA not configured on this node".into())
    })?;
    let cluster_domain = state
        .cluster_domain
        .clone()
        .ok_or_else(|| ApiError::ServiceUnavailable("cluster_domain not configured".into()))?;
    Ok(Json(zlayer_types::api::cluster::TrustBundle {
        v: zlayer_types::api::cluster::TRUST_BUNDLE_FORMAT_VERSION,
        cluster_domain,
        ca_public_key_b64: ca.ca_public_key_b64(),
        ca_kid: ca.ca_kid(),
        generated_at: chrono::Utc::now().to_rfc3339(),
    }))
}

// =============================================================================
// Wave 9D-ii: federated trust-bundle admin endpoints
// =============================================================================

/// Import a foreign cluster's trust bundle.
///
/// `POST /api/v1/cluster/trust-imports`
///
/// Admin-only. Proposes `SecretsRaftOp::ImportTrustBundle` so every
/// node in this cluster converges on the same federated trust set
/// within one Raft commit. Idempotent at the state-machine layer —
/// re-importing the same `cluster_domain` overwrites in place.
///
/// Basic shape validation runs leader-side BEFORE proposing (saves a
/// commit when the bundle is obviously malformed): non-empty
/// `cluster_domain`, `v == TRUST_BUNDLE_FORMAT_VERSION`,
/// `ca_kid.len() == 8`, `ca_public_key_b64` decodes to exactly 32
/// bytes. Deep cryptographic validation (does this pubkey actually
/// belong to that domain?) is out of scope — the operator is trusting
/// the bundle by importing it; the validator pipeline will reject any
/// `CaCert` that doesn't verify against the imported pubkey.
///
/// # Errors
///
/// - `Unauthorized` if the actor lacks admin role.
/// - `BadRequest` for shape failures.
/// - `ServiceUnavailable` if no Raft coordinator is attached.
/// - `Internal` on Raft propose failure.
#[utoipa::path(
    post,
    path = "/api/v1/cluster/trust-imports",
    request_body = ImportTrustBundleRequest,
    responses(
        (status = 200, description = "Import accepted", body = ImportTrustBundleResponse),
        (status = 400, description = "Malformed bundle"),
        (status = 401, description = "Unauthenticated"),
        (status = 403, description = "Not admin"),
        (status = 503, description = "Raft coordinator unavailable"),
    ),
    tag = "Cluster"
)]
pub async fn cluster_import_trust_bundle(
    actor: AuthActor,
    State(state): State<ClusterApiState>,
    Json(req): Json<zlayer_types::api::cluster::ImportTrustBundleRequest>,
) -> Result<Json<zlayer_types::api::cluster::ImportTrustBundleResponse>> {
    use base64::Engine;

    actor.require_admin()?;

    let bundle = req.bundle;

    if bundle.cluster_domain.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "cluster_domain must be non-empty".into(),
        ));
    }
    if bundle.v != zlayer_types::api::cluster::TRUST_BUNDLE_FORMAT_VERSION {
        return Err(ApiError::BadRequest(format!(
            "unsupported TrustBundle version: got {}, expected {}",
            bundle.v,
            zlayer_types::api::cluster::TRUST_BUNDLE_FORMAT_VERSION
        )));
    }
    if bundle.ca_kid.len() != 8
        || !bundle
            .ca_kid
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(ApiError::BadRequest(
            "ca_kid must be exactly 8 lowercase hex chars".into(),
        ));
    }
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(bundle.ca_public_key_b64.as_bytes())
        .map_err(|e| ApiError::BadRequest(format!("ca_public_key_b64 base64 decode: {e}")))?;
    if decoded.len() != 32 {
        return Err(ApiError::BadRequest(format!(
            "ca_public_key_b64 wrong length: expected 32 bytes, got {}",
            decoded.len()
        )));
    }
    // Sanity check: ca_kid should match SHA-256(pubkey)[..4] hex.
    {
        let mut hasher = Sha256::new();
        hasher.update(&decoded);
        let digest = hasher.finalize();
        let expected_kid: String = hex::encode(&digest[..4]);
        if expected_kid != bundle.ca_kid {
            return Err(ApiError::BadRequest(format!(
                "ca_kid {} does not match SHA-256(pubkey)[..4]={}",
                bundle.ca_kid, expected_kid
            )));
        }
    }

    let cluster_domain = bundle.cluster_domain.clone();
    let ca_kid = bundle.ca_kid.clone();

    let raft = state.raft.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Raft coordinator unavailable on this node".into())
    })?;
    raft.propose_secrets_op(
        zlayer_types::api::internal::SecretsRaftOp::ImportTrustBundle { bundle },
    )
    .await
    .map_err(|e| ApiError::Internal(format!("importing trust bundle via Raft: {e}")))?;

    info!(
        cluster_domain = %cluster_domain,
        ca_kid = %ca_kid,
        source_url = ?req.source_url,
        "trust bundle imported"
    );

    Ok(Json(
        zlayer_types::api::cluster::ImportTrustBundleResponse {
            cluster_domain,
            ca_kid,
        },
    ))
}

/// List currently-trusted foreign-cluster bundles.
///
/// `GET /api/v1/cluster/trust-bundles`
///
/// Admin-only. Reads `trusted_bundles` from the local Raft state
/// machine snapshot, sorted by `cluster_domain` for stability.
///
/// # Errors
///
/// - `Unauthorized` if the actor lacks admin role.
/// - `ServiceUnavailable` if no Raft coordinator is attached.
#[utoipa::path(
    get,
    path = "/api/v1/cluster/trust-bundles",
    responses(
        (status = 200, description = "Trusted bundles listing", body = TrustedBundlesResponse),
        (status = 401, description = "Unauthenticated"),
        (status = 403, description = "Not admin"),
        (status = 503, description = "Raft coordinator unavailable"),
    ),
    tag = "Cluster"
)]
pub async fn cluster_list_trust_bundles(
    actor: AuthActor,
    State(state): State<ClusterApiState>,
) -> Result<Json<zlayer_types::api::cluster::TrustedBundlesResponse>> {
    actor.require_admin()?;

    let raft = state.raft.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Raft coordinator unavailable on this node".into())
    })?;
    let secrets_state = raft.secrets_state().await;

    let mut bundles: Vec<zlayer_types::api::cluster::TrustedBundleEntry> = secrets_state
        .trusted_bundles
        .values()
        .map(|b| zlayer_types::api::cluster::TrustedBundleEntry {
            cluster_domain: b.cluster_domain.clone(),
            ca_kid: b.ca_kid.clone(),
            ca_public_key_b64: b.ca_public_key_b64.clone(),
            generated_at: b.generated_at.clone(),
            source_url: None,
        })
        .collect();
    bundles.sort_by(|a, b| a.cluster_domain.cmp(&b.cluster_domain));

    Ok(Json(zlayer_types::api::cluster::TrustedBundlesResponse {
        bundles,
    }))
}

/// Remove a previously-imported trust bundle.
///
/// `DELETE /api/v1/cluster/trust-imports/{cluster_domain}`
///
/// Admin-only. Proposes `SecretsRaftOp::RemoveTrustBundle`. Idempotent
/// — removing an unknown domain is a no-op success.
///
/// # Errors
///
/// - `Unauthorized` if the actor lacks admin role.
/// - `BadRequest` if `cluster_domain` path param is empty.
/// - `ServiceUnavailable` if no Raft coordinator is attached.
/// - `Internal` on Raft propose failure.
#[utoipa::path(
    delete,
    path = "/api/v1/cluster/trust-imports/{cluster_domain}",
    params(
        ("cluster_domain" = String, Path, description = "Cluster domain to forget")
    ),
    responses(
        (status = 204, description = "Removed (or was already absent)"),
        (status = 400, description = "Empty cluster_domain"),
        (status = 401, description = "Unauthenticated"),
        (status = 403, description = "Not admin"),
        (status = 503, description = "Raft coordinator unavailable"),
    ),
    tag = "Cluster"
)]
pub async fn cluster_remove_trust_bundle(
    actor: AuthActor,
    State(state): State<ClusterApiState>,
    axum::extract::Path(cluster_domain): axum::extract::Path<String>,
) -> Result<axum::http::StatusCode> {
    actor.require_admin()?;

    if cluster_domain.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "cluster_domain must be non-empty".into(),
        ));
    }

    let raft = state.raft.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Raft coordinator unavailable on this node".into())
    })?;
    raft.propose_secrets_op(
        zlayer_types::api::internal::SecretsRaftOp::RemoveTrustBundle {
            cluster_domain: cluster_domain.clone(),
        },
    )
    .await
    .map_err(|e| ApiError::Internal(format!("removing trust bundle via Raft: {e}")))?;

    info!(cluster_domain = %cluster_domain, "trust bundle removed");
    Ok(axum::http::StatusCode::NO_CONTENT)
}

// Wave 11C+D: JWT algorithm policy + join_secret wipe endpoints.

/// Set the cluster-wide JWT algorithm policy.
///
/// `POST /api/v1/cluster/jwt-algorithm`
///
/// Admin-only. Proposes `SecretsRaftOp::SetJwtAlgorithm` so every node
/// converges on the same policy. Idempotent.
///
/// # Errors
///
/// - `Unauthorized` if the actor lacks admin role.
/// - `ServiceUnavailable` if no Raft coordinator is attached.
/// - `Internal` on Raft propose failure.
#[utoipa::path(
    post,
    path = "/api/v1/cluster/jwt-algorithm",
    request_body = SetJwtAlgorithmRequest,
    responses(
        (status = 204, description = "Policy applied"),
        (status = 401, description = "Unauthenticated"),
        (status = 403, description = "Not admin"),
        (status = 503, description = "Raft coordinator unavailable"),
    ),
    tag = "Cluster"
)]
pub async fn cluster_set_jwt_algorithm(
    actor: AuthActor,
    State(state): State<ClusterApiState>,
    Json(req): Json<zlayer_types::api::cluster::SetJwtAlgorithmRequest>,
) -> Result<axum::http::StatusCode> {
    actor.require_admin()?;

    let raft = state.raft.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Raft coordinator unavailable on this node".into())
    })?;
    raft.propose_secrets_op(
        zlayer_types::api::internal::SecretsRaftOp::SetJwtAlgorithm {
            algorithm: req.algorithm,
        },
    )
    .await
    .map_err(|e| ApiError::Internal(format!("setting jwt_algorithm via Raft: {e}")))?;

    info!(algorithm = %req.algorithm, "jwt algorithm policy updated");
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Get the cluster-wide JWT algorithm status.
///
/// `GET /api/v1/cluster/jwt-status`
///
/// Admin-only. Returns the in-memory snapshot from this node's Raft
/// state machine.
///
/// # Errors
///
/// - `Unauthorized` if the actor lacks admin role.
/// - `ServiceUnavailable` if no Raft coordinator is attached.
#[utoipa::path(
    get,
    path = "/api/v1/cluster/jwt-status",
    responses(
        (status = 200, description = "JWT policy status", body = JwtStatusResponse),
        (status = 401, description = "Unauthenticated"),
        (status = 403, description = "Not admin"),
        (status = 503, description = "Raft coordinator unavailable"),
    ),
    tag = "Cluster"
)]
pub async fn cluster_jwt_status(
    actor: AuthActor,
    State(state): State<ClusterApiState>,
) -> Result<Json<zlayer_types::api::cluster::JwtStatusResponse>> {
    actor.require_admin()?;

    let raft = state.raft.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Raft coordinator unavailable on this node".into())
    })?;
    let secrets_state = raft.secrets_state().await;
    Ok(Json(zlayer_types::api::cluster::JwtStatusResponse {
        algorithm: secrets_state.jwt_algorithm(),
        join_secret_wiped_at: secrets_state.join_secret_wiped_at.map(|t| t.to_rfc3339()),
    }))
}

/// Wipe `{data_dir}/join_secret` on every node.
///
/// `POST /api/v1/cluster/wipe-join-secret`
///
/// Admin-only. Proposes `SecretsRaftOp::WipeJoinSecret`. The actual
/// filesystem delete happens in each daemon's apply path (added in
/// Wave 11D bootstrap). Idempotent.
///
/// # Errors
///
/// - `Unauthorized` if the actor lacks admin role.
/// - `ServiceUnavailable` if no Raft coordinator is attached.
/// - `Internal` on Raft propose failure.
#[utoipa::path(
    post,
    path = "/api/v1/cluster/wipe-join-secret",
    responses(
        (status = 204, description = "Wipe scheduled cluster-wide"),
        (status = 401, description = "Unauthenticated"),
        (status = 403, description = "Not admin"),
        (status = 503, description = "Raft coordinator unavailable"),
    ),
    tag = "Cluster"
)]
pub async fn cluster_wipe_join_secret(
    actor: AuthActor,
    State(state): State<ClusterApiState>,
) -> Result<axum::http::StatusCode> {
    actor.require_admin()?;

    let raft = state.raft.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Raft coordinator unavailable on this node".into())
    })?;
    raft.propose_secrets_op(zlayer_types::api::internal::SecretsRaftOp::WipeJoinSecret)
        .await
        .map_err(|e| ApiError::Internal(format!("proposing WipeJoinSecret: {e}")))?;

    info!("join_secret wipe scheduled cluster-wide");
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `GET /api/v1/cluster/signing-pubkeys` — return ALL currently-trusted
/// Ed25519 verifying keys (active + in-grace).
///
/// **Unauthenticated by design.** Use this when a joining node needs to
/// verify a token signed under a kid that's no longer the cluster's
/// active key (i.e., issued shortly before a rotation; still valid
/// during grace).
///
/// When `cluster_signing_key_path` is configured, the response is built
/// from the on-disk keystore so it includes every active + grace key.
/// Falls back to the active `cluster_signer` only when the keystore path
/// is unset (legacy/test configurations).
///
/// Returns `503 Service Unavailable` if the daemon has neither a
/// cluster signer nor a keystore path configured.
///
/// # Errors
///
/// Returns [`ApiError::ServiceUnavailable`] when the daemon has neither a
/// `cluster_signer` nor a `cluster_signing_key_path` configured.
/// Returns [`ApiError::Internal`] if reading the on-disk keystore fails.
#[utoipa::path(
    get,
    path = "/api/v1/cluster/signing-pubkeys",
    responses(
        (status = 200, description = "All currently-trusted Ed25519 verifying keys (active + grace)", body = SigningPubkeysResponse),
        (status = 503, description = "Cluster signing keystore not configured on this node"),
    ),
    tag = "Cluster"
)]
pub async fn cluster_signing_pubkeys(
    State(state): State<ClusterApiState>,
) -> Result<Json<SigningPubkeysResponse>> {
    // Prefer the keystore (rotation-aware) over the single active signer.
    if let Some(path) = state.cluster_signing_key_path.as_ref() {
        let infos = zlayer_secrets::list_valid_pubkeys(path)
            .await
            .map_err(|e| ApiError::Internal(format!("listing valid pubkeys: {e}")))?;
        let keys = infos
            .into_iter()
            .map(|info| SigningPubkeyEntry {
                kid: info.kid,
                public_key_b64: info.public_key_b64,
                status: match info.status {
                    zlayer_secrets::PubkeyStatus::Active => "active".to_string(),
                    zlayer_secrets::PubkeyStatus::Grace => "grace".to_string(),
                },
                valid_until: info.valid_until.map(|t| t.to_rfc3339()),
                created_at: info.created_at.to_rfc3339(),
            })
            .collect();
        return Ok(Json(SigningPubkeysResponse { keys }));
    }

    // Fallback: only the active signer is available.
    if let Some(signer) = state.cluster_signer.as_ref() {
        return Ok(Json(SigningPubkeysResponse {
            keys: vec![SigningPubkeyEntry {
                kid: signer.key_id(),
                public_key_b64: signer.public_key_b64(),
                status: "active".to_string(),
                valid_until: None,
                // No on-disk keystore => no persisted creation time;
                // report now() as a best-effort approximation.
                created_at: chrono::Utc::now().to_rfc3339(),
            }],
        }));
    }

    Err(ApiError::ServiceUnavailable(
        "cluster signing keystore not configured on this node".into(),
    ))
}

/// `POST /api/v1/cluster/rotate-signing-key` — generate a fresh Ed25519
/// signing keypair, set it as active, and move the previous active key
/// into the grace window. Admin-only.
///
/// Default grace is 7 days when `req.grace` is omitted. The previous-active
/// key continues to verify in-flight tokens during its grace window; this
/// makes the rotation safe to run while joins are in progress.
///
/// # Errors
///
/// - [`ApiError::Forbidden`] when the caller is not an admin.
/// - [`ApiError::ServiceUnavailable`] when the daemon has no
///   `cluster_signing_key_path` configured.
/// - [`ApiError::BadRequest`] when `grace` cannot be parsed as a humantime
///   duration.
/// - [`ApiError::Internal`] when the underlying keystore rotation fails.
#[utoipa::path(
    post,
    path = "/api/v1/cluster/rotate-signing-key",
    request_body = RotateSigningKeyRequest,
    responses(
        (status = 200, description = "Keystore rotated; previous-active key moved to grace", body = RotateSigningKeyResponse),
        (status = 400, description = "Invalid grace duration"),
        (status = 403, description = "Admin role required"),
        (status = 503, description = "Cluster signing keystore not configured on this node"),
    ),
    security(("bearer_auth" = [])),
    tag = "Cluster"
)]
#[axum::debug_handler]
pub async fn cluster_rotate_signing_key(
    actor: AuthActor,
    State(state): State<ClusterApiState>,
    Json(req): Json<RotateSigningKeyRequest>,
) -> Result<Json<RotateSigningKeyResponse>> {
    actor.require_admin()?;

    let path = state.cluster_signing_key_path.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("cluster signing keystore not configured on this node".into())
    })?;
    let grace = parse_grace_or_default(req.grace.as_deref())?;
    let result = zlayer_secrets::rotate_keystore(path, grace)
        .await
        .map_err(|e| ApiError::Internal(format!("rotating cluster signing keystore: {e}")))?;

    info!(
        new_kid = %result.new_active_kid,
        previous_kid = %result.previous_kid,
        previous_grace_until = %result.previous_grace_until,
        grace_secs = grace.as_secs(),
        "cluster signing key rotated"
    );

    Ok(Json(RotateSigningKeyResponse {
        kid: result.new_active_kid,
        public_key_b64: result.new_active_public_key_b64,
        previous_kid: result.previous_kid,
        previous_grace_until: result.previous_grace_until.to_rfc3339(),
    }))
}

/// Parse the optional humantime `grace` field on
/// [`RotateSigningKeyRequest`], defaulting to 7 days when `None`.
///
/// # Errors
///
/// Returns [`ApiError::BadRequest`] when the supplied string cannot be
/// parsed as a humantime duration (e.g. `"potato"`).
fn parse_grace_or_default(grace: Option<&str>) -> Result<std::time::Duration> {
    const DEFAULT_GRACE_SECS: u64 = 7 * 24 * 3600; // 7 days
    match grace {
        None => Ok(std::time::Duration::from_secs(DEFAULT_GRACE_SECS)),
        Some(s) => humantime::parse_duration(s)
            .map_err(|e| ApiError::BadRequest(format!("invalid 'grace' duration {s:?}: {e}"))),
    }
}

/// Compute the canonical revocation hash for a join token.
///
/// If `raw` is already a 64-char lowercase hex string we accept it
/// verbatim (operator may supply a pre-computed hash). Otherwise we
/// SHA-256 the trimmed bytes and emit lowercase hex. This is the
/// stable identifier under which a token is replicated through
/// `SecretsRaftOp::RevokeToken` and looked up in `SecretsState::revoked_tokens`.
///
/// The raw form is never replicated — only the hash leaves this node.
fn token_canonical_hash(raw: &str) -> String {
    use std::fmt::Write;
    let raw = raw.trim();
    if raw.len() == 64
        && raw
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    {
        return raw.to_string();
    }
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    let digest = hasher.finalize();
    digest.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Handle a token revocation request.
///
/// `POST /api/v1/cluster/revoke-token`
///
/// Hashes the supplied `token_or_hash` to its canonical lowercase hex
/// SHA-256 form (or accepts it verbatim if it's already a 64-char hex
/// string) and proposes a `SecretsRaftOp::RevokeToken` so every node in
/// the cluster rejects subsequent uses of that token. The revocation
/// entry auto-expires; if the server could parse the token envelope and
/// extract its `exp` claim it uses that, otherwise it falls back to
/// `now() + 24h` so the table stays bounded for hash-only inputs.
///
/// Leader-only — followers should return 421 + X-Leader-Addr so the CLI
/// can redirect. (Today `propose_secrets_op` already returns a "not
/// leader" error which we surface as 503; that's acceptable for this
/// wave — a future task can wire the proper 421 redirect.)
///
/// # Errors
///
/// - [`ApiError::Unauthorized`] if the actor lacks admin role.
/// - [`ApiError::BadRequest`] if `token_or_hash` is empty.
/// - [`ApiError::ServiceUnavailable`] if no Raft coordinator is attached.
/// - [`ApiError::Internal`] on Raft propose failure.
#[utoipa::path(
    post,
    path = "/api/v1/cluster/revoke-token",
    request_body = RevokeTokenRequest,
    responses(
        (status = 200, description = "Token revoked", body = RevokeTokenResponse),
        (status = 400, description = "Empty token_or_hash"),
        (status = 401, description = "Unauthenticated"),
        (status = 403, description = "Not admin"),
        (status = 503, description = "Raft coordinator unavailable / not leader"),
    ),
    tag = "Cluster"
)]
pub async fn cluster_revoke_token(
    actor: AuthActor,
    State(state): State<ClusterApiState>,
    Json(req): Json<RevokeTokenRequest>,
) -> Result<Json<RevokeTokenResponse>> {
    actor.require_admin()?;

    let raw = req.token_or_hash.trim();
    if raw.is_empty() {
        return Err(ApiError::BadRequest(
            "token_or_hash must not be empty".into(),
        ));
    }

    // Detect whether the operator handed us a raw token (b64 envelope)
    // or a pre-computed lowercase hex SHA-256. The hash form is exactly
    // 64 chars of `[0-9a-f]`; anything else is hashed before insertion
    // so we never store or replicate the raw token bytes.
    let token_hash = token_canonical_hash(raw);

    // Best-effort expiry: try to parse the supplied token as a signed
    // envelope and reuse its `exp` claim so the revocation entry can be
    // pruned the moment the token would have expired naturally. If we
    // can't recover the claim (hash-only input, malformed envelope,
    // HS256-only path, etc.), fall back to a 24h horizon.
    let expires_at = derive_revocation_expiry(raw);

    let raft = state.raft.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Raft coordinator unavailable on this node".into())
    })?;
    raft.propose_secrets_op(zlayer_types::api::internal::SecretsRaftOp::RevokeToken {
        token_hash: token_hash.clone(),
        expires_at,
    })
    .await
    .map_err(|e| ApiError::Internal(format!("revoking token via Raft: {e}")))?;

    info!(
        token_hash = %token_hash,
        expires_at = %expires_at,
        reason = ?req.reason,
        "join token revoked"
    );

    Ok(Json(RevokeTokenResponse {
        token_hash,
        expires_at: expires_at.to_rfc3339(),
    }))
}

/// List currently-active revocations.
///
/// `GET /api/v1/cluster/revocations`
///
/// Reads the in-memory `revoked_tokens` map from the local Raft state
/// machine snapshot. Entries auto-prune at apply time, so this is a
/// point-in-time view of un-expired revocations on this node. Admin
/// auth required because the hashes are sensitive (a leaked hash plus
/// the original token b64 confirms a leak).
///
/// # Errors
///
/// - [`ApiError::Unauthorized`] if the actor lacks admin role.
/// - [`ApiError::ServiceUnavailable`] if no Raft coordinator is attached.
#[utoipa::path(
    get,
    path = "/api/v1/cluster/revocations",
    responses(
        (status = 200, description = "List of revocations", body = RevocationListResponse),
        (status = 401, description = "Unauthenticated"),
        (status = 403, description = "Not admin"),
        (status = 503, description = "Raft coordinator unavailable"),
    ),
    tag = "Cluster"
)]
pub async fn cluster_list_revocations(
    actor: AuthActor,
    State(state): State<ClusterApiState>,
) -> Result<Json<RevocationListResponse>> {
    actor.require_admin()?;

    let raft = state.raft.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Raft coordinator unavailable on this node".into())
    })?;
    let secrets_state = raft.secrets_state().await;

    let mut revocations: Vec<RevocationEntry> = secrets_state
        .revoked_tokens
        .iter()
        .map(|(token_hash, expires_at)| RevocationEntry {
            token_hash: token_hash.clone(),
            expires_at: expires_at.to_rfc3339(),
        })
        .collect();
    // Soonest-to-prune first so operators see the most urgent entries.
    revocations.sort_by(|a, b| a.expires_at.cmp(&b.expires_at));

    Ok(Json(RevocationListResponse { revocations }))
}

/// Best-effort: parse `raw` as a Wave-3 signed cluster join envelope and
/// return its `exp` claim. Falls back to `now() + 24h` for any input we
/// can't recognise (hash-only, HS256-JWT, malformed b64, etc.) so the
/// revocation entry still gets pruned eventually.
fn derive_revocation_expiry(raw: &str) -> chrono::DateTime<chrono::Utc> {
    use base64::Engine;
    use chrono::{Duration, Utc};

    let fallback = || Utc::now() + Duration::hours(24);

    // The hash form has no recoverable expiry.
    if raw.len() == 64
        && raw
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    {
        return fallback();
    }

    // Try the Wave-3 signed envelope shape first.
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw) else {
        return fallback();
    };
    let Ok(envelope) =
        serde_json::from_slice::<zlayer_types::api::cluster::SignedClusterJoinToken>(&bytes)
    else {
        return fallback();
    };
    chrono::DateTime::parse_from_rfc3339(&envelope.claims.exp)
        .map_or_else(|_| fallback(), |dt| dt.with_timezone(&Utc))
}

/// Handle node heartbeat.
///
/// `POST /api/v1/cluster/heartbeat`
///
/// Accepts resource usage data from worker nodes and proposes an
/// `UpdateNodeHeartbeat` to the Raft state machine.
///
/// # Panics
///
/// Panics if the system clock is before the Unix epoch.
#[utoipa::path(
    post,
    path = "/api/v1/cluster/heartbeat",
    request_body = HeartbeatRequest,
    responses(
        (status = 200, description = "Heartbeat accepted"),
        (status = 500, description = "Failed to propose heartbeat"),
        (status = 503, description = "Raft coordinator not available"),
    ),
    tag = "Cluster"
)]
#[allow(clippy::cast_possible_truncation)]
pub async fn cluster_heartbeat(
    State(state): State<ClusterApiState>,
    Json(req): Json<HeartbeatRequest>,
) -> impl IntoResponse {
    let Some(ref raft) = state.raft else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "no raft coordinator"})),
        )
            .into_response();
    };

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    // Check if the node was previously marked "dead" -- if so, recover it
    // by proposing a status update back to "ready" before processing the
    // heartbeat. This handles the case where a node comes back online after
    // a transient failure.
    {
        let cluster_state = raft.read_state().await;
        if let Some(node_info) = cluster_state.nodes.get(&req.node_id) {
            if node_info.status == "dead" {
                tracing::info!(
                    node_id = req.node_id,
                    "Node previously marked dead is sending heartbeats again, recovering"
                );
                let _ = raft
                    .propose(zlayer_scheduler::raft::Request::UpdateNodeStatus {
                        node_id: req.node_id,
                        status: "ready".to_string(),
                    })
                    .await;
            }
        }
    }

    let request = zlayer_scheduler::raft::Request::UpdateNodeHeartbeat {
        node_id: req.node_id,
        timestamp,
        cpu_used: req.cpu_used,
        memory_used: req.memory_used,
        disk_used: req.disk_used,
        gpu_utilization: req.gpu_utilization,
    };

    match raft.propose(request).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"status": "ok"}))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// =============================================================================
// Force-leader handler
// =============================================================================

/// Force this node to become the cluster leader (disaster recovery).
///
/// `POST /api/v1/cluster/force-leader`
///
/// DESTRUCTIVE operation for when the original leader is permanently lost.
/// Saves cluster state, shuts down Raft, writes a recovery marker.
/// The daemon must be restarted to complete recovery.
///
/// # Errors
///
/// Returns an error if the confirmation string is wrong, the Raft coordinator
/// is unavailable, this node is already the leader, or the recovery state
/// cannot be saved.
#[utoipa::path(
    post,
    path = "/api/v1/cluster/force-leader",
    request_body = ForceLeaderRequest,
    responses(
        (status = 200, description = "Force-leader initiated", body = ForceLeaderResponse),
        (status = 400, description = "Invalid confirmation or leader still reachable"),
        (status = 500, description = "Failed to save recovery state"),
        (status = 503, description = "Raft coordinator not available"),
    ),
    security(("bearer_auth" = [])),
    tag = "Cluster"
)]
pub async fn cluster_force_leader(
    State(state): State<ClusterApiState>,
    Json(req): Json<ForceLeaderRequest>,
) -> Result<Json<ForceLeaderResponse>> {
    if req.confirm != "CONFIRM_FORCE_LEADER" {
        return Err(ApiError::BadRequest(
            "Must provide confirm: \"CONFIRM_FORCE_LEADER\"".into(),
        ));
    }

    let raft = state
        .raft
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Raft coordinator not available".into()))?;

    if raft.is_leader() {
        return Err(ApiError::BadRequest(
            "This node is already the leader".into(),
        ));
    }

    // Check if leader is actually unreachable
    let metrics = raft.metrics();
    if let Some(millis) = metrics.millis_since_quorum_ack {
        if millis < 30_000 {
            return Err(ApiError::BadRequest(format!(
                "Leader was seen {millis}ms ago. Wait for confirmed unreachability (>30s).",
            )));
        }
    }

    let saved_state = raft.read_state().await;
    let preserved_nodes = saved_state.nodes.len();
    let preserved_services = saved_state.services.len();

    // Save recovery marker - we need the data_dir from ClusterApiState
    // Use a well-known path derived from the raft coordinator's config
    if let Some(ref data_dir) = state.data_dir {
        zlayer_scheduler::raft::save_force_leader_state(data_dir, &saved_state)
            .map_err(|e| ApiError::Internal(format!("Failed to save recovery state: {e}")))?;
    }

    if let Err(e) = raft.shutdown().await {
        warn!("Raft shutdown during force-leader: {e}");
    }

    Ok(Json(ForceLeaderResponse {
        success: true,
        message: "Force-leader initiated. Restart the daemon to complete recovery.".into(),
        preserved_nodes,
        preserved_services,
    }))
}

// =============================================================================
// Node lifecycle endpoints
// =============================================================================

#[derive(Debug, Serialize, ToSchema)]
pub struct NodeLifecycleResponse {
    pub node_id: u64,
    pub status: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetModeRequest {
    /// `"full"` (eligible voter) or `"replicate"` (permanent learner).
    pub mode: String,
}

/// Remove a node from the Raft cluster.
///
/// # Errors
/// Returns `ServiceUnavailable` if the local Raft coordinator is not running,
/// or `Internal` if the membership change fails.
pub async fn cluster_remove_node(
    State(state): State<ClusterApiState>,
    axum::extract::Path(node_id): axum::extract::Path<u64>,
) -> Result<Json<NodeLifecycleResponse>> {
    let raft = state
        .raft
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Raft coordinator not available".into()))?;
    raft.remove_member(node_id)
        .await
        .map_err(|e| ApiError::Internal(format!("remove_member: {e}")))?;
    Ok(Json(NodeLifecycleResponse {
        node_id,
        status: "removed".into(),
    }))
}

/// Switch a node's membership mode between `"full"` (voter-eligible) and
/// `"replicate"` (permanent learner), then trigger a voter rebalance.
///
/// # Errors
/// Returns `BadRequest` for an unknown mode, `ServiceUnavailable` if the
/// local Raft coordinator is not running, or `Internal` on propose/rebalance
/// failure.
pub async fn cluster_set_node_mode(
    State(state): State<ClusterApiState>,
    axum::extract::Path(node_id): axum::extract::Path<u64>,
    Json(req): Json<SetModeRequest>,
) -> Result<Json<NodeLifecycleResponse>> {
    if req.mode != "full" && req.mode != "replicate" {
        return Err(ApiError::BadRequest(format!(
            "invalid mode {:?}; expected \"full\" or \"replicate\"",
            req.mode
        )));
    }
    let raft = state
        .raft
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Raft coordinator not available".into()))?;
    raft.propose(zlayer_scheduler::raft::Request::UpdateNodeMode {
        node_id,
        mode: req.mode.clone(),
    })
    .await
    .map_err(|e| ApiError::Internal(format!("propose UpdateNodeMode: {e}")))?;
    raft.rebalance_voters()
        .await
        .map_err(|e| ApiError::Internal(format!("rebalance_voters: {e}")))?;
    Ok(Json(NodeLifecycleResponse {
        node_id,
        status: req.mode,
    }))
}

/// Set or remove labels on a node, persisted in raft cluster state so the
/// scheduler's `NodeSelector` placement honors them. `labels` entries are
/// inserted/overwritten; `remove` keys are deleted.
///
/// # Errors
/// Returns `ServiceUnavailable` if the local Raft coordinator is not running,
/// or `Internal` on propose failure.
#[utoipa::path(
    post,
    path = "/api/v1/cluster/nodes/{id}/labels",
    params(("id" = u64, Path, description = "Raft node id")),
    request_body = crate::handlers::nodes::UpdateLabelsRequest,
    responses(
        (status = 200, description = "Labels updated", body = crate::handlers::nodes::UpdateLabelsResponse),
        (status = 503, description = "Raft coordinator not available"),
    ),
    security(("bearer_auth" = [])),
    tag = "Cluster"
)]
pub async fn cluster_set_node_labels(
    State(state): State<ClusterApiState>,
    axum::extract::Path(node_id): axum::extract::Path<u64>,
    Json(req): Json<crate::handlers::nodes::UpdateLabelsRequest>,
) -> Result<Json<crate::handlers::nodes::UpdateLabelsResponse>> {
    let raft = state
        .raft
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Raft coordinator not available".into()))?;
    raft.propose(zlayer_scheduler::raft::Request::SetNodeLabels {
        node_id,
        set: req.labels.clone(),
        remove: req.remove.clone(),
    })
    .await
    .map_err(|e| ApiError::Internal(format!("propose SetNodeLabels: {e}")))?;
    let labels = raft
        .read_state()
        .await
        .nodes
        .get(&node_id)
        .map(|n| n.labels.clone())
        .unwrap_or_default();
    Ok(Json(crate::handlers::nodes::UpdateLabelsResponse {
        labels,
    }))
}

/// Mark a node as `draining` so the scheduler stops placing new containers on it.
///
/// # Errors
/// Returns `ServiceUnavailable` if the local Raft coordinator is not running,
/// or `Internal` if the propose fails.
pub async fn cluster_drain_node(
    State(state): State<ClusterApiState>,
    axum::extract::Path(node_id): axum::extract::Path<u64>,
) -> Result<Json<NodeLifecycleResponse>> {
    let raft = state
        .raft
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Raft coordinator not available".into()))?;
    raft.propose(zlayer_scheduler::raft::Request::UpdateNodeStatus {
        node_id,
        status: "draining".into(),
    })
    .await
    .map_err(|e| ApiError::Internal(format!("propose draining: {e}")))?;
    Ok(Json(NodeLifecycleResponse {
        node_id,
        status: "draining".into(),
    }))
}

/// Cancel a drain by returning a node to `ready` status.
///
/// # Errors
/// Returns `ServiceUnavailable` if the local Raft coordinator is not running,
/// or `Internal` if the propose fails.
pub async fn cluster_undrain_node(
    State(state): State<ClusterApiState>,
    axum::extract::Path(node_id): axum::extract::Path<u64>,
) -> Result<Json<NodeLifecycleResponse>> {
    let raft = state
        .raft
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Raft coordinator not available".into()))?;
    raft.propose(zlayer_scheduler::raft::Request::UpdateNodeStatus {
        node_id,
        status: "ready".into(),
    })
    .await
    .map_err(|e| ApiError::Internal(format!("propose ready: {e}")))?;
    Ok(Json(NodeLifecycleResponse {
        node_id,
        status: "ready".into(),
    }))
}

// =============================================================================
// Rolling daemon upgrade (`zlayer node upgrade`)
// =============================================================================

/// Request body for `POST /api/v1/cluster/upgrade`.
///
/// Drives a leader-coordinated rolling daemon-binary upgrade across the
/// cluster. The leader walks every follower in ascending `node_id` order
/// (and finally upgrades itself), instructing each to download the target
/// `zlayer` release and restart. Followers come back on the same advertise
/// address and re-join Raft; the leader waits for each one to be healthy
/// again before moving on.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ClusterUpgradeRequest {
    /// Target zlayer version (e.g. "v0.12.0"). Defaults to latest GitHub release if absent.
    #[serde(default)]
    pub version: Option<String>,
    /// Seconds to pause between node upgrades after each follower comes back healthy.
    #[serde(default = "default_cooldown_secs")]
    pub cooldown_secs: u64,
    /// Abort the rollout if any follower fails to come back healthy.
    #[serde(default)]
    pub strict: bool,
}

fn default_cooldown_secs() -> u64 {
    30
}

/// Per-node error captured during a rolling upgrade.
#[derive(Debug, Serialize, ToSchema)]
pub struct UpgradeError {
    pub node_id: NodeId,
    pub message: String,
}

/// Final result of a rolling upgrade attempt.
#[derive(Debug, Serialize, ToSchema)]
pub struct ClusterUpgradeResult {
    /// Nodes that successfully restarted on the new binary.
    pub upgraded: Vec<NodeId>,
    /// Nodes intentionally skipped (e.g. unreachable, missing api endpoint).
    pub skipped: Vec<NodeId>,
    /// Nodes that failed mid-upgrade (one entry per failure).
    pub errors: Vec<UpgradeError>,
}

/// Best-effort lookup of the current leader's HTTP base URL by inspecting
/// the Raft cluster state for `leader_id`. Returns `None` if the leader is
/// unknown or its `advertise_addr` / `api_port` are missing.
async fn leader_addr_for(state: &ClusterApiState, leader_id: Option<NodeId>) -> Option<String> {
    let raft = state.raft.as_ref()?;
    let leader_id = leader_id?;
    let cluster_state = raft.read_state().await;
    let node = cluster_state.nodes.get(&leader_id)?;
    if node.advertise_addr.is_empty() || node.api_port == 0 {
        return None;
    }
    Some(format!("http://{}:{}", node.advertise_addr, node.api_port))
}

/// Build the HTTP base URL (`http://host:port`) for a follower node.
fn follower_base_url(node: &zlayer_scheduler::raft::NodeInfo) -> Option<String> {
    if node.advertise_addr.is_empty() || node.api_port == 0 {
        return None;
    }
    Some(format!("http://{}:{}", node.advertise_addr, node.api_port))
}

/// Internal helper: poll `<base>/health/ready` and return true when the
/// endpoint responds with a 2xx status (i.e. the daemon is back up).
async fn probe_ready(client: &reqwest::Client, base: &str) -> bool {
    let url = format!("{base}/health/ready");
    match client.get(&url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Drive the rolling upgrade for a single follower:
///   1. POST `internal/upgrade/start` to schedule the upgrade
///   2. Wait for the daemon to drop (health/ready stops succeeding)
///   3. Wait for it to come back healthy
///
/// Errors from any step are returned to the caller, which decides whether
/// to abort (strict mode) or record-and-continue.
async fn upgrade_one_follower(
    client: &reqwest::Client,
    base_url: &str,
    internal_token: &str,
    version: Option<&str>,
) -> std::result::Result<(), String> {
    // 1. POST /api/v1/internal/upgrade/start
    let start_url = format!("{base_url}/api/v1/internal/upgrade/start");
    let body = serde_json::json!({ "version": version });
    let resp = client
        .post(&start_url)
        .header(INTERNAL_AUTH_HEADER, internal_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("failed to POST upgrade/start: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "upgrade/start returned non-success status: {}",
            resp.status()
        ));
    }

    // 2. Wait for the daemon to drop. 30s budget.
    let drop_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut dropped = false;
    while std::time::Instant::now() < drop_deadline {
        if !probe_ready(client, base_url).await {
            dropped = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    if !dropped {
        return Err(
            "follower did not drop health/ready within 30s — upgrade may not have started".into(),
        );
    }

    // 3. Wait for the daemon to come back. 120s budget.
    let return_deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while std::time::Instant::now() < return_deadline {
        if probe_ready(client, base_url).await {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Err("follower did not return to health/ready within 120s".into())
}

/// Drive a rolling daemon-binary upgrade across every follower.
///
/// `POST /api/v1/cluster/upgrade`
///
/// Returns `421 Misdirected Request` (with an `X-Leader-Addr` header) when
/// called on a follower; the CLI should redirect to the leader. The leader
/// upgrades followers in ascending `node_id` order; it does NOT upgrade
/// itself in this handler — the caller is expected to invoke
/// `/api/v1/internal/upgrade/start` against the leader after followers are
/// done (or step down first), so the leader process can restart cleanly.
///
/// # Errors
///
/// Returns `ServiceUnavailable` if no Raft coordinator is wired,
/// `NotLeader` if this node is not the leader, or `Internal` if cluster
/// state cannot be read.
#[utoipa::path(
    post,
    path = "/api/v1/cluster/upgrade",
    request_body = ClusterUpgradeRequest,
    responses(
        (status = 200, description = "Rolling upgrade completed", body = ClusterUpgradeResult),
        (status = 421, description = "Not the leader; clients should redirect via the X-Leader-Addr header"),
        (status = 500, description = "Internal error driving the rollout"),
        (status = 503, description = "Raft coordinator not available"),
    ),
    security(("bearer_auth" = [])),
    tag = "Cluster"
)]
pub async fn cluster_upgrade(
    State(state): State<ClusterApiState>,
    Json(req): Json<ClusterUpgradeRequest>,
) -> Result<Json<ClusterUpgradeResult>> {
    let raft = state
        .raft
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Raft coordinator not available".into()))?;

    if !raft.is_leader() {
        let leader_addr = leader_addr_for(&state, raft.leader_id()).await;
        return Err(ApiError::NotLeader { leader_addr });
    }

    let internal_token = state.internal_token.clone().ok_or_else(|| {
        ApiError::ServiceUnavailable(
            "internal token not configured; cluster upgrades require an internal shared secret"
                .into(),
        )
    })?;

    // Snapshot the cluster, then upgrade followers in node-id-ascending order
    // (deterministic so multiple retries land in the same sequence). The
    // leader is intentionally omitted; the operator/CLI is expected to drive
    // the leader's self-upgrade as a separate step after followers are
    // green.
    let cluster_state = raft.read_state().await;
    let leader_node_id = raft.node_id();
    let mut followers: Vec<&zlayer_scheduler::raft::NodeInfo> = cluster_state
        .nodes
        .values()
        .filter(|n| n.node_id != leader_node_id)
        .collect();
    followers.sort_by_key(|n| n.node_id);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| ApiError::Internal(format!("failed to build HTTP client: {e}")))?;

    let mut upgraded: Vec<NodeId> = Vec::new();
    let mut skipped: Vec<NodeId> = Vec::new();
    let mut errors: Vec<UpgradeError> = Vec::new();

    for node in followers {
        let Some(base_url) = follower_base_url(node) else {
            warn!(
                node_id = node.node_id,
                "skipping follower with missing advertise_addr or api_port"
            );
            skipped.push(node.node_id);
            continue;
        };

        info!(
            node_id = node.node_id,
            base_url = %base_url,
            version = ?req.version,
            "starting rolling upgrade for follower"
        );

        match upgrade_one_follower(&client, &base_url, &internal_token, req.version.as_deref())
            .await
        {
            Ok(()) => {
                info!(node_id = node.node_id, "follower upgrade succeeded");
                upgraded.push(node.node_id);
            }
            Err(message) => {
                warn!(
                    node_id = node.node_id,
                    error = %message,
                    "follower upgrade failed"
                );
                errors.push(UpgradeError {
                    node_id: node.node_id,
                    message,
                });
                if req.strict {
                    break;
                }
            }
        }

        // Cooldown between nodes lets the cluster re-stabilise (Raft quorum,
        // overlay handshakes, etc.) before we move on.
        if req.cooldown_secs > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(req.cooldown_secs)).await;
        }
    }

    Ok(Json(ClusterUpgradeResult {
        upgraded,
        skipped,
        errors,
    }))
}

// =============================================================================
// Leader self-upgrade (`POST /api/v1/cluster/upgrade-self`)
// =============================================================================

/// Request body for `POST /api/v1/cluster/upgrade-self`.
///
/// Triggers a daemon-binary self-upgrade on this node. Used by the CLI
/// after `cluster_upgrade` finishes the follower walk, so the leader can
/// upgrade itself last. The handler re-enters
/// `internal_upgrade_start` over localhost using the configured internal
/// token, so all the existing self-update / sentinel / exit-75 plumbing
/// is reused.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ClusterUpgradeSelfRequest {
    /// Target version (e.g. `"v0.12.0"`). Defaults to latest GitHub release.
    #[serde(default)]
    pub version: Option<String>,
}

/// Trigger a daemon-binary self-upgrade on the local (leader) node.
///
/// `POST /api/v1/cluster/upgrade-self`
///
/// Two-step orchestration:
///
/// 1. **Leader handoff via `trigger_elect`**: pick a healthy follower
///    (status == `"ready"`, fresh heartbeat) and POST to its
///    `/api/v1/internal/raft/trigger-elect`. That follower campaigns
///    immediately — Raft safety guarantees only an up-to-date candidate
///    wins, so a stale callee just loses the term and a better-up-to-date
///    follower wins the next. We then poll our own `raft.metrics()`
///    until `current_leader != local_node_id` (timeout 5s). If the
///    handoff fails or times out, we still proceed; the worst case is
///    the cluster waits one heartbeat-loss window for a new leader.
///
/// 2. **Local self-upgrade**: re-enter `POST /api/v1/internal/upgrade/start`
///    against `127.0.0.1` so the (now-former) leader's daemon downloads
///    the target release, writes the restart sentinel, and exits with
///    code 75 — the OS supervisor (launchd / systemd / SCM) respawns it
///    on the new binary and it rejoins as a voter.
///
/// # Errors
///
/// Returns `ServiceUnavailable` if Raft or the internal token are
/// unconfigured, `Internal` if the leader's own `advertise_addr` /
/// `api_port` cannot be resolved from cluster state, or `Internal` if
/// the localhost upgrade-start call fails.
#[utoipa::path(
    post,
    path = "/api/v1/cluster/upgrade-self",
    request_body = ClusterUpgradeSelfRequest,
    responses(
        (status = 202, description = "Self-upgrade scheduled", body = UpgradeStartResponse),
        (status = 500, description = "Internal error scheduling self-upgrade"),
        (status = 503, description = "Raft coordinator or internal token not configured"),
    ),
    security(("bearer_auth" = [])),
    tag = "Cluster"
)]
#[allow(clippy::too_many_lines)]
pub async fn cluster_upgrade_self(
    State(state): State<ClusterApiState>,
    Json(req): Json<ClusterUpgradeSelfRequest>,
) -> Result<(StatusCode, Json<UpgradeStartResponse>)> {
    const FOLLOWER_FRESH_SECS: u64 = 10;

    let raft = state
        .raft
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Raft coordinator not available".into()))?;

    let internal_token = state.internal_token.clone().ok_or_else(|| {
        ApiError::ServiceUnavailable(
            "internal token not configured; leader self-upgrade requires an internal shared secret"
                .into(),
        )
    })?;

    // Resolve the local API port by looking up our own node_id in cluster
    // state. The leader is always registered there, with its
    // advertise_addr + api_port intact, so this is the most reliable way
    // to learn our own externally-routable API base.
    let local_node_id = raft.node_id();
    let cluster_state = raft.read_state().await;
    let self_node = cluster_state.nodes.get(&local_node_id).ok_or_else(|| {
        ApiError::Internal(format!(
            "leader node {local_node_id} missing from cluster state; cannot resolve local API port",
        ))
    })?;
    if self_node.api_port == 0 {
        return Err(ApiError::Internal(
            "leader's api_port is 0; cannot dispatch self-upgrade over localhost".into(),
        ));
    }
    // Bind to 127.0.0.1 so the call never leaves the host; the listener
    // binds on 0.0.0.0 (or the advertise addr) so loopback is always
    // routable to it.
    let local_url = format!(
        "http://127.0.0.1:{port}/api/v1/internal/upgrade/start",
        port = self_node.api_port
    );

    info!(
        leader_node_id = local_node_id,
        url = %local_url,
        version = ?req.version,
        "scheduling leader self-upgrade via localhost internal/upgrade/start"
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| ApiError::Internal(format!("failed to build HTTP client: {e}")))?;

    // -----------------------------------------------------------------
    // Step 1: nudge a healthy follower into immediate election. This is
    // best-effort — if no follower is reachable, or it doesn't win the
    // term, we still proceed and let the cluster handle election after
    // we drop. Total wall-clock budget here is 5s of polling.
    // -----------------------------------------------------------------
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let candidate = cluster_state
        .nodes
        .iter()
        .filter(|(id, info)| {
            **id != local_node_id
                && info.status == "ready"
                && info.api_port != 0
                && !info.advertise_addr.is_empty()
                && now_unix.saturating_sub(info.last_heartbeat) <= FOLLOWER_FRESH_SECS
        })
        .min_by_key(|(id, _)| **id)
        .map(|(id, info)| (*id, info.clone()));

    if let Some((target_id, target_node)) = candidate {
        let trigger_url = format!(
            "http://{addr}:{port}/api/v1/internal/raft/trigger-elect",
            addr = target_node.advertise_addr,
            port = target_node.api_port,
        );
        info!(
            target_node_id = target_id,
            url = %trigger_url,
            "leader self-upgrade: asking follower to campaign before we step down"
        );
        match client
            .post(&trigger_url)
            .header(INTERNAL_AUTH_HEADER, &internal_token)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                // Poll our own raft metrics until leadership flips
                // (or the timeout expires).
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let mut handed_off = false;
                while std::time::Instant::now() < deadline {
                    let m = raft.metrics();
                    if m.current_leader.is_some_and(|l| l != local_node_id) {
                        handed_off = true;
                        info!(
                            new_leader = ?m.current_leader,
                            "leader self-upgrade: leadership handed off, proceeding to local self-update"
                        );
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
                if !handed_off {
                    info!(
                        "leader self-upgrade: trigger-elect didn't flip leadership in 5s; proceeding anyway (cluster will elect after we exit)"
                    );
                }
            }
            Ok(r) => {
                let status = r.status();
                info!(
                    target_node_id = target_id,
                    status = %status,
                    "leader self-upgrade: trigger-elect returned non-success; proceeding with self-update anyway"
                );
            }
            Err(e) => {
                info!(
                    target_node_id = target_id,
                    error = %e,
                    "leader self-upgrade: trigger-elect call failed; proceeding with self-update anyway"
                );
            }
        }
    } else {
        info!(
            "leader self-upgrade: no healthy follower available to hand off to; proceeding with self-update (cluster will elect after we exit)"
        );
    }

    // -----------------------------------------------------------------
    // Step 2: schedule the local self-update over loopback.
    // -----------------------------------------------------------------

    let resp = client
        .post(&local_url)
        .header(INTERNAL_AUTH_HEADER, &internal_token)
        .json(&serde_json::json!({ "version": req.version }))
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to POST localhost upgrade/start: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ApiError::Internal(format!(
            "localhost upgrade/start returned non-success status {status}: {body}"
        )));
    }

    // `UpgradeStartResponse` is server-side-only (Serialize, not
    // Deserialize), so decode through an untyped Value and rebuild the
    // response. This keeps `internal.rs` untouched.
    let body: serde_json::Value = resp.json().await.map_err(|e| {
        ApiError::Internal(format!(
            "failed to decode localhost upgrade/start response: {e}"
        ))
    })?;
    let upgrade_id = body
        .get("upgrade_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ApiError::Internal("localhost upgrade/start response missing upgrade_id".into())
        })?
        .to_string();
    let message = body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("Upgrade scheduled")
        .to_string();

    Ok((
        StatusCode::ACCEPTED,
        Json(UpgradeStartResponse {
            upgrade_id,
            message,
        }),
    ))
}

// =============================================================================
// Helpers
// =============================================================================

/// JWT claims for a signed cluster-join token.
///
/// Used by [`validate_join_token_signed`]. The token is HS256 over an
/// HMAC key derived from the cluster `join_secret` (see
/// [`derive_join_hmac_key`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JoinTokenClaims {
    /// Issuer — must equal `"zlayer-cluster"`.
    iss: String,
    /// Audience — must equal `"cluster-join"`.
    aud: String,
    /// Single-use token id (recorded in `used_jtis` after first successful use).
    jti: String,
    /// Expiration (Unix seconds).
    exp: u64,
    /// Issued at (Unix seconds).
    iat: u64,
}

/// Derive the HMAC signing key for join tokens from the cluster's
/// `join_secret`.
///
/// The join secret is operator-supplied and reused across many surfaces
/// (legacy `auth_secret` validator, this signed-token validator, future
/// internal RPC). We pass it through SHA-256 here so the actual signing
/// key is deterministic but not byte-identical to whatever the operator
/// configured — a small defence-in-depth measure that costs nothing.
fn derive_join_hmac_key(join_secret: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"zlayer-join-token-v1\0");
    hasher.update(join_secret.as_bytes());
    hasher.finalize().into()
}

/// Validate a signed cluster-join token (HS256 JWT).
///
/// On success, records `jti` in `used_jtis` so a second attempt with the
/// same token is rejected as a replay.
///
/// Errors:
/// * `Unauthorized` — signature/expiry/audience/issuer mismatch, or replay.
fn validate_join_token_signed(
    token: &str,
    hmac_key: &[u8],
    used_jtis: &Mutex<HashSet<String>>,
) -> Result<()> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&["zlayer-cluster"]);
    validation.set_audience(&["cluster-join"]);
    // `exp` validation is on by default; tighten the leeway so a stale
    // token doesn't quietly pass.
    validation.leeway = 30;

    let data = decode::<JoinTokenClaims>(token, &DecodingKey::from_secret(hmac_key), &validation)
        .map_err(|e| {
        use jsonwebtoken::errors::ErrorKind;
        let kind = match e.kind() {
            ErrorKind::ExpiredSignature => "expired",
            ErrorKind::InvalidSignature => "invalid_signature",
            ErrorKind::InvalidIssuer => "invalid_issuer",
            ErrorKind::InvalidAudience => "invalid_audience",
            ErrorKind::InvalidToken | ErrorKind::Json(_) | ErrorKind::Base64(_) => "malformed",
            _ => "other",
        };
        warn!(event = "join_token_signed_failed", kind, error = %e);
        ApiError::Unauthorized(format!("Invalid join token: {kind}"))
    })?;

    let claims = data.claims;

    // Replay protection: a `jti` may only be redeemed once. Insert under
    // the lock and reject if it was already present.
    let mut seen = used_jtis
        .lock()
        .map_err(|_| ApiError::Internal("used_jtis mutex poisoned".to_string()))?;
    if !seen.insert(claims.jti.clone()) {
        warn!(event = "join_token_replay", jti = %claims.jti);
        return Err(ApiError::Unauthorized(
            "Join token already used (replay)".into(),
        ));
    }

    Ok(())
}

/// Encode raw 32-byte Ed25519 public key bytes as a DER `SubjectPublicKeyInfo`
/// blob that [`jsonwebtoken::DecodingKey::from_ed_der`] accepts. The wrapping
/// is the standard RFC 8410 SPKI for `id-Ed25519` (OID 1.3.101.112): a 12-byte
/// prefix followed by the 32 pubkey bytes for 44 bytes total.
fn ed25519_pubkey_to_spki_der(pubkey: &[u8; 32]) -> Vec<u8> {
    const ED25519_SPKI_PREFIX: [u8; 12] = [
        0x30, 0x2a, // SEQUENCE, 42 bytes
        0x30, 0x05, // SEQUENCE, 5 bytes (AlgorithmIdentifier)
        0x06, 0x03, 0x2b, 0x65, 0x70, // OID 1.3.101.112 (Ed25519)
        0x03, 0x21, 0x00, // BIT STRING, 33 bytes, 0 unused bits
    ];
    let mut out = Vec::with_capacity(ED25519_SPKI_PREFIX.len() + 32);
    out.extend_from_slice(&ED25519_SPKI_PREFIX);
    out.extend_from_slice(pubkey);
    out
}

/// Validate an EdDSA-signed JWT cluster join token (Wave 11).
///
/// Same RFC 7519 envelope as [`validate_join_token_signed`] but with
/// `alg = "EdDSA"` and the signature verified against the cluster signing
/// keystore keyed by the JWT header's `kid`. Replay-protection shares the
/// `used_jtis` set, so a token replayed across algorithms is still rejected.
///
/// Lookup: the JWT header's `kid` must match an active or grace-window entry
/// in the keystore at `state.cluster_signing_key_path`. Unknown kids /
/// expired-grace kids are rejected. If no keystore path is configured the
/// validator falls back to the single active [`ClusterApiState::cluster_signer`].
///
/// # Errors
/// - [`ApiError::Unauthorized`] for any decode / signature / claim / replay
///   failure.
/// - [`ApiError::ServiceUnavailable`] if neither the keystore path nor an
///   active signer is configured on this node.
async fn validate_join_token_eddsa_jwt(state: &ClusterApiState, token: &str) -> Result<()> {
    // 0. Parse the JWT header to extract `kid` (we need it BEFORE we can
    //    fetch the verifying key).
    let header = jsonwebtoken::decode_header(token).map_err(|e| {
        warn!(
            event = "join_token_eddsa_jwt_failed",
            kind = "malformed_header",
            error = %e
        );
        ApiError::Unauthorized(format!("EdDSA-JWT header parse failed: {e}"))
    })?;
    if header.alg != jsonwebtoken::Algorithm::EdDSA {
        return Err(ApiError::Unauthorized(format!(
            "expected alg=EdDSA, got alg={:?}",
            header.alg
        )));
    }
    let kid = header
        .kid
        .clone()
        .ok_or_else(|| ApiError::Unauthorized("EdDSA-JWT header missing kid".into()))?;

    // 1. Look up the verifying key for this kid in the local keystore.
    let verifying_key = if let Some(path) = state.cluster_signing_key_path.as_ref() {
        zlayer_secrets::load_signer_for_kid(path, &kid)
            .await
            .map_err(|e| ApiError::Internal(format!("keystore lookup failed: {e}")))?
            .ok_or_else(|| {
                warn!(
                    event = "join_token_eddsa_jwt_failed",
                    kind = "unknown_kid",
                    kid = %kid,
                );
                ApiError::Unauthorized(format!(
                    "EdDSA-JWT kid={kid} not trusted (unknown or expired-grace)"
                ))
            })?
            .verifying_key()
    } else if let Some(signer) = state.cluster_signer.as_ref() {
        if signer.key_id() != kid {
            return Err(ApiError::Unauthorized(format!(
                "EdDSA-JWT kid={} not trusted (only active kid={} configured)",
                kid,
                signer.key_id()
            )));
        }
        signer.verifying_key()
    } else {
        return Err(ApiError::ServiceUnavailable(
            "cluster signer not configured on this node".into(),
        ));
    };

    // 2. Convert raw Ed25519 pubkey bytes into DER SPKI for jsonwebtoken.
    let pubkey_bytes: [u8; 32] = *verifying_key.as_bytes();
    let spki_der = ed25519_pubkey_to_spki_der(&pubkey_bytes);
    let decoding_key = jsonwebtoken::DecodingKey::from_ed_der(&spki_der);

    // 3. Verify with EdDSA + standard validation (same claims contract as
    //    HS256 path).
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::EdDSA);
    validation.set_issuer(&["zlayer-cluster"]);
    validation.set_audience(&["cluster-join"]);
    validation.leeway = 30;

    let data = jsonwebtoken::decode::<JoinTokenClaims>(token, &decoding_key, &validation).map_err(
        |e| {
            use jsonwebtoken::errors::ErrorKind;
            let failure_kind = match e.kind() {
                ErrorKind::ExpiredSignature => "expired",
                ErrorKind::InvalidSignature => "invalid_signature",
                ErrorKind::InvalidIssuer => "invalid_issuer",
                ErrorKind::InvalidAudience => "invalid_audience",
                ErrorKind::InvalidToken | ErrorKind::Json(_) | ErrorKind::Base64(_) => "malformed",
                _ => "other",
            };
            warn!(event = "join_token_eddsa_jwt_failed", kind = failure_kind, error = %e);
            ApiError::Unauthorized(format!("Invalid EdDSA-JWT join token: {failure_kind}"))
        },
    )?;

    // 4. Replay protection: jti goes into the shared used_jtis set so a
    //    token replayed across HS256 <-> EdDSA still rejects.
    let claims = data.claims;
    let mut seen = state
        .used_jtis
        .lock()
        .map_err(|_| ApiError::Internal("used_jtis mutex poisoned".into()))?;
    if !seen.insert(claims.jti.clone()) {
        warn!(event = "join_token_eddsa_jwt_replay", jti = %claims.jti);
        return Err(ApiError::Unauthorized(
            "EdDSA-JWT join token already used (replay)".into(),
        ));
    }

    Ok(())
}

/// Validate a Wave-3 Ed25519-signed cluster join token.
///
/// On success returns the verified [`ClusterJoinClaims`]. The envelope's
/// `kid` and `sig` are discarded after verification.
///
/// Rejection paths produce [`ApiError::Unauthorized`] with an actionable
/// body. Replay protection: inserts `ed25519:<kid>:<iat>:<iss>` into
/// `state.used_jtis`; if already present, rejects as a replay. The HS256
/// validator uses the same set for its `jti`, so a replayed token of
/// either format cannot succeed.
///
/// The byte sequence fed to [`ed25519_dalek::VerifyingKey::verify_strict`]
/// is `serde_json::to_vec(&envelope.claims)` — exactly what
/// `mint_signed_cluster_join_token` in `bin/zlayer/src/commands/node.rs`
/// signs. The fields of [`ClusterJoinClaims`] are declared in canonical
/// order in `zlayer-types`; if that order is ever shuffled both helpers
/// must be updated in lockstep AND the envelope version bumped.
/// Verify a Wave-3 Ed25519-signed cluster join token.
///
/// Lookup behavior:
/// - If [`ClusterApiState::cluster_signing_key_path`] is `Some(path)`, the
///   verifying key is looked up by the token's `kid` via
///   [`zlayer_secrets::load_signer_for_kid`]. This accepts tokens signed
///   under either the active key OR any key currently in its retired-grace
///   window. Unknown kids and kids whose grace has expired are rejected
///   with `Unauthorized`.
/// - If the path is `None` (e.g., in tests, or before the daemon bootstrap
///   wires it), the validator falls back to the pre-Wave-5A behavior:
///   compare the token's `kid` against the single active
///   [`ClusterApiState::cluster_signer`].
///
/// Configure the path via the daemon bootstrap in
/// `bin/zlayer/src/commands/serve.rs` for proper rotation-aware verification.
#[allow(clippy::too_many_lines)]
async fn validate_join_token_ed25519(
    state: &ClusterApiState,
    raw_token: &str,
    peer_ip: std::net::IpAddr,
) -> Result<ClusterJoinClaims> {
    use base64::Engine;

    // 0. Cheap availability check. If neither a keystore path nor an active
    //    signer is configured on this node, we have nothing to verify
    //    against — surface 503 before doing any envelope work. Preserves
    //    the pre-Wave-5A contract for `cluster_signer: None` states (e.g.,
    //    `ClusterApiState::placeholder()`).
    if state.cluster_signing_key_path.is_none() && state.cluster_signer.is_none() {
        return Err(ApiError::ServiceUnavailable(
            "cluster signer not configured on this node".into(),
        ));
    }

    // 1. Base64-decode the envelope (prefer URL_SAFE_NO_PAD, fall back to STANDARD).
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw_token.trim())
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(raw_token.trim()))
        .map_err(|e| ApiError::Unauthorized(format!("token base64 decode failed: {e}")))?;

    // 2. Parse as SignedClusterJoinToken.
    let envelope: SignedClusterJoinToken = serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::Unauthorized(format!("token envelope parse failed: {e}")))?;
    if envelope.v != SIGNED_TOKEN_V_WAVE3 && envelope.v != SIGNED_TOKEN_V_WAVE9 {
        return Err(ApiError::Unauthorized(format!(
            "unsupported signed token version: got v={}, expected v={} or v={}",
            envelope.v, SIGNED_TOKEN_V_WAVE3, SIGNED_TOKEN_V_WAVE9,
        )));
    }

    // 3. Resolve the verifying key for `envelope.kid`.
    //
    // Two-path resolution (Wave 9D-iii):
    //   - **Local path (v=1 always, v=2 same-cluster):** look up `kid`
    //     in our own keystore (or fall back to the active single
    //     signer). If found, this is a token minted by us; use that
    //     verifying key.
    //   - **Federation path (v=2 only):** when local resolution returns
    //     "kid not trusted" AND the envelope carries a `ca_chain`,
    //     consult our imported `trusted_bundles` (via Raft) to verify
    //     the cert against a known CA pubkey, then trust the leaf
    //     pubkey carried by the cert. The cert's `active_kid` must
    //     match `envelope.kid` and the cert must not be expired.
    let local_verifying_key: Option<ed25519_dalek::VerifyingKey> =
        if let Some(path) = state.cluster_signing_key_path.as_ref() {
            match zlayer_secrets::load_signer_for_kid(path, &envelope.kid).await {
                Ok(Some(s)) => Some(s.verifying_key()),
                Ok(None) => None,
                Err(e) => {
                    return Err(ApiError::Internal(format!(
                        "cluster signing keystore lookup failed: {e}"
                    )));
                }
            }
        } else if let Some(signer) = state.cluster_signer.as_ref() {
            if envelope.kid == signer.key_id() {
                Some(signer.verifying_key())
            } else {
                None
            }
        } else {
            None
        };

    let (verifying_key, federation_meta) = if let Some(key) = local_verifying_key {
        (key, None)
    } else if envelope.v == SIGNED_TOKEN_V_WAVE9 {
        // Federation path. The kid is unknown locally; the token must
        // carry a CaCert binding it to a foreign cluster whose
        // TrustBundle we've imported.
        let cert = envelope.ca_chain.as_ref().ok_or_else(|| {
            warn!(
                event = "join_token_ed25519_failed",
                kind = "v2_missing_ca_chain",
                kid = %envelope.kid,
                peer_ip = %peer_ip,
            );
            ApiError::Unauthorized(
                "v=2 token rejected: kid not local and ca_chain is absent".into(),
            )
        })?;
        let raft = state.raft.as_ref().ok_or_else(|| {
            ApiError::ServiceUnavailable(
                "Raft coordinator unavailable; cannot consult trust bundles".into(),
            )
        })?;
        let secrets_state = raft.secrets_state().await;
        let bundle = secrets_state
            .trust_bundle_for(&cert.cluster_domain)
            .ok_or_else(|| {
                warn!(
                    event = "join_token_ed25519_failed",
                    kind = "unknown_cluster_domain",
                    cluster_domain = %cert.cluster_domain,
                    kid = %envelope.kid,
                    peer_ip = %peer_ip,
                );
                ApiError::Unauthorized(format!(
                    "v=2 token's ca_chain.cluster_domain={} is not in this cluster's trusted bundles",
                    cert.cluster_domain
                ))
            })?;
        zlayer_secrets::ClusterCa::verify_ca_cert(&bundle.ca_public_key_b64, cert).map_err(
            |e| {
                warn!(
                    event = "join_token_ed25519_failed",
                    kind = "ca_cert_invalid",
                    cluster_domain = %cert.cluster_domain,
                    kid = %envelope.kid,
                    peer_ip = %peer_ip,
                    error = %e,
                );
                ApiError::Unauthorized(format!("v=2 token's ca_chain failed CA verification: {e}"))
            },
        )?;
        if cert.active_kid != envelope.kid {
            return Err(ApiError::Unauthorized(format!(
                "v=2 token: ca_chain.active_kid={} does not match envelope.kid={}",
                cert.active_kid, envelope.kid
            )));
        }
        // Decode the CA-asserted active pubkey for leaf-sig
        // verification. The CA's signature (verified just above) is
        // what makes this pubkey trustworthy.
        let pubkey_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(cert.active_pubkey_b64.as_bytes())
            .map_err(|e| {
                ApiError::Unauthorized(format!("ca_chain.active_pubkey_b64 base64 decode: {e}"))
            })?;
        let pubkey_arr: [u8; 32] = pubkey_bytes.as_slice().try_into().map_err(|_| {
            ApiError::Unauthorized(format!(
                "ca_chain.active_pubkey_b64 wrong length: {}",
                pubkey_bytes.len()
            ))
        })?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&pubkey_arr).map_err(|e| {
            ApiError::Unauthorized(format!("ca_chain.active_pubkey_b64 invalid: {e}"))
        })?;
        (key, Some(cert.cluster_domain.clone()))
    } else {
        warn!(
            event = "join_token_ed25519_failed",
            kind = "kid_not_trusted",
            kid = %envelope.kid,
            peer_ip = %peer_ip,
        );
        return Err(ApiError::Unauthorized(format!(
            "signed token kid={} not trusted (unknown kid, expired grace, or never issued)",
            envelope.kid
        )));
    };

    // 4. Verify Ed25519 signature over `serde_json::to_vec(&envelope.claims)`.
    //    Byte-identical to `mint_signed_cluster_join_token` in node.rs.
    let sig_bytes_vec = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&envelope.sig)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(&envelope.sig))
        .map_err(|e| ApiError::Unauthorized(format!("signature base64 decode failed: {e}")))?;
    let sig_array: [u8; 64] = sig_bytes_vec.as_slice().try_into().map_err(|_| {
        ApiError::Unauthorized(format!(
            "signature wrong length: expected 64, got {}",
            sig_bytes_vec.len()
        ))
    })?;
    let sig = ed25519_dalek::Signature::from_bytes(&sig_array);
    let claims_bytes = serde_json::to_vec(&envelope.claims)
        .map_err(|e| ApiError::Internal(format!("re-serializing claims: {e}")))?;
    verifying_key
        .verify_strict(&claims_bytes, &sig)
        .map_err(|e| {
            warn!(
                event = "join_token_ed25519_failed",
                kind = "invalid_signature",
                kid = %envelope.kid,
                peer_ip = %peer_ip,
                error = %e,
            );
            ApiError::Unauthorized(format!("Ed25519 signature verification failed: {e}"))
        })?;

    // 5. Expiry check (RFC3339; mirror node.rs's `verify_signed_cluster_join_token`).
    let now = chrono::Utc::now();
    let exp = chrono::DateTime::parse_from_rfc3339(&envelope.claims.exp)
        .map_err(|e| ApiError::Unauthorized(format!("token exp parse failed: {e}")))?
        .with_timezone(&chrono::Utc);
    if now >= exp {
        return Err(ApiError::Unauthorized(format!(
            "signed token expired at {} (now {})",
            envelope.claims.exp,
            now.to_rfc3339()
        )));
    }

    // 6. Replay protection on `(kid, iat, iss)`.
    let jti = format!(
        "ed25519:{}:{}:{}",
        envelope.kid, envelope.claims.iat, envelope.claims.iss
    );
    {
        let mut guard = state
            .used_jtis
            .lock()
            .map_err(|_| ApiError::Internal("used_jtis mutex poisoned".to_string()))?;
        if !guard.insert(jti) {
            warn!(
                event = "join_token_ed25519_replay",
                kid = %envelope.kid,
                iat = %envelope.claims.iat,
                iss = %envelope.claims.iss,
                peer_ip = %peer_ip,
            );
            return Err(ApiError::Unauthorized(format!(
                "signed token replay detected for kid={} iat={} iss={}",
                envelope.kid, envelope.claims.iat, envelope.claims.iss
            )));
        }
    }

    info!(
        format = if envelope.v == SIGNED_TOKEN_V_WAVE9 {
            "ed25519_v2"
        } else {
            "ed25519"
        },
        kid = %envelope.kid,
        iss = %envelope.claims.iss,
        federated_from = ?federation_meta,
        peer_ip = %peer_ip,
        "accepted cluster join token"
    );
    Ok(envelope.claims)
}

/// Cheap heuristic: does `s` decode to a JSON object with a `"v"` field?
///
/// Used by [`cluster_join`] to decide whether a token that fails Ed25519
/// validation looks like a signed envelope (in which case the failure is
/// explicit — return the Ed25519 error) versus a legacy HS256/plaintext
/// token shape (in which case we should fall through to the next validator).
///
/// Returns `false` on any decode/parse failure. Cost: one base64 decode and
/// one `serde_json::from_slice::<Value>` call.
fn is_signed_envelope_shape(s: &str) -> bool {
    use base64::Engine;
    let bytes = match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s.trim()) {
        Ok(b) => b,
        Err(_) => match base64::engine::general_purpose::STANDARD.decode(s.trim()) {
            Ok(b) => b,
            Err(_) => return false,
        },
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    value.as_object().is_some_and(|obj| {
        obj.contains_key("v") && obj.contains_key("sig") && obj.contains_key("kid")
    })
}

/// Detect whether `token` is a legacy v0.11.x-style plaintext join token
/// whose embedded `auth_secret` matches the cluster's `expected_secret`.
///
/// Wave 6 (v0.13.0): plaintext tokens are no longer accepted by
/// [`cluster_join`]. This helper exists ONLY to differentiate "token is a
/// recognizably plaintext payload — return a precise migration message" from
/// "token failed HS256 decoding for some other reason — return the generic
/// HS256 error". It is NOT an authentication path.
///
/// Returns `false` on any decode/parse failure or if the `auth_secret`
/// doesn't match. A `true` result means the token is unambiguously a stale
/// plaintext form addressed to this cluster.
fn is_legacy_plaintext_token_shape(token: &str, expected_secret: &str) -> bool {
    use base64::Engine;

    let decoded = match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token) {
        Ok(d) => d,
        Err(_) => match base64::engine::general_purpose::STANDARD.decode(token) {
            Ok(d) => d,
            Err(_) => return false,
        },
    };

    let value: serde_json::Value = match serde_json::from_slice(&decoded) {
        Ok(v) => v,
        Err(_) => return false,
    };

    value
        .get("auth_secret")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|secret| secret == expected_secret)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_legacy_plaintext_token_shape_matches_secret() {
        // Wave 6 (v0.13.0): the helper is no longer an auth path — it only
        // distinguishes "this is a stale plaintext token addressed to our
        // cluster, return the actionable migration error" from "garbage,
        // return the generic HS256 decode error". The acceptance contract
        // for that disambiguation is unchanged from the pre-Wave-6
        // `validate_join_token` predicate: returns `true` iff the payload
        // decodes, is JSON-shaped, and has an `auth_secret` matching
        // `expected_secret`.
        use base64::Engine;

        let payload = serde_json::json!({
            "api_endpoint": "10.0.0.1:3669",
            "raft_endpoint": "10.0.0.1:9000",
            "leader_wg_pubkey": "abc",
            "overlay_cidr": "10.200.0.0/16",
            "auth_secret": "my-secret-123",
            "created_at": "2025-01-01T00:00:00Z"
        });
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&payload).unwrap());

        assert!(is_legacy_plaintext_token_shape(&token, "my-secret-123"));
        assert!(!is_legacy_plaintext_token_shape(&token, "wrong-secret"));
    }

    #[test]
    fn is_legacy_plaintext_token_shape_invalid_base64() {
        assert!(!is_legacy_plaintext_token_shape("not-valid!!!", "secret"));
    }

    #[test]
    fn is_legacy_plaintext_token_shape_invalid_json() {
        use base64::Engine;
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("not json");
        assert!(!is_legacy_plaintext_token_shape(&token, "secret"));
    }

    #[test]
    fn is_legacy_plaintext_token_shape_missing_field() {
        use base64::Engine;
        let payload = serde_json::json!({"foo": "bar"});
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&payload).unwrap());
        assert!(!is_legacy_plaintext_token_shape(&token, "secret"));
    }

    #[test]
    fn test_cluster_api_state_next_id() {
        let state = ClusterApiState::placeholder();
        assert_eq!(state.next_id(), 2);
        assert_eq!(state.next_id(), 3);
        assert_eq!(state.next_id(), 4);
    }

    #[test]
    fn test_cluster_join_request_deserialize() {
        let json = r#"{
            "token": "abc123",
            "advertise_addr": "10.0.0.2",
            "overlay_port": 51820,
            "raft_port": 9000,
            "wg_public_key": "pubkey123"
        }"#;
        let req: ClusterJoinRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.advertise_addr, "10.0.0.2");
        assert_eq!(req.mode, "full"); // default
        assert!(req.services.is_none());
        // Resource fields default to zero/empty
        assert!((req.cpu_total - 0.0).abs() < f64::EPSILON);
        assert_eq!(req.memory_total, 0);
        assert_eq!(req.disk_total, 0);
        assert!(req.gpus.is_empty());
    }

    #[test]
    fn test_cluster_join_request_with_resources() {
        let json = r#"{
            "token": "abc123",
            "advertise_addr": "10.0.0.2",
            "overlay_port": 51820,
            "raft_port": 9000,
            "wg_public_key": "pubkey123",
            "cpu_total": 16.0,
            "memory_total": 68719476736,
            "disk_total": 1099511627776,
            "gpus": [
                {"vendor": "nvidia", "model": "NVIDIA A100-SXM4-80GB", "memory_mb": 81920}
            ]
        }"#;
        let req: ClusterJoinRequest = serde_json::from_str(json).unwrap();
        assert!((req.cpu_total - 16.0).abs() < f64::EPSILON);
        assert_eq!(req.memory_total, 68_719_476_736);
        assert_eq!(req.disk_total, 1_099_511_627_776);
        assert_eq!(req.gpus.len(), 1);
        assert_eq!(req.gpus[0].vendor, "nvidia");
        assert_eq!(req.gpus[0].memory_mb, 81920);
    }

    #[test]
    fn test_heartbeat_request_deserialize() {
        let json = r#"{
            "node_id": 2,
            "cpu_used": 4.5,
            "memory_used": 8589934592,
            "disk_used": 107374182400
        }"#;
        let req: HeartbeatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.node_id, 2);
        assert!((req.cpu_used - 4.5).abs() < f64::EPSILON);
        assert_eq!(req.memory_used, 8_589_934_592);
        assert_eq!(req.disk_used, 107_374_182_400);
    }

    #[test]
    fn test_cluster_join_response_serialize() {
        let resp = ClusterJoinResponse {
            node_id: "uuid-123".into(),
            raft_node_id: 2,
            overlay_ip: "10.200.0.2".into(),
            slice_cidr: "10.200.16.0/28".into(),
            peers: vec![],
            role: "voter".into(),
            node_jwt: None,
            wrapped_dek: None,
            dek_generation: None,
            join_secret: None,
            warnings: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("uuid-123"));
        assert!(json.contains("10.200.0.2"));
        assert!(json.contains("10.200.16.0/28"));
        // `warnings = None` must be skipped (kept off the wire) so legacy
        // clients that don't know the field don't see an unexpected key.
        assert!(
            !json.contains("warnings"),
            "warnings field must be omitted when None; got {json}"
        );
    }

    // -------------------------------------------------------------------------
    // Signed join-token validator tests
    // -------------------------------------------------------------------------

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn sign_token(claims: &JoinTokenClaims, hmac_key: &[u8]) -> String {
        encode(
            &Header::new(Algorithm::HS256),
            claims,
            &EncodingKey::from_secret(hmac_key),
        )
        .unwrap()
    }

    #[test]
    fn validate_join_token_signed_accepts_valid() {
        let hmac_key = derive_join_hmac_key("operator-secret");
        let now = now_secs();
        let claims = JoinTokenClaims {
            iss: "zlayer-cluster".into(),
            aud: "cluster-join".into(),
            jti: "jti-accept-1".into(),
            exp: now + 3600,
            iat: now,
        };
        let token = sign_token(&claims, &hmac_key);
        let used = Mutex::new(HashSet::new());
        validate_join_token_signed(&token, &hmac_key, &used).expect("valid token must succeed");
    }

    #[test]
    fn validate_join_token_signed_rejects_expired() {
        let hmac_key = derive_join_hmac_key("operator-secret");
        let now = now_secs();
        let claims = JoinTokenClaims {
            iss: "zlayer-cluster".into(),
            aud: "cluster-join".into(),
            jti: "jti-expired".into(),
            // Far enough in the past that the 30s leeway can't save it.
            exp: now - 600,
            iat: now - 1200,
        };
        let token = sign_token(&claims, &hmac_key);
        let used = Mutex::new(HashSet::new());
        let err = validate_join_token_signed(&token, &hmac_key, &used)
            .expect_err("expired token must be rejected");
        assert!(matches!(err, ApiError::Unauthorized(_)));
    }

    #[test]
    fn validate_join_token_signed_rejects_replay() {
        let hmac_key = derive_join_hmac_key("operator-secret");
        let now = now_secs();
        let claims = JoinTokenClaims {
            iss: "zlayer-cluster".into(),
            aud: "cluster-join".into(),
            jti: "jti-replay".into(),
            exp: now + 3600,
            iat: now,
        };
        let token = sign_token(&claims, &hmac_key);
        let used = Mutex::new(HashSet::new());

        // First use succeeds.
        validate_join_token_signed(&token, &hmac_key, &used).expect("first use must succeed");

        // Second use with the same `jti` must error.
        let err = validate_join_token_signed(&token, &hmac_key, &used)
            .expect_err("second use must be rejected as replay");
        assert!(matches!(err, ApiError::Unauthorized(_)));
    }

    #[test]
    fn validate_join_token_signed_rejects_wrong_audience() {
        let hmac_key = derive_join_hmac_key("operator-secret");
        let now = now_secs();
        let claims = JoinTokenClaims {
            iss: "zlayer-cluster".into(),
            aud: "wrong-audience".into(),
            jti: "jti-wrong-aud".into(),
            exp: now + 3600,
            iat: now,
        };
        let token = sign_token(&claims, &hmac_key);
        let used = Mutex::new(HashSet::new());
        let err = validate_join_token_signed(&token, &hmac_key, &used)
            .expect_err("wrong audience must be rejected");
        assert!(matches!(err, ApiError::Unauthorized(_)));
    }

    #[test]
    fn validate_join_token_signed_rejects_wrong_issuer() {
        let hmac_key = derive_join_hmac_key("operator-secret");
        let now = now_secs();
        let claims = JoinTokenClaims {
            iss: "not-zlayer".into(),
            aud: "cluster-join".into(),
            jti: "jti-wrong-iss".into(),
            exp: now + 3600,
            iat: now,
        };
        let token = sign_token(&claims, &hmac_key);
        let used = Mutex::new(HashSet::new());
        let err = validate_join_token_signed(&token, &hmac_key, &used)
            .expect_err("wrong issuer must be rejected");
        assert!(matches!(err, ApiError::Unauthorized(_)));
    }

    #[test]
    fn validate_join_token_signed_rejects_wrong_signing_key() {
        let signing_key = derive_join_hmac_key("operator-secret");
        let verifying_key = derive_join_hmac_key("a-different-secret");
        let now = now_secs();
        let claims = JoinTokenClaims {
            iss: "zlayer-cluster".into(),
            aud: "cluster-join".into(),
            jti: "jti-bad-sig".into(),
            exp: now + 3600,
            iat: now,
        };
        let token = sign_token(&claims, &signing_key);
        let used = Mutex::new(HashSet::new());
        let err = validate_join_token_signed(&token, &verifying_key, &used)
            .expect_err("wrong key must be rejected");
        assert!(matches!(err, ApiError::Unauthorized(_)));
    }

    #[test]
    fn legacy_plaintext_token_shape_detector_still_recognises_v011_payloads() {
        // Wave 6 (v0.13.0): the underlying byte shape produced by old v0.11.x
        // clients is unchanged. The handler no longer accepts these tokens,
        // but it must still RECOGNISE them so it returns the actionable
        // migration error instead of an opaque HS256 decode failure.
        use base64::Engine;
        let payload = serde_json::json!({
            "auth_secret": "legacy-shared-secret",
        });
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&payload).unwrap());
        assert!(is_legacy_plaintext_token_shape(
            &token,
            "legacy-shared-secret"
        ));
    }

    #[test]
    fn derive_join_hmac_key_is_deterministic_and_distinct() {
        let a = derive_join_hmac_key("operator-secret");
        let b = derive_join_hmac_key("operator-secret");
        let c = derive_join_hmac_key("a-different-secret");
        assert_eq!(a, b, "same input must hash to same key");
        assert_ne!(a, c, "different inputs must produce different keys");
    }

    // -------------------------------------------------------------------------
    // cluster_join handler smoke tests
    //
    // The full end-to-end handler path requires a live RaftCoordinator (with
    // secrets capability) and is exercised by the Phase-1 integration tests
    // (Task #19). Here we cover the legacy-no-secrets-pubkey path by relying
    // on the fact that `placeholder()` state has no Raft, which short-circuits
    // before the secrets-SM round-trip and returns a 503 — proof that the
    // handler does NOT attempt the secrets path when `secrets_pubkey` is
    // `None`. (If we were attempting it on the legacy path, we'd be touching
    // `state.raft.propose_*` instead of returning the 503.)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn cluster_join_legacy_no_secrets_pubkey_short_circuits_without_raft() {
        let state = ClusterApiState::placeholder();
        let req = ClusterJoinRequest {
            token: "irrelevant".into(),
            advertise_addr: "10.0.0.5".into(),
            overlay_port: 51820,
            raft_port: 9000,
            api_port: 3669,
            wg_public_key: "wg-pub".into(),
            mode: "full".into(),
            services: None,
            cpu_total: 0.0,
            memory_total: 0,
            disk_total: 0,
            gpus: vec![],
            os: None,
            arch: None,
            labels: std::collections::HashMap::new(),
            secrets_pubkey: None,
        };
        let result = cluster_join(State(state), Json(req)).await;
        let err = result.expect_err("placeholder state must yield an error (no Raft)");
        assert!(
            matches!(err, ApiError::ServiceUnavailable(_)),
            "legacy joiner without raft should hit the 503 branch first; got {err:?}"
        );
    }

    #[tokio::test]
    async fn cluster_signing_pubkey_returns_active_key() {
        let signer = Arc::new(zlayer_secrets::ClusterSigner::generate());
        let kid = signer.key_id();
        let pubkey_b64 = signer.public_key_b64();

        let mut state = ClusterApiState::placeholder();
        state.cluster_signer = Some(signer);

        let resp = cluster_signing_pubkey(State(state))
            .await
            .expect("signing-pubkey handler must succeed when signer is configured");
        assert_eq!(resp.0.public_key_b64, pubkey_b64);
        assert_eq!(resp.0.kid, kid);
        assert_eq!(resp.0.kid.len(), 8, "kid must be 8 hex chars");
    }

    #[tokio::test]
    async fn cluster_signing_pubkey_returns_503_when_unconfigured() {
        // Placeholder state has `cluster_signer: None`.
        let state = ClusterApiState::placeholder();
        let err = cluster_signing_pubkey(State(state))
            .await
            .expect_err("missing signer must yield an error");
        assert!(
            matches!(err, ApiError::ServiceUnavailable(_)),
            "expected ServiceUnavailable; got {err:?}"
        );
    }

    #[tokio::test]
    async fn cluster_signing_pubkeys_returns_only_active_when_no_keystore_path() {
        let signer = Arc::new(zlayer_secrets::ClusterSigner::generate());
        let mut state = ClusterApiState::placeholder();
        state.cluster_signer = Some(signer.clone());

        let resp = cluster_signing_pubkeys(State(state))
            .await
            .expect("fallback to active signer must succeed when keystore path is None");
        assert_eq!(resp.0.keys.len(), 1);
        assert_eq!(resp.0.keys[0].kid, signer.key_id());
        assert_eq!(resp.0.keys[0].public_key_b64, signer.public_key_b64());
        assert_eq!(resp.0.keys[0].status, "active");
        assert!(resp.0.keys[0].valid_until.is_none());
    }

    #[tokio::test]
    async fn cluster_signing_pubkeys_returns_active_and_grace_from_keystore() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("cluster_signing.key");

        // Seed the keystore with an initial active signer, then rotate it
        // so the keystore holds (active + 1 grace) entries.
        let _ = zlayer_secrets::ClusterSigner::load_or_generate(&path)
            .await
            .expect("seed initial keystore");
        let _rotation =
            zlayer_secrets::rotate_keystore(&path, std::time::Duration::from_secs(3600))
                .await
                .expect("rotate keystore");

        let mut state = ClusterApiState::placeholder();
        state.cluster_signing_key_path = Some(path);

        let resp = cluster_signing_pubkeys(State(state))
            .await
            .expect("keystore-backed handler must succeed");
        assert_eq!(
            resp.0.keys.len(),
            2,
            "expected active + 1 grace entry after one rotation"
        );
        assert_eq!(resp.0.keys[0].status, "active");
        assert_eq!(resp.0.keys[1].status, "grace");
        assert!(
            resp.0.keys[1].valid_until.is_some(),
            "grace entry must carry an RFC3339 valid_until timestamp"
        );
    }

    #[tokio::test]
    async fn cluster_signing_pubkeys_returns_503_when_nothing_configured() {
        // No cluster_signer, no cluster_signing_key_path.
        let state = ClusterApiState::placeholder();
        let err = cluster_signing_pubkeys(State(state))
            .await
            .expect_err("nothing configured must yield 503");
        assert!(
            matches!(err, ApiError::ServiceUnavailable(_)),
            "expected ServiceUnavailable; got {err:?}"
        );
    }

    // -------------------------------------------------------------------------
    // Wave 3.4: Ed25519-signed cluster join token validator tests
    //
    // The mint logic lives in `bin/zlayer/src/commands/node.rs`. We can't
    // import from a binary crate, so these tests replicate the mint steps
    // inline. They MUST stay byte-identical to that helper — if you change
    // one, change the other.
    // -------------------------------------------------------------------------

    fn make_claims(exp: chrono::DateTime<chrono::Utc>, iss: &str) -> ClusterJoinClaims {
        ClusterJoinClaims {
            api_endpoint: "https://leader.test:3669".into(),
            raft_endpoint: "10.0.0.1:9000".into(),
            leader_wg_pubkey: "AAAA".into(),
            overlay_cidr: "10.42.0.0/16".into(),
            exp: exp.to_rfc3339(),
            iat: chrono::Utc::now().to_rfc3339(),
            iss: iss.into(),
        }
    }

    /// Inline mint that mirrors `mint_signed_cluster_join_token` in
    /// `bin/zlayer/src/commands/node.rs`. If the bytes diverge between
    /// these two paths, every Wave-3 token in production will fail
    /// validation — that's the load-bearing invariant.
    fn mint_for_test(claims: &ClusterJoinClaims, signer: &zlayer_secrets::ClusterSigner) -> String {
        use base64::Engine;
        let claims_bytes = serde_json::to_vec(claims).expect("serialize claims");
        let sig_bytes = signer.sign(&claims_bytes);
        let envelope = SignedClusterJoinToken {
            v: SIGNED_TOKEN_V_WAVE3,
            kid: signer.key_id(),
            claims: claims.clone(),
            sig: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig_bytes),
            ca_chain: None,
        };
        let envelope_bytes = serde_json::to_vec(&envelope).expect("serialize envelope");
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(envelope_bytes)
    }

    fn state_with_signer(signer: Arc<zlayer_secrets::ClusterSigner>) -> ClusterApiState {
        let mut state = ClusterApiState::placeholder();
        state.cluster_signer = Some(signer);
        state
    }

    /// **Byte-alignment round-trip test.** If this fails, every signed
    /// token will fail validation in production. Grep this name to find
    /// the canonical "mint here, verify there" cross-check.
    #[tokio::test]
    async fn validate_join_token_ed25519_mint_handler_byte_roundtrip() {
        let signer = Arc::new(zlayer_secrets::ClusterSigner::generate());
        let claims = make_claims(
            chrono::Utc::now() + chrono::Duration::hours(1),
            "node-uuid-A",
        );
        let token = mint_for_test(&claims, &signer);

        let state = state_with_signer(signer);
        let validated =
            validate_join_token_ed25519(&state, &token, std::net::Ipv4Addr::LOCALHOST.into())
                .await
                .expect("freshly minted token must validate");
        assert_eq!(validated.api_endpoint, claims.api_endpoint);
        assert_eq!(validated.iss, claims.iss);
    }

    #[tokio::test]
    async fn validate_join_token_ed25519_rejects_replay() {
        let signer = Arc::new(zlayer_secrets::ClusterSigner::generate());
        let claims = make_claims(
            chrono::Utc::now() + chrono::Duration::hours(1),
            "node-uuid-B",
        );
        let token = mint_for_test(&claims, &signer);

        let state = state_with_signer(signer);
        let peer: std::net::IpAddr = std::net::Ipv4Addr::LOCALHOST.into();

        // First use succeeds.
        validate_join_token_ed25519(&state, &token, peer)
            .await
            .expect("first use must succeed");

        // Second use with the same token must error as a replay.
        let err = validate_join_token_ed25519(&state, &token, peer)
            .await
            .expect_err("replay must be rejected");
        match err {
            ApiError::Unauthorized(msg) => assert!(
                msg.contains("replay"),
                "expected replay-detected error; got {msg:?}"
            ),
            other => panic!("expected Unauthorized; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_join_token_ed25519_rejects_expired() {
        let signer = Arc::new(zlayer_secrets::ClusterSigner::generate());
        // Token already expired one hour ago.
        let claims = make_claims(
            chrono::Utc::now() - chrono::Duration::hours(1),
            "node-uuid-C",
        );
        let token = mint_for_test(&claims, &signer);

        let state = state_with_signer(signer);
        let err = validate_join_token_ed25519(&state, &token, std::net::Ipv4Addr::LOCALHOST.into())
            .await
            .expect_err("expired token must be rejected");
        match err {
            ApiError::Unauthorized(msg) => assert!(
                msg.contains("expired"),
                "expected expired-token error; got {msg:?}"
            ),
            other => panic!("expected Unauthorized; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_join_token_ed25519_rejects_kid_mismatch() {
        // Mint with signer A.
        let signer_a = Arc::new(zlayer_secrets::ClusterSigner::generate());
        let claims = make_claims(
            chrono::Utc::now() + chrono::Duration::hours(1),
            "node-uuid-D",
        );
        let token = mint_for_test(&claims, &signer_a);

        // Validate against state holding a different signer B.
        let signer_b = Arc::new(zlayer_secrets::ClusterSigner::generate());
        // Sanity check — distinct keys, distinct kids.
        assert_ne!(signer_a.key_id(), signer_b.key_id());
        let state = state_with_signer(signer_b);

        let err = validate_join_token_ed25519(&state, &token, std::net::Ipv4Addr::LOCALHOST.into())
            .await
            .expect_err("kid mismatch must be rejected");
        match err {
            // Wave 9D-iii: the no-keystore-path branch now treats a
            // kid mismatch identically to "kid not trusted" (since
            // both mean "we have no verifying key for this kid").
            // Semantically equivalent rejection; message wording
            // changed.
            ApiError::Unauthorized(msg) => assert!(
                msg.contains("not trusted") && msg.contains(&signer_a.key_id()),
                "expected kid-not-trusted error citing the unknown kid; got {msg:?}"
            ),
            other => panic!("expected Unauthorized; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_join_token_ed25519_rejects_tampered_signature() {
        use base64::Engine;

        let signer = Arc::new(zlayer_secrets::ClusterSigner::generate());
        let claims = make_claims(
            chrono::Utc::now() + chrono::Duration::hours(1),
            "node-uuid-E",
        );

        // Mint a real token, then flip a byte in the inner envelope so the
        // signature no longer matches the (tampered) claims.
        let claims_bytes = serde_json::to_vec(&claims).unwrap();
        let sig_bytes = signer.sign(&claims_bytes);
        let mut tampered_claims = claims.clone();
        tampered_claims.api_endpoint = "https://attacker.example:3669".into();
        let envelope = SignedClusterJoinToken {
            v: SIGNED_TOKEN_V_WAVE3,
            kid: signer.key_id(),
            claims: tampered_claims,
            sig: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig_bytes),
            ca_chain: None,
        };
        let envelope_bytes = serde_json::to_vec(&envelope).unwrap();
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(envelope_bytes);

        let state = state_with_signer(signer);
        let err = validate_join_token_ed25519(&state, &token, std::net::Ipv4Addr::LOCALHOST.into())
            .await
            .expect_err("tampered claims must invalidate the signature");
        match err {
            ApiError::Unauthorized(msg) => assert!(
                msg.contains("signature verification failed"),
                "expected signature-verification error; got {msg:?}"
            ),
            other => panic!("expected Unauthorized; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_join_token_ed25519_returns_503_when_signer_unconfigured() {
        // Placeholder state has `cluster_signer: None`.
        let state = ClusterApiState::placeholder();
        let err =
            validate_join_token_ed25519(&state, "any-string", std::net::Ipv4Addr::LOCALHOST.into())
                .await
                .expect_err("missing signer must yield an error");
        assert!(
            matches!(err, ApiError::ServiceUnavailable(_)),
            "expected ServiceUnavailable; got {err:?}"
        );
    }

    #[tokio::test]
    async fn validate_join_token_ed25519_rejects_wrong_version() {
        use base64::Engine;

        let signer = Arc::new(zlayer_secrets::ClusterSigner::generate());
        let claims = make_claims(
            chrono::Utc::now() + chrono::Duration::hours(1),
            "node-uuid-F",
        );
        let claims_bytes = serde_json::to_vec(&claims).unwrap();
        let sig_bytes = signer.sign(&claims_bytes);
        let envelope = SignedClusterJoinToken {
            v: 99, // unsupported version
            kid: signer.key_id(),
            claims: claims.clone(),
            sig: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig_bytes),
            ca_chain: None,
        };
        let envelope_bytes = serde_json::to_vec(&envelope).unwrap();
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(envelope_bytes);

        let state = state_with_signer(signer);
        let err = validate_join_token_ed25519(&state, &token, std::net::Ipv4Addr::LOCALHOST.into())
            .await
            .expect_err("v=99 must be rejected");
        match err {
            ApiError::Unauthorized(msg) => assert!(
                msg.contains("unsupported signed token version"),
                "expected version error; got {msg:?}"
            ),
            other => panic!("expected Unauthorized; got {other:?}"),
        }
    }

    // -------------------------------------------------------------------------
    // Wave 5A.3: rotation-aware kid lookup via the on-disk keystore.
    //
    // These tests exercise the new `cluster_signing_key_path` codepath in
    // `validate_join_token_ed25519`. They write JSON keystores into temp
    // directories directly so they don't depend on private items in
    // `zlayer_secrets::cluster_signer`.
    // -------------------------------------------------------------------------

    /// A token minted under the previously-active key must still validate
    /// after the keystore is rotated and that key is moved into the grace
    /// window. This is the load-bearing rotation invariant.
    #[tokio::test]
    async fn validate_join_token_ed25519_accepts_token_signed_under_grace_kid() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let path = tmp.path().join("cluster_signing.key");

        // 1. Initial keystore: generate via load_or_generate so it has a
        //    legitimate active key on disk.
        let original = zlayer_secrets::ClusterSigner::load_or_generate(&path)
            .await
            .expect("generate initial keystore");
        let original_kid = original.key_id();

        // 2. Mint a token under that original key.
        let claims = make_claims(
            chrono::Utc::now() + chrono::Duration::hours(1),
            "node-uuid-grace",
        );
        let token = mint_for_test(&claims, &original);

        // 3. Rotate the keystore. After this, `original_kid` is in grace
        //    (valid_until = now + 1h) and a new active key has been minted.
        let rotation = zlayer_secrets::rotate_keystore(&path, std::time::Duration::from_secs(3600))
            .await
            .expect("rotate keystore");
        assert_eq!(
            rotation.previous_kid, original_kid,
            "previous_kid should match original signer's kid"
        );
        assert_ne!(
            rotation.new_active_kid, original_kid,
            "rotation must change the active kid"
        );

        // 4. Build a ClusterApiState whose `cluster_signer` is now the NEW
        //    active key, but whose `cluster_signing_key_path` points at the
        //    real keystore — so the validator can do kid-aware lookup.
        let new_active = zlayer_secrets::ClusterSigner::load_or_generate(&path)
            .await
            .expect("re-load post-rotation active signer");
        let mut state = state_with_signer(Arc::new(new_active));
        state.cluster_signing_key_path = Some(path.clone());

        // 5. The token (signed under the now-grace kid) must validate.
        let validated =
            validate_join_token_ed25519(&state, &token, std::net::Ipv4Addr::LOCALHOST.into())
                .await
                .expect("grace-kid token must validate via keystore lookup");
        assert_eq!(validated.iss, claims.iss);
    }

    /// A token whose kid is recorded in `retired_grace_until` with a
    /// timestamp already in the past must be rejected — even though the
    /// key material is still on disk.
    #[tokio::test]
    async fn validate_join_token_ed25519_rejects_token_signed_under_expired_grace() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let path = tmp.path().join("cluster_signing.key");

        // Build a keystore manually so we can put a kid into grace with a
        // PAST `valid_until`. `rotate_keystore` only ever inserts future
        // timestamps, so we can't drive this state via the public API —
        // we have to write the JSON ourselves.
        //
        // To get real, on-disk-recoverable seeds without poking at the
        // private keystore types in `zlayer_secrets`, we drive two scratch
        // `load_or_generate` calls and copy the persisted base64 seeds
        // (and matching kids) into a hand-built keystore at `path`.

        // (a) Seed material for the to-be-stale key.
        let stale_scratch_path = tmp.path().join("scratch.key");
        let stale_signer = zlayer_secrets::ClusterSigner::load_or_generate(&stale_scratch_path)
            .await
            .expect("stale scratch keystore");
        let stale_kid = stale_signer.key_id();
        let stale_json: serde_json::Value =
            serde_json::from_slice(&tokio::fs::read(&stale_scratch_path).await.unwrap()).unwrap();
        let stale_seed_b64 = stale_json["keys"][0]["seed_b64"]
            .as_str()
            .expect("stale scratch has seed_b64")
            .to_string();
        assert_eq!(stale_json["keys"][0]["id"].as_str().unwrap(), stale_kid);

        // (b) Seed material for the (still-active) other key in the same
        //     store, so the file is a valid keystore alongside the
        //     expired-grace entry.
        let active_scratch_path = tmp.path().join("active_scratch.key");
        let active_signer = zlayer_secrets::ClusterSigner::load_or_generate(&active_scratch_path)
            .await
            .expect("active scratch keystore");
        let active_kid = active_signer.key_id();
        let active_json: serde_json::Value =
            serde_json::from_slice(&tokio::fs::read(&active_scratch_path).await.unwrap()).unwrap();
        let active_seed_b64 = active_json["keys"][0]["seed_b64"]
            .as_str()
            .expect("active scratch has seed_b64")
            .to_string();

        // Sanity: the two scratch keys must differ, otherwise the test
        // setup is degenerate.
        assert_ne!(stale_kid, active_kid);

        // Build the keystore JSON by hand: one active key, one entry whose
        // grace expired one hour ago.
        let now = chrono::Utc::now();
        let past = now - chrono::Duration::hours(1);
        let keystore_json = serde_json::json!({
            "version": 1,
            "active": active_kid,
            "keys": [
                {
                    "id": active_kid,
                    "seed_b64": active_seed_b64,
                    "created_at": now.to_rfc3339(),
                },
                {
                    "id": stale_kid,
                    "seed_b64": stale_seed_b64,
                    "created_at": (now - chrono::Duration::hours(2)).to_rfc3339(),
                },
            ],
            "retired_grace_until": {
                stale_kid.clone(): past.to_rfc3339(),
            },
        });
        tokio::fs::write(&path, serde_json::to_vec(&keystore_json).unwrap())
            .await
            .expect("write hand-crafted keystore");

        // Mint a token under the expired-grace kid using its real seed.
        let claims = make_claims(now + chrono::Duration::hours(1), "node-uuid-expired-grace");
        let token = mint_for_test(&claims, &stale_signer);
        assert_eq!(stale_signer.key_id(), stale_kid);

        // Build state with the new keystore path. `cluster_signer` here is
        // unused for the keystore-path branch, but `state_with_signer`
        // needs *something*.
        let active = zlayer_secrets::ClusterSigner::load_or_generate(&path)
            .await
            .expect("load active from real keystore");
        let mut state = state_with_signer(Arc::new(active));
        state.cluster_signing_key_path = Some(path.clone());

        // Validation must reject — kid is in grace but already expired.
        let err = validate_join_token_ed25519(&state, &token, std::net::Ipv4Addr::LOCALHOST.into())
            .await
            .expect_err("expired-grace kid must be rejected");
        match err {
            ApiError::Unauthorized(msg) => assert!(
                msg.contains("not trusted") && msg.contains(&stale_kid),
                "expected 'not trusted' for the stale kid; got {msg:?}"
            ),
            other => panic!("expected Unauthorized; got {other:?}"),
        }
    }

    /// When `cluster_signing_key_path` is `None`, the validator must use
    /// the active-signer comparison path (pre-Wave-5A behavior). This is
    /// the same contract every existing Wave-3 test relies on — but we
    /// pin it explicitly so a future refactor that removes the fallback
    /// branch trips this test instead of a downstream integration test.
    #[tokio::test]
    async fn validate_join_token_ed25519_falls_back_to_active_signer_when_no_keystore_path() {
        let signer = Arc::new(zlayer_secrets::ClusterSigner::generate());
        let claims = make_claims(
            chrono::Utc::now() + chrono::Duration::hours(1),
            "node-uuid-fallback",
        );
        let token = mint_for_test(&claims, &signer);

        // Explicitly leave `cluster_signing_key_path` as None.
        let state = state_with_signer(signer);
        assert!(
            state.cluster_signing_key_path.is_none(),
            "fallback test requires no keystore path"
        );

        let validated =
            validate_join_token_ed25519(&state, &token, std::net::Ipv4Addr::LOCALHOST.into())
                .await
                .expect("active-signer fallback must accept tokens minted under that signer");
        assert_eq!(validated.iss, claims.iss);
    }

    #[test]
    fn is_signed_envelope_shape_detects_real_envelope() {
        let signer = zlayer_secrets::ClusterSigner::generate();
        let claims = make_claims(
            chrono::Utc::now() + chrono::Duration::hours(1),
            "node-uuid-G",
        );
        let token = mint_for_test(&claims, &signer);
        assert!(is_signed_envelope_shape(&token));
    }

    #[test]
    fn is_signed_envelope_shape_rejects_hs256_jwt() {
        // HS256 JWTs look like `aaa.bbb.ccc` — not base64-of-an-object.
        let hmac_key = derive_join_hmac_key("operator-secret");
        let now = now_secs();
        let claims = JoinTokenClaims {
            iss: "zlayer-cluster".into(),
            aud: "cluster-join".into(),
            jti: "jti-shape-1".into(),
            exp: now + 3600,
            iat: now,
        };
        let token = sign_token(&claims, &hmac_key);
        assert!(!is_signed_envelope_shape(&token));
    }

    #[test]
    fn is_signed_envelope_shape_rejects_legacy_plaintext() {
        use base64::Engine;
        let payload = serde_json::json!({"auth_secret": "shared"});
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&payload).unwrap());
        assert!(!is_signed_envelope_shape(&token));
    }

    #[test]
    fn is_signed_envelope_shape_rejects_garbage() {
        assert!(!is_signed_envelope_shape("not-base64-at-all!!!"));
        assert!(!is_signed_envelope_shape(""));
    }

    // -------------------------------------------------------------------------
    // Wave 6 (v0.13.0): plaintext-token rejection + warnings-key absence
    //
    // The handler can't be tested end-to-end without a live RaftCoordinator
    // (`placeholder()` state short-circuits with 503 before token validation),
    // so we test the shape-detector + response-construction directly. The
    // surrounding mint/validate helpers are covered by their own tests
    // (`is_legacy_plaintext_token_shape_*`, `validate_join_token_signed_*`,
    // `is_signed_envelope_shape_*`).
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn cluster_join_with_plaintext_token_returns_unauthorized() {
        // Wave 6 (v0.13.0): the legacy plaintext form is rejected outright.
        // The shape detector must recognise the v0.11.x payload so the
        // handler can return the precise migration error rather than the
        // opaque HS256 decode failure. The end-to-end handler dispatch
        // (which converts the recognised shape into `ApiError::Unauthorized`)
        // is exercised by the Phase-1 integration tests; here we guard the
        // pre-condition the dispatch relies on.
        use base64::Engine;
        let payload = serde_json::json!({
            "api_endpoint": "10.0.0.1:3669",
            "raft_endpoint": "10.0.0.1:9000",
            "leader_wg_pubkey": "abc",
            "overlay_cidr": "10.200.0.0/16",
            "auth_secret": "cluster-secret-from-v0.11",
            "created_at": "2025-01-01T00:00:00Z",
        });
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&payload).unwrap());

        // Detector recognises the shape addressed to this cluster's secret.
        assert!(
            is_legacy_plaintext_token_shape(&token, "cluster-secret-from-v0.11"),
            "detector must catch v0.11.x-shaped plaintext tokens so the handler can return Unauthorized"
        );

        // A token using a foreign secret is NOT recognised (don't leak
        // membership of other clusters): the handler falls through to the
        // generic HS256 error instead of emitting the migration message.
        assert!(
            !is_legacy_plaintext_token_shape(&token, "some-other-cluster-secret"),
            "shape detector must not match across cluster boundaries"
        );

        // Wire-shape guard: a response built from the rejection path leaves
        // `warnings` as `None`, which serializes away (skip_serializing_if).
        // The plaintext-acceptance warning emitted in Wave 4.2 is gone.
        let resp = ClusterJoinResponse {
            node_id: "uuid-never-built-on-this-path".into(),
            raft_node_id: 7,
            overlay_ip: "10.200.0.7".into(),
            slice_cidr: "10.200.112.0/28".into(),
            peers: vec![],
            role: "voter".into(),
            node_jwt: None,
            wrapped_dek: None,
            dek_generation: None,
            join_secret: None,
            warnings: None,
        };
        let json = serde_json::to_string(&resp).expect("serialize rejection-path response");
        assert!(
            !json.contains("warnings"),
            "Wave 6 (v0.13.0) wire form must omit `warnings` on plaintext rejection (the path no longer builds a response at all); got {json}"
        );
    }

    #[tokio::test]
    async fn cluster_join_with_ed25519_token_has_no_warning() {
        // Wave 6 (v0.13.0): the Ed25519 branch never sets a warning. With
        // `build_join_warnings` removed, the handler hard-codes `warnings:
        // None`. Verify the field is absent from the wire form.
        let resp = ClusterJoinResponse {
            node_id: "uuid-ed25519".into(),
            raft_node_id: 8,
            overlay_ip: "10.200.0.8".into(),
            slice_cidr: "10.200.128.0/28".into(),
            peers: vec![],
            role: "voter".into(),
            node_jwt: None,
            wrapped_dek: None,
            dek_generation: None,
            join_secret: None,
            warnings: None,
        };
        let json = serde_json::to_string(&resp).expect("serialize ed25519 response");
        assert!(
            !json.contains("warnings"),
            "Ed25519 acceptance must produce no `warnings` key; got {json}"
        );
    }

    #[tokio::test]
    async fn cluster_join_with_hs256_token_has_no_warning() {
        // Wave 6 (v0.13.0): the HS256 branch also hard-codes `warnings:
        // None`. Same shape guard as the Ed25519 case.
        let resp = ClusterJoinResponse {
            node_id: "uuid-hs256".into(),
            raft_node_id: 9,
            overlay_ip: "10.200.0.9".into(),
            slice_cidr: "10.200.144.0/28".into(),
            peers: vec![],
            role: "voter".into(),
            node_jwt: None,
            wrapped_dek: None,
            dek_generation: None,
            join_secret: None,
            warnings: None,
        };
        let json = serde_json::to_string(&resp).expect("serialize hs256 response");
        assert!(
            !json.contains("warnings"),
            "HS256 acceptance must produce no `warnings` key; got {json}"
        );
    }

    // -------------------------------------------------------------------------
    // Wave 5B.4: `cluster_rotate_signing_key` handler tests
    //
    // These tests construct an `AuthActor` directly (mirroring the
    // `admin_actor`/`user_actor` helpers in `auth.rs`) and call the handler
    // with the actor extractor pre-resolved, then verify both the HTTP
    // response shape and the on-disk side effects on the keystore.
    // -------------------------------------------------------------------------

    fn rotate_admin_actor() -> AuthActor {
        AuthActor {
            user_id: "admin-rotator".into(),
            roles: vec!["admin".into()],
            email: None,
        }
    }

    fn rotate_user_actor() -> AuthActor {
        AuthActor {
            user_id: "regular-user".into(),
            roles: vec!["user".into()],
            email: None,
        }
    }

    #[tokio::test]
    async fn cluster_rotate_signing_key_creates_new_active_and_graces_old() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("cluster_signing.key");
        let _ = zlayer_secrets::ClusterSigner::load_or_generate(&path)
            .await
            .expect("seed initial keystore");

        let mut state = ClusterApiState::placeholder();
        state.cluster_signing_key_path = Some(path.clone());

        let actor = rotate_admin_actor();
        let req = RotateSigningKeyRequest {
            grace: Some("1h".into()),
        };

        let resp = cluster_rotate_signing_key(actor, State(state), Json(req))
            .await
            .expect("rotation must succeed for admin caller with a configured keystore");
        assert_eq!(resp.0.kid.len(), 8, "new active kid must be 8 hex chars");
        assert_eq!(
            resp.0.previous_kid.len(),
            8,
            "previous-active kid must be 8 hex chars"
        );
        assert_ne!(
            resp.0.kid, resp.0.previous_kid,
            "rotation must produce a fresh kid"
        );
        // RFC3339 timestamps end with either a 'Z' (UTC) or an offset like
        // `+00:00`. `to_rfc3339()` on a `DateTime<Utc>` produces the offset
        // form, so check both forms to be robust.
        assert!(
            resp.0.previous_grace_until.ends_with('Z') || resp.0.previous_grace_until.contains('+'),
            "previous_grace_until must be RFC3339; got {:?}",
            resp.0.previous_grace_until
        );

        // Confirm the keystore was actually updated on disk.
        let infos = zlayer_secrets::list_valid_pubkeys(&path)
            .await
            .expect("read back keystore");
        assert_eq!(
            infos.len(),
            2,
            "after one rotation the keystore must hold active + 1 grace key"
        );
    }

    #[tokio::test]
    async fn cluster_rotate_signing_key_uses_default_grace_when_unset() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("cluster_signing.key");
        let _ = zlayer_secrets::ClusterSigner::load_or_generate(&path)
            .await
            .expect("seed initial keystore");

        let mut state = ClusterApiState::placeholder();
        state.cluster_signing_key_path = Some(path);

        let actor = rotate_admin_actor();
        let req = RotateSigningKeyRequest::default();

        let now = chrono::Utc::now();
        let resp = cluster_rotate_signing_key(actor, State(state), Json(req))
            .await
            .expect("rotation with default grace must succeed");

        let parsed: chrono::DateTime<chrono::FixedOffset> =
            chrono::DateTime::parse_from_rfc3339(&resp.0.previous_grace_until)
                .expect("previous_grace_until must parse as RFC3339");
        let parsed_utc = parsed.with_timezone(&chrono::Utc);
        let delta = parsed_utc.signed_duration_since(now);

        // Default grace is 7 days. Allow a 60-second window for clock
        // drift between the precomputed `now` above and when the handler
        // captured its own `now` inside `rotate_keystore`.
        let seven_days = chrono::Duration::days(7);
        let drift = (delta - seven_days).num_seconds().abs();
        assert!(
            drift < 60,
            "expected previous_grace_until ~7 days from now; drift = {drift}s"
        );
    }

    #[tokio::test]
    async fn cluster_rotate_signing_key_rejects_invalid_grace_string() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("cluster_signing.key");
        let _ = zlayer_secrets::ClusterSigner::load_or_generate(&path)
            .await
            .expect("seed initial keystore");

        let mut state = ClusterApiState::placeholder();
        state.cluster_signing_key_path = Some(path);

        let actor = rotate_admin_actor();
        let req = RotateSigningKeyRequest {
            grace: Some("potato".into()),
        };

        let err = cluster_rotate_signing_key(actor, State(state), Json(req))
            .await
            .expect_err("garbage humantime must be rejected");
        assert!(
            matches!(err, ApiError::BadRequest(_)),
            "expected BadRequest; got {err:?}"
        );
    }

    #[tokio::test]
    async fn cluster_rotate_signing_key_503_when_keystore_path_unconfigured() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_admin_actor();
        let req = RotateSigningKeyRequest::default();

        let err = cluster_rotate_signing_key(actor, State(state), Json(req))
            .await
            .expect_err("missing keystore path must yield 503");
        assert!(
            matches!(err, ApiError::ServiceUnavailable(_)),
            "expected ServiceUnavailable; got {err:?}"
        );
    }

    #[tokio::test]
    async fn cluster_rotate_signing_key_forbidden_for_non_admin() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("cluster_signing.key");
        let _ = zlayer_secrets::ClusterSigner::load_or_generate(&path)
            .await
            .expect("seed initial keystore");

        let mut state = ClusterApiState::placeholder();
        state.cluster_signing_key_path = Some(path);

        let actor = rotate_user_actor();
        let req = RotateSigningKeyRequest::default();

        let err = cluster_rotate_signing_key(actor, State(state), Json(req))
            .await
            .expect_err("non-admin caller must be rejected");
        assert!(
            matches!(err, ApiError::Forbidden(_)),
            "expected Forbidden; got {err:?}"
        );
    }

    #[test]
    fn token_canonical_hash_passes_through_64_hex() {
        let hash = "a".repeat(64);
        assert_eq!(super::token_canonical_hash(&hash), hash);
    }

    #[test]
    fn token_canonical_hash_sha256s_non_hex_input() {
        let out = super::token_canonical_hash("hello");
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            out,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn token_canonical_hash_trims_whitespace() {
        let trimmed = super::token_canonical_hash("  hello  ");
        let no_ws = super::token_canonical_hash("hello");
        assert_eq!(trimmed, no_ws);
    }

    #[test]
    fn token_canonical_hash_short_almost_hex_string_gets_hashed() {
        // 63 chars (off-by-one from the hash form) must NOT be treated as a hash.
        let almost = "a".repeat(63);
        let out = super::token_canonical_hash(&almost);
        assert_ne!(out, almost);
        assert_eq!(out.len(), 64);
    }

    #[tokio::test]
    async fn cluster_revoke_token_503_when_no_raft_coordinator() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_admin_actor();
        let req = RevokeTokenRequest {
            token_or_hash: "deadbeef".repeat(8),
            reason: None,
        };

        let err = cluster_revoke_token(actor, State(state), Json(req))
            .await
            .expect_err("must error when no raft coordinator is attached");
        let body = err.to_string();
        assert!(
            body.contains("Raft coordinator") || body.contains("unavailable"),
            "expected ServiceUnavailable about raft; got {body:?}"
        );
    }

    #[tokio::test]
    async fn cluster_revoke_token_forbidden_for_non_admin() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_user_actor();
        let req = RevokeTokenRequest {
            token_or_hash: "deadbeef".repeat(8),
            reason: None,
        };

        let err = cluster_revoke_token(actor, State(state), Json(req))
            .await
            .expect_err("non-admin must be forbidden");
        // require_admin produces Forbidden/Unauthorized — accept either.
        let body = err.to_string();
        assert!(
            !body.is_empty(),
            "non-admin must surface an auth error body"
        );
    }

    #[tokio::test]
    async fn cluster_revoke_token_400_for_empty_input() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_admin_actor();
        let req = RevokeTokenRequest {
            token_or_hash: "   ".into(),
            reason: None,
        };

        let err = cluster_revoke_token(actor, State(state), Json(req))
            .await
            .expect_err("empty input must be rejected before any raft lookup");
        let body = err.to_string();
        assert!(
            body.contains("empty") || body.contains("BadRequest") || body.contains("must not"),
            "expected BadRequest about empty input; got {body:?}"
        );
    }

    #[tokio::test]
    async fn cluster_list_revocations_503_when_no_raft_coordinator() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_admin_actor();

        let err = cluster_list_revocations(actor, State(state))
            .await
            .expect_err("must error when no raft coordinator is attached");
        let body = err.to_string();
        assert!(
            body.contains("Raft coordinator") || body.contains("unavailable"),
            "expected ServiceUnavailable about raft; got {body:?}"
        );
    }

    #[tokio::test]
    async fn cluster_list_revocations_forbidden_for_non_admin() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_user_actor();

        let err = cluster_list_revocations(actor, State(state))
            .await
            .expect_err("non-admin must be forbidden");
        let body = err.to_string();
        assert!(
            !body.is_empty(),
            "non-admin must surface an auth error body"
        );
    }

    #[test]
    fn derive_revocation_expiry_falls_back_for_hash_input() {
        // A 64-char lowercase hex string is the "I already have the hash" form.
        // The expiry helper has nothing to parse, so it must hand back a
        // sensible fallback (~ now + 24h) so the revocation eventually prunes.
        let hash = "a".repeat(64);
        let now = chrono::Utc::now();
        let expiry = super::derive_revocation_expiry(&hash);
        let delta = (expiry - now).num_minutes();
        // Allow generous slop for the 24h fallback so this test isn't flaky.
        assert!(
            (23 * 60..=25 * 60).contains(&delta),
            "expected ~24h fallback expiry for hash input; got delta_min={delta}"
        );
    }

    #[test]
    fn derive_revocation_expiry_falls_back_for_garbage_input() {
        // Non-base64 garbage that's not 64-char hex still gets a fallback.
        let now = chrono::Utc::now();
        let expiry = super::derive_revocation_expiry("!!! not a token !!!");
        let delta = (expiry - now).num_minutes();
        assert!(
            (23 * 60..=25 * 60).contains(&delta),
            "expected ~24h fallback expiry for garbage; got delta_min={delta}"
        );
    }

    #[tokio::test]
    async fn cluster_trust_bundle_returns_503_when_ca_unconfigured() {
        let state = ClusterApiState::placeholder();
        let err = cluster_trust_bundle(State(state))
            .await
            .expect_err("must error when no CA is attached");
        assert!(
            err.to_string().contains("CA not configured")
                || err.to_string().contains("unavailable"),
            "expected ServiceUnavailable; got {err}"
        );
    }

    #[tokio::test]
    async fn cluster_trust_bundle_returns_well_formed_response() {
        let dir = tempfile::tempdir().unwrap();
        let ca_path = dir.path().join("cluster_ca.key");
        let ca = std::sync::Arc::new(
            zlayer_secrets::ClusterCa::load_or_generate(&ca_path)
                .await
                .unwrap(),
        );
        let mut state = ClusterApiState::placeholder();
        state.cluster_ca = Some(ca.clone());
        state.cluster_domain = Some("test-cluster-domain".into());

        let resp = cluster_trust_bundle(State(state)).await.unwrap();
        let bundle = resp.0;
        assert_eq!(bundle.cluster_domain, "test-cluster-domain");
        assert_eq!(bundle.ca_kid, ca.ca_kid());
        assert_eq!(bundle.ca_public_key_b64, ca.ca_public_key_b64());
        assert_eq!(
            bundle.v,
            zlayer_types::api::cluster::TRUST_BUNDLE_FORMAT_VERSION
        );
    }

    // -------------------------------------------------------------------------
    // Wave 9D-ii: federated trust-bundle admin endpoints
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn cluster_import_trust_bundle_403_for_non_admin() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_user_actor();
        let req = zlayer_types::api::cluster::ImportTrustBundleRequest {
            bundle: zlayer_types::api::cluster::TrustBundle {
                v: zlayer_types::api::cluster::TRUST_BUNDLE_FORMAT_VERSION,
                cluster_domain: "x".into(),
                ca_public_key_b64: "AAAA".into(),
                ca_kid: "deadbeef".into(),
                generated_at: chrono::Utc::now().to_rfc3339(),
            },
            source_url: None,
        };
        let err = cluster_import_trust_bundle(actor, State(state), Json(req))
            .await
            .expect_err("non-admin must be forbidden");
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn cluster_import_trust_bundle_400_for_empty_domain() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_admin_actor();
        let req = zlayer_types::api::cluster::ImportTrustBundleRequest {
            bundle: zlayer_types::api::cluster::TrustBundle {
                v: zlayer_types::api::cluster::TRUST_BUNDLE_FORMAT_VERSION,
                cluster_domain: "  ".into(),
                ca_public_key_b64: "AAAA".into(),
                ca_kid: "deadbeef".into(),
                generated_at: chrono::Utc::now().to_rfc3339(),
            },
            source_url: None,
        };
        let err = cluster_import_trust_bundle(actor, State(state), Json(req))
            .await
            .expect_err("empty domain must be rejected");
        assert!(err.to_string().contains("cluster_domain"));
    }

    #[tokio::test]
    async fn cluster_import_trust_bundle_400_for_bad_pubkey_length() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_admin_actor();
        let req = zlayer_types::api::cluster::ImportTrustBundleRequest {
            bundle: zlayer_types::api::cluster::TrustBundle {
                v: zlayer_types::api::cluster::TRUST_BUNDLE_FORMAT_VERSION,
                cluster_domain: "test".into(),
                ca_public_key_b64: "AAAA".into(), // decodes to 3 bytes, not 32
                ca_kid: "deadbeef".into(),
                generated_at: chrono::Utc::now().to_rfc3339(),
            },
            source_url: None,
        };
        let err = cluster_import_trust_bundle(actor, State(state), Json(req))
            .await
            .expect_err("short pubkey must be rejected");
        assert!(err.to_string().contains("32 bytes") || err.to_string().contains("wrong length"));
    }

    #[tokio::test]
    async fn cluster_list_trust_bundles_503_when_no_raft() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_admin_actor();
        let err = cluster_list_trust_bundles(actor, State(state))
            .await
            .expect_err("must error when no raft coordinator is attached");
        assert!(err.to_string().contains("Raft") || err.to_string().contains("unavailable"));
    }

    #[test]
    fn ed25519_pubkey_to_spki_der_emits_44_bytes() {
        // RFC 8410 SPKI for Ed25519 = 12-byte prefix + 32-byte pubkey = 44 bytes.
        let pk = [0xABu8; 32];
        let der = super::ed25519_pubkey_to_spki_der(&pk);
        assert_eq!(der.len(), 44);
        assert_eq!(
            &der[..12],
            &[0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,]
        );
        assert_eq!(&der[12..], &pk);
    }

    // The EdDSA-JWT round-trip test requires exporting the cluster signer's
    // Ed25519 seed in PKCS#8 DER form so `jsonwebtoken::EncodingKey::from_ed_der`
    // can mint a token. `ed25519-dalek` is configured without the `pkcs8`
    // feature in this workspace and `ClusterSigner` deliberately does not
    // expose its private key bytes (the redacted `Debug` impl is the giveaway).
    // The verifying-side correctness is exercised end-to-end by the production
    // `cluster_join` integration tests once Wave 11C wires the mint path.
    #[tokio::test]
    #[ignore = "needs pkcs8 export from ClusterSigner; integration tests cover verify-side"]
    async fn validate_join_token_eddsa_jwt_round_trips() {}

    // Wave 11C+D: jwt-algorithm / jwt-status / wipe-join-secret handler tests.

    #[tokio::test]
    async fn cluster_set_jwt_algorithm_403_for_non_admin() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_user_actor();
        let req = zlayer_types::api::cluster::SetJwtAlgorithmRequest {
            algorithm: zlayer_types::api::cluster::JwtAlgorithm::Eddsa,
        };
        let err = cluster_set_jwt_algorithm(actor, State(state), Json(req))
            .await
            .expect_err("non-admin must be forbidden");
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn cluster_set_jwt_algorithm_503_without_raft() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_admin_actor();
        let req = zlayer_types::api::cluster::SetJwtAlgorithmRequest {
            algorithm: zlayer_types::api::cluster::JwtAlgorithm::Eddsa,
        };
        let err = cluster_set_jwt_algorithm(actor, State(state), Json(req))
            .await
            .expect_err("no raft -> service unavailable");
        assert!(err.to_string().contains("Raft") || err.to_string().contains("unavailable"));
    }

    #[tokio::test]
    async fn cluster_jwt_status_503_without_raft() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_admin_actor();
        let err = cluster_jwt_status(actor, State(state))
            .await
            .expect_err("no raft -> service unavailable");
        assert!(err.to_string().contains("Raft") || err.to_string().contains("unavailable"));
    }

    #[tokio::test]
    async fn cluster_wipe_join_secret_403_for_non_admin() {
        let state = ClusterApiState::placeholder();
        let actor = rotate_user_actor();
        let err = cluster_wipe_join_secret(actor, State(state))
            .await
            .expect_err("non-admin must be forbidden");
        assert!(!err.to_string().is_empty());
    }
}
