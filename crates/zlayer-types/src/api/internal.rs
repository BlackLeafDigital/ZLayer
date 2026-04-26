//! Internal API DTOs for scheduler-to-agent communication.
//!
//! These types describe the request/response payloads for the internal
//! endpoints used by the distributed scheduler to trigger operations on
//! agents. They use a shared secret for authentication rather than JWT
//! tokens.

use serde::{Deserialize, Serialize};
#[cfg(feature = "utoipa")]
use utoipa::ToSchema;

/// Request to scale a service
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct InternalScaleRequest {
    /// Service name to scale
    pub service: String,
    /// Target replica count
    pub replicas: u32,
}

/// Response from internal scale operation
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct InternalAddPeerRequest {
    /// New peer's `WireGuard` public key (base64)
    pub wg_public_key: String,
    /// New peer's overlay IP (e.g. "10.200.0.3")
    pub overlay_ip: String,
    /// New peer's `WireGuard` endpoint (e.g. "203.0.113.5:51820")
    pub endpoint: String,
}

/// Response from internal add-peer operation
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct InternalAddPeerResponse {
    /// Whether the operation succeeded
    pub success: bool,
    /// Optional message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
