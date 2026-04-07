//! Network management endpoints
//!
//! Provides CRUD operations for network access-control groups.
//! Networks define membership (users, groups, nodes, CIDRs) and access rules
//! that control which services members can reach.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;
use utoipa::ToSchema;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use zlayer_spec::NetworkPolicySpec;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Shared state for network endpoints.
#[derive(Clone)]
pub struct NetworkApiState {
    /// In-memory store of network definitions.
    pub networks: Arc<RwLock<Vec<NetworkPolicySpec>>>,
}

impl NetworkApiState {
    /// Create a new, empty network state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            networks: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for NetworkApiState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Response / request wrappers
// ---------------------------------------------------------------------------

/// Summary returned when listing networks.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NetworkSummary {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub cidr_count: usize,
    pub member_count: usize,
    pub rule_count: usize,
}

impl From<&NetworkPolicySpec> for NetworkSummary {
    fn from(spec: &NetworkPolicySpec) -> Self {
        Self {
            name: spec.name.clone(),
            description: spec.description.clone(),
            cidr_count: spec.cidrs.len(),
            member_count: spec.members.len(),
            rule_count: spec.access_rules.len(),
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// List all networks.
///
/// Returns a summary of every network defined in the system.
///
/// # Errors
///
/// Returns an error if the user is not authenticated.
#[utoipa::path(
    get,
    path = "/api/v1/networks",
    responses(
        (status = 200, description = "List of networks", body = Vec<NetworkSummary>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Networks"
)]
pub async fn list_networks(
    _user: AuthUser,
    State(state): State<NetworkApiState>,
) -> Result<Json<Vec<NetworkSummary>>> {
    let networks = state.networks.read().await;
    let summaries: Vec<NetworkSummary> = networks.iter().map(NetworkSummary::from).collect();
    Ok(Json(summaries))
}

/// Get a specific network by name.
///
/// Returns the full `NetworkPolicySpec` for the named network.
///
/// # Errors
///
/// Returns an error if the network is not found or the user is not authenticated.
#[utoipa::path(
    get,
    path = "/api/v1/networks/{name}",
    params(
        ("name" = String, Path, description = "Network name"),
    ),
    responses(
        (status = 200, description = "Network details", body = serde_json::Value),
        (status = 404, description = "Network not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Networks"
)]
pub async fn get_network(
    _user: AuthUser,
    State(state): State<NetworkApiState>,
    Path(name): Path<String>,
) -> Result<Json<NetworkPolicySpec>> {
    let networks = state.networks.read().await;
    let net = networks
        .iter()
        .find(|n| n.name == name)
        .ok_or_else(|| ApiError::NotFound(format!("Network '{name}' not found")))?;
    Ok(Json(net.clone()))
}

/// Create a new network.
///
/// The network name must be unique. Returns the created spec.
///
/// # Errors
///
/// Returns an error if validation fails, the network already exists, or the
/// user lacks the operator role.
#[utoipa::path(
    post,
    path = "/api/v1/networks",
    request_body = serde_json::Value,
    responses(
        (status = 201, description = "Network created", body = serde_json::Value),
        (status = 400, description = "Invalid request"),
        (status = 409, description = "Network already exists"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer_auth" = [])),
    tag = "Networks"
)]
pub async fn create_network(
    user: AuthUser,
    State(state): State<NetworkApiState>,
    Json(spec): Json<NetworkPolicySpec>,
) -> Result<(StatusCode, Json<NetworkPolicySpec>)> {
    user.require_role("operator")?;

    if spec.name.is_empty() {
        return Err(ApiError::BadRequest(
            "Network name cannot be empty".to_string(),
        ));
    }

    if spec.name.len() > 256 {
        return Err(ApiError::BadRequest(
            "Network name cannot exceed 256 characters".to_string(),
        ));
    }

    let mut networks = state.networks.write().await;

    if networks.iter().any(|n| n.name == spec.name) {
        return Err(ApiError::Conflict(format!(
            "Network '{}' already exists",
            spec.name
        )));
    }

    info!(name = %spec.name, "Creating network");
    networks.push(spec.clone());

    Ok((StatusCode::CREATED, Json(spec)))
}

/// Update an existing network.
///
/// Replaces the network definition for the given name. The name in the URL
/// must match the name in the body.
///
/// # Errors
///
/// Returns an error if the network is not found, the URL name does not match
/// the body, or the user lacks the operator role.
#[utoipa::path(
    put,
    path = "/api/v1/networks/{name}",
    params(
        ("name" = String, Path, description = "Network name"),
    ),
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Network updated", body = serde_json::Value),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Network not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer_auth" = [])),
    tag = "Networks"
)]
pub async fn update_network(
    user: AuthUser,
    State(state): State<NetworkApiState>,
    Path(name): Path<String>,
    Json(spec): Json<NetworkPolicySpec>,
) -> Result<Json<NetworkPolicySpec>> {
    user.require_role("operator")?;

    if spec.name != name {
        return Err(ApiError::BadRequest(format!(
            "URL name '{name}' does not match body name '{}'",
            spec.name
        )));
    }

    let mut networks = state.networks.write().await;

    let entry = networks
        .iter_mut()
        .find(|n| n.name == name)
        .ok_or_else(|| ApiError::NotFound(format!("Network '{name}' not found")))?;

    info!(name = %name, "Updating network");
    *entry = spec.clone();

    Ok(Json(spec))
}

/// Delete a network.
///
/// Permanently removes a network definition.
///
/// # Errors
///
/// Returns an error if the network is not found or the user lacks the operator role.
#[utoipa::path(
    delete,
    path = "/api/v1/networks/{name}",
    params(
        ("name" = String, Path, description = "Network name"),
    ),
    responses(
        (status = 204, description = "Network deleted"),
        (status = 404, description = "Network not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer_auth" = [])),
    tag = "Networks"
)]
pub async fn delete_network(
    user: AuthUser,
    State(state): State<NetworkApiState>,
    Path(name): Path<String>,
) -> Result<StatusCode> {
    user.require_role("operator")?;

    let mut networks = state.networks.write().await;

    let idx = networks
        .iter()
        .position(|n| n.name == name)
        .ok_or_else(|| ApiError::NotFound(format!("Network '{name}' not found")))?;

    info!(name = %name, "Deleting network");
    networks.remove(idx);

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Helper: access-rule matching
// ---------------------------------------------------------------------------

/// Check whether `source_ip` falls within any of the CIDRs defined in `spec`.
///
/// This is a simple textual prefix match on the parsed network portion.
/// For production use, prefer a proper CIDR library, but this is sufficient
/// for unit-testable matching logic.
#[must_use]
pub fn ip_matches_network(source_ip: &str, spec: &NetworkPolicySpec) -> bool {
    let Ok(addr) = source_ip.parse::<std::net::IpAddr>() else {
        return false;
    };

    for cidr_str in &spec.cidrs {
        if let Some((net_str, prefix_str)) = cidr_str.split_once('/') {
            let Ok(net_addr) = net_str.parse::<std::net::IpAddr>() else {
                continue;
            };
            let Ok(prefix_len) = prefix_str.parse::<u32>() else {
                continue;
            };
            if cidr_contains(net_addr, prefix_len, addr) {
                return true;
            }
        }
    }

    false
}

/// Returns `true` if `addr` is within the CIDR `network/prefix_len`.
fn cidr_contains(network: std::net::IpAddr, prefix_len: u32, addr: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match (network, addr) {
        (IpAddr::V4(net), IpAddr::V4(ip)) => {
            let prefix_len = prefix_len.min(32);
            if prefix_len == 0 {
                return true;
            }
            let mask = u32::MAX.checked_shl(32 - prefix_len).unwrap_or(0);
            (u32::from(net) & mask) == (u32::from(ip) & mask)
        }
        (IpAddr::V6(net), IpAddr::V6(ip)) => {
            let prefix_len = prefix_len.min(128);
            if prefix_len == 0 {
                return true;
            }
            let mask = u128::MAX.checked_shl(128 - prefix_len).unwrap_or(0);
            (u128::from(net) & mask) == (u128::from(ip) & mask)
        }
        _ => false, // v4 vs v6 mismatch
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_spec::{AccessAction, AccessRule, MemberKind, NetworkMember};

    #[test]
    fn test_network_summary_from_spec() {
        let spec = NetworkPolicySpec {
            name: "test-net".to_string(),
            description: Some("A test network".to_string()),
            cidrs: vec!["10.0.0.0/8".to_string()],
            members: vec![
                NetworkMember {
                    name: "alice".to_string(),
                    kind: MemberKind::User,
                },
                NetworkMember {
                    name: "ops".to_string(),
                    kind: MemberKind::Group,
                },
            ],
            access_rules: vec![AccessRule {
                service: "api".to_string(),
                deployment: "*".to_string(),
                ports: Some(vec![443]),
                action: AccessAction::Allow,
            }],
        };

        let summary = NetworkSummary::from(&spec);
        assert_eq!(summary.name, "test-net");
        assert_eq!(summary.description.as_deref(), Some("A test network"));
        assert_eq!(summary.cidr_count, 1);
        assert_eq!(summary.member_count, 2);
        assert_eq!(summary.rule_count, 1);
    }

    #[tokio::test]
    async fn test_create_list_get_delete_roundtrip() {
        let state = NetworkApiState::new();

        // Start empty
        {
            let networks = state.networks.read().await;
            assert!(networks.is_empty());
        }

        // Create
        let spec = NetworkPolicySpec {
            name: "corp-vpn".to_string(),
            description: Some("Corporate VPN".to_string()),
            cidrs: vec!["10.200.0.0/16".to_string()],
            members: vec![NetworkMember {
                name: "alice".to_string(),
                kind: MemberKind::User,
            }],
            access_rules: vec![AccessRule {
                service: "*".to_string(),
                deployment: "*".to_string(),
                ports: None,
                action: AccessAction::Allow,
            }],
        };

        {
            let mut networks = state.networks.write().await;
            networks.push(spec.clone());
        }

        // List
        {
            let networks = state.networks.read().await;
            assert_eq!(networks.len(), 1);
            assert_eq!(networks[0].name, "corp-vpn");
        }

        // Get
        {
            let networks = state.networks.read().await;
            let found = networks.iter().find(|n| n.name == "corp-vpn");
            assert!(found.is_some());
            assert_eq!(found.unwrap().cidrs, vec!["10.200.0.0/16"]);
        }

        // Delete
        {
            let mut networks = state.networks.write().await;
            let idx = networks.iter().position(|n| n.name == "corp-vpn").unwrap();
            networks.remove(idx);
        }

        // Verify empty
        {
            let networks = state.networks.read().await;
            assert!(networks.is_empty());
        }
    }

    #[test]
    fn test_ip_matches_network_v4() {
        let spec = NetworkPolicySpec {
            name: "test".to_string(),
            cidrs: vec!["10.200.0.0/16".to_string(), "192.168.1.0/24".to_string()],
            ..Default::default()
        };

        // Inside first CIDR
        assert!(ip_matches_network("10.200.5.42", &spec));
        assert!(ip_matches_network("10.200.0.1", &spec));
        assert!(ip_matches_network("10.200.255.255", &spec));

        // Inside second CIDR
        assert!(ip_matches_network("192.168.1.100", &spec));

        // Outside both
        assert!(!ip_matches_network("10.201.0.1", &spec));
        assert!(!ip_matches_network("192.168.2.1", &spec));
        assert!(!ip_matches_network("172.16.0.1", &spec));
    }

    #[test]
    fn test_ip_matches_network_v6() {
        let spec = NetworkPolicySpec {
            name: "v6-net".to_string(),
            cidrs: vec!["fd00::/64".to_string()],
            ..Default::default()
        };

        assert!(ip_matches_network("fd00::1", &spec));
        assert!(ip_matches_network("fd00::ffff", &spec));
        assert!(!ip_matches_network("fd01::1", &spec));
    }

    #[test]
    fn test_ip_matches_network_invalid_input() {
        let spec = NetworkPolicySpec {
            name: "test".to_string(),
            cidrs: vec!["10.0.0.0/8".to_string()],
            ..Default::default()
        };

        assert!(!ip_matches_network("not-an-ip", &spec));
        assert!(!ip_matches_network("", &spec));
    }

    #[test]
    fn test_ip_matches_network_empty_cidrs() {
        let spec = NetworkPolicySpec {
            name: "empty".to_string(),
            cidrs: vec![],
            ..Default::default()
        };

        assert!(!ip_matches_network("10.0.0.1", &spec));
    }

    #[test]
    fn test_cidr_contains_edge_cases() {
        use std::net::IpAddr;

        // /0 matches everything
        let net: IpAddr = "0.0.0.0".parse().unwrap();
        let addr: IpAddr = "192.168.1.1".parse().unwrap();
        assert!(cidr_contains(net, 0, addr));

        // /32 matches exact IP
        let net: IpAddr = "10.0.0.1".parse().unwrap();
        let exact: IpAddr = "10.0.0.1".parse().unwrap();
        let other: IpAddr = "10.0.0.2".parse().unwrap();
        assert!(cidr_contains(net, 32, exact));
        assert!(!cidr_contains(net, 32, other));

        // v4 vs v6 mismatch
        let v4: IpAddr = "10.0.0.0".parse().unwrap();
        let v6: IpAddr = "::1".parse().unwrap();
        assert!(!cidr_contains(v4, 8, v6));
    }
}
