//! Node management endpoints

use std::collections::HashMap;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};

/// Node summary for list operations
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NodeSummary {
    /// Node identifier
    pub id: u64,
    /// Node network address
    pub address: String,
    /// Current node status (e.g., "ready", "notready", "disconnected")
    pub status: String,
    /// Node role (e.g., "leader", "worker")
    pub role: String,
    /// Node labels for scheduling
    pub labels: HashMap<String, String>,
    /// Last seen timestamp (Unix timestamp)
    pub last_seen: u64,
}

/// Node resource information
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NodeResourceInfo {
    /// Total CPU cores
    pub cpu_total: f64,
    /// Used CPU cores
    pub cpu_used: f64,
    /// CPU usage percentage
    pub cpu_percent: f64,
    /// Total memory in bytes
    pub memory_total: u64,
    /// Used memory in bytes
    pub memory_used: u64,
    /// Memory usage percentage
    pub memory_percent: f64,
}

/// Detailed node information
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NodeDetails {
    /// Node identifier
    pub id: u64,
    /// Node network address
    pub address: String,
    /// Current node status
    pub status: String,
    /// Node role
    pub role: String,
    /// Node labels for scheduling
    pub labels: HashMap<String, String>,
    /// Last seen timestamp (Unix timestamp)
    pub last_seen: u64,
    /// Node resource information
    pub resources: NodeResourceInfo,
    /// Services running on this node
    pub services: Vec<String>,
    /// When the node was registered (Unix timestamp)
    pub registered_at: u64,
    /// Last heartbeat timestamp (Unix timestamp)
    pub last_heartbeat: u64,
}

/// Request to update node labels
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateLabelsRequest {
    /// Labels to add or update
    pub labels: HashMap<String, String>,
    /// Label keys to remove
    pub remove: Vec<String>,
}

/// Response after updating labels
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateLabelsResponse {
    /// Current labels after the update
    pub labels: HashMap<String, String>,
}

/// Join token response for cluster joining
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JoinTokenResponse {
    /// Join token for authenticating the new node
    pub token: String,
    /// Leader endpoint to connect to
    pub leader_endpoint: String,
    /// Leader's public key for secure communication
    pub leader_public_key: String,
    /// Overlay network CIDR
    pub overlay_cidr: String,
    /// Allocated IP address for the joining node
    pub allocated_ip: String,
    /// Token expiration timestamp (Unix timestamp)
    pub expires_at: u64,
}

/// State for node endpoints
///
/// Placeholder state that will be populated when integrated with the scheduler.
#[derive(Clone)]
pub struct NodeApiState {
    // Placeholder - will be filled when integrated with scheduler
}

impl NodeApiState {
    /// Create a new node API state
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for NodeApiState {
    fn default() -> Self {
        Self::new()
    }
}

/// List all nodes in the cluster
#[utoipa::path(
    get,
    path = "/api/v1/nodes",
    responses(
        (status = 200, description = "List of cluster nodes", body = Vec<NodeSummary>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Nodes"
)]
pub async fn list_nodes(
    _user: AuthUser,
    State(_state): State<NodeApiState>,
) -> Result<Json<Vec<NodeSummary>>> {
    // Placeholder: return empty list until scheduler integration
    Ok(Json(Vec::new()))
}

/// Get detailed information about a specific node
#[utoipa::path(
    get,
    path = "/api/v1/nodes/{id}",
    params(
        ("id" = u64, Path, description = "Node identifier"),
    ),
    responses(
        (status = 200, description = "Node details", body = NodeDetails),
        (status = 404, description = "Node not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Nodes"
)]
pub async fn get_node(
    _user: AuthUser,
    State(_state): State<NodeApiState>,
    Path(id): Path<u64>,
) -> Result<Json<NodeDetails>> {
    // Placeholder: return not found until scheduler integration
    Err(ApiError::NotFound(format!("Node '{}' not found", id)))
}

/// Update labels on a node
#[utoipa::path(
    post,
    path = "/api/v1/nodes/{id}/labels",
    params(
        ("id" = u64, Path, description = "Node identifier"),
    ),
    request_body = UpdateLabelsRequest,
    responses(
        (status = 200, description = "Labels updated", body = UpdateLabelsResponse),
        (status = 404, description = "Node not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - admin role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Nodes"
)]
pub async fn update_node_labels(
    user: AuthUser,
    State(_state): State<NodeApiState>,
    Path(id): Path<u64>,
    Json(_request): Json<UpdateLabelsRequest>,
) -> Result<Json<UpdateLabelsResponse>> {
    // Require admin role
    user.require_role("admin")?;

    // Placeholder: return not found until scheduler integration
    Err(ApiError::NotFound(format!("Node '{}' not found", id)))
}

/// Generate a join token for new nodes to join the cluster
#[utoipa::path(
    post,
    path = "/api/v1/nodes/join-token",
    responses(
        (status = 200, description = "Join token generated", body = JoinTokenResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - admin role required"),
        (status = 503, description = "Service unavailable - cluster not ready"),
    ),
    security(("bearer_auth" = [])),
    tag = "Nodes"
)]
pub async fn generate_join_token(
    user: AuthUser,
    State(_state): State<NodeApiState>,
) -> Result<Json<JoinTokenResponse>> {
    // Require admin role
    user.require_role("admin")?;

    // Placeholder: return service unavailable until cluster management is implemented
    Err(ApiError::ServiceUnavailable(
        "Cluster management not yet available".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_summary_serialize() {
        let summary = NodeSummary {
            id: 1,
            address: "192.168.1.10:9090".to_string(),
            status: "ready".to_string(),
            role: "worker".to_string(),
            labels: HashMap::from([("zone".to_string(), "us-east-1".to_string())]),
            last_seen: 1706745600,
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("192.168.1.10"));
        assert!(json.contains("ready"));
        assert!(json.contains("worker"));
    }

    #[test]
    fn test_node_details_serialize() {
        let details = NodeDetails {
            id: 1,
            address: "192.168.1.10:9090".to_string(),
            status: "ready".to_string(),
            role: "worker".to_string(),
            labels: HashMap::new(),
            last_seen: 1706745600,
            resources: NodeResourceInfo {
                cpu_total: 4.0,
                cpu_used: 1.5,
                cpu_percent: 37.5,
                memory_total: 8589934592,
                memory_used: 4294967296,
                memory_percent: 50.0,
            },
            services: vec!["api".to_string(), "worker".to_string()],
            registered_at: 1706659200,
            last_heartbeat: 1706745600,
        };
        let json = serde_json::to_string(&details).unwrap();
        assert!(json.contains("resources"));
        assert!(json.contains("services"));
        assert!(json.contains("cpu_total"));
    }

    #[test]
    fn test_update_labels_request_deserialize() {
        let json = r#"{"labels": {"zone": "us-west-2"}, "remove": ["old-label"]}"#;
        let request: UpdateLabelsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.labels.get("zone"), Some(&"us-west-2".to_string()));
        assert_eq!(request.remove, vec!["old-label".to_string()]);
    }

    #[test]
    fn test_join_token_response_serialize() {
        let response = JoinTokenResponse {
            token: "abc123".to_string(),
            leader_endpoint: "192.168.1.1:9090".to_string(),
            leader_public_key: "pubkey".to_string(),
            overlay_cidr: "10.0.0.0/16".to_string(),
            allocated_ip: "10.0.1.5".to_string(),
            expires_at: 1706832000,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("abc123"));
        assert!(json.contains("overlay_cidr"));
    }

    #[test]
    fn test_node_api_state_default() {
        let _state = NodeApiState::default();
    }
}
