//! Cluster management endpoints
//!
//! Provides the `/api/v1/cluster/join` endpoint for new nodes joining
//! the cluster, and `/api/v1/cluster/nodes` for listing cluster members.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};
use utoipa::ToSchema;
use zlayer_overlay::IpAllocator;

use crate::error::{ApiError, Result};
use zlayer_scheduler::{AddMemberParams, RaftCoordinator};

// =============================================================================
// Types
// =============================================================================

/// Request body for `POST /api/v1/cluster/join`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ClusterJoinRequest {
    /// Base64-encoded join token (contains auth_secret for validation)
    pub token: String,
    /// Joining node's advertise address (IP)
    pub advertise_addr: String,
    /// Joining node's overlay port (WireGuard)
    pub overlay_port: u16,
    /// Joining node's Raft RPC port
    pub raft_port: u16,
    /// Joining node's API server port
    #[serde(default = "default_api_port")]
    pub api_port: u16,
    /// Joining node's WireGuard public key
    pub wg_public_key: String,
    /// Node mode: "full" or "replicate"
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Services to replicate (only if mode == "replicate")
    pub services: Option<Vec<String>>,
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
    /// WireGuard public key
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
}

impl ClusterApiState {
    /// Create a new cluster API state.
    ///
    /// `raft` may be `None` if the coordinator was not available.
    /// `join_secret` is the auth_secret that must appear in the join token.
    /// `ip_allocator` is a CIDR-aware allocator for overlay IPs.
    /// `ip_allocator_path` is the file path used to persist allocator state.
    pub fn new(
        raft: Option<Arc<RaftCoordinator>>,
        join_secret: Option<String>,
        ip_allocator: Arc<RwLock<IpAllocator>>,
        ip_allocator_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            raft,
            next_raft_id: Arc::new(AtomicU64::new(2)),
            join_secret,
            ip_allocator,
            ip_allocator_path,
        }
    }

    /// Create a placeholder state (no Raft).
    ///
    /// Uses the default overlay CIDR `10.200.0.0/16`.
    pub fn placeholder() -> Self {
        let allocator = IpAllocator::new(zlayer_overlay::DEFAULT_OVERLAY_CIDR)
            .expect("default overlay CIDR must be valid");
        Self {
            raft: None,
            next_raft_id: Arc::new(AtomicU64::new(2)),
            join_secret: None,
            ip_allocator: Arc::new(RwLock::new(allocator)),
            ip_allocator_path: None,
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
    raft.add_member(AddMemberParams {
        node_id: raft_node_id,
        addr: raft_addr,
        wg_public_key: req.wg_public_key.clone(),
        overlay_ip: overlay_ip.clone(),
        overlay_port: req.overlay_port,
        advertise_addr: req.advertise_addr.clone(),
        api_port: req.api_port,
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

    Ok(Json(ClusterJoinResponse {
        node_id: node_uuid,
        raft_node_id,
        overlay_ip,
        peers,
    }))
}

/// List all nodes visible in the Raft cluster state.
///
/// `GET /api/v1/cluster/nodes`
pub async fn cluster_list_nodes(
    State(state): State<ClusterApiState>,
) -> Result<Json<Vec<ClusterNodeSummary>>> {
    let raft = match &state.raft {
        Some(r) => r,
        None => {
            // No Raft -- return empty
            return Ok(Json(Vec::new()));
        }
    };

    let cluster_state = raft.read_state().await;
    let leader_id = raft.leader_id();

    let nodes: Vec<ClusterNodeSummary> = cluster_state
        .nodes
        .values()
        .map(|n| ClusterNodeSummary {
            id: format!("{}", n.node_id),
            address: n.address.clone(),
            status: "ready".to_string(),
            mode: if Some(n.node_id) == leader_id {
                "leader".to_string()
            } else {
                "worker".to_string()
            },
            services: Vec::new(),
            is_leader: Some(n.node_id) == leader_id,
        })
        .collect();

    Ok(Json(nodes))
}

// =============================================================================
// Helpers
// =============================================================================

/// Validate a join token by decoding the base64 payload and checking auth_secret.
fn validate_join_token(token: &str, expected_secret: &str) -> bool {
    use base64::Engine;

    let decoded = match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token) {
        Ok(d) => d,
        Err(_) => {
            // Try standard base64 as fallback
            match base64::engine::general_purpose::STANDARD.decode(token) {
                Ok(d) => d,
                Err(_) => {
                    warn!("Join token is not valid base64");
                    return false;
                }
            }
        }
    };

    let value: serde_json::Value = match serde_json::from_slice(&decoded) {
        Ok(v) => v,
        Err(_) => {
            warn!("Join token payload is not valid JSON");
            return false;
        }
    };

    match value.get("auth_secret").and_then(|v| v.as_str()) {
        Some(secret) => secret == expected_secret,
        None => {
            warn!("Join token missing auth_secret field");
            false
        }
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
    }

    #[test]
    fn test_cluster_join_response_serialize() {
        let resp = ClusterJoinResponse {
            node_id: "uuid-123".into(),
            raft_node_id: 2,
            overlay_ip: "10.200.0.2".into(),
            peers: vec![],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("uuid-123"));
        assert!(json.contains("10.200.0.2"));
    }
}
