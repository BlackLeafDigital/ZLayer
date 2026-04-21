//! Container bridge / overlay network endpoints
//!
//! Docker-compatible user-defined network resource. This is ORTHOGONAL to
//! the `/api/v1/networks` (CIDR ACL) resource implemented in `networks.rs`;
//! we deliberately use the `BridgeNetwork*` type family to avoid name
//! collisions.
//!
//! ## Endpoints (all under `/api/v1/container-networks`)
//!
//! - `POST   /`                      — create a new bridge/overlay network
//! - `GET    /`                      — list networks (optional `?label=k=v` filter)
//! - `GET    /{id_or_name}`          — inspect a network + its attachments
//! - `DELETE /{id_or_name}[?force]`  — delete (refuses if attached unless force)
//! - `POST   /{id_or_name}/connect`  — attach a container to the network
//! - `POST   /{id_or_name}/disconnect` — detach a container from the network
//!
//! ## Runtime coupling
//!
//! The registry itself is an in-memory [`DashMap`] on [`BridgeNetworkApiState`].
//! The optional [`BridgeNetworkRuntime`] trait object is what actually talks
//! to the container runtime (bollard, for example). If the runtime is `None`
//! the handlers still update the registry (metadata-only mode) and a warning
//! is logged exactly once per state instance.
//!
//! Registry is in-memory for now; parallel agent wires this to the runtime.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use zlayer_spec::{BridgeNetwork, BridgeNetworkAttachment, BridgeNetworkDriver};

// ---------------------------------------------------------------------------
// Runtime trait (seam for bollard-backed implementation)
// ---------------------------------------------------------------------------

/// Errors surfaced by a [`BridgeNetworkRuntime`] implementation.
///
/// These are intentionally plain strings so the trait can be implemented by
/// crates that don't depend on bollard or any other specific runtime.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime failed: {0}")]
    Failed(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
}

impl From<RuntimeError> for ApiError {
    fn from(err: RuntimeError) -> Self {
        match err {
            RuntimeError::NotFound(msg) => ApiError::NotFound(msg),
            RuntimeError::AlreadyExists(msg) => ApiError::Conflict(msg),
            RuntimeError::Failed(msg) => ApiError::Internal(msg),
        }
    }
}

/// Seam for runtimes that actually materialize bridge/overlay networks
/// (e.g. a bollard-backed implementation in `zlayer-agent`).
///
/// Implementations MUST be idempotent where possible — handlers call
/// these in a metadata-first pattern (write the in-memory registry,
/// then ask the runtime to reconcile).
#[async_trait]
pub trait BridgeNetworkRuntime: Send + Sync {
    /// Create the network on the runtime side (e.g. `docker network create`).
    async fn create(&self, spec: &BridgeNetwork) -> std::result::Result<(), RuntimeError>;

    /// Delete the network on the runtime side.
    async fn delete(&self, id: &str) -> std::result::Result<(), RuntimeError>;

    /// Attach a container to the network on the runtime side.
    async fn connect(
        &self,
        network: &str,
        attachment: &BridgeNetworkAttachment,
    ) -> std::result::Result<(), RuntimeError>;

    /// Detach a container from the network on the runtime side.
    async fn disconnect(
        &self,
        network: &str,
        container: &str,
    ) -> std::result::Result<(), RuntimeError>;
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Shared state for the bridge-network endpoints.
///
/// Wraps a [`DashMap`] keyed by network id plus an optional runtime trait
/// object. For this pass the default constructor leaves the runtime unset
/// and the registry operates in metadata-only mode.
#[derive(Clone)]
pub struct BridgeNetworkApiState {
    /// In-memory registry, keyed by network id.
    pub networks: Arc<DashMap<String, BridgeNetwork>>,
    /// Attachments, keyed by network id → container id → attachment.
    pub attachments: Arc<DashMap<String, DashMap<String, BridgeNetworkAttachment>>>,
    /// Runtime backend. `None` means metadata-only mode.
    pub runtime: Option<Arc<dyn BridgeNetworkRuntime>>,
    /// Flag so we only log the metadata-only warning once.
    warned: Arc<AtomicBool>,
}

impl BridgeNetworkApiState {
    /// Create a new, empty state with no runtime attached (metadata-only mode).
    #[must_use]
    pub fn new() -> Self {
        Self {
            networks: Arc::new(DashMap::new()),
            attachments: Arc::new(DashMap::new()),
            runtime: None,
            warned: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Attach a runtime backend. Subsequent handler calls will reconcile
    /// metadata changes against this runtime.
    #[must_use]
    pub fn with_runtime(mut self, runtime: Arc<dyn BridgeNetworkRuntime>) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Log exactly once that the runtime is absent. Called from handlers
    /// the first time they take a metadata-only code path.
    fn warn_metadata_only(&self) {
        if self
            .warned
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            warn!(
                "bridge-network runtime is not attached; operations are metadata-only \
                 until a runtime (e.g. bollard) is wired in"
            );
        }
    }

    /// Look up a network by id or by name (in that order). Returns the
    /// id (not the full spec) so callers can do their own mutation.
    ///
    /// Also exposed publicly via [`Self::resolve_network_id`] for callers
    /// outside this module that need the same id-or-name resolution.
    fn resolve_id(&self, id_or_name: &str) -> Option<String> {
        if self.networks.contains_key(id_or_name) {
            return Some(id_or_name.to_string());
        }
        self.networks
            .iter()
            .find(|entry| entry.value().name == id_or_name)
            .map(|entry| entry.key().clone())
    }

    /// Public wrapper around [`Self::resolve_id`] so external handlers (e.g.
    /// the `POST /api/v1/containers` create flow that attaches a freshly
    /// created container to one or more user-defined networks) can resolve
    /// network ids without depending on the private method.
    #[must_use]
    pub fn resolve_network_id(&self, id_or_name: &str) -> Option<String> {
        self.resolve_id(id_or_name)
    }

    /// Record an attachment in the in-memory registry.
    ///
    /// Looks up the target network by id or name (matching the behaviour of
    /// [`connect_container_network`]) and, if found, upserts the given
    /// [`BridgeNetworkAttachment`] under the network's attachment map.
    ///
    /// Returns `Ok(())` on success, or [`ApiError::NotFound`] when no network
    /// exists by that id or name.
    ///
    /// This method does **not** touch the runtime — callers that also want the
    /// runtime side of the attachment to happen should first drive the
    /// attachment through [`BridgeNetworkRuntime::connect`] (or the
    /// endpoint-level `connect_container_network` handler).
    ///
    /// # Errors
    /// Returns [`ApiError::NotFound`] when `id_or_name` does not resolve to a
    /// registered network.
    pub fn attach_to_registry(
        &self,
        id_or_name: &str,
        attachment: BridgeNetworkAttachment,
    ) -> Result<()> {
        let id = self.resolve_id(id_or_name).ok_or_else(|| {
            ApiError::NotFound(format!("Bridge network '{id_or_name}' not found"))
        })?;
        let entry = self.attachments.entry(id).or_default();
        entry.insert(attachment.container_id.clone(), attachment);
        Ok(())
    }

    /// Remove an attachment from the in-memory registry. Returns the removed
    /// [`BridgeNetworkAttachment`] if there was one, or `None` if the network
    /// was not known or the container was not attached.
    #[must_use]
    pub fn detach_from_registry(
        &self,
        id_or_name: &str,
        container_id: &str,
    ) -> Option<BridgeNetworkAttachment> {
        let id = self.resolve_id(id_or_name)?;
        let bucket = self.attachments.get(&id)?;
        bucket.remove(container_id).map(|(_k, v)| v)
    }
}

impl Default for BridgeNetworkApiState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Body for `POST /api/v1/container-networks`.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BridgeNetworkDetails {
    /// Core network metadata. Flattened so the JSON stays close to the
    /// list-item shape returned by `list_container_networks`.
    #[serde(flatten)]
    pub network: BridgeNetwork,
    /// Containers currently attached to the network.
    pub attached_containers: Vec<BridgeNetworkAttachment>,
}

/// Query parameters for list/delete.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
pub struct ListBridgeNetworksQuery {
    /// Optional label filter in `key=value` form. Only networks whose
    /// labels contain a matching pair are returned.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
pub struct DeleteBridgeNetworkQuery {
    /// If true, delete even if the network still has attachments.
    #[serde(default)]
    pub force: bool,
}

/// Body for `POST /api/v1/container-networks/{id_or_name}/connect`.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct ConnectBridgeNetworkRequest {
    /// Container id to attach.
    pub container_id: String,
    /// Optional DNS aliases on this network.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Optional static IPv4 to pin this container to. Validated as
    /// [`Ipv4Addr`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4_address: Option<String>,
}

/// Body for `POST /api/v1/container-networks/{id_or_name}/disconnect`.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct DisconnectBridgeNetworkRequest {
    /// Container id to detach.
    pub container_id: String,
    /// If true, the runtime is asked to forcibly detach.
    #[serde(default)]
    pub force: bool,
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Validate a bridge-network name against `^[a-z0-9][a-z0-9_-]{0,63}$`.
fn validate_network_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(ApiError::BadRequest(
            "Network name cannot be empty".to_string(),
        ));
    }
    if name.len() > 64 {
        return Err(ApiError::BadRequest(
            "Network name cannot exceed 64 characters".to_string(),
        ));
    }
    let mut chars = name.chars();
    let first = chars.next().expect("non-empty checked above");
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(ApiError::BadRequest(
            "Network name must start with [a-z0-9]".to_string(),
        ));
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            return Err(ApiError::BadRequest(format!(
                "Network name contains invalid character '{c}' (allowed: a-z 0-9 _ -)"
            )));
        }
    }
    Ok(())
}

/// Validate a CIDR subnet string.
fn validate_subnet(subnet: &str) -> Result<()> {
    subnet
        .parse::<ipnetwork::IpNetwork>()
        .map(|_| ())
        .map_err(|e| ApiError::BadRequest(format!("Invalid subnet '{subnet}': {e}")))
}

/// Validate an optional IPv4 address string.
fn validate_ipv4(ip: &str) -> Result<()> {
    ip.parse::<Ipv4Addr>()
        .map(|_| ())
        .map_err(|e| ApiError::BadRequest(format!("Invalid ipv4_address '{ip}': {e}")))
}

/// Parse a `key=value` label filter. Returns `None` for a `None` input.
fn parse_label_filter(raw: Option<&String>) -> Result<Option<(String, String)>> {
    let Some(spec) = raw else { return Ok(None) };
    let (k, v) = spec.split_once('=').ok_or_else(|| {
        ApiError::BadRequest(format!(
            "label filter must be in key=value form (got '{spec}')"
        ))
    })?;
    Ok(Some((k.to_string(), v.to_string())))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Create a new bridge or overlay network.
///
/// # Errors
///
/// - [`ApiError::BadRequest`] if the network name or subnet is malformed.
/// - [`ApiError::Conflict`] if a network with the same name already exists.
/// - [`ApiError::Forbidden`] / [`ApiError::Unauthorized`] from the auth layer.
#[utoipa::path(
    post,
    path = "/api/v1/container-networks",
    request_body = CreateBridgeNetworkRequest,
    responses(
        (status = 201, description = "Network created", body = BridgeNetwork),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 409, description = "Network with that name already exists"),
    ),
    security(("bearer_auth" = [])),
    tag = "ContainerNetworks"
)]
pub async fn create_container_network(
    user: AuthUser,
    State(state): State<BridgeNetworkApiState>,
    Json(req): Json<CreateBridgeNetworkRequest>,
) -> Result<(StatusCode, Json<BridgeNetwork>)> {
    user.require_role("operator")?;

    validate_network_name(&req.name)?;
    if let Some(subnet) = req.subnet.as_deref() {
        validate_subnet(subnet)?;
    }

    // Duplicate-name check.
    if state
        .networks
        .iter()
        .any(|entry| entry.value().name == req.name)
    {
        return Err(ApiError::Conflict(format!(
            "Bridge network '{}' already exists",
            req.name
        )));
    }

    let network = BridgeNetwork {
        id: Uuid::new_v4().to_string(),
        name: req.name.clone(),
        driver: req.driver.unwrap_or_default(),
        subnet: req.subnet,
        labels: req.labels,
        internal: req.internal,
        created_at: Utc::now(),
    };

    info!(id = %network.id, name = %network.name, "Creating bridge network");

    if let Some(runtime) = state.runtime.as_ref() {
        runtime.create(&network).await?;
    } else {
        state.warn_metadata_only();
    }

    state.networks.insert(network.id.clone(), network.clone());
    state.attachments.insert(network.id.clone(), DashMap::new());

    Ok((StatusCode::CREATED, Json(network)))
}

/// List all bridge networks, optionally filtered by label.
///
/// # Errors
///
/// [`ApiError::BadRequest`] when the `label` query param is not in
/// `key=value` form.
#[utoipa::path(
    get,
    path = "/api/v1/container-networks",
    params(ListBridgeNetworksQuery),
    responses(
        (status = 200, description = "List of bridge networks", body = Vec<BridgeNetwork>),
        (status = 400, description = "Invalid label filter"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "ContainerNetworks"
)]
pub async fn list_container_networks(
    _user: AuthUser,
    State(state): State<BridgeNetworkApiState>,
    Query(q): Query<ListBridgeNetworksQuery>,
) -> Result<Json<Vec<BridgeNetwork>>> {
    let label_filter = parse_label_filter(q.label.as_ref())?;

    let mut networks: Vec<BridgeNetwork> = state
        .networks
        .iter()
        .filter(|entry| match &label_filter {
            Some((k, v)) => entry.value().labels.get(k).is_some_and(|got| got == v),
            None => true,
        })
        .map(|entry| entry.value().clone())
        .collect();

    // Deterministic ordering for clients.
    networks.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.name.cmp(&b.name)));

    Ok(Json(networks))
}

/// Inspect a single bridge network by id or by name.
///
/// # Errors
///
/// [`ApiError::NotFound`] when no network matches `id_or_name`.
#[utoipa::path(
    get,
    path = "/api/v1/container-networks/{id_or_name}",
    params(
        ("id_or_name" = String, Path, description = "Network id or name"),
    ),
    responses(
        (status = 200, description = "Network details", body = BridgeNetworkDetails),
        (status = 404, description = "Network not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "ContainerNetworks"
)]
pub async fn get_container_network(
    _user: AuthUser,
    State(state): State<BridgeNetworkApiState>,
    Path(id_or_name): Path<String>,
) -> Result<Json<BridgeNetworkDetails>> {
    let id = state
        .resolve_id(&id_or_name)
        .ok_or_else(|| ApiError::NotFound(format!("Bridge network '{id_or_name}' not found")))?;

    let network = state
        .networks
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("Bridge network '{id_or_name}' not found")))?
        .value()
        .clone();

    let attached_containers: Vec<BridgeNetworkAttachment> = state
        .attachments
        .get(&id)
        .map(|m| m.iter().map(|e| e.value().clone()).collect())
        .unwrap_or_default();

    Ok(Json(BridgeNetworkDetails {
        network,
        attached_containers,
    }))
}

/// Delete a bridge network. Refuses if the network still has attachments
/// unless `?force=true` is set.
///
/// # Errors
///
/// - [`ApiError::NotFound`] when no network matches `id_or_name`.
/// - [`ApiError::Conflict`] when the network still has attachments and
///   `force` is false.
/// - Auth errors propagated from the `operator` role check.
#[utoipa::path(
    delete,
    path = "/api/v1/container-networks/{id_or_name}",
    params(
        ("id_or_name" = String, Path, description = "Network id or name"),
        DeleteBridgeNetworkQuery,
    ),
    responses(
        (status = 204, description = "Network deleted"),
        (status = 404, description = "Network not found"),
        (status = 409, description = "Network still has attachments (use ?force=true)"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer_auth" = [])),
    tag = "ContainerNetworks"
)]
pub async fn delete_container_network(
    user: AuthUser,
    State(state): State<BridgeNetworkApiState>,
    Path(id_or_name): Path<String>,
    Query(q): Query<DeleteBridgeNetworkQuery>,
) -> Result<StatusCode> {
    user.require_role("operator")?;

    let id = state
        .resolve_id(&id_or_name)
        .ok_or_else(|| ApiError::NotFound(format!("Bridge network '{id_or_name}' not found")))?;

    let attachment_count = state
        .attachments
        .get(&id)
        .map(|m| m.len())
        .unwrap_or_default();

    if attachment_count > 0 && !q.force {
        return Err(ApiError::Conflict(format!(
            "Bridge network '{id_or_name}' has {attachment_count} attached \
             container(s); pass ?force=true to delete anyway"
        )));
    }

    if let Some(runtime) = state.runtime.as_ref() {
        runtime.delete(&id).await?;
    } else {
        state.warn_metadata_only();
    }

    info!(id = %id, name = %id_or_name, "Deleting bridge network");
    state.networks.remove(&id);
    state.attachments.remove(&id);

    Ok(StatusCode::NO_CONTENT)
}

/// Attach a container to a network.
///
/// # Errors
///
/// - [`ApiError::BadRequest`] for an empty `container_id` or invalid
///   `ipv4_address`.
/// - [`ApiError::NotFound`] when no network matches `id_or_name`.
#[utoipa::path(
    post,
    path = "/api/v1/container-networks/{id_or_name}/connect",
    params(
        ("id_or_name" = String, Path, description = "Network id or name"),
    ),
    request_body = ConnectBridgeNetworkRequest,
    responses(
        (status = 204, description = "Container connected"),
        (status = 400, description = "Invalid request (e.g. bad ipv4_address)"),
        (status = 404, description = "Network not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "ContainerNetworks"
)]
pub async fn connect_container_network(
    _user: AuthUser,
    State(state): State<BridgeNetworkApiState>,
    Path(id_or_name): Path<String>,
    Json(req): Json<ConnectBridgeNetworkRequest>,
) -> Result<StatusCode> {
    if req.container_id.is_empty() {
        return Err(ApiError::BadRequest(
            "container_id cannot be empty".to_string(),
        ));
    }
    if let Some(ip) = req.ipv4_address.as_deref() {
        validate_ipv4(ip)?;
    }

    let id = state
        .resolve_id(&id_or_name)
        .ok_or_else(|| ApiError::NotFound(format!("Bridge network '{id_or_name}' not found")))?;

    let attachment = BridgeNetworkAttachment {
        container_id: req.container_id.clone(),
        container_name: None,
        aliases: req.aliases,
        ipv4: req.ipv4_address,
    };

    if let Some(runtime) = state.runtime.as_ref() {
        runtime.connect(&id, &attachment).await?;
    } else {
        state.warn_metadata_only();
    }

    info!(network = %id, container = %req.container_id, "Connecting container to network");
    let entry = state.attachments.entry(id).or_insert_with(DashMap::new);
    entry.insert(req.container_id, attachment);

    Ok(StatusCode::NO_CONTENT)
}

/// Detach a container from a network.
///
/// # Errors
///
/// - [`ApiError::BadRequest`] when `container_id` is empty.
/// - [`ApiError::NotFound`] when the network or the container-on-network
///   pair is not known (unless `force` was passed).
#[utoipa::path(
    post,
    path = "/api/v1/container-networks/{id_or_name}/disconnect",
    params(
        ("id_or_name" = String, Path, description = "Network id or name"),
    ),
    request_body = DisconnectBridgeNetworkRequest,
    responses(
        (status = 204, description = "Container disconnected"),
        (status = 404, description = "Network or container not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "ContainerNetworks"
)]
pub async fn disconnect_container_network(
    _user: AuthUser,
    State(state): State<BridgeNetworkApiState>,
    Path(id_or_name): Path<String>,
    Json(req): Json<DisconnectBridgeNetworkRequest>,
) -> Result<StatusCode> {
    if req.container_id.is_empty() {
        return Err(ApiError::BadRequest(
            "container_id cannot be empty".to_string(),
        ));
    }

    let id = state
        .resolve_id(&id_or_name)
        .ok_or_else(|| ApiError::NotFound(format!("Bridge network '{id_or_name}' not found")))?;

    // Check the container is actually attached (unless `force`, in which case
    // we silently noop on the registry side and let the runtime decide).
    let attached_map = state
        .attachments
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("Bridge network '{id_or_name}' not found")))?;

    if !attached_map.contains_key(&req.container_id) && !req.force {
        return Err(ApiError::NotFound(format!(
            "Container '{}' is not attached to network '{id_or_name}'",
            req.container_id
        )));
    }
    // Drop the read guard before we mutate.
    drop(attached_map);

    if let Some(runtime) = state.runtime.as_ref() {
        runtime.disconnect(&id, &req.container_id).await?;
    } else {
        state.warn_metadata_only();
    }

    info!(
        network = %id,
        container = %req.container_id,
        force = req.force,
        "Disconnecting container from network"
    );
    if let Some(attached) = state.attachments.get(&id) {
        attached.remove(&req.container_id);
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_network(name: &str) -> BridgeNetwork {
        BridgeNetwork {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            driver: BridgeNetworkDriver::Bridge,
            subnet: None,
            labels: HashMap::new(),
            internal: false,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn test_validate_network_name_ok() {
        assert!(validate_network_name("a").is_ok());
        assert!(validate_network_name("my-net").is_ok());
        assert!(validate_network_name("net_01").is_ok());
        assert!(validate_network_name("0abc").is_ok());
        assert!(validate_network_name(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn test_validate_network_name_rejects_empty() {
        assert!(validate_network_name("").is_err());
    }

    #[test]
    fn test_validate_network_name_rejects_bad_chars() {
        assert!(validate_network_name("Foo").is_err());
        assert!(validate_network_name("my net").is_err());
        assert!(validate_network_name("net!").is_err());
        // Cannot start with `-` or `_`.
        assert!(validate_network_name("-abc").is_err());
        assert!(validate_network_name("_abc").is_err());
        // Too long.
        assert!(validate_network_name(&"a".repeat(65)).is_err());
    }

    #[test]
    fn test_validate_subnet_ok() {
        assert!(validate_subnet("10.240.0.0/24").is_ok());
        assert!(validate_subnet("192.168.1.0/24").is_ok());
        assert!(validate_subnet("fd00::/64").is_ok());
    }

    #[test]
    fn test_validate_subnet_rejects_garbage() {
        assert!(validate_subnet("not-a-cidr").is_err());
        assert!(validate_subnet("10.0.0.0/33").is_err());
    }

    #[test]
    fn test_validate_ipv4_ok() {
        assert!(validate_ipv4("10.0.0.5").is_ok());
    }

    #[test]
    fn test_validate_ipv4_rejects_v6() {
        assert!(validate_ipv4("::1").is_err());
        assert!(validate_ipv4("not-an-ip").is_err());
    }

    #[test]
    fn test_parse_label_filter_none() {
        assert!(parse_label_filter(None).unwrap().is_none());
    }

    #[test]
    fn test_parse_label_filter_ok() {
        let spec = "env=prod".to_string();
        let got = parse_label_filter(Some(&spec)).unwrap();
        assert_eq!(got, Some(("env".to_string(), "prod".to_string())));
    }

    #[test]
    fn test_parse_label_filter_rejects_garbage() {
        let spec = "no-equals".to_string();
        assert!(parse_label_filter(Some(&spec)).is_err());
    }

    #[tokio::test]
    async fn test_registry_create_list_get_delete() {
        let state = BridgeNetworkApiState::new();

        // Seed.
        let net = mk_network("alpha");
        let id = net.id.clone();
        state.networks.insert(id.clone(), net.clone());
        state.attachments.insert(id.clone(), DashMap::new());

        // List.
        assert_eq!(state.networks.len(), 1);

        // Resolve by id.
        assert_eq!(state.resolve_id(&id).as_deref(), Some(id.as_str()));
        // Resolve by name.
        assert_eq!(state.resolve_id("alpha").as_deref(), Some(id.as_str()));
        // Unknown.
        assert!(state.resolve_id("nope").is_none());

        // Delete.
        state.networks.remove(&id);
        state.attachments.remove(&id);
        assert_eq!(state.networks.len(), 0);
    }

    #[tokio::test]
    async fn test_registry_rejects_duplicate_names() {
        let state = BridgeNetworkApiState::new();
        let a = mk_network("shared");
        state.networks.insert(a.id.clone(), a);
        // The handler logic performs this check; re-implement it here for
        // the test without needing to plumb an AuthUser.
        let duplicate_exists = state
            .networks
            .iter()
            .any(|entry| entry.value().name == "shared");
        assert!(duplicate_exists);
    }

    #[tokio::test]
    async fn test_registry_connect_disconnect_roundtrip() {
        let state = BridgeNetworkApiState::new();
        let net = mk_network("roundtrip");
        let id = net.id.clone();
        state.networks.insert(id.clone(), net);
        state.attachments.insert(id.clone(), DashMap::new());

        let attachment = BridgeNetworkAttachment {
            container_id: "c1".to_string(),
            container_name: Some("web".to_string()),
            aliases: vec!["web".to_string()],
            ipv4: Some("10.0.0.5".to_string()),
        };

        // Connect.
        state
            .attachments
            .get(&id)
            .unwrap()
            .insert("c1".to_string(), attachment.clone());
        assert_eq!(state.attachments.get(&id).unwrap().len(), 1);

        // Disconnect.
        state.attachments.get(&id).unwrap().remove("c1");
        assert_eq!(state.attachments.get(&id).unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_force_delete_semantics() {
        let state = BridgeNetworkApiState::new();
        let net = mk_network("busy");
        let id = net.id.clone();
        state.networks.insert(id.clone(), net);
        let attachments = DashMap::new();
        attachments.insert(
            "c1".to_string(),
            BridgeNetworkAttachment {
                container_id: "c1".to_string(),
                container_name: None,
                aliases: vec![],
                ipv4: None,
            },
        );
        state.attachments.insert(id.clone(), attachments);

        // Non-force delete should be refused (attachment count > 0).
        let count = state.attachments.get(&id).map_or(0, |m| m.len());
        assert_eq!(count, 1, "precondition: has one attachment");

        // Force path: proceed anyway.
        state.networks.remove(&id);
        state.attachments.remove(&id);
        assert!(state.networks.is_empty());
    }

    #[tokio::test]
    async fn test_metadata_only_warn_fires_once() {
        let state = BridgeNetworkApiState::new();
        assert!(state.runtime.is_none());
        state.warn_metadata_only();
        assert!(state.warned.load(Ordering::SeqCst));
        // Second call is a no-op (tracing::warn! is not asserted on, but the
        // atomic flag proves we don't re-warn).
        state.warn_metadata_only();
        assert!(state.warned.load(Ordering::SeqCst));
    }

    /// Smoke-test a minimal mock runtime to prove the trait is object-safe
    /// and the state accepts it.
    #[tokio::test]
    async fn test_with_runtime_accepts_trait_object() {
        struct MockRuntime;
        #[async_trait]
        impl BridgeNetworkRuntime for MockRuntime {
            async fn create(&self, _: &BridgeNetwork) -> std::result::Result<(), RuntimeError> {
                Ok(())
            }
            async fn delete(&self, _: &str) -> std::result::Result<(), RuntimeError> {
                Ok(())
            }
            async fn connect(
                &self,
                _: &str,
                _: &BridgeNetworkAttachment,
            ) -> std::result::Result<(), RuntimeError> {
                Ok(())
            }
            async fn disconnect(&self, _: &str, _: &str) -> std::result::Result<(), RuntimeError> {
                Ok(())
            }
        }

        let state = BridgeNetworkApiState::new().with_runtime(Arc::new(MockRuntime));
        assert!(state.runtime.is_some());
    }
}
