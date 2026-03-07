//! Cluster management endpoints
//!
//! Provides the `/api/v1/cluster/join` endpoint for new nodes joining
//! the cluster, and `/api/v1/cluster/nodes` for listing cluster members.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use utoipa::ToSchema;
use zlayer_overlay::IpAllocator;

use crate::error::{ApiError, Result};
use crate::handlers::internal::{InternalAddPeerRequest, INTERNAL_AUTH_HEADER};
use zlayer_scheduler::{AddMemberParams, RaftCoordinator};

// =============================================================================
// Types
// =============================================================================

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
    pub gpus: Vec<zlayer_scheduler::raft::GpuInfoSummary>,
}

fn default_mode() -> String {
    "full".to_string()
}

fn default_api_port() -> u16 {
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
    /// Existing peers in the cluster
    pub peers: Vec<ClusterPeer>,
    /// Role assigned to this node: "voter" or "learner"
    pub role: String,
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
    /// Network address
    pub address: String,
    /// Current status (e.g. "ready", "unknown")
    pub status: String,
    /// Role: "leader" or "worker"
    pub mode: String,
    /// Services running on this node
    pub services: Vec<String>,
    /// Whether this node is the Raft leader
    pub is_leader: bool,
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
    /// IP allocator for overlay network addresses (CIDR-aware, collision-safe)
    pub ip_allocator: Arc<RwLock<IpAllocator>>,
    /// Path for persisting IP allocator state
    ip_allocator_path: Option<std::path::PathBuf>,
    /// Internal shared secret for authenticating peer-broadcast calls to existing nodes.
    /// Required for the leader to POST `/api/v1/internal/add-peer` on other nodes.
    pub internal_token: Option<String>,
    /// Data directory for Raft storage and recovery markers.
    pub data_dir: Option<std::path::PathBuf>,
}

impl ClusterApiState {
    /// Create a new cluster API state.
    ///
    /// `raft` may be `None` if the coordinator was not available.
    /// `join_secret` is the `auth_secret` that must appear in the join token.
    /// `ip_allocator` is a CIDR-aware allocator for overlay IPs.
    /// `ip_allocator_path` is the file path used to persist allocator state.
    /// `data_dir` is the Raft data directory for recovery markers.
    pub fn new(
        raft: Option<Arc<RaftCoordinator>>,
        join_secret: Option<String>,
        ip_allocator: Arc<RwLock<IpAllocator>>,
        ip_allocator_path: Option<std::path::PathBuf>,
        data_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            raft,
            next_raft_id: Arc::new(AtomicU64::new(2)),
            join_secret,
            ip_allocator,
            ip_allocator_path,
            internal_token: None,
            data_dir,
        }
    }

    /// Create a new cluster API state with an internal token for peer broadcasting.
    pub fn with_internal_token(
        raft: Option<Arc<RaftCoordinator>>,
        join_secret: Option<String>,
        ip_allocator: Arc<RwLock<IpAllocator>>,
        ip_allocator_path: Option<std::path::PathBuf>,
        internal_token: String,
        data_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            raft,
            next_raft_id: Arc::new(AtomicU64::new(2)),
            join_secret,
            ip_allocator,
            ip_allocator_path,
            internal_token: Some(internal_token),
            data_dir,
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
        Self {
            raft: None,
            next_raft_id: Arc::new(AtomicU64::new(2)),
            join_secret: None,
            ip_allocator: Arc::new(RwLock::new(allocator)),
            ip_allocator_path: None,
            internal_token: None,
            data_dir: None,
        }
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

    // 2. Validate the join token (if a secret is configured)
    if let Some(expected_secret) = &state.join_secret {
        // The token is base64-encoded JSON containing an auth_secret field.
        // We decode and verify.
        let valid = validate_join_token(&req.token, expected_secret);
        if !valid {
            return Err(ApiError::Unauthorized("Invalid join token".into()));
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

    Ok(Json(ClusterJoinResponse {
        node_id: node_uuid,
        raft_node_id,
        overlay_ip,
        peers,
        role: match role {
            zlayer_scheduler::raft::MemberRole::Voter => "voter".to_string(),
            zlayer_scheduler::raft::MemberRole::Learner => "learner".to_string(),
        },
    }))
}

/// List all nodes visible in the Raft cluster state.
///
/// `GET /api/v1/cluster/nodes`
///
/// # Errors
///
/// Returns an error if the cluster state cannot be read.
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

    let nodes: Vec<ClusterNodeSummary> = cluster_state
        .nodes
        .values()
        .map(|n| {
            let mode = if Some(n.node_id) == leader_id {
                "leader".to_string()
            } else if voter_ids.contains(&n.node_id) {
                "voter".to_string()
            } else {
                "learner".to_string()
            };

            ClusterNodeSummary {
                id: format!("{}", n.node_id),
                address: n.address.clone(),
                status: n.status.clone(),
                mode,
                services: Vec::new(),
                is_leader: Some(n.node_id) == leader_id,
            }
        })
        .collect();

    Ok(Json(nodes))
}

/// Heartbeat request from a worker node.
#[derive(Debug, Deserialize)]
pub struct HeartbeatRequest {
    pub node_id: u64,
    pub cpu_used: f64,
    pub memory_used: u64,
    pub disk_used: u64,
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
// Force-leader types and handler
// =============================================================================

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
// Helpers
// =============================================================================

/// Validate a join token by decoding the base64 payload and checking `auth_secret`.
fn validate_join_token(token: &str, expected_secret: &str) -> bool {
    use base64::Engine;

    let decoded = match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token) {
        Ok(d) => d,
        Err(_) => {
            // Try standard base64 as fallback
            if let Ok(d) = base64::engine::general_purpose::STANDARD.decode(token) {
                d
            } else {
                warn!("Join token is not valid base64");
                return false;
            }
        }
    };

    let value: serde_json::Value = if let Ok(v) = serde_json::from_slice(&decoded) {
        v
    } else {
        warn!("Join token payload is not valid JSON");
        return false;
    };

    if let Some(secret) = value.get("auth_secret").and_then(|v| v.as_str()) {
        secret == expected_secret
    } else {
        warn!("Join token missing auth_secret field");
        false
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_join_token_valid() {
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

        assert!(validate_join_token(&token, "my-secret-123"));
        assert!(!validate_join_token(&token, "wrong-secret"));
    }

    #[test]
    fn test_validate_join_token_invalid_base64() {
        assert!(!validate_join_token("not-valid!!!", "secret"));
    }

    #[test]
    fn test_validate_join_token_invalid_json() {
        use base64::Engine;
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("not json");
        assert!(!validate_join_token(&token, "secret"));
    }

    #[test]
    fn test_validate_join_token_missing_field() {
        use base64::Engine;
        let payload = serde_json::json!({"foo": "bar"});
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&payload).unwrap());
        assert!(!validate_join_token(&token, "secret"));
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
            peers: vec![],
            role: "voter".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("uuid-123"));
        assert!(json.contains("10.200.0.2"));
    }
}
