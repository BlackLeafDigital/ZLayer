//! Container bridge / overlay network API DTOs.
//!
//! Wire types for the `/api/v1/container-networks` endpoint family.
//! Server-side state, the runtime trait, and handler functions live in
//! `zlayer-api`; this module is the SDK-facing shape only.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::spec::{BridgeNetwork, BridgeNetworkAttachment, BridgeNetworkDriver};

/// Body for `POST /api/v1/container-networks`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CreateBridgeNetworkRequest {
    /// Network name (must match `^[a-z0-9][a-z0-9_-]{0,63}$`).
    pub name: String,
    /// Driver, defaults to `bridge`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<BridgeNetworkDriver>,
    /// Subnet CIDR (e.g. `"10.240.0.0/24"`). Validated as
    /// [`ipnetwork::IpNetwork`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subnet: Option<String>,
    /// Arbitrary labels.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    /// Internal-only (no egress) network.
    #[serde(default)]
    pub internal: bool,
}

/// Response body for `GET /api/v1/container-networks/{id_or_name}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct BridgeNetworkDetails {
    /// Core network metadata. Flattened so the JSON stays close to the
    /// list-item shape returned by `list_container_networks`.
    #[serde(flatten)]
    pub network: BridgeNetwork,
    /// Containers currently attached to the network.
    pub attached_containers: Vec<BridgeNetworkAttachment>,
}

/// Query parameters for list/delete.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::IntoParams))]
pub struct ListBridgeNetworksQuery {
    /// Optional label filter in `key=value` form. Only networks whose
    /// labels contain a matching pair are returned.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::IntoParams))]
pub struct DeleteBridgeNetworkQuery {
    /// If true, delete even if the network still has attachments.
    #[serde(default)]
    pub force: bool,
}

/// Body for `POST /api/v1/container-networks/{id_or_name}/connect`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ConnectBridgeNetworkRequest {
    /// Container id to attach.
    pub container_id: String,
    /// Optional DNS aliases on this network.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Optional static IPv4 to pin this container to. Validated as
    /// [`std::net::Ipv4Addr`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4_address: Option<String>,
}

/// Body for `POST /api/v1/container-networks/{id_or_name}/disconnect`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct DisconnectBridgeNetworkRequest {
    /// Container id to detach.
    pub container_id: String,
    /// If true, the runtime is asked to forcibly detach.
    #[serde(default)]
    pub force: bool,
}
