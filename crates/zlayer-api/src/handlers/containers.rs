//! Raw container lifecycle endpoints
//!
//! Provides direct container management endpoints for use by CI runners and
//! other tooling that needs to manage containers independently of the
//! deployment/service abstraction.

use std::collections::HashMap;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use bytes::Bytes;
use dashmap::DashMap;
use futures_util::{Stream, StreamExt};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info};

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use crate::event_bus::{ContainerEvent, ContainerEventBus};
use crate::handlers::container_id_map::{compute_hex, ContainerIdMap, ZLAYER_CONTAINER_ID_LABEL};
use crate::handlers::container_networks::BridgeNetworkApiState;
use crate::handlers::exec_instances::ExecInstances;
use crate::storage::{
    ComposeProjectStorage, InMemoryComposeProjectStorage, InMemoryStandaloneContainerStorage,
    StandaloneContainerStorage,
};
use zlayer_agent::runtime::{
    ArchivePutOptions, ContainerId, ContainerInspectDetails, ContainerState, ExecEvent,
    HealthDetail, LogChannel, LogChunk, LogsStreamOptions, NetworkAttachmentDetail, PathStat,
    Runtime, StatsSample,
};
use zlayer_spec::BridgeNetworkAttachment;

pub use zlayer_types::api::containers::*;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// State for raw container endpoints.
///
/// Holds a reference to the container runtime so handlers can perform
/// container lifecycle operations without going through the service manager.
///
/// Also carries the daemon-wide [`ContainerEventBus`] so container lifecycle
/// handlers can emit start/die/oom/health events to any subscribers of the
/// `GET /api/v1/events` SSE stream.
#[derive(Clone)]
pub struct ContainerApiState {
    /// Container runtime for lifecycle operations
    pub runtime: Arc<dyn Runtime + Send + Sync>,
    /// In-memory tracking of standalone containers (id -> metadata)
    pub containers: Arc<RwLock<HashMap<String, StandaloneContainer>>>,
    /// Daemon-wide container lifecycle event bus.
    pub event_bus: ContainerEventBus,
    /// Optional bridge-network registry. Populated by the daemon when a
    /// bridge-network runtime is available (e.g. Docker is reachable);
    /// `None` on hosts that don't support user-defined networks. When set,
    /// [`CreateContainerRequest::networks`] entries are honoured on container
    /// create.
    pub bridge_networks: Option<BridgeNetworkApiState>,
    // -- §3.10: registry credential resolution --------------------------------
    /// Optional persistent registry-credential store. Populated by the
    /// daemon so the container-create handler can resolve
    /// [`CreateContainerRequest::registry_credential_id`] into username +
    /// password. When `None`, only inline
    /// [`CreateContainerRequest::registry_auth`] is honoured; a request that
    /// carries `registry_credential_id` without a configured store is
    /// rejected with `400`.
    pub registry_store: Option<
        Arc<zlayer_secrets::RegistryCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>>,
    >,
    /// Bidirectional map between Docker-shaped 64-char hex container IDs and
    /// the native [`ContainerId { service, replica }`] handles. Populated by
    /// the create handler and consulted by [`resolve_container_id`] so that
    /// REST clients (and the Docker compat layer) can address containers by
    /// their hex IDs while internal storage keeps using the service-name key.
    pub id_map: Arc<ContainerIdMap>,
    /// Persistent backing store for standalone-container metadata. Every
    /// create / delete on the in-memory `containers` cache is mirrored here
    /// first so the daemon can repopulate the cache from disk on restart.
    /// Defaults to an [`InMemoryStandaloneContainerStorage`] when constructed
    /// via [`Self::new`] / [`Self::with_daemon_uuid`] / [`Self::with_event_bus`];
    /// production daemons swap in a `SqliteStandaloneContainerStorage` via
    /// [`Self::with_standalone_storage`].
    pub standalone_storage: Arc<dyn StandaloneContainerStorage>,
    /// Persistent backing store for compose-project metadata recorded by
    /// `zlayer compose up`. Survives daemon restarts so a later
    /// `compose down` / `compose ls` can recover the project's identity
    /// (working directory, layered compose files, active profiles, env files,
    /// service set, and the concrete container ids spawned).
    ///
    /// Defaults to an [`InMemoryComposeProjectStorage`] when constructed via
    /// [`Self::new`] / [`Self::with_daemon_uuid`] / [`Self::with_event_bus`];
    /// production daemons swap in a `SqliteComposeProjectStorage` via
    /// [`Self::with_compose_storage`].
    pub compose_storage: Arc<dyn ComposeProjectStorage>,
    /// In-memory registry of pending/running exec instances, keyed by their
    /// 64-char hex exec ID. Populated by `POST /containers/{id}/exec` and
    /// consumed by `POST /exec/{id}/start`, `GET /exec/{id}/json`, and
    /// `DELETE /exec/{id}`. Defaults to a fresh empty registry; daemons
    /// share a single instance across `ContainerApiState` clones via
    /// `Arc::clone`.
    pub exec_instances: Arc<ExecInstances>,
    /// Active PTY resize senders for the *main* container processes (not
    /// exec'd ones), keyed by [`ContainerId`]. Populated when a container is
    /// started with TTY enabled and the runtime hands back a resize sender;
    /// consulted by `POST /api/v1/containers/{id}/resize` to forward
    /// terminal-size hints. Currently only populated for containers that the
    /// daemon attached interactively — non-TTY containers have no entry, and
    /// the resize endpoint returns `400` for those.
    pub container_pty_resizers: Arc<DashMap<ContainerId, mpsc::Sender<(u16, u16)>>>,
}

/// Metadata for a standalone container (not managed by a deployment)
#[derive(Debug, Clone)]
pub struct StandaloneContainer {
    /// Container identifier used by the runtime
    pub container_id: ContainerId,
    /// OCI image reference
    pub image: String,
    /// Human-readable name (if provided)
    pub name: Option<String>,
    /// Labels for filtering/grouping
    pub labels: HashMap<String, String>,
    /// When the container was created
    pub created_at: String,
    /// Mirror of [`zlayer_spec::LifecycleSpec::delete_on_exit`] from the
    /// originating create request (Docker `--rm` / `HostConfig.AutoRemove`).
    /// Consulted by the daemon-side auto-remove subscriber
    /// ([`auto_remove::start_auto_remove_subscriber`]) on every
    /// `container.die` event: when `true` the corresponding container record
    /// is stopped, removed, and dropped from the cache; when `false` the
    /// record is retained so the container can be inspected post-mortem.
    pub delete_on_exit: bool,
}

impl ContainerApiState {
    /// Create a new container API state with a runtime and a fresh event bus.
    ///
    /// The container-id map is seeded with a freshly-generated daemon UUID;
    /// production callers should use [`Self::with_daemon_uuid`] (or attach a
    /// pre-built map via [`Self::with_id_map`]) so the same hash space is
    /// shared across daemon restarts.
    pub fn new(runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        let daemon_uuid = uuid::Uuid::new_v4().as_simple().to_string();
        Self {
            runtime,
            containers: Arc::new(RwLock::new(HashMap::new())),
            event_bus: ContainerEventBus::new(),
            bridge_networks: None,
            registry_store: None,
            id_map: Arc::new(ContainerIdMap::new(daemon_uuid)),
            standalone_storage: Arc::new(InMemoryStandaloneContainerStorage::new()),
            compose_storage: Arc::new(InMemoryComposeProjectStorage::new()),
            exec_instances: Arc::new(ExecInstances::new()),
            container_pty_resizers: Arc::new(DashMap::new()),
        }
    }

    /// Create a new container API state with a runtime, a fresh event bus,
    /// and an explicit daemon UUID. Used by the daemon entrypoint so the
    /// hex-id hash space is stable across restarts.
    pub fn with_daemon_uuid(runtime: Arc<dyn Runtime + Send + Sync>, daemon_uuid: String) -> Self {
        Self {
            runtime,
            containers: Arc::new(RwLock::new(HashMap::new())),
            event_bus: ContainerEventBus::new(),
            bridge_networks: None,
            registry_store: None,
            id_map: Arc::new(ContainerIdMap::new(daemon_uuid)),
            standalone_storage: Arc::new(InMemoryStandaloneContainerStorage::new()),
            compose_storage: Arc::new(InMemoryComposeProjectStorage::new()),
            exec_instances: Arc::new(ExecInstances::new()),
            container_pty_resizers: Arc::new(DashMap::new()),
        }
    }

    /// Create a new container API state with a runtime and an existing event
    /// bus. Used when the event bus must be shared across multiple route
    /// groups (e.g. `/api/v1/containers` and `/api/v1/events`).
    pub fn with_event_bus(
        runtime: Arc<dyn Runtime + Send + Sync>,
        event_bus: ContainerEventBus,
    ) -> Self {
        let daemon_uuid = uuid::Uuid::new_v4().as_simple().to_string();
        Self {
            runtime,
            containers: Arc::new(RwLock::new(HashMap::new())),
            event_bus,
            bridge_networks: None,
            registry_store: None,
            id_map: Arc::new(ContainerIdMap::new(daemon_uuid)),
            standalone_storage: Arc::new(InMemoryStandaloneContainerStorage::new()),
            compose_storage: Arc::new(InMemoryComposeProjectStorage::new()),
            exec_instances: Arc::new(ExecInstances::new()),
            container_pty_resizers: Arc::new(DashMap::new()),
        }
    }

    /// Replace the [`ContainerIdMap`] on this state with the supplied
    /// instance. Useful for daemons that share a single map across multiple
    /// `ContainerApiState` clones (e.g. event subscribers).
    #[must_use]
    pub fn with_id_map(mut self, id_map: Arc<ContainerIdMap>) -> Self {
        self.id_map = id_map;
        self
    }

    /// Replace the event bus on this state. Used by the daemon entrypoint to
    /// share a single [`ContainerEventBus`] (`DaemonEventBus`) across the
    /// container, image, network, and volume states so all four resource
    /// families publish on the same broadcast channel.
    #[must_use]
    pub fn with_shared_event_bus(mut self, event_bus: ContainerEventBus) -> Self {
        self.event_bus = event_bus;
        self
    }

    /// Attach the bridge-network registry so the container create handler
    /// can honour [`CreateContainerRequest::networks`] entries.
    ///
    /// Fluent companion to [`Self::new`] / [`Self::with_event_bus`].
    #[must_use]
    pub fn with_bridge_networks(mut self, bridge_networks: BridgeNetworkApiState) -> Self {
        self.bridge_networks = Some(bridge_networks);
        self
    }

    /// Attach the persistent registry-credential store so the container
    /// create handler can resolve
    /// [`CreateContainerRequest::registry_credential_id`] into inline
    /// credentials passed to the runtime's `pull_image_with_policy`. Added
    /// for §3.10 of `ZLAYER_SDK_FIXES.md`.
    #[must_use]
    pub fn with_registry_store(
        mut self,
        registry_store: Arc<
            zlayer_secrets::RegistryCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>,
        >,
    ) -> Self {
        self.registry_store = Some(registry_store);
        self
    }

    /// Replace the standalone-container storage backend on this state.
    /// Production daemons attach a `SqliteStandaloneContainerStorage` here so
    /// create/delete on `/api/v1/containers` survives daemon restarts; tests
    /// keep the default in-memory backend.
    #[must_use]
    pub fn with_standalone_storage(
        mut self,
        standalone_storage: Arc<dyn StandaloneContainerStorage>,
    ) -> Self {
        self.standalone_storage = standalone_storage;
        self
    }

    /// Replace the compose-project storage backend on this state.
    /// Production daemons attach a `SqliteComposeProjectStorage` here so
    /// `zlayer compose up` records survive daemon restarts; tests keep the
    /// default in-memory backend.
    #[must_use]
    pub fn with_compose_storage(mut self, compose_storage: Arc<dyn ComposeProjectStorage>) -> Self {
        self.compose_storage = compose_storage;
        self
    }

    /// Repopulate the in-memory `containers` cache from
    /// [`Self::standalone_storage`]. The cache is keyed by
    /// [`ContainerId::service`] (matching what `create_container` writes), so
    /// the storage list is normalised to that key here as well.
    ///
    /// Called once at daemon startup — after the storage handle is attached
    /// via [`Self::with_standalone_storage`] — to make previously-created
    /// standalone containers visible to list/inspect/delete handlers without
    /// requiring a fresh API call.
    ///
    /// # Errors
    ///
    /// Returns the storage layer's [`StorageError`](crate::storage::StorageError)
    /// if listing fails. The cache is left untouched on error.
    pub async fn repopulate_cache_from_storage(&self) -> Result<()> {
        let entries = self.standalone_storage.list().await?;
        let mut cache = self.containers.write().await;
        for entry in entries {
            let key = entry.container_id.service.clone();
            cache.insert(key, entry);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Validate a volume name against the same rules enforced by the `/volumes`
/// handler. Kept as a local copy to avoid cross-module coupling (see
/// `handlers/volumes.rs::validate_volume_name`).
///
/// Equivalent regex: `^[a-z0-9][a-z0-9_-]{0,63}$`.
fn validate_volume_name_local(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(ApiError::BadRequest("volume name is required".to_string()));
    }
    if name.len() > 64 {
        return Err(ApiError::BadRequest(format!(
            "volume name '{name}' exceeds 64 characters"
        )));
    }
    let mut chars = name.chars();
    let first = chars.next().expect("non-empty checked above");
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(ApiError::BadRequest(format!(
            "volume name '{name}' must start with [a-z0-9]"
        )));
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
            return Err(ApiError::BadRequest(format!(
                "volume name '{name}' contains invalid character '{c}'; allowed: [a-z0-9_-]"
            )));
        }
    }
    Ok(())
}

/// Translate the wire-format [`HealthCheckRequest`] into the internal
/// [`zlayer_spec::HealthSpec`].
///
/// Free function (rather than `impl HealthCheckRequest`) because
/// `HealthCheckRequest` is foreign to this crate, which would violate the
/// orphan rule.
///
/// # Errors
/// Returns `ApiError::BadRequest` for:
/// - Unknown `type` (must be `"tcp"`, `"http"`, or `"command"`).
/// - Missing required fields per variant (e.g. `port == 0` for tcp, empty `url`
///   for http, missing/empty `command` for command).
/// - Malformed humantime strings on `interval`, `timeout`, or `start_period`.
pub fn health_request_to_spec(req: &HealthCheckRequest) -> Result<zlayer_spec::HealthSpec> {
    use zlayer_spec::{HealthCheck, HealthSpec};

    let check = match req.check_type.as_str() {
        "tcp" => {
            let port = req.port.ok_or_else(|| {
                ApiError::BadRequest(
                    "health_check.port is required when type == \"tcp\"".to_string(),
                )
            })?;
            if port == 0 {
                return Err(ApiError::BadRequest(
                    "health_check.port must be between 1 and 65535".to_string(),
                ));
            }
            HealthCheck::Tcp { port }
        }
        "http" => {
            let url = req
                .url
                .as_ref()
                .filter(|u| !u.is_empty())
                .ok_or_else(|| {
                    ApiError::BadRequest(
                        "health_check.url is required when type == \"http\"".to_string(),
                    )
                })?
                .clone();
            let expect_status = req.expect_status.unwrap_or(200);
            HealthCheck::Http { url, expect_status }
        }
        "command" => {
            let argv = req
                .command
                .as_ref()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    ApiError::BadRequest(
                        "health_check.command is required and must be non-empty when type == \"command\""
                            .to_string(),
                    )
                })?;
            // Join argv tokens with spaces; the health monitor passes the
            // result to `sh -c`. Matches the compose-to-ZLayer convention
            // in `zlayer-docker/src/compose/convert.rs`.
            HealthCheck::Command {
                command: argv.join(" "),
            }
        }
        other => {
            return Err(ApiError::BadRequest(format!(
                "health_check.type must be one of \"tcp\", \"http\", \"command\"; got {other:?}"
            )))
        }
    };

    let interval = parse_optional_duration(req.interval.as_deref(), "health_check.interval")?
        .or_else(|| Some(Duration::from_secs(30)));
    let timeout = parse_optional_duration(req.timeout.as_deref(), "health_check.timeout")?;
    let start_grace =
        parse_optional_duration(req.start_period.as_deref(), "health_check.start_period")?;
    let retries = req.retries.unwrap_or(3);

    Ok(HealthSpec {
        start_grace,
        interval,
        timeout,
        retries,
        check,
    })
}

/// Parse an optional humantime duration string, producing a consistent
/// `ApiError::BadRequest` on malformed input. Returns `Ok(None)` if the input
/// is `None`.
fn parse_optional_duration(input: Option<&str>, field: &str) -> Result<Option<Duration>> {
    match input {
        None => Ok(None),
        Some(s) => humantime::parse_duration(s).map(Some).map_err(|e| {
            ApiError::BadRequest(format!(
                "{field} is not a valid duration: {s:?} ({e}); expected humantime (e.g. \"10s\", \"1m\", \"500ms\")"
            ))
        }),
    }
}

/// Validate `dns` entries — each must be a plausible IPv4 or IPv6 address.
fn validate_dns_entries(entries: &[String]) -> Result<()> {
    for entry in entries {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            return Err(ApiError::BadRequest(
                "dns entries cannot be empty strings".to_string(),
            ));
        }
        if trimmed.parse::<std::net::IpAddr>().is_err() {
            return Err(ApiError::BadRequest(format!(
                "dns entry is not a valid IP address: {entry:?}"
            )));
        }
    }
    Ok(())
}

/// Validate `extra_hosts` entries — each must be of the form `hostname:ip`.
/// The ip half may be the literal `host-gateway` (resolved by bollard/Docker
/// to the host-visible gateway address, e.g. `host.docker.internal:host-gateway`).
/// Splits on the *first* colon so IPv6 addresses (which contain colons
/// themselves) can be used as-is.
fn validate_extra_hosts(entries: &[String]) -> Result<()> {
    for entry in entries {
        let (host, ip) = entry.split_once(':').ok_or_else(|| {
            ApiError::BadRequest(format!(
                "extra_hosts entry must be in the form 'hostname:ip': {entry:?}"
            ))
        })?;
        if host.trim().is_empty() {
            return Err(ApiError::BadRequest(format!(
                "extra_hosts hostname cannot be empty: {entry:?}"
            )));
        }
        if ip.trim().is_empty() {
            return Err(ApiError::BadRequest(format!(
                "extra_hosts ip cannot be empty: {entry:?}"
            )));
        }
        if ip == "host-gateway" {
            continue;
        }
        if ip.parse::<std::net::IpAddr>().is_err() {
            return Err(ApiError::BadRequest(format!(
                "extra_hosts ip is not a valid address: {entry:?} (got {ip:?})"
            )));
        }
    }
    Ok(())
}

/// Build a [`NetworkAttachmentInfo`] from a runtime [`NetworkAttachmentDetail`].
///
/// Free function (rather than
/// `impl From<NetworkAttachmentDetail> for NetworkAttachmentInfo`) because both
/// types are foreign to this crate, which would violate the orphan rule.
#[must_use]
pub fn network_attachment_info_from_detail(d: NetworkAttachmentDetail) -> NetworkAttachmentInfo {
    NetworkAttachmentInfo {
        network: d.network,
        aliases: d.aliases,
        ipv4: d.ipv4,
    }
}

/// Build a [`ContainerHealthInfo`] from a runtime [`HealthDetail`].
///
/// Free function (rather than `impl From<HealthDetail> for ContainerHealthInfo`)
/// because both types are foreign to this crate, which would violate the
/// orphan rule.
#[must_use]
pub fn container_health_info_from_detail(d: HealthDetail) -> ContainerHealthInfo {
    ContainerHealthInfo {
        status: d.status,
        failing_streak: d.failing_streak,
        last_output: d.last_output,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate a unique container ID string for standalone containers.
///
/// Format: `standalone-{name_or_uuid}-rep-0`
fn generate_container_id(name: Option<&str>) -> (String, ContainerId) {
    let service_name = match name {
        Some(n) => format!("standalone-{n}"),
        None => format!("standalone-{}", uuid::Uuid::new_v4().as_simple()),
    };
    let cid = ContainerId::new(service_name.clone(), 0);
    (service_name, cid)
}

/// Resolution outcome from [`resolve_container_lookup`]: the underlying
/// [`ContainerId`] plus the storage key (service-name string) used to look up
/// the container in [`ContainerApiState::containers`].
pub(crate) struct ResolvedContainer {
    pub(crate) container_id: ContainerId,
    pub(crate) storage_key: String,
}

/// Resolve a raw URL-supplied container identifier into the canonical
/// [`ContainerId`] used by the runtime, plus the storage-map key.
///
/// Accepts:
/// 1. A 64-character lowercase hex hash registered in the [`ContainerIdMap`].
/// 2. A 12+ character hex prefix that uniquely matches one registered hex.
/// 3. The legacy service-name string used as the storage key (e.g.
///    `"standalone-myapp"`).
///
/// Returns `None` when the identifier matches none of the above.
pub(crate) async fn resolve_container_lookup(
    state: &ContainerApiState,
    raw: &str,
) -> Option<ResolvedContainer> {
    let is_hex = !raw.is_empty()
        && raw
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());

    // Full 64-char hex: direct lookup.
    if is_hex && raw.len() == 64 {
        if let Some(cid) = state.id_map.lookup_hex(raw) {
            let storage_key = cid.service.clone();
            return Some(ResolvedContainer {
                container_id: cid,
                storage_key,
            });
        }
    }

    // Short hex prefix: scan registered entries for a unique match. The
    // standalone-container set is expected to stay small; a linear scan over
    // the in-memory `containers` map (which we keyed by service-name) plus a
    // hex recompute via `compute_hex` avoids exposing the internals of
    // `ContainerIdMap` just for this lookup.
    if is_hex && raw.len() >= 12 && raw.len() < 64 {
        let snapshot: Vec<(String, ContainerId)> = {
            let g = state.containers.read().await;
            g.iter()
                .map(|(k, v)| (k.clone(), v.container_id.clone()))
                .collect()
        };
        let mut matches = snapshot.into_iter().filter_map(|(key, cid)| {
            let hex = state.id_map.lookup_container(&cid).unwrap_or_else(|| {
                crate::handlers::container_id_map::compute_hex(state.id_map.daemon_uuid(), &cid)
            });
            if hex.starts_with(raw) {
                Some(ResolvedContainer {
                    container_id: cid,
                    storage_key: key,
                })
            } else {
                None
            }
        });
        if let Some(first) = matches.next() {
            // Reject ambiguous matches.
            if matches.next().is_none() {
                return Some(first);
            }
        }
    }

    // Fall-back: legacy service-name lookup. The storage map is keyed by
    // the service-name string, so a present entry means the raw identifier
    // is itself a valid storage key.
    let g = state.containers.read().await;
    if let Some(meta) = g.get(raw) {
        return Some(ResolvedContainer {
            container_id: meta.container_id.clone(),
            storage_key: raw.to_string(),
        });
    }
    None
}

/// Build a minimal `ServiceSpec` from the create request.
///
/// This converts the simplified container request into the full `ServiceSpec`
/// that the Runtime trait expects. Many fields default to sensible values
/// since standalone containers don't need scaling, etc.
///
/// # Errors
/// Returns `ApiError::BadRequest` if the request's `health_check` is malformed
/// (unknown variant, missing required fields, bad humantime durations). When
/// `health_check` is absent, the spec's `health` field falls back to a no-op
/// `HealthCheck::Tcp { port: 0 }` placeholder — the health monitor treats
/// `port == 0` as "skip".
#[allow(clippy::too_many_lines)]
fn build_service_spec(request: &CreateContainerRequest) -> Result<zlayer_spec::ServiceSpec> {
    use zlayer_spec::{
        CommandSpec, ErrorsSpec, HealthCheck, HealthSpec, ImageSpec, InitSpec, NodeMode,
        PullPolicy, ResourceType, ResourcesSpec, ScaleSpec, ServiceNetworkSpec, ServiceSpec,
        ServiceType, StorageSpec,
    };

    validate_dns_entries(&request.dns)?;
    validate_extra_hosts(&request.extra_hosts)?;
    validate_restart_policy(request.restart_policy.as_ref())?;

    let command_spec = if let Some(ref cmd) = request.command {
        CommandSpec {
            entrypoint: if cmd.is_empty() {
                None
            } else {
                Some(vec![cmd[0].clone()])
            },
            args: if cmd.len() > 1 {
                Some(cmd[1..].to_vec())
            } else {
                None
            },
            workdir: request.work_dir.clone(),
        }
    } else {
        CommandSpec {
            entrypoint: None,
            args: None,
            workdir: request.work_dir.clone(),
        }
    };

    let resources = ResourcesSpec {
        cpu: request.resources.as_ref().and_then(|r| r.cpu),
        memory: request.resources.as_ref().and_then(|r| r.memory.clone()),
        gpu: None,
        pids_limit: request.pids_limit,
        cpuset: request.cpuset.clone(),
        cpu_shares: request.cpu_shares,
        memory_swap: request.memory_swap.clone(),
        memory_reservation: request.memory_reservation.clone(),
        memory_swappiness: request.memory_swappiness,
        oom_score_adj: request.oom_score_adj,
        oom_kill_disable: request.oom_kill_disable,
        blkio_weight: request.blkio_weight,
    };

    let storage: Vec<StorageSpec> = request
        .volumes
        .iter()
        .map(|v| -> Result<StorageSpec> {
            match v.mount_type.unwrap_or(VolumeMountType::Bind) {
                VolumeMountType::Bind => {
                    let source = v.source.as_deref().ok_or_else(|| {
                        ApiError::BadRequest(format!(
                            "bind volume (target={}) requires 'source' (host path)",
                            v.target
                        ))
                    })?;
                    if !source.starts_with('/') {
                        return Err(ApiError::BadRequest(format!(
                            "bind volume source '{source}' must be an absolute path"
                        )));
                    }
                    Ok(StorageSpec::Bind {
                        source: source.to_string(),
                        target: v.target.clone(),
                        readonly: v.readonly,
                    })
                }
                VolumeMountType::Volume => {
                    let name = v.source.as_deref().ok_or_else(|| {
                        ApiError::BadRequest(format!(
                            "named volume (target={}) requires 'source' (volume name)",
                            v.target
                        ))
                    })?;
                    validate_volume_name_local(name)?;
                    Ok(StorageSpec::Named {
                        name: name.to_string(),
                        target: v.target.clone(),
                        readonly: v.readonly,
                        tier: zlayer_spec::StorageTier::default(),
                        size: None,
                    })
                }
                VolumeMountType::Tmpfs => {
                    if let Some(s) = v.source.as_deref() {
                        if !s.is_empty() {
                            return Err(ApiError::BadRequest(format!(
                                "tmpfs volume (target={}) must not set 'source' (got {s:?})",
                                v.target
                            )));
                        }
                    }
                    Ok(StorageSpec::Tmpfs {
                        target: v.target.clone(),
                        size: None,
                        mode: None,
                    })
                }
            }
        })
        .collect::<Result<Vec<_>>>()?;

    // Translate the optional `health_check` request into the spec's
    // `HealthSpec`. `ServiceSpec.health` is non-optional (see
    // `zlayer_spec::types::ServiceSpec.health: HealthSpec`), so when the
    // caller omits `health_check` we install a no-op placeholder — the
    // health monitor treats `HealthCheck::Tcp { port: 0 }` as "skip",
    // matching the crate-wide default in `default_health()`.
    let health = match &request.health_check {
        Some(hc) => health_request_to_spec(hc)?,
        None => HealthSpec {
            start_grace: None,
            interval: None,
            timeout: None,
            retries: 3,
            check: HealthCheck::Tcp { port: 0 },
        },
    };

    Ok(ServiceSpec {
        rtype: ResourceType::Service,
        schedule: None,
        image: ImageSpec {
            name: request.image.parse().map_err(|e| {
                ApiError::BadRequest(format!("invalid image reference {:?}: {e}", request.image))
            })?,
            pull_policy: match request.pull_policy.as_deref() {
                Some("always") => PullPolicy::Always,
                Some("never") => PullPolicy::Never,
                _ => PullPolicy::IfNotPresent,
            },
        },
        resources,
        env: request.env.clone(),
        command: command_spec,
        network: ServiceNetworkSpec::default(),
        endpoints: Vec::new(),
        scale: ScaleSpec::Manual,
        depends: Vec::new(),
        health,
        init: InitSpec::default(),
        errors: ErrorsSpec::default(),
        lifecycle: request.lifecycle.clone(),
        devices: request.devices.clone(),
        storage,
        port_mappings: request.ports.clone(),
        capabilities: request.cap_add.clone(),
        cap_drop: request.cap_drop.clone(),
        privileged: request.privileged.unwrap_or(false),
        node_mode: NodeMode::default(),
        node_selector: None,
        platform: None,
        service_type: ServiceType::default(),
        wasm: None,
        logs: None,
        host_network: matches!(request.network_mode, Some(zlayer_spec::NetworkMode::Host)),
        hostname: request.hostname.clone(),
        dns: request.dns.clone(),
        extra_hosts: request.extra_hosts.clone(),
        restart_policy: request.restart_policy.clone(),
        labels: request.labels.clone(),
        user: request.user.clone(),
        stop_signal: request.stop_signal.clone(),
        stop_grace_period: request.stop_grace_period,
        sysctls: request.sysctls.clone(),
        ulimits: request.ulimits.clone(),
        security_opt: request.security_opt.clone(),
        pid_mode: request.pid_mode.clone(),
        ipc_mode: request.ipc_mode.clone(),
        network_mode: request
            .network_mode
            .clone()
            .unwrap_or(zlayer_spec::NetworkMode::Default),
        extra_groups: request.extra_groups.clone(),
        read_only_root_fs: request.read_only_root_fs,
        init_container: request.init_container,
        tty: false,
        stdin_open: false,
        userns_mode: None,
        cgroup_parent: None,
        expose: Vec::new(),
        replica_groups: None,
        isolation: None,
        overlay: None,
    })
}

/// Validate an optional [`zlayer_spec::ContainerRestartPolicy`].
///
/// Returns `ApiError::BadRequest` when `delay` is set but does not parse as
/// a `humantime::Duration`. All other fields are type-checked by serde. A
/// `None` policy is always OK.
fn validate_restart_policy(policy: Option<&zlayer_spec::ContainerRestartPolicy>) -> Result<()> {
    let Some(p) = policy else { return Ok(()) };
    if let Some(delay) = p.delay.as_deref() {
        delay.parse::<humantime::Duration>().map_err(|e| {
            ApiError::BadRequest(format!(
                "restart_policy.delay must be a humantime duration (e.g. \"500ms\"), \
                 got {delay:?}: {e}"
            ))
        })?;
    }
    Ok(())
}

/// Resolve inline or stored registry credentials for a pull (§3.10).
///
/// Precedence (matches the contract documented on
/// [`CreateContainerRequest::registry_auth`] and
/// [`crate::handlers::images::PullImageRequest::registry_auth`]):
///
/// 1. Inline `registry_auth` — used verbatim, no store lookup.
/// 2. `registry_credential_id` — fetched from the provided credential store.
/// 3. Neither — returns `None`, runtime falls back to its existing
///    hostname-based lookup (or anonymous access).
///
/// # Errors
///
/// * `400 Bad Request` when `registry_credential_id` is set but the daemon
///   has no credential store configured.
/// * `404 Not Found` when the referenced credential id doesn't exist.
/// * `500 Internal` on store read/decrypt failures.
async fn resolve_registry_auth(
    inline: Option<&zlayer_spec::RegistryAuth>,
    credential_id: Option<&str>,
    store: Option<
        &zlayer_secrets::RegistryCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>,
    >,
) -> Result<Option<zlayer_spec::RegistryAuth>> {
    if let Some(auth) = inline {
        // Inline wins; never logged.
        return Ok(Some(auth.clone()));
    }
    let Some(id) = credential_id else {
        return Ok(None);
    };
    let Some(store) = store else {
        return Err(ApiError::BadRequest(
            "registry_credential_id is set but the daemon has no registry credential store \
             configured; either omit the field or configure the store at startup"
                .to_string(),
        ));
    };
    let meta = store
        .get(id)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to look up registry credential: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("registry credential '{id}' not found")))?;
    let password = store
        .get_password(id)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to load registry credential: {e}")))?;
    let auth_type = match meta.auth_type {
        zlayer_secrets::RegistryAuthType::Basic => zlayer_spec::RegistryAuthType::Basic,
        zlayer_secrets::RegistryAuthType::Token => zlayer_spec::RegistryAuthType::Token,
    };
    Ok(Some(zlayer_spec::RegistryAuth {
        username: meta.username,
        password: password.expose().to_string(),
        auth_type,
    }))
}

/// Convert a `ContainerState` to a human-readable string.
fn state_to_string(state: &ContainerState) -> String {
    match state {
        ContainerState::Pending => "pending".to_string(),
        ContainerState::Initializing => "initializing".to_string(),
        ContainerState::Running => "running".to_string(),
        ContainerState::Stopping => "stopping".to_string(),
        ContainerState::Exited { code } => format!("exited({code})"),
        ContainerState::Failed { reason } => format!("failed: {reason}"),
    }
}

/// Wire-format projection of [`ContainerInspectDetails`] — the fields
/// [`ContainerInfo`] carries on top of the basic identity/state fields.
///
/// Exists so both `get_container` and `list_containers` can populate their
/// `ContainerInfo` without duplicating the `Into` conversions for the network
/// and health subtypes. Passes through `ports`, `ipv4`, and `exit_code`
/// verbatim; maps `networks` and `health` through their `From` impls.
struct InspectFields {
    ports: Vec<zlayer_spec::PortMapping>,
    networks: Vec<NetworkAttachmentInfo>,
    ipv4: Option<String>,
    health: Option<ContainerHealthInfo>,
    exit_code: Option<i32>,
}

impl From<ContainerInspectDetails> for InspectFields {
    fn from(inspect: ContainerInspectDetails) -> Self {
        let ContainerInspectDetails {
            ports,
            networks,
            ipv4,
            health,
            exit_code,
        } = inspect;
        Self {
            ports,
            networks: networks
                .into_iter()
                .map(network_attachment_info_from_detail)
                .collect(),
            ipv4,
            health: health.map(container_health_info_from_detail),
            exit_code,
        }
    }
}

/// The Docker-visible container name for a runtime-level `ContainerId`.
///
/// Mirrors the `container_name` helper in
/// `zlayer-agent::runtimes::docker::container_name` — the daemon spawns every
/// container as `zlayer-{service}-{replica}`, and that's the name bollard's
/// network endpoints expect for `NetworkConnectRequest.container`.
fn runtime_container_name(id: &ContainerId) -> String {
    format!("zlayer-{}-{}", id.service, id.replica)
}

/// Validate the `networks` field of a [`CreateContainerRequest`] up-front.
///
/// Checks that:
/// 1. A `bridge_networks` registry is plumbed into [`ContainerApiState`].
/// 2. Each referenced network already exists (by id or name).
/// 3. Any `ipv4_address` parses as a plain [`std::net::Ipv4Addr`].
///
/// On success, returns `Ok(())`. On failure, returns an [`ApiError`] with the
/// appropriate status code — callers should propagate it before touching the
/// runtime so we never leave a partially-created container behind.
fn validate_network_attachments(
    attachments: &[NetworkAttachmentRequest],
    registry: Option<&BridgeNetworkApiState>,
) -> Result<()> {
    let Some(registry) = registry else {
        return Err(ApiError::BadRequest(
            "bridge-network attachments requested but no bridge-network runtime is \
             configured on this daemon"
                .to_string(),
        ));
    };

    for attach in attachments {
        if attach.network.trim().is_empty() {
            return Err(ApiError::BadRequest(
                "network attachment entry has empty 'network' field".to_string(),
            ));
        }
        if registry.resolve_network_id(&attach.network).is_none() {
            return Err(ApiError::NotFound(format!(
                "bridge network '{}' not found",
                attach.network
            )));
        }
        if let Some(ip) = attach.ipv4_address.as_deref() {
            ip.parse::<std::net::Ipv4Addr>().map_err(|e| {
                ApiError::BadRequest(format!(
                    "network attachment for '{}': invalid ipv4_address '{ip}': {e}",
                    attach.network
                ))
            })?;
        }
    }

    Ok(())
}

/// Connect a started container to every network in `attachments`, rolling
/// back partial progress (disconnect + stop + remove) on any failure.
///
/// The caller MUST only invoke this once the container is successfully
/// started, because:
/// 1. Docker requires a live container handle for `connect_network`.
/// 2. Rollback assumes it is safe to call `runtime.stop_container` +
///    `runtime.remove_container` on the id.
///
/// On success, also records the attachments in the in-memory registry via
/// [`BridgeNetworkApiState::attach_to_registry`] so subsequent
/// `GET /container-networks/{id}` inspect calls reflect the connection.
async fn attach_container_to_networks(
    registry: Option<&BridgeNetworkApiState>,
    runtime: &Arc<dyn Runtime + Send + Sync>,
    container_id: &ContainerId,
    container_name: Option<&str>,
    attachments: &[NetworkAttachmentRequest],
) -> Result<()> {
    // `validate_network_attachments` has already returned early when
    // `registry` is `None`, so an empty registry here is a logic error.
    let Some(registry) = registry else {
        return Err(ApiError::Internal(
            "bridge-network attachments reached runtime step with no registry; \
             this should have been rejected during validation"
                .to_string(),
        ));
    };

    let Some(bridge_runtime) = registry.runtime.as_ref() else {
        // The registry exists but has no runtime wired — e.g. Docker was not
        // reachable at daemon startup. We cannot actually attach to anything,
        // so fail the request rather than silently dropping the attachments.
        return Err(ApiError::Internal(
            "bridge-network runtime not attached; cannot connect container to user-defined networks"
                .to_string(),
        ));
    };

    // Bollard needs the Docker-visible container name, which the daemon
    // always spawns as `zlayer-{service}-{replica}` (see
    // `zlayer-agent::runtimes::docker::container_name`).
    let docker_name = runtime_container_name(container_id);

    // Track successful attachments so we can best-effort roll them back on
    // failure. Stored as (resolved_network_id, docker_container_name).
    let mut completed: Vec<(String, String)> = Vec::with_capacity(attachments.len());

    for attach in attachments {
        // Resolve again at this point — the set of networks could conceivably
        // have changed between validate and here, and we want an up-to-date
        // id. If it vanished, surface the original name to the caller.
        let Some(network_id) = registry.resolve_network_id(&attach.network) else {
            rollback_attachments(registry.runtime.as_ref(), runtime, container_id, &completed)
                .await;
            return Err(ApiError::NotFound(format!(
                "bridge network '{}' not found",
                attach.network
            )));
        };

        let attachment = BridgeNetworkAttachment {
            container_id: docker_name.clone(),
            container_name: container_name.map(str::to_string),
            aliases: attach.aliases.clone(),
            ipv4: attach.ipv4_address.clone(),
        };

        if let Err(e) = bridge_runtime.connect(&network_id, &attachment).await {
            rollback_attachments(registry.runtime.as_ref(), runtime, container_id, &completed)
                .await;
            return Err(ApiError::Internal(format!(
                "failed to attach container to bridge network '{}': {e}",
                attach.network
            )));
        }

        // Mirror the runtime-side success into the in-memory registry so
        // `GET /container-networks/{id}` reflects the attachment.
        if let Err(e) = registry.attach_to_registry(&network_id, attachment) {
            // This should be unreachable -- we just resolved the id -- but if
            // it does happen, roll back for safety.
            rollback_attachments(registry.runtime.as_ref(), runtime, container_id, &completed)
                .await;
            return Err(e);
        }

        completed.push((network_id, docker_name.clone()));
    }

    Ok(())
}

/// Best-effort rollback helper used by [`attach_container_to_networks`] when
/// an attach fails partway through. Disconnects any already-attached networks
/// on the runtime side, then stops + removes the container. Swallows errors
/// from the rollback itself so the caller sees only the original attach
/// failure.
async fn rollback_attachments(
    bridge_runtime: Option<&Arc<dyn crate::handlers::container_networks::BridgeNetworkRuntime>>,
    runtime: &Arc<dyn Runtime + Send + Sync>,
    container_id: &ContainerId,
    completed: &[(String, String)],
) {
    if let Some(br) = bridge_runtime {
        for (network_id, docker_name) in completed {
            if let Err(e) = br.disconnect(network_id, docker_name).await {
                debug!(
                    network = %network_id,
                    container = %docker_name,
                    error = %e,
                    "best-effort disconnect during rollback failed"
                );
            }
        }
    }

    // Give the container a short grace period before we force-remove.
    if let Err(e) = runtime
        .stop_container(container_id, Duration::from_secs(5))
        .await
    {
        debug!(
            container_id = %container_id,
            error = %e,
            "best-effort stop during rollback failed"
        );
    }
    if let Err(e) = runtime.remove_container(container_id).await {
        debug!(
            container_id = %container_id,
            error = %e,
            "best-effort remove during rollback failed"
        );
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Create and start a container.
///
/// Pulls the image if needed, creates the container, and starts it.
/// Returns the container info including its assigned ID.
///
/// # Errors
///
/// Returns an error if image pull fails, container creation fails, or the
/// user lacks the operator role.
#[utoipa::path(
    post,
    path = "/api/v1/containers",
    request_body = CreateContainerRequest,
    responses(
        (status = 201, description = "Container created and started", body = ContainerInfo),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
#[allow(clippy::too_many_lines)]
pub async fn create_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Json(request): Json<CreateContainerRequest>,
) -> Result<(axum::http::StatusCode, Json<ContainerInfo>)> {
    user.require_role("operator")?;

    // Validate image
    if request.image.is_empty() {
        return Err(ApiError::BadRequest("Image is required".to_string()));
    }

    // Validate name (if provided) - alphanumeric, hyphens, underscores only
    if let Some(ref name) = request.name {
        if name.is_empty() || name.len() > 128 {
            return Err(ApiError::BadRequest(
                "Name must be 1-128 characters".to_string(),
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ApiError::BadRequest(
                "Name must contain only alphanumeric characters, hyphens, and underscores"
                    .to_string(),
            ));
        }

        // Check for duplicate name
        let containers = state.containers.read().await;
        let duplicate = containers
            .values()
            .any(|c| c.name.as_deref() == Some(name.as_str()));
        if duplicate {
            return Err(ApiError::Conflict(format!(
                "Container with name '{name}' already exists"
            )));
        }
    }

    // Pre-validate bridge-network attachments: every referenced network must
    // already exist in the registry (if attached) and any static IPv4 must
    // parse. We do this up-front — before the image pull — so we fail fast
    // without side effects when the request is malformed.
    if !request.networks.is_empty() {
        validate_network_attachments(&request.networks, state.bridge_networks.as_ref())?;
    }

    let (id_str, container_id) = generate_container_id(request.name.as_deref());
    let mut spec = build_service_spec(&request)?;

    // Stamp the reserved `com.zlayer.container_id` label onto the spec so the
    // runtime forwards it into `Config.Labels`. The hex is the same value the
    // id-map will register below (and is what we surface as the public ID),
    // so reconciliation logic can identify ZLayer-managed containers from a
    // foreign Docker daemon listing without consulting the id-map.
    let hex_id = compute_hex(state.id_map.daemon_uuid(), &container_id);
    spec.labels
        .insert(ZLAYER_CONTAINER_ID_LABEL.to_string(), hex_id.clone());

    info!(
        container_id = %container_id,
        image = %request.image,
        name = ?request.name,
        "Creating standalone container"
    );

    // Pull the image with the requested policy
    let pull_policy = match request.pull_policy.as_deref() {
        Some("always") => zlayer_spec::PullPolicy::Always,
        Some("never") => zlayer_spec::PullPolicy::Never,
        _ => zlayer_spec::PullPolicy::IfNotPresent,
    };
    // §3.10: resolve inline / stored registry credentials (inline wins).
    let resolved_auth = resolve_registry_auth(
        request.registry_auth.as_ref(),
        request.registry_credential_id.as_deref(),
        state.registry_store.as_deref(),
    )
    .await?;
    state
        .runtime
        .pull_image_with_policy(&request.image, pull_policy, resolved_auth.as_ref())
        .await
        .map_err(|e| {
            ApiError::Internal(format!("Failed to pull image '{}': {e}", request.image))
        })?;

    // Create the container
    state
        .runtime
        .create_container(&container_id, &spec)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create container: {e}")))?;

    // Start the container
    state
        .runtime
        .start_container(&container_id)
        .await
        .map_err(|e| {
            // Best-effort cleanup: remove the created-but-not-started container
            let rt = state.runtime.clone();
            let cid = container_id.clone();
            tokio::spawn(async move {
                let _ = rt.remove_container(&cid).await;
            });
            ApiError::Internal(format!("Failed to start container: {e}"))
        })?;

    // Attach the container to any user-defined bridge networks requested in
    // the body. We do this AFTER the container is started so Docker has a
    // concrete container to wire an endpoint up to. If any attachment fails,
    // best-effort roll back already-completed attachments and stop/remove
    // the container before returning the error.
    if !request.networks.is_empty() {
        attach_container_to_networks(
            state.bridge_networks.as_ref(),
            &state.runtime,
            &container_id,
            request.name.as_deref(),
            &request.networks,
        )
        .await?;
    }

    // Get PID
    let pid = state
        .runtime
        .get_container_pid(&container_id)
        .await
        .ok()
        .flatten();

    let now = chrono::Utc::now().to_rfc3339();

    // Track the container. Persist to the storage backend FIRST so a daemon
    // crash between the persist and the cache write leaves the next restart
    // with a recoverable record; the in-memory cache is then updated only
    // after the persistent write succeeded.
    let standalone = StandaloneContainer {
        container_id: container_id.clone(),
        image: request.image.clone(),
        name: request.name.clone(),
        labels: request.labels.clone(),
        created_at: now.clone(),
        delete_on_exit: request.lifecycle.delete_on_exit,
    };

    state.standalone_storage.insert(standalone.clone()).await?;

    state
        .containers
        .write()
        .await
        .insert(id_str.clone(), standalone);

    // Register the container in the id-map so REST and Docker compat clients
    // can address it by its 64-char hex form. The daemon UUID stamped onto the
    // map is used as the salt; the resulting hex is what we surface in the
    // response and in subsequent list/inspect output.
    //
    // We already computed `hex_id` up-front (so we could stamp the
    // `com.zlayer.container_id` label onto the spec before the runtime call);
    // `register` is idempotent and deterministic, so calling it here just
    // populates the bidirectional map without recomputing the hash. The
    // returned hex is identical to the one we computed earlier; we discard it
    // and continue using the value we already have.
    let _ = state
        .id_map
        .register(state.id_map.daemon_uuid(), &container_id);

    // Emit container.start event on the daemon-wide bus.
    state.event_bus.publish(ContainerEvent::start(
        hex_id.clone(),
        request.labels.clone(),
    ));

    let info = ContainerInfo {
        id: hex_id,
        name: request.name,
        image: request.image,
        state: "running".to_string(),
        labels: request.labels,
        created_at: now,
        pid,
        ports: Vec::new(),
        networks: Vec::new(),
        ipv4: None,
        health: None,
        exit_code: None,
    };

    Ok((axum::http::StatusCode::CREATED, Json(info)))
}

/// List standalone containers.
///
/// Returns all containers managed through this API. Optionally filter by
/// label using the `label` query parameter in `key=value` format.
///
/// # Errors
///
/// Returns an error if the user is not authenticated.
#[utoipa::path(
    get,
    path = "/api/v1/containers",
    params(ListContainersQuery),
    responses(
        (status = 200, description = "List of containers", body = Vec<ContainerInfo>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn list_containers(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Query(query): Query<ListContainersQuery>,
) -> Result<Json<Vec<ContainerInfo>>> {
    let containers = state.containers.read().await;

    // Parse label filter if provided
    let label_filter: Option<(String, String)> = query.label.and_then(|l| {
        let parts: Vec<&str> = l.splitn(2, '=').collect();
        if parts.len() == 2 {
            Some((parts[0].to_string(), parts[1].to_string()))
        } else {
            None
        }
    });

    let mut results = Vec::with_capacity(containers.len());

    for (_id_str, meta) in containers.iter() {
        // Apply label filter
        if let Some((ref key, ref value)) = label_filter {
            match meta.labels.get(key) {
                Some(v) if v == value => {}
                _ => continue,
            }
        }

        // Query runtime state
        let runtime_state = state
            .runtime
            .container_state(&meta.container_id)
            .await
            .unwrap_or(ContainerState::Failed {
                reason: "state unavailable".to_string(),
            });

        let pid = state
            .runtime
            .get_container_pid(&meta.container_id)
            .await
            .ok()
            .flatten();

        // §3.15: pull richer inspect fields (ports, networks, ipv4, health,
        // exit_code) — runtimes that don't implement this just return the
        // default empty record, so the response stays backwards compatible.
        let inspect: InspectFields = state
            .runtime
            .inspect_detailed(&meta.container_id)
            .await
            .unwrap_or_default()
            .into();

        // Surface the hex container id so REST + Docker compat clients see
        // a stable 64-char identifier rather than the internal service-name
        // storage key.
        let hex_id = state
            .id_map
            .lookup_container(&meta.container_id)
            .unwrap_or_else(|| {
                state
                    .id_map
                    .register(state.id_map.daemon_uuid(), &meta.container_id)
            });

        results.push(ContainerInfo {
            id: hex_id,
            name: meta.name.clone(),
            image: meta.image.clone(),
            state: state_to_string(&runtime_state),
            labels: meta.labels.clone(),
            created_at: meta.created_at.clone(),
            pid,
            ports: inspect.ports,
            networks: inspect.networks,
            ipv4: inspect.ipv4,
            health: inspect.health,
            exit_code: inspect.exit_code,
        });
    }

    Ok(Json(results))
}

/// Get details for a specific container.
///
/// # Errors
///
/// Returns an error if the container is not found or the user is not authenticated.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    responses(
        (status = 200, description = "Container details", body = ContainerInfo),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn get_container(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
) -> Result<Json<ContainerInfo>> {
    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;

    let containers = state.containers.read().await;
    let meta = containers
        .get(&resolved.storage_key)
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;

    let runtime_state = state
        .runtime
        .container_state(&meta.container_id)
        .await
        .unwrap_or(ContainerState::Failed {
            reason: "state unavailable".to_string(),
        });

    let pid = state
        .runtime
        .get_container_pid(&meta.container_id)
        .await
        .ok()
        .flatten();

    // §3.15: pull richer inspect fields (ports, networks, ipv4, health,
    // exit_code) — runtimes that don't implement this just return the default
    // empty record, so the response stays backwards compatible.
    let inspect: InspectFields = state
        .runtime
        .inspect_detailed(&meta.container_id)
        .await
        .unwrap_or_default()
        .into();

    // Always emit the hex form so clients see a stable identifier whether
    // they addressed the container by hex, hex prefix, or service-name.
    let hex_id = state
        .id_map
        .lookup_container(&resolved.container_id)
        .unwrap_or_else(|| {
            state
                .id_map
                .register(state.id_map.daemon_uuid(), &resolved.container_id)
        });

    Ok(Json(ContainerInfo {
        id: hex_id,
        name: meta.name.clone(),
        image: meta.image.clone(),
        state: state_to_string(&runtime_state),
        labels: meta.labels.clone(),
        created_at: meta.created_at.clone(),
        pid,
        ports: inspect.ports,
        networks: inspect.networks,
        ipv4: inspect.ipv4,
        health: inspect.health,
        exit_code: inspect.exit_code,
    }))
}

/// Stop and remove a container.
///
/// Sends a stop signal (with a 30-second timeout), then removes the container.
///
/// # Errors
///
/// Returns an error if the container is not found, stop/remove fails, or the
/// user lacks the operator role.
#[utoipa::path(
    delete,
    path = "/api/v1/containers/{id}",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    responses(
        (status = 204, description = "Container stopped and removed"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn delete_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode> {
    user.require_role("operator")?;

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;

    let labels = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&resolved.storage_key)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.labels.clone()
    };
    let container_id = resolved.container_id.clone();
    let hex_id = state
        .id_map
        .lookup_container(&container_id)
        .unwrap_or_else(|| {
            state
                .id_map
                .register(state.id_map.daemon_uuid(), &container_id)
        });

    info!(container_id = %container_id, "Stopping and removing standalone container");

    delete_standalone_container(&state, &container_id, &resolved.storage_key, true).await?;

    // Emit container.die event after successful stop+remove.
    state.event_bus.publish(ContainerEvent::die(
        hex_id,
        labels,
        None,
        Some("deleted".to_string()),
    ));

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Stop+remove a standalone container and drop its bookkeeping.
///
/// This is the shared core of the `DELETE /api/v1/containers/{id}` REST path
/// and the daemon-side auto-remove subscriber
/// ([`super::auto_remove::start_auto_remove_subscriber`]). It performs, in
/// order:
///
/// 1. Best-effort `runtime.stop_container` (ignored if already stopped).
/// 2. `runtime.remove_container` — fatal on error when `expect_present` is
///    `true`; treated as already-gone (returns `Ok`) when `false`.
/// 3. Persistent-storage delete.
/// 4. Cache eviction.
/// 5. `id_map` unregister so a future container with the same service-name
///    doesn't collide with a stale hex entry.
///
/// The caller is responsible for any event-bus emission. Splitting that out
/// is deliberate: the REST path emits a synthetic `container.die` to mark the
/// administrative deletion, while the auto-remove subscriber is reacting to a
/// real `container.die` and must not republish one.
///
/// `expect_present` controls how aggressive the runtime-remove failure is:
/// the REST path passes `true` (the user just looked the record up), the
/// auto-remove subscriber passes `false` so a race with another deleter — or
/// a runtime that has already cleaned up the bundle — is silently absorbed.
///
/// # Errors
///
/// Returns the storage layer's error if the persistent delete fails, or an
/// `Internal` error wrapping the runtime-remove failure when
/// `expect_present == true`.
pub(super) async fn delete_standalone_container(
    state: &ContainerApiState,
    container_id: &ContainerId,
    storage_key: &str,
    expect_present: bool,
) -> Result<()> {
    // Stop with 30s timeout (ignore errors if container is already stopped)
    let stop_result = state
        .runtime
        .stop_container(container_id, Duration::from_secs(30))
        .await;
    if let Err(ref e) = stop_result {
        debug!(error = %e, "Stop returned error (container may already be stopped)");
    }

    // Remove the container.
    if let Err(e) = state.runtime.remove_container(container_id).await {
        if expect_present {
            return Err(ApiError::Internal(format!(
                "Failed to remove container: {e}"
            )));
        }
        debug!(error = %e, "Remove returned error (container may already be gone)");
    }

    // Remove from persistent storage first, then drop the cache entry. As
    // with create, the persist step happens before the cache mutation so a
    // crash between the two leaves the next restart's repopulate step in
    // sync with disk rather than re-introducing a ghost entry.
    state
        .standalone_storage
        .delete(&container_id.to_string())
        .await?;

    // Remove from tracking
    state.containers.write().await.remove(storage_key);

    // Drop the hex<->ContainerId mapping so a future container with the same
    // service-name doesn't collide with a stale entry.
    state.id_map.unregister_by_container(container_id);

    Ok(())
}

/// Stop a running container.
///
/// Sends the runtime's graceful stop signal and waits up to `timeout` seconds
/// before force-killing. The container is **not** removed; use
/// `DELETE /api/v1/containers/{id}` to stop-and-remove. Idempotent: calling
/// stop on an already-stopped container is not an error.
///
/// # Errors
///
/// Returns an error if the container is not found, stop fails, or the user
/// lacks the operator role.
#[utoipa::path(
    post,
    path = "/api/v1/containers/{id}/stop",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    request_body = StopContainerRequest,
    responses(
        (status = 204, description = "Container stopped"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn stop_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Json(request): Json<StopContainerRequest>,
) -> Result<axum::http::StatusCode> {
    user.require_role("operator")?;

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();
    let labels = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&resolved.storage_key)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.labels.clone()
    };
    let hex_id = state
        .id_map
        .lookup_container(&container_id)
        .unwrap_or_else(|| {
            state
                .id_map
                .register(state.id_map.daemon_uuid(), &container_id)
        });

    let timeout = Duration::from_secs(request.timeout.unwrap_or(30));

    info!(container_id = %container_id, timeout_secs = timeout.as_secs(), "Stopping standalone container");

    state
        .runtime
        .stop_container(&container_id, timeout)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            other => ApiError::Internal(format!("Failed to stop container: {other}")),
        })?;

    // Emit container.die event after successful stop.
    state.event_bus.publish(ContainerEvent::die(
        hex_id,
        labels,
        None,
        Some("stopped".to_string()),
    ));

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Start a previously-created container.
///
/// Useful for re-starting a container that was stopped via
/// `POST /api/v1/containers/{id}/stop` without being removed.
///
/// # Errors
///
/// Returns an error if the container is not found, start fails, or the user
/// lacks the operator role.
#[utoipa::path(
    post,
    path = "/api/v1/containers/{id}/start",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    responses(
        (status = 204, description = "Container started"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn start_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode> {
    user.require_role("operator")?;

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();
    let labels = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&resolved.storage_key)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.labels.clone()
    };
    let hex_id = state
        .id_map
        .lookup_container(&container_id)
        .unwrap_or_else(|| {
            state
                .id_map
                .register(state.id_map.daemon_uuid(), &container_id)
        });

    info!(container_id = %container_id, "Starting standalone container");

    state
        .runtime
        .start_container(&container_id)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            other => ApiError::Internal(format!("Failed to start container: {other}")),
        })?;

    // Emit container.start event after successful start.
    state
        .event_bus
        .publish(ContainerEvent::start(hex_id, labels));

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Restart a container: stop then start.
///
/// Composes `stop_container(timeout)` followed by `start_container`. Errors
/// during the stop phase are ignored (the container may already be stopped);
/// errors during the start phase are surfaced.
///
/// # Errors
///
/// Returns an error if the container is not found, the start phase fails, or
/// the user lacks the operator role.
#[utoipa::path(
    post,
    path = "/api/v1/containers/{id}/restart",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    request_body = RestartContainerRequest,
    responses(
        (status = 204, description = "Container restarted"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn restart_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Json(request): Json<RestartContainerRequest>,
) -> Result<axum::http::StatusCode> {
    user.require_role("operator")?;

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();
    let labels = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&resolved.storage_key)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.labels.clone()
    };
    let hex_id = state
        .id_map
        .lookup_container(&container_id)
        .unwrap_or_else(|| {
            state
                .id_map
                .register(state.id_map.daemon_uuid(), &container_id)
        });

    let timeout = Duration::from_secs(request.timeout.unwrap_or(30));

    info!(container_id = %container_id, timeout_secs = timeout.as_secs(), "Restarting standalone container");

    // Best-effort stop: ignore errors (container may already be stopped).
    let stop_ok = state
        .runtime
        .stop_container(&container_id, timeout)
        .await
        .is_ok();
    if stop_ok {
        // Emit container.die event for the stop half of the restart.
        state.event_bus.publish(ContainerEvent::die(
            hex_id.clone(),
            labels.clone(),
            None,
            Some("restarting".to_string()),
        ));
    } else {
        debug!("Stop returned error during restart (container may already be stopped)");
    }

    state
        .runtime
        .start_container(&container_id)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            other => {
                ApiError::Internal(format!("Failed to start container during restart: {other}"))
            }
        })?;

    // Emit container.start event for the start half of the restart.
    state
        .event_bus
        .publish(ContainerEvent::start(hex_id, labels));

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Send a signal to a running container.
///
/// Mirrors Docker-compat `POST /containers/{id}/kill`. When the request body's
/// `signal` field is omitted, the runtime sends `SIGKILL`. Accepted signals
/// are `SIGKILL`, `SIGTERM`, `SIGINT`, `SIGHUP`, `SIGUSR1`, `SIGUSR2` (with
/// or without the `SIG` prefix); any other value is rejected with `400`.
///
/// # Errors
///
/// Returns `400` for unknown signals, `404` if the container is not found,
/// `403` if the caller lacks the `operator` role, `501` if the runtime does
/// not support `kill_container`, and `500` for other runtime errors.
#[utoipa::path(
    post,
    path = "/api/v1/containers/{id}/kill",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    request_body = KillContainerRequest,
    responses(
        (status = 204, description = "Signal delivered"),
        (status = 400, description = "Invalid signal"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 501, description = "Runtime does not support kill"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn kill_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Json(request): Json<KillContainerRequest>,
) -> Result<axum::http::StatusCode> {
    user.require_role("operator")?;

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();
    let labels = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&resolved.storage_key)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.labels.clone()
    };
    let hex_id = state
        .id_map
        .lookup_container(&container_id)
        .unwrap_or_else(|| {
            state
                .id_map
                .register(state.id_map.daemon_uuid(), &container_id)
        });

    info!(
        container_id = %container_id,
        signal = ?request.signal,
        "Killing standalone container"
    );

    state
        .runtime
        .kill_container(&container_id, request.signal.as_deref())
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            zlayer_agent::AgentError::InvalidSpec(reason) => ApiError::BadRequest(reason),
            zlayer_agent::AgentError::Unsupported(reason) => {
                ApiError::Internal(format!("Runtime does not support kill: {reason}"))
            }
            other => ApiError::Internal(format!("Failed to kill container: {other}")),
        })?;

    // Emit container.die event after a successful signal delivery. Note the
    // container may not be fully dead yet (e.g. SIGTERM with a catching
    // process), but the kill API contract treats this as the lifecycle
    // transition point -- mirrors Docker event semantics.
    let reason = request
        .signal
        .as_deref()
        .map_or_else(|| "killed".to_string(), |s| format!("killed:{s}"));
    state
        .event_bus
        .publish(ContainerEvent::die(hex_id, labels, None, Some(reason)));

    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Pause / Unpause / Top / Changes / Port / Prune (tasks 5.5.1–5.5.6)
// ---------------------------------------------------------------------------

/// Pause a running container by freezing its cgroup.
///
/// Mirrors Docker-compat `POST /containers/{id}/pause`. Returns 204 on
/// success, 404 if the container isn't found, 501 if the runtime cannot
/// pause containers, and 500 for other runtime errors.
///
/// # Errors
///
/// Returns an error if authentication fails, the container is missing, the
/// runtime doesn't support pause, or the underlying call fails.
#[utoipa::path(
    post,
    path = "/api/v1/containers/{id}/pause",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    responses(
        (status = 204, description = "Container paused"),
        (status = 400, description = "Container not in a pausable state"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 501, description = "Runtime does not support pause"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn pause_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode> {
    user.require_role("operator")?;
    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();

    info!(container_id = %container_id, "Pausing standalone container");

    state
        .runtime
        .pause_container(&container_id)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            zlayer_agent::AgentError::InvalidSpec(reason) => ApiError::BadRequest(reason),
            zlayer_agent::AgentError::Unsupported(reason) => ApiError::NotImplemented(reason),
            other => ApiError::Internal(format!("Failed to pause container: {other}")),
        })?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Resume a previously-paused container.
///
/// Mirrors Docker-compat `POST /containers/{id}/unpause`. Returns 204 on
/// success.
///
/// # Errors
///
/// Returns the same error envelope as [`pause_container`].
#[utoipa::path(
    post,
    path = "/api/v1/containers/{id}/unpause",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    responses(
        (status = 204, description = "Container unpaused"),
        (status = 400, description = "Container not in a resumable state"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 501, description = "Runtime does not support unpause"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn unpause_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode> {
    user.require_role("operator")?;
    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();

    info!(container_id = %container_id, "Unpausing standalone container");

    state
        .runtime
        .unpause_container(&container_id)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            zlayer_agent::AgentError::InvalidSpec(reason) => ApiError::BadRequest(reason),
            zlayer_agent::AgentError::Unsupported(reason) => ApiError::NotImplemented(reason),
            other => ApiError::Internal(format!("Failed to unpause container: {other}")),
        })?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// List the processes running inside a container.
///
/// Mirrors Docker-compat `GET /containers/{id}/top?ps_args=`. Returns the
/// runtime's process listing as `Titles` + `Processes` matrix.
///
/// # Errors
///
/// Returns 404 when the container can't be resolved, 501 when the runtime
/// doesn't expose process listings, and 500 for other failures.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/top",
    params(
        ("id" = String, Path, description = "Container identifier"),
        ContainerTopQuery,
    ),
    responses(
        (status = 200, description = "Process listing", body = ContainerTopResponse),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 501, description = "Runtime does not support top"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn top_container(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Query(q): Query<ContainerTopQuery>,
) -> Result<Json<ContainerTopResponse>> {
    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();

    // Split the optional ps_args string on whitespace; an empty / missing
    // value yields an empty slice that the runtime interprets as "use
    // defaults".
    let ps_args: Vec<String> = q
        .ps_args
        .as_deref()
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();

    let output = state
        .runtime
        .top_container(&container_id, &ps_args)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            zlayer_agent::AgentError::InvalidSpec(reason) => ApiError::BadRequest(reason),
            zlayer_agent::AgentError::Unsupported(reason) => ApiError::NotImplemented(reason),
            other => ApiError::Internal(format!("Failed to list container processes: {other}")),
        })?;

    Ok(Json(ContainerTopResponse {
        titles: output.titles,
        processes: output.processes,
    }))
}

/// Report changes to the container's filesystem.
///
/// Mirrors Docker-compat `GET /containers/{id}/changes`. Returns one
/// `ContainerChangeEntry` per added / modified / deleted path in the
/// container's writable layer. Runtimes without a layered filesystem
/// (e.g. youki) return 501.
///
/// # Errors
///
/// Returns 404 when the container can't be resolved, 501 when the runtime
/// doesn't compute layer diffs.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/changes",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    responses(
        (status = 200, description = "Filesystem changes", body = Vec<ContainerChangeEntry>),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 501, description = "Runtime does not support filesystem diffs"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn changes_container(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<ContainerChangeEntry>>> {
    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();

    let entries = state
        .runtime
        .changes_container(&container_id)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            zlayer_agent::AgentError::Unsupported(reason) => ApiError::NotImplemented(reason),
            other => ApiError::Internal(format!("Failed to fetch changes: {other}")),
        })?;

    let docker_entries: Vec<ContainerChangeEntry> = entries
        .into_iter()
        .map(|e| ContainerChangeEntry {
            path: e.path,
            kind: e.kind.as_docker_kind(),
        })
        .collect();
    Ok(Json(docker_entries))
}

/// Report the published port mappings for a container.
///
/// Mirrors Docker-compat `GET /containers/{id}/port`. Groups the runtime's
/// port-binding entries by `<container_port>/<protocol>` so the wire shape
/// matches Docker's `{"Ports": {"80/tcp": [...]}}` body.
///
/// # Errors
///
/// Returns 404 when the container can't be resolved, 501 when the runtime
/// doesn't expose port bindings.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/port",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    responses(
        (status = 200, description = "Port mappings", body = ContainerPortResponse),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 501, description = "Runtime does not support port-mapping inspection"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn port_container(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
) -> Result<Json<ContainerPortResponse>> {
    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();

    let entries = state
        .runtime
        .port_mappings_container(&container_id)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            zlayer_agent::AgentError::Unsupported(reason) => ApiError::NotImplemented(reason),
            other => ApiError::Internal(format!("Failed to fetch port mappings: {other}")),
        })?;

    // Group bindings by `"{container_port}/{protocol}"` key. Entries with no
    // host binding yield `Some(empty Vec)` so consumers can distinguish
    // "exposed but unpublished" from "key missing entirely".
    let mut grouped: HashMap<String, Option<Vec<ContainerPortBinding>>> = HashMap::new();
    for entry in entries {
        let key = format!("{}/{}", entry.container_port, entry.protocol);
        let bucket = grouped.entry(key).or_insert_with(|| Some(Vec::new()));
        match (entry.host_ip.as_deref(), entry.host_port) {
            (None, None) => {
                // exposed-only entry: the bucket already has a Vec; nothing
                // to push, since Docker's wire form distinguishes "exposed"
                // by *empty Vec*.
            }
            _ => {
                if let Some(bindings) = bucket {
                    bindings.push(ContainerPortBinding {
                        host_ip: entry.host_ip,
                        host_port: entry.host_port.map(|p| p.to_string()),
                    });
                }
            }
        }
    }

    Ok(Json(ContainerPortResponse { ports: grouped }))
}

// ---------------------------------------------------------------------------
// Archive (docker cp / TAR)
// ---------------------------------------------------------------------------

/// Query parameters for `GET|HEAD /api/v1/containers/{id}/archive`.
///
/// Mirrors Docker's `path=` query parameter. The `path` is the absolute path
/// inside the container that should be archived (or stat'd).
#[derive(Debug, Default, serde::Deserialize)]
pub struct ArchivePathQuery {
    /// Container-side path to archive or stat.
    #[serde(default)]
    pub path: Option<String>,
}

/// Query parameters for `PUT /api/v1/containers/{id}/archive`.
///
/// Mirrors Docker's `path=` plus `noOverwriteDirNonDir=` and `copyUIDGID=`.
/// Both flags accept the literal `"1"` / `"true"` to enable, anything else
/// (or missing) is treated as disabled.
#[derive(Debug, Default, serde::Deserialize)]
pub struct ArchivePutQuery {
    /// Container-side path to extract the TAR archive into.
    #[serde(default)]
    pub path: Option<String>,
    /// Reject the put when an entry would replace a directory with a
    /// non-directory (or vice versa). `"1"` / `"true"` enables.
    #[serde(default, rename = "noOverwriteDirNonDir")]
    pub no_overwrite_dir_non_dir: Option<String>,
    /// Preserve UID/GID of files in the archive verbatim. `"1"` / `"true"`
    /// enables.
    #[serde(default, rename = "copyUIDGID")]
    pub copy_uid_gid: Option<String>,
}

/// Parse a Docker-style boolean query parameter (`"1"` / `"true"` enables).
fn parse_bool_flag(s: Option<&str>) -> bool {
    matches!(
        s.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes")
    )
}

/// Build the `X-Docker-Container-Path-Stat` header value from a [`PathStat`].
///
/// Returns the base64-encoded JSON expected by Docker clients on the wire.
fn encode_path_stat_header(stat: &PathStat) -> Result<String> {
    use base64::Engine;
    // Wire shape uses Docker's PascalCase field names so existing Docker
    // CLIs decode it without translation.
    let payload = serde_json::json!({
        "name": stat.name,
        "size": stat.size,
        "mode": stat.mode,
        "mtime": stat.mtime,
        "linkTarget": stat.link_target,
    });
    let bytes = serde_json::to_vec(&payload)
        .map_err(|e| ApiError::Internal(format!("failed to serialize path stat: {e}")))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Map a runtime archive error onto the matching `ApiError` variant.
fn map_archive_error(e: zlayer_agent::AgentError, ctx: &str) -> ApiError {
    match e {
        zlayer_agent::AgentError::NotFound { reason, .. } => {
            ApiError::NotFound(format!("{ctx}: {reason}"))
        }
        zlayer_agent::AgentError::Unsupported(reason) => ApiError::NotImplemented(reason),
        zlayer_agent::AgentError::InvalidSpec(reason) => ApiError::BadRequest(reason),
        other => ApiError::Internal(format!("{ctx}: {other}")),
    }
}

/// `GET /api/v1/containers/{id}/archive?path=<...>` — stream a TAR archive
/// of the requested file or directory inside the container.
///
/// Returns `application/x-tar` bytes. The `X-Docker-Container-Path-Stat`
/// header is also emitted (matching Docker's behaviour) so callers can
/// inspect the path's metadata without a separate HEAD round-trip.
///
/// # Errors
///
/// * `400 Bad Request` when the `path` query parameter is missing.
/// * `404 Not Found` when the container or path does not exist.
/// * `501 Not Implemented` when the runtime cannot produce a TAR archive.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/archive",
    params(
        ("id" = String, Path, description = "Container identifier"),
        ("path" = String, Query, description = "Container-side path to archive"),
    ),
    responses(
        (status = 200, description = "TAR archive (application/x-tar)"),
        (status = 400, description = "Missing path query parameter"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Container or path not found"),
        (status = 501, description = "Runtime does not support archive download"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn archive_get(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Query(q): Query<ArchivePathQuery>,
) -> Result<Response> {
    let path = q
        .path
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError::BadRequest("missing required 'path' query parameter".into()))?;

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();

    // Probe with HEAD first so the path-stat header always reflects the
    // archive about to be streamed.
    let stat = state
        .runtime
        .archive_head(&container_id, &path)
        .await
        .map_err(|e| map_archive_error(e, "archive_head"))?;
    let header_value = encode_path_stat_header(&stat)?;

    let stream = state
        .runtime
        .archive_get(&container_id, &path)
        .await
        .map_err(|e| map_archive_error(e, "archive_get"))?;

    let mapped = stream.map(|item| -> std::result::Result<Bytes, Infallible> {
        match item {
            Ok(b) => Ok(b),
            Err(e) => {
                debug!(error = %e, "archive_get stream error; truncating body");
                Ok(Bytes::new())
            }
        }
    });

    let body = Body::from_stream(mapped);
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-tar"),
    );
    response.headers_mut().insert(
        "X-Docker-Container-Path-Stat",
        HeaderValue::from_str(&header_value).map_err(|e| {
            ApiError::Internal(format!(
                "failed to encode X-Docker-Container-Path-Stat: {e}"
            ))
        })?,
    );
    Ok(response)
}

/// `PUT /api/v1/containers/{id}/archive?path=<...>` — extract a TAR
/// archive into the container at the given path.
///
/// Accepts an `application/x-tar` body. Honours Docker's
/// `noOverwriteDirNonDir` and `copyUIDGID` query parameters.
///
/// # Errors
///
/// * `400 Bad Request` when the `path` query parameter is missing or the
///   archive payload is malformed.
/// * `404 Not Found` when the container or destination path does not exist.
/// * `501 Not Implemented` when the runtime cannot extract archives.
#[utoipa::path(
    put,
    path = "/api/v1/containers/{id}/archive",
    params(
        ("id" = String, Path, description = "Container identifier"),
        ("path" = String, Query, description = "Container-side path to extract into"),
    ),
    request_body(
        content = Vec<u8>,
        content_type = "application/x-tar",
        description = "Uncompressed TAR archive to extract into the container",
    ),
    responses(
        (status = 200, description = "Archive extracted"),
        (status = 400, description = "Missing path or invalid archive"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 404, description = "Container or destination not found"),
        (status = 501, description = "Runtime does not support archive upload"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn archive_put(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Query(q): Query<ArchivePutQuery>,
    body: Bytes,
) -> Result<StatusCode> {
    user.require_role("operator")?;

    let path = q
        .path
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError::BadRequest("missing required 'path' query parameter".into()))?;

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();

    let opts = ArchivePutOptions {
        no_overwrite_dir_non_dir: parse_bool_flag(q.no_overwrite_dir_non_dir.as_deref()),
        copy_uid_gid: parse_bool_flag(q.copy_uid_gid.as_deref()),
    };

    state
        .runtime
        .archive_put(&container_id, &path, body, opts)
        .await
        .map_err(|e| map_archive_error(e, "archive_put"))?;

    Ok(StatusCode::OK)
}

/// `HEAD /api/v1/containers/{id}/archive?path=<...>` — return path-stat
/// metadata in the `X-Docker-Container-Path-Stat` header without
/// materializing the TAR archive.
///
/// # Errors
///
/// * `400 Bad Request` when the `path` query parameter is missing.
/// * `404 Not Found` when the container or path does not exist.
/// * `501 Not Implemented` when the runtime cannot stat container paths.
#[utoipa::path(
    head,
    path = "/api/v1/containers/{id}/archive",
    params(
        ("id" = String, Path, description = "Container identifier"),
        ("path" = String, Query, description = "Container-side path to stat"),
    ),
    responses(
        (status = 200, description = "Path stat header set"),
        (status = 400, description = "Missing path query parameter"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Container or path not found"),
        (status = 501, description = "Runtime does not support archive head"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn archive_head(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Query(q): Query<ArchivePathQuery>,
) -> Result<Response> {
    let path = q
        .path
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError::BadRequest("missing required 'path' query parameter".into()))?;

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();

    let stat = state
        .runtime
        .archive_head(&container_id, &path)
        .await
        .map_err(|e| map_archive_error(e, "archive_head"))?;
    let header_value = encode_path_stat_header(&stat)?;

    let mut response = Response::new(Body::empty());
    response.headers_mut().insert(
        "X-Docker-Container-Path-Stat",
        HeaderValue::from_str(&header_value).map_err(|e| {
            ApiError::Internal(format!(
                "failed to encode X-Docker-Container-Path-Stat: {e}"
            ))
        })?,
    );
    Ok(response)
}

/// Prune stopped containers from the runtime.
///
/// Mirrors Docker-compat `POST /containers/prune`. Returns the IDs of
/// containers that were removed plus the bytes reclaimed.
///
/// # Errors
///
/// Returns 501 when the runtime doesn't support a global prune sweep.
#[utoipa::path(
    post,
    path = "/api/v1/containers/prune",
    responses(
        (status = 200, description = "Prune result", body = ContainerPruneResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 501, description = "Runtime does not support container prune"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn prune_containers(
    user: AuthUser,
    State(state): State<ContainerApiState>,
) -> Result<Json<ContainerPruneResponse>> {
    user.require_role("operator")?;

    info!("Pruning stopped containers");

    let result = state
        .runtime
        .prune_containers()
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::Unsupported(reason) => ApiError::NotImplemented(reason),
            other => ApiError::Internal(format!("Failed to prune containers: {other}")),
        })?;

    Ok(Json(ContainerPruneResponse {
        containers_deleted: result.deleted,
        space_reclaimed: result.space_reclaimed,
    }))
}

// ---------------------------------------------------------------------------
// Logs streaming
// ---------------------------------------------------------------------------

/// Build a [`LogsStreamOptions`] from the parsed query string. Implements the
/// "neither stdout nor stderr explicitly set" Docker default: both are
/// streamed.
fn logs_stream_options_from_query(query: &ContainerLogQuery) -> LogsStreamOptions {
    let stdout = query.stdout.unwrap_or(true);
    let stderr = query.stderr.unwrap_or(true);
    // If a caller explicitly sends `stdout=false&stderr=false`, the Docker
    // contract is "include both" rather than "include nothing" (sending no
    // stream is a useless request). Mirror that.
    let (stdout, stderr) = if !stdout && !stderr {
        (true, true)
    } else {
        (stdout, stderr)
    };
    let tail = match query.tail {
        0 => None,
        n => Some(n as u64),
    };
    LogsStreamOptions {
        follow: query.follow,
        tail,
        since: query.since,
        until: query.until,
        timestamps: query.timestamps,
        stdout,
        stderr,
    }
}

/// Encode a `LogChunk` as Docker's multiplexed stdcopy frame.
///
/// `zlayer-docker` already exposes
/// [`zlayer_docker::socket::streaming::log_frame::encode_frame`], but
/// `zlayer-docker` depends on `zlayer-api` (the Docker compat shim re-uses
/// API state types) — adding the reverse dep would create a cycle. The
/// header is fixed and trivial (8 bytes: 1 stream id, 3 zero-padding, 4
/// big-endian length), so we mirror it locally and keep the canonical
/// encoder in `zlayer-docker` as the single source of truth for the
/// compat shim.
fn encode_docker_log_frame(channel: LogChannel, payload: &[u8]) -> Bytes {
    use bytes::{BufMut, BytesMut};
    let stream_id: u8 = match channel {
        LogChannel::Stdin => 0,
        LogChannel::Stdout => 1,
        LogChannel::Stderr => 2,
    };
    // Truncate any payload that overflows a `u32` length field; Docker's
    // wire format leaves no other choice and runtime chunks are far below
    // this in practice.
    let len_u32 = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let len_usize = len_u32 as usize;
    let mut buf = BytesMut::with_capacity(8 + len_usize);
    buf.put_u8(stream_id);
    buf.put_u8(0);
    buf.put_u8(0);
    buf.put_u8(0);
    buf.put_u32(len_u32);
    buf.extend_from_slice(&payload[..len_usize]);
    buf.freeze()
}

/// Boxed NDJSON / raw-frame stream returned to `axum::body::Body::from_stream`
/// by both [`get_container_logs`] and [`get_container_stats`]. Aliased so
/// the handler signatures don't trip `clippy::type_complexity`.
type StreamingBody = Pin<Box<dyn Stream<Item = std::result::Result<Bytes, Infallible>> + Send>>;

/// Render one [`LogChunk`] as a single NDJSON line (including the trailing
/// newline). Bytes that aren't valid UTF-8 are base64-encoded and emitted
/// under `data_b64` so the stream remains a well-formed JSON document.
fn ndjson_line_for_chunk(chunk: &LogChunk) -> Bytes {
    use base64::Engine as _;
    let stream_str = match chunk.stream {
        LogChannel::Stdin => "stdin",
        LogChannel::Stdout => "stdout",
        LogChannel::Stderr => "stderr",
    };
    let timestamp = chunk.timestamp.map(|ts| ts.to_rfc3339());
    let mut payload = serde_json::Map::new();
    payload.insert(
        "stream".to_string(),
        serde_json::Value::String(stream_str.to_string()),
    );
    if let Some(ts) = timestamp {
        payload.insert("timestamp".to_string(), serde_json::Value::String(ts));
    }
    if let Ok(s) = std::str::from_utf8(&chunk.bytes) {
        payload.insert("data".to_string(), serde_json::Value::String(s.to_string()));
    } else {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&chunk.bytes);
        payload.insert("data_b64".to_string(), serde_json::Value::String(encoded));
    }
    let mut bytes =
        serde_json::to_vec(&serde_json::Value::Object(payload)).unwrap_or_else(|_| b"{}".to_vec());
    bytes.push(b'\n');
    Bytes::from(bytes)
}

/// Encode a stream-error (the `Err(AgentError)` arm of a `LogsStream` item)
/// as a final JSON line so clients learn why their stream stopped instead of
/// seeing a silent connection close.
fn ndjson_line_for_error(err: &zlayer_agent::AgentError) -> Bytes {
    let mut bytes = serde_json::to_vec(&serde_json::json!({
        "error": err.to_string(),
    }))
    .unwrap_or_else(|_| b"{\"error\":\"unknown\"}".to_vec());
    bytes.push(b'\n');
    Bytes::from(bytes)
}

/// Stream container logs.
///
/// Backed by [`Runtime::logs_stream`]. Two wire formats are supported:
///
/// * `format=json` (the default) — emit one NDJSON `LogChunk` per line:
///   `{"stream":"stdout|stderr","timestamp":"...","data":"<utf8>"}` (or
///   `data_b64` when the bytes aren't valid UTF-8). `Content-Type:
///   application/json`.
///
/// * `format=raw` — emit Docker's multiplexed stdcopy framing
///   (`application/vnd.docker.raw-stream`). Consumed by the Docker compat
///   shim in `zlayer-docker`.
///
/// `follow=true` keeps the stream open until the runtime reports EOF or the
/// client disconnects. Stream errors mid-flight are emitted as a final
/// `{"error": "..."}` line (json mode) or simply terminate the body (raw
/// mode) — Docker's raw framing has no in-band error escape.
///
/// # Errors
///
/// Returns `404` when the container can't be resolved, `500` on a runtime
/// failure to open the stream.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/logs",
    params(
        ("id" = String, Path, description = "Container identifier"),
        ContainerLogQuery,
    ),
    responses(
        (status = 200, description = "Container logs (NDJSON when format=json, Docker stdcopy frames when format=raw)", body = String),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn get_container_logs(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Query(query): Query<ContainerLogQuery>,
) -> Result<Response> {
    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id;

    let opts = logs_stream_options_from_query(&query);
    let format = query.format.unwrap_or_default();

    let log_stream = state
        .runtime
        .logs_stream(&container_id, opts)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            other => ApiError::Internal(format!("Failed to start log stream: {other}")),
        })?;

    let (content_type, body_stream): (&'static str, StreamingBody) = match format {
        ContainerLogFormat::Json => {
            let mapped = log_stream.map(|item| -> std::result::Result<Bytes, Infallible> {
                match item {
                    Ok(chunk) => Ok(ndjson_line_for_chunk(&chunk)),
                    Err(err) => Ok(ndjson_line_for_error(&err)),
                }
            });
            ("application/json", Box::pin(mapped))
        }
        ContainerLogFormat::Raw => {
            let mapped = log_stream.filter_map(|item| async move {
                match item {
                    Ok(chunk) => Some(Ok::<Bytes, Infallible>(encode_docker_log_frame(
                        chunk.stream,
                        &chunk.bytes,
                    ))),
                    Err(err) => {
                        // Raw stdcopy has no error frame; surface mid-stream
                        // failures via tracing and end the body.
                        debug!(error = %err, "log stream error in raw mode; ending body");
                        None
                    }
                }
            });
            ("application/vnd.docker.raw-stream", Box::pin(mapped))
        }
    };

    let body = Body::from_stream(body_stream);
    let mut response = Response::new(body);
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    response
        .headers_mut()
        .insert("X-Accel-Buffering", HeaderValue::from_static("no"));
    Ok(response)
}

/// Execute a command in a running container.
///
/// When `stream=true` is passed as a query parameter, this endpoint upgrades
/// to a Server-Sent Events stream emitting:
/// - `event: stdout\ndata: <line>\n\n` for each stdout line
/// - `event: stderr\ndata: <line>\n\n` for each stderr line
/// - `event: exit\ndata: {"exit_code": N}\n\n` as the final event
///
/// # Errors
///
/// Returns an error if the container is not found, the command is invalid,
/// execution fails, or the user lacks the operator role.
#[utoipa::path(
    post,
    path = "/api/v1/containers/{id}/exec",
    params(
        ("id" = String, Path, description = "Container identifier"),
        ExecQuery,
    ),
    request_body = ContainerExecRequest,
    responses(
        (status = 200, description = "Command executed (JSON body when stream=false, SSE stream when stream=true)", body = ContainerExecResponse),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn exec_in_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Query(query): Query<ExecQuery>,
    Json(request): Json<ContainerExecRequest>,
) -> Result<Response> {
    user.require_role("operator")?;

    if request.command.is_empty() {
        return Err(ApiError::BadRequest("Command cannot be empty".to_string()));
    }

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id;

    if query.stream {
        let events = state
            .runtime
            .exec_stream(&container_id, &request.command)
            .await
            .map_err(|e| match e {
                zlayer_agent::AgentError::NotFound { reason, .. } => {
                    ApiError::NotFound(format!("Container not found: {reason}"))
                }
                other => ApiError::Internal(format!("Exec failed: {other}")),
            })?;

        let sse_stream = events.map(|event| -> std::result::Result<Event, Infallible> {
            match event {
                ExecEvent::Stdout(line) => Ok(Event::default().event("stdout").data(line)),
                ExecEvent::Stderr(line) => Ok(Event::default().event("stderr").data(line)),
                ExecEvent::Exit(code) => {
                    let payload = serde_json::json!({ "exit_code": code }).to_string();
                    Ok(Event::default().event("exit").data(payload))
                }
            }
        });

        let sse = Sse::new(sse_stream).keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        );
        return Ok(sse.into_response());
    }

    let (exit_code, stdout, stderr) = state
        .runtime
        .exec(&container_id, &request.command)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            other => ApiError::Internal(format!("Exec failed: {other}")),
        })?;

    Ok(Json(ContainerExecResponse {
        exit_code,
        stdout,
        stderr,
    })
    .into_response())
}

/// Query parameters for `POST /api/v1/containers/{id}/resize`.
///
/// Mirrors Docker's `POST /containers/{id}/resize?h=<rows>&w=<cols>` so the
/// Docker compatibility shim can pass them through verbatim.
#[derive(Debug, Default, serde::Deserialize)]
pub struct ResizeQuery {
    /// New PTY height in rows.
    #[serde(default)]
    pub h: Option<u16>,
    /// New PTY width in columns.
    #[serde(default)]
    pub w: Option<u16>,
}

/// Resize the main PTY of a running container.
///
/// Forwards `(rows, cols)` to the runtime-side resize sender registered when
/// the container was started with TTY enabled. Returns:
///
/// * `400 Bad Request` when the container has no main PTY (i.e. it wasn't
///   started with TTY) or when `h`/`w` are missing.
/// * `404 Not Found` when the container can't be resolved.
/// * `409 Conflict` when the runtime's resize channel has been closed (the
///   container's PTY is gone).
/// * `200 OK` on success.
///
/// # Errors
///
/// See the status codes above. Resize hints are best-effort: if the bounded
/// channel is full, the oldest hint is dropped server-side and the call still
/// returns `200`.
pub async fn resize_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Query(query): Query<ResizeQuery>,
) -> Result<StatusCode> {
    user.require_role("operator")?;

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id;

    let (Some(rows), Some(cols)) = (query.h, query.w) else {
        return Err(ApiError::BadRequest(
            "resize requires both 'h' (rows) and 'w' (cols) query parameters".to_string(),
        ));
    };

    let sender = state
        .container_pty_resizers
        .get(&container_id)
        .map(|s| s.clone())
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Container '{id}' has no main PTY; resize requires the container \
                 to have been started with TTY enabled"
            ))
        })?;

    match sender.try_send((rows, cols)) {
        Ok(()) => Ok(StatusCode::OK),
        Err(mpsc::error::TrySendError::Full(_)) => {
            // Best-effort: drop the hint and return success. The resize
            // channel is bounded and a full channel just means we're getting
            // resize events faster than the runtime can apply them.
            debug!(
                container_id = %container_id,
                "container PTY resize channel full; dropping oldest hint"
            );
            Ok(StatusCode::OK)
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            // Channel closed: the runtime side has dropped the receiver,
            // which means the PTY is no longer alive. Drop the stale entry
            // so subsequent calls return the cleaner "no PTY" 400.
            state.container_pty_resizers.remove(&container_id);
            Err(ApiError::Conflict(format!(
                "Container '{id}' PTY is no longer active"
            )))
        }
    }
}

/// Wait for a container to exit and return its exit code.
///
/// This endpoint blocks until the container exits. Useful for CI runners
/// that need to wait for a build/test container to complete.
///
/// # Errors
///
/// Returns an error if the container is not found or the wait fails.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/wait",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    responses(
        (status = 200, description = "Container exited", body = ContainerWaitResponse),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn wait_container(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
) -> Result<Json<ContainerWaitResponse>> {
    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();
    let labels = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&resolved.storage_key)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.labels.clone()
    };
    let hex_id = state
        .id_map
        .lookup_container(&container_id)
        .unwrap_or_else(|| {
            state
                .id_map
                .register(state.id_map.daemon_uuid(), &container_id)
        });

    let outcome = state
        .runtime
        .wait_outcome(&container_id)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            other => ApiError::Internal(format!("Wait failed: {other}")),
        })?;

    // Convert the typed `WaitReason` to its snake_case wire form for both the
    // HTTP body and the event-bus payload.
    let reason_wire = serde_json::to_value(outcome.reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned));
    let finished_at_wire = outcome.finished_at.map(|ts| ts.to_rfc3339());

    // Emit container.die with the observed exit code and classified reason.
    // When the runtime reports an OOM kill, also emit a `container.oom`
    // event so subscribers of that channel see it too.
    state.event_bus.publish(ContainerEvent::die(
        hex_id.clone(),
        labels.clone(),
        Some(outcome.exit_code),
        reason_wire.clone(),
    ));
    if outcome.reason == zlayer_agent::runtime::WaitReason::OomKilled {
        state.event_bus.publish(ContainerEvent::oom(
            hex_id.clone(),
            labels,
            Some(outcome.exit_code),
            Some("oom_killed".to_string()),
        ));
    }

    Ok(Json(ContainerWaitResponse {
        id: hex_id,
        exit_code: outcome.exit_code,
        reason: reason_wire,
        signal: outcome.signal,
        finished_at: finished_at_wire,
    }))
}

/// Wait for a container to exit and return Docker-shaped JSON.
///
/// Mirrors Docker Engine's `POST /containers/{id}/wait?condition=` 1:1.
/// Used by the `zlayer-docker` compatibility shim and any SDK callers
/// that consume the Docker shape directly. The legacy
/// `GET /api/v1/containers/{id}/wait` returns a richer
/// [`ContainerWaitResponse`] for clients that need extended classification
/// fields (`reason`, `signal`, `finished_at`).
///
/// `condition` is one of:
/// * `"not-running"` (default) — block until the container is no longer running.
/// * `"next-exit"` — wait for the next observed exit.
/// * `"removed"` — wait until the container is removed.
///
/// # Errors
///
/// Returns `400` for an unknown condition, `404` when the container can't
/// be resolved, and `500` if the runtime fails the wait.
#[utoipa::path(
    post,
    path = "/api/v1/containers/{id}/wait",
    params(
        ("id" = String, Path, description = "Container identifier"),
        WaitContainerQuery,
    ),
    responses(
        (status = 200, description = "Container reached the requested condition", body = ContainerWaitDockerResponse),
        (status = 400, description = "Invalid condition"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn wait_container_post(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Query(q): Query<WaitContainerQuery>,
) -> Result<Json<ContainerWaitDockerResponse>> {
    let condition = match q.condition.as_deref() {
        None | Some("") => zlayer_agent::runtime::WaitCondition::NotRunning,
        Some(other) => zlayer_agent::runtime::WaitCondition::from_wire_str(other)
            .ok_or_else(|| ApiError::BadRequest(format!("invalid wait condition '{other}'")))?,
    };

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();
    let labels = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&resolved.storage_key)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.labels.clone()
    };
    let hex_id = state
        .id_map
        .lookup_container(&container_id)
        .unwrap_or_else(|| {
            state
                .id_map
                .register(state.id_map.daemon_uuid(), &container_id)
        });

    match state
        .runtime
        .wait_outcome_with_condition(&container_id, condition)
        .await
    {
        Ok(outcome) => {
            // Emit container.die so subscribers see the lifecycle transition.
            let reason_wire = serde_json::to_value(outcome.reason)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned));
            state.event_bus.publish(ContainerEvent::die(
                hex_id,
                labels,
                Some(outcome.exit_code),
                reason_wire,
            ));
            Ok(Json(ContainerWaitDockerResponse {
                status_code: i64::from(outcome.exit_code),
                error: None,
            }))
        }
        Err(zlayer_agent::AgentError::NotFound { reason, .. }) => {
            Err(ApiError::NotFound(format!("Container not found: {reason}")))
        }
        Err(other) => Ok(Json(ContainerWaitDockerResponse {
            status_code: -1,
            error: Some(ContainerWaitDockerError {
                message: format!("{other}"),
            }),
        })),
    }
}

/// Rename a standalone container.
///
/// Mirrors Docker Engine's `POST /containers/{id}/rename?name=<new>`
/// endpoint: the container's stored display name is updated to `name`,
/// and the underlying runtime is asked to apply the rename so external
/// tooling (e.g. `docker ps`) sees the new name.
///
/// # Errors
///
/// * `400` if the `name` query parameter is missing or empty.
/// * `404` if the container cannot be resolved.
/// * `409` if the runtime rejects the rename (e.g. name already in use).
/// * `500` for other runtime failures.
#[utoipa::path(
    post,
    path = "/api/v1/containers/{id}/rename",
    params(
        ("id" = String, Path, description = "Container identifier"),
        RenameContainerQuery,
    ),
    responses(
        (status = 204, description = "Container renamed"),
        (status = 400, description = "Missing or empty `name` query parameter"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn rename_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Query(q): Query<RenameContainerQuery>,
) -> Result<axum::http::StatusCode> {
    user.require_role("operator")?;

    let new_name = q.name.unwrap_or_default();
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(ApiError::BadRequest(
            "missing or empty `name` query parameter".to_string(),
        ));
    }

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();
    let storage_key = resolved.storage_key.clone();

    info!(
        container_id = %container_id,
        new_name = %new_name,
        "Renaming standalone container"
    );

    // Try the runtime first; if the runtime can't rename (e.g. Youki returns
    // `Unsupported`), surface the error rather than silently updating the
    // metadata cache.
    state
        .runtime
        .rename_container(&container_id, new_name)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            zlayer_agent::AgentError::Unsupported(reason) => {
                ApiError::Internal(format!("Runtime does not support rename: {reason}"))
            }
            other => {
                let msg = format!("{other}");
                if msg.to_ascii_lowercase().contains("already")
                    && msg.to_ascii_lowercase().contains("use")
                {
                    ApiError::Conflict(format!("Container name '{new_name}' already in use"))
                } else {
                    ApiError::Internal(format!("Failed to rename container: {other}"))
                }
            }
        })?;

    // Update the in-memory metadata so subsequent `inspect` calls reflect
    // the new name. The persistent store mirrors the cache on container
    // create / delete; rename is intentionally cache-only because the
    // hex id (and storage key) remain unchanged.
    {
        let mut containers = state.containers.write().await;
        if let Some(meta) = containers.get_mut(&storage_key) {
            meta.name = Some(new_name.to_string());
        }
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Update a standalone container's resource limits and/or restart policy.
///
/// Mirrors Docker Engine's `POST /containers/{id}/update`. The request body
/// matches Docker's wire shape (`CpuShares`, `Memory`, `RestartPolicy`,
/// ...) so the `zlayer-docker` compatibility shim can pass it through
/// unchanged. Returns Docker's `{"Warnings": [...]}` response on success;
/// the warnings vector is always present (possibly empty) so clients that
/// match on field presence don't break.
///
/// # Errors
///
/// * `404` if the container cannot be resolved.
/// * `400` if the runtime rejects a field as invalid.
/// * `501` if the runtime does not support resource updates (the trait
///   default returns `Unsupported`).
/// * `500` for other runtime failures.
#[utoipa::path(
    post,
    path = "/api/v1/containers/{id}/update",
    params(
        ("id" = String, Path, description = "Container identifier"),
    ),
    request_body = ContainerUpdateRequest,
    responses(
        (status = 200, description = "Update applied", body = ContainerUpdateResponse),
        (status = 400, description = "Invalid update field"),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 501, description = "Runtime does not support update"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn update_container(
    user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Json(body): Json<ContainerUpdateRequest>,
) -> Result<Json<ContainerUpdateResponse>> {
    user.require_role("operator")?;

    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();

    info!(container_id = %container_id, "Updating standalone container resources");

    let restart_policy = body.restart_policy.as_ref().map(|rp| {
        zlayer_agent::runtime::ContainerRestartPolicyUpdate {
            name: rp.name.clone(),
            maximum_retry_count: rp.maximum_retry_count,
        }
    });

    let update = zlayer_agent::runtime::ContainerResourceUpdate {
        cpu_shares: body.cpu_shares,
        memory: body.memory,
        cpu_period: body.cpu_period,
        cpu_quota: body.cpu_quota,
        cpu_realtime_period: body.cpu_realtime_period,
        cpu_realtime_runtime: body.cpu_realtime_runtime,
        cpuset_cpus: body.cpuset_cpus.clone(),
        cpuset_mems: body.cpuset_mems.clone(),
        memory_reservation: body.memory_reservation,
        memory_swap: body.memory_swap,
        kernel_memory: body.kernel_memory,
        blkio_weight: body.blkio_weight,
        pids_limit: body.pids_limit,
        restart_policy,
    };

    let outcome = state
        .runtime
        .update_container_resources(&container_id, &update)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            zlayer_agent::AgentError::InvalidSpec(reason) => ApiError::BadRequest(reason),
            zlayer_agent::AgentError::Unsupported(reason) => ApiError::NotImplemented(reason),
            other => ApiError::Internal(format!("Failed to update container: {other}")),
        })?;

    Ok(Json(ContainerUpdateResponse {
        warnings: outcome.warnings,
    }))
}

// ---------------------------------------------------------------------------
// Stats streaming
// ---------------------------------------------------------------------------

/// Render one [`StatsSample`] as a single NDJSON line, terminated with `\n`.
fn ndjson_line_for_stats(sample: &StatsSample) -> Bytes {
    let mut bytes = serde_json::to_vec(&serde_json::json!({
        "cpu_total_ns": sample.cpu_total_ns,
        "cpu_system_ns": sample.cpu_system_ns,
        "online_cpus": sample.online_cpus,
        "mem_used_bytes": sample.mem_used_bytes,
        "mem_limit_bytes": sample.mem_limit_bytes,
        "net_rx_bytes": sample.net_rx_bytes,
        "net_tx_bytes": sample.net_tx_bytes,
        "blkio_read_bytes": sample.blkio_read_bytes,
        "blkio_write_bytes": sample.blkio_write_bytes,
        "pids_current": sample.pids_current,
        "pids_limit": sample.pids_limit,
        "timestamp": sample.timestamp.to_rfc3339(),
    }))
    .unwrap_or_else(|_| b"{}".to_vec());
    bytes.push(b'\n');
    Bytes::from(bytes)
}

/// Stream container resource statistics.
///
/// Backed by [`Runtime::stats_stream`]. The handler always advertises
/// `Content-Type: application/json` and ships one [`StatsSample`] per NDJSON
/// line.
///
/// * `stream=false` (the default — Docker parity) — await the first
///   `StatsSample` and close the body. Returns `200` with a single line.
/// * `stream=true` — keep the body open, forwarding samples as they arrive
///   until the runtime reports EOF or the client disconnects.
///
/// Mid-stream errors are emitted as a final `{"error":"..."}` line and end
/// the body. The legacy `interval` query parameter is accepted for
/// backwards-compatibility but ignored: pacing is now driven by the
/// runtime's own sampling cadence.
///
/// # Errors
///
/// `404` when the container can't be resolved, `500` if the runtime fails
/// to open the stream.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/stats",
    params(
        ("id" = String, Path, description = "Container identifier"),
        StatsQuery,
    ),
    responses(
        (status = 200, description = "Container statistics (NDJSON; one sample when stream=false, continuous when stream=true)", body = ContainerStatsResponse),
        (status = 404, description = "Container not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Containers"
)]
pub async fn get_container_stats(
    _user: AuthUser,
    State(state): State<ContainerApiState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Response> {
    let resolved = resolve_container_lookup(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
    let container_id = resolved.container_id.clone();
    // Pre-register the hex id so it is allocated before we start streaming.
    // Even though we don't (yet) put the hex id on each `StatsSample` line,
    // the side-effect matches the previous handler so callers that exec a
    // subsequent /containers/{hex} lookup keep working.
    let _ = state
        .id_map
        .lookup_container(&container_id)
        .unwrap_or_else(|| {
            state
                .id_map
                .register(state.id_map.daemon_uuid(), &container_id)
        });

    let stats_stream = state
        .runtime
        .stats_stream(&container_id)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            other => ApiError::Internal(format!("Failed to start stats stream: {other}")),
        })?;

    let body_stream: StreamingBody = if query.stream {
        let mapped = stats_stream.map(|item| -> std::result::Result<Bytes, Infallible> {
            match item {
                Ok(sample) => Ok(ndjson_line_for_stats(&sample)),
                Err(err) => Ok(ndjson_line_for_error(&err)),
            }
        });
        Box::pin(mapped)
    } else {
        // One-shot Docker-compat mode: take exactly the first sample (or
        // error) the runtime produces and close. The handler still uses
        // `take(1)` semantics under the hood, but routing through a `StreamExt`
        // adapter keeps both branches' types identical.
        let first = stats_stream.take(1);
        let mapped = first.map(|item| -> std::result::Result<Bytes, Infallible> {
            match item {
                Ok(sample) => Ok(ndjson_line_for_stats(&sample)),
                Err(err) => Ok(ndjson_line_for_error(&err)),
            }
        });
        Box::pin(mapped)
    };

    let body = Body::from_stream(body_stream);
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    response
        .headers_mut()
        .insert("X-Accel-Buffering", HeaderValue::from_static("no"));
    Ok(response)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_container_request_deserialize() {
        let json = r#"{
            "image": "nginx:latest",
            "name": "my-nginx",
            "env": {"PORT": "8080"},
            "command": ["nginx", "-g", "daemon off;"],
            "labels": {"app": "web", "ci": "true"},
            "resources": {"cpu": 0.5, "memory": "256Mi"},
            "volumes": [{"source": "/data", "target": "/app/data", "readonly": true}],
            "work_dir": "/app"
        }"#;
        let request: CreateContainerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.image, "nginx:latest");
        assert_eq!(request.name.as_deref(), Some("my-nginx"));
        assert_eq!(request.env.get("PORT").unwrap(), "8080");
        assert_eq!(request.command.as_ref().unwrap().len(), 3);
        assert_eq!(request.labels.get("ci").unwrap(), "true");
        assert!(request.resources.is_some());
        assert_eq!(request.volumes.len(), 1);
        assert!(request.volumes[0].readonly);
        assert_eq!(request.work_dir.as_deref(), Some("/app"));
    }

    #[test]
    fn test_create_container_request_minimal() {
        let json = r#"{"image": "alpine:3.19"}"#;
        let request: CreateContainerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.image, "alpine:3.19");
        assert!(request.name.is_none());
        assert!(request.env.is_empty());
        assert!(request.command.is_none());
        assert!(request.labels.is_empty());
        assert!(request.resources.is_none());
        assert!(request.volumes.is_empty());
        assert!(request.work_dir.is_none());
    }

    #[test]
    fn test_container_info_serialize() {
        let info = ContainerInfo {
            id: "standalone-test-rep-0".to_string(),
            name: Some("test".to_string()),
            image: "nginx:latest".to_string(),
            state: "running".to_string(),
            labels: HashMap::from([("app".to_string(), "web".to_string())]),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            pid: Some(12345),
            ports: Vec::new(),
            networks: Vec::new(),
            ipv4: None,
            health: None,
            exit_code: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("standalone-test-rep-0"));
        assert!(json.contains("running"));
        assert!(json.contains("12345"));
    }

    #[test]
    fn test_container_info_serialize_no_optional() {
        let info = ContainerInfo {
            id: "standalone-abc-rep-0".to_string(),
            name: None,
            image: "alpine:latest".to_string(),
            state: "exited(0)".to_string(),
            labels: HashMap::new(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            pid: None,
            ports: Vec::new(),
            networks: Vec::new(),
            ipv4: None,
            health: None,
            exit_code: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("name"));
        assert!(!json.contains("pid"));
        // §3.15: new optional fields must also be skipped when empty / None.
        assert!(!json.contains("ports"));
        assert!(!json.contains("networks"));
        assert!(!json.contains("ipv4"));
        assert!(!json.contains("health"));
        assert!(!json.contains("exit_code"));
    }

    /// §3.15: `ContainerInfo` with all rich fields populated round-trips
    /// through serde correctly and ships every expected key on the wire.
    #[test]
    fn test_container_info_serde_roundtrip_with_rich_fields() {
        let info = ContainerInfo {
            id: "standalone-rich-rep-0".to_string(),
            name: Some("rich".to_string()),
            image: "nginx:latest".to_string(),
            state: "running".to_string(),
            labels: HashMap::new(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            pid: Some(42),
            ports: vec![zlayer_spec::PortMapping {
                host_port: Some(8080),
                container_port: 80,
                protocol: zlayer_spec::PortProtocol::Tcp,
                host_ip: "0.0.0.0".to_string(),
            }],
            networks: vec![NetworkAttachmentInfo {
                network: "bridge".to_string(),
                aliases: vec!["rich".to_string()],
                ipv4: Some("172.17.0.2".to_string()),
            }],
            ipv4: Some("172.17.0.2".to_string()),
            health: Some(ContainerHealthInfo {
                status: "healthy".to_string(),
                failing_streak: Some(0),
                last_output: Some("OK".to_string()),
            }),
            exit_code: None,
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"ports\""), "ports field present");
        assert!(json.contains("\"networks\""), "networks field present");
        assert!(json.contains("\"ipv4\""), "ipv4 field present");
        assert!(json.contains("\"health\""), "health field present");
        assert!(json.contains("\"healthy\""), "health status serialised");
        assert!(json.contains("\"bridge\""), "network name serialised");
        // exit_code is None -> must be skipped.
        assert!(!json.contains("exit_code"));

        let parsed: ContainerInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.ports.len(), 1);
        assert_eq!(parsed.ports[0].host_port, Some(8080));
        assert_eq!(parsed.ports[0].container_port, 80);
        assert_eq!(parsed.networks.len(), 1);
        assert_eq!(parsed.networks[0].network, "bridge");
        assert_eq!(parsed.networks[0].aliases, vec!["rich".to_string()]);
        assert_eq!(parsed.ipv4.as_deref(), Some("172.17.0.2"));
        let health = parsed.health.expect("health parsed");
        assert_eq!(health.status, "healthy");
        assert_eq!(health.failing_streak, Some(0));
        assert_eq!(health.last_output.as_deref(), Some("OK"));
        assert!(parsed.exit_code.is_none());
    }

    /// §3.15: `ContainerInfo` deserialises correctly from payloads that
    /// predate the rich inspect fields — missing fields must default to
    /// empty/None without forcing clients to send them.
    #[test]
    fn test_container_info_deserialize_backwards_compat() {
        let json = r#"{
            "id": "standalone-old-rep-0",
            "image": "alpine:latest",
            "state": "running",
            "labels": {},
            "created_at": "2026-01-01T00:00:00Z"
        }"#;
        let parsed: ContainerInfo = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.id, "standalone-old-rep-0");
        assert!(parsed.ports.is_empty());
        assert!(parsed.networks.is_empty());
        assert!(parsed.ipv4.is_none());
        assert!(parsed.health.is_none());
        assert!(parsed.exit_code.is_none());
    }

    #[test]
    fn test_exec_request_deserialize() {
        let json = r#"{"command": ["echo", "hello"]}"#;
        let request: ContainerExecRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.command, vec!["echo", "hello"]);
    }

    /// `ExecQuery`'s `Default` impl must return `stream=false` so existing
    /// non-streaming clients that send no query string are unaffected.
    #[test]
    fn test_exec_query_default_stream_false() {
        let q = ExecQuery::default();
        assert!(!q.stream);
    }

    /// `?stream=false` explicitly deserialises to the buffered-JSON branch
    /// (we deserialise through `serde_json` as a proxy for `serde_urlencoded`
    /// — both use `Deserialize` the same way for simple structs).
    #[test]
    fn test_exec_query_stream_false_explicit() {
        let q: ExecQuery = serde_json::from_str(r#"{"stream": false}"#).unwrap();
        assert!(!q.stream);
    }

    /// `?stream=true` deserialises to the SSE branch.
    #[test]
    fn test_exec_query_stream_true() {
        let q: ExecQuery = serde_json::from_str(r#"{"stream": true}"#).unwrap();
        assert!(q.stream);
    }

    /// Exercise the default `exec_stream` trait method on a runtime that only
    /// implements buffered `exec`. Verifies the buffered fallback yields the
    /// expected sequence: one `Stdout`, one `Stderr`, one `Exit`.
    #[tokio::test]
    async fn test_default_exec_stream_emits_buffered_events() {
        use futures_util::StreamExt;
        use zlayer_agent::runtime::{ExecEvent, MockRuntime, Runtime};

        let runtime = MockRuntime::new();
        let id = ContainerId::new("test-svc".to_string(), 0);
        // `MockRuntime::exec` returns `(0, cmd.join(" "), "")`, so with a
        // non-empty stdout and empty stderr the default fallback should emit
        // exactly two events: Stdout then Exit.
        let mut stream = runtime
            .exec_stream(&id, &["echo".to_string(), "hi".to_string()])
            .await
            .expect("exec_stream should succeed");

        let mut got = Vec::new();
        while let Some(event) = stream.next().await {
            got.push(event);
        }

        assert_eq!(got.len(), 2, "expected stdout + exit, got {got:?}");
        match &got[0] {
            ExecEvent::Stdout(s) => assert_eq!(s, "echo hi"),
            other => panic!("expected Stdout, got {other:?}"),
        }
        match &got[1] {
            ExecEvent::Exit(code) => assert_eq!(*code, 0),
            other => panic!("expected Exit, got {other:?}"),
        }
    }

    #[test]
    fn test_wait_response_serialize() {
        let response = ContainerWaitResponse {
            id: "test-container".to_string(),
            exit_code: 0,
            reason: None,
            signal: None,
            finished_at: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("test-container"));
        assert!(json.contains('0'));
        // Unset optional fields must be elided from the wire payload so old
        // clients keep seeing exactly the same shape they saw before §3.12.
        assert!(!json.contains("reason"));
        assert!(!json.contains("signal"));
        assert!(!json.contains("finished_at"));
    }

    #[test]
    fn test_wait_response_with_signal_serialize() {
        let response = ContainerWaitResponse {
            id: "sigkill-container".to_string(),
            exit_code: 137,
            reason: Some("signal".to_string()),
            signal: Some("SIGKILL".to_string()),
            finished_at: Some("2026-04-20T12:34:56Z".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"reason\":\"signal\""));
        assert!(json.contains("\"signal\":\"SIGKILL\""));
        assert!(json.contains("\"finished_at\":\"2026-04-20T12:34:56Z\""));
        assert!(json.contains("\"exit_code\":137"));
    }

    #[test]
    fn test_wait_response_oom_roundtrip() {
        // Round-trip: deserialize a representative `oom_killed` payload the
        // Docker runtime would produce and re-serialize it back out. This
        // exercises the exact wire format ZArcRunner will consume.
        let wire = r#"{"id":"x","exit_code":137,"reason":"oom_killed","finished_at":"2026-04-20T12:00:00Z"}"#;
        let parsed: ContainerWaitResponse = serde_json::from_str(wire).unwrap();
        assert_eq!(parsed.reason.as_deref(), Some("oom_killed"));
        assert!(parsed.signal.is_none());
        assert_eq!(parsed.finished_at.as_deref(), Some("2026-04-20T12:00:00Z"));
        let round = serde_json::to_string(&parsed).unwrap();
        assert!(round.contains("\"reason\":\"oom_killed\""));
    }

    #[test]
    fn test_stats_response_serialize() {
        let response = ContainerStatsResponse {
            id: "test-container".to_string(),
            cpu_usage_usec: 1_000_000,
            memory_bytes: 50 * 1024 * 1024,
            memory_limit: 256 * 1024 * 1024,
            memory_percent: 19.53125,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("cpu_usage_usec"));
        assert!(json.contains("memory_percent"));
    }

    #[test]
    fn test_state_to_string() {
        assert_eq!(state_to_string(&ContainerState::Running), "running");
        assert_eq!(state_to_string(&ContainerState::Pending), "pending");
        assert_eq!(
            state_to_string(&ContainerState::Exited { code: 0 }),
            "exited(0)"
        );
        assert_eq!(
            state_to_string(&ContainerState::Failed {
                reason: "oom".to_string()
            }),
            "failed: oom"
        );
    }

    #[test]
    fn test_generate_container_id_with_name() {
        let (id, cid) = generate_container_id(Some("myapp"));
        assert_eq!(id, "standalone-myapp");
        assert_eq!(cid.service, "standalone-myapp");
        assert_eq!(cid.replica, 0);
    }

    #[test]
    fn test_generate_container_id_without_name() {
        let (id, cid) = generate_container_id(None);
        assert!(id.starts_with("standalone-"));
        assert_eq!(cid.service, id);
        assert_eq!(cid.replica, 0);
    }

    #[test]
    fn test_log_query_defaults() {
        let query: ContainerLogQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(query.tail, 100);
        assert!(!query.follow);
    }

    #[test]
    fn test_stats_query_defaults() {
        // Empty query -> one-shot mode (legacy `interval` field is now
        // ignored by the streaming-backed handler but still parses for
        // backwards-compat with older clients).
        let query: StatsQuery = serde_json::from_str("{}").unwrap();
        assert!(!query.stream);
        assert!(query.interval.is_none());
    }

    #[test]
    fn test_stats_query_parses_stream_and_interval() {
        // `stream=true` + explicit `interval`. Both alias forms still parse.
        let query: StatsQuery =
            serde_json::from_str(r#"{"stream": true, "interval": 5}"#).expect("parse stats query");
        assert!(query.stream);
        assert_eq!(query.interval, Some(5));

        let alias: StatsQuery = serde_json::from_str(r#"{"stream": true, "interval_seconds": 7}"#)
            .expect("parse stats query alias");
        assert!(alias.stream);
        assert_eq!(alias.interval, Some(7));
    }

    #[test]
    fn test_list_containers_query_no_label() {
        let query: ListContainersQuery = serde_json::from_str("{}").unwrap();
        assert!(query.label.is_none());
    }

    #[test]
    fn build_service_spec_threads_port_mappings() {
        use zlayer_spec::{PortMapping, PortProtocol};

        let ports = vec![
            PortMapping {
                host_port: Some(8080),
                container_port: 80,
                protocol: PortProtocol::Tcp,
                host_ip: "0.0.0.0".to_string(),
            },
            PortMapping {
                host_port: None,
                container_port: 53,
                protocol: PortProtocol::Udp,
                host_ip: "127.0.0.1".to_string(),
            },
        ];

        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            ports: ports.clone(),
            ..CreateContainerRequest::default()
        };
        let spec = build_service_spec(&request).expect("spec should build");
        assert_eq!(spec.port_mappings, ports);
    }

    #[test]
    fn test_build_service_spec_minimal() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            ..CreateContainerRequest::default()
        };
        let spec = build_service_spec(&request).expect("minimal spec should build");
        assert_eq!(spec.image.name.whole(), "docker.io/library/alpine:latest");
        assert!(spec.env.is_empty());
        assert!(spec.storage.is_empty());
        // With no health_check, the spec falls back to the no-op placeholder.
        assert!(matches!(
            spec.health.check,
            zlayer_spec::HealthCheck::Tcp { port: 0 }
        ));
    }

    #[test]
    fn test_build_service_spec_full() {
        let request = CreateContainerRequest {
            image: "node:20".to_string(),
            name: Some("build-runner".to_string()),
            env: HashMap::from([("NODE_ENV".to_string(), "production".to_string())]),
            command: Some(vec!["node".to_string(), "server.js".to_string()]),
            labels: HashMap::from([("ci".to_string(), "true".to_string())]),
            resources: Some(ContainerResourceLimits {
                cpu: Some(2.0),
                memory: Some("1Gi".to_string()),
            }),
            volumes: vec![VolumeMount {
                mount_type: None,
                source: Some("/workspace".to_string()),
                target: "/app".to_string(),
                readonly: false,
            }],
            work_dir: Some("/app".to_string()),
            ..CreateContainerRequest::default()
        };
        let spec = build_service_spec(&request).expect("full spec should build");
        assert_eq!(spec.image.name.whole(), "docker.io/library/node:20");
        assert_eq!(spec.env.get("NODE_ENV").unwrap(), "production");
        assert_eq!(
            spec.command.entrypoint.as_deref(),
            Some(["node".to_string()].as_slice())
        );
        assert_eq!(
            spec.command.args.as_deref(),
            Some(["server.js".to_string()].as_slice())
        );
        assert_eq!(spec.command.workdir.as_deref(), Some("/app"));
        assert_eq!(spec.resources.cpu, Some(2.0));
        assert_eq!(spec.resources.memory.as_deref(), Some("1Gi"));
        assert_eq!(spec.storage.len(), 1);
    }

    #[test]
    fn test_health_check_request_tcp_ok() {
        let req = HealthCheckRequest {
            check_type: "tcp".to_string(),
            port: Some(5432),
            url: None,
            expect_status: None,
            command: None,
            interval: Some("10s".to_string()),
            timeout: Some("2s".to_string()),
            retries: Some(5),
            start_period: Some("30s".to_string()),
        };
        let spec = health_request_to_spec(&req).expect("valid tcp spec");
        assert!(matches!(
            spec.check,
            zlayer_spec::HealthCheck::Tcp { port: 5432 }
        ));
        assert_eq!(spec.retries, 5);
        assert_eq!(spec.interval, Some(Duration::from_secs(10)));
        assert_eq!(spec.timeout, Some(Duration::from_secs(2)));
        assert_eq!(spec.start_grace, Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_health_check_request_http_defaults() {
        let req = HealthCheckRequest {
            check_type: "http".to_string(),
            port: None,
            url: Some("http://localhost:8080/health".to_string()),
            expect_status: None,
            command: None,
            interval: None,
            timeout: None,
            retries: None,
            start_period: None,
        };
        let spec = health_request_to_spec(&req).expect("valid http spec");
        match spec.check {
            zlayer_spec::HealthCheck::Http { url, expect_status } => {
                assert_eq!(url, "http://localhost:8080/health");
                assert_eq!(expect_status, 200);
            }
            other => panic!("expected Http, got {other:?}"),
        }
        assert_eq!(spec.retries, 3);
        // interval falls back to 30s default when omitted.
        assert_eq!(spec.interval, Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_health_check_request_command_joins_argv() {
        let req = HealthCheckRequest {
            check_type: "command".to_string(),
            port: None,
            url: None,
            expect_status: None,
            command: Some(vec![
                "pg_isready".to_string(),
                "-U".to_string(),
                "postgres".to_string(),
            ]),
            interval: None,
            timeout: None,
            retries: None,
            start_period: None,
        };
        let spec = health_request_to_spec(&req).expect("valid command spec");
        match spec.check {
            zlayer_spec::HealthCheck::Command { command } => {
                assert_eq!(command, "pg_isready -U postgres");
            }
            other => panic!("expected Command, got {other:?}"),
        }
    }

    #[test]
    fn test_health_check_request_unknown_type() {
        let req = HealthCheckRequest {
            check_type: "bogus".to_string(),
            port: None,
            url: None,
            expect_status: None,
            command: None,
            interval: None,
            timeout: None,
            retries: None,
            start_period: None,
        };
        let err = health_request_to_spec(&req).expect_err("unknown type must error");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("bogus"),
            "error should mention bad type: {msg}"
        );
    }

    #[test]
    fn test_health_check_request_tcp_missing_port() {
        let req = HealthCheckRequest {
            check_type: "tcp".to_string(),
            port: None,
            url: None,
            expect_status: None,
            command: None,
            interval: None,
            timeout: None,
            retries: None,
            start_period: None,
        };
        assert!(health_request_to_spec(&req).is_err());
    }

    #[test]
    fn test_health_check_request_tcp_port_zero_rejected() {
        let req = HealthCheckRequest {
            check_type: "tcp".to_string(),
            port: Some(0),
            url: None,
            expect_status: None,
            command: None,
            interval: None,
            timeout: None,
            retries: None,
            start_period: None,
        };
        assert!(health_request_to_spec(&req).is_err());
    }

    #[test]
    fn test_health_check_request_command_empty_rejected() {
        let req = HealthCheckRequest {
            check_type: "command".to_string(),
            port: None,
            url: None,
            expect_status: None,
            command: Some(Vec::new()),
            interval: None,
            timeout: None,
            retries: None,
            start_period: None,
        };
        assert!(health_request_to_spec(&req).is_err());
    }

    #[test]
    fn test_health_check_request_bad_humantime() {
        let req = HealthCheckRequest {
            check_type: "tcp".to_string(),
            port: Some(80),
            url: None,
            expect_status: None,
            command: None,
            interval: Some("not-a-duration".to_string()),
            timeout: None,
            retries: None,
            start_period: None,
        };
        let err = health_request_to_spec(&req).expect_err("invalid humantime must error");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("not-a-duration"),
            "error should mention offending input: {msg}"
        );
    }

    #[test]
    fn test_create_container_request_health_check_deserialize() {
        let json = r#"{
            "image": "postgres:16",
            "health_check": {
                "type": "command",
                "command": ["pg_isready", "-U", "postgres"],
                "interval": "10s",
                "timeout": "5s",
                "retries": 3,
                "start_period": "30s"
            }
        }"#;
        let request: CreateContainerRequest = serde_json::from_str(json).unwrap();
        let hc = request
            .health_check
            .as_ref()
            .expect("health_check should parse");
        assert_eq!(hc.check_type, "command");
        assert_eq!(hc.command.as_deref().unwrap().len(), 3);
        assert_eq!(hc.retries, Some(3));
    }

    #[test]
    fn test_container_api_state_new() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ContainerApiState::new(runtime);
        // State builds without error
        assert!(Arc::strong_count(&state.containers) >= 1);
    }

    /// 1.4.3 — proves `with_standalone_storage` + `repopulate_cache_from_storage`
    /// actually round-trip records out of the storage trait into the in-memory
    /// `containers` cache, keyed by `ContainerId::service` (the same key
    /// `create_container` writes with). This is the contract the daemon
    /// startup path depends on.
    #[tokio::test]
    async fn repopulate_cache_loads_records_from_storage() {
        use crate::storage::InMemoryStandaloneContainerStorage;

        let storage = Arc::new(InMemoryStandaloneContainerStorage::new());

        // Seed the storage with two records — one "named" container and one
        // anonymous — that the cache should pick up on repopulate.
        let alpha = StandaloneContainer {
            container_id: ContainerId::new("standalone-alpha".to_string(), 0),
            image: "alpine:latest".to_string(),
            name: Some("alpha".to_string()),
            labels: HashMap::new(),
            created_at: "2026-05-03T00:00:00Z".to_string(),
            delete_on_exit: false,
        };
        let beta = StandaloneContainer {
            container_id: ContainerId::new("standalone-beta".to_string(), 0),
            image: "alpine:3.20".to_string(),
            name: None,
            labels: HashMap::new(),
            created_at: "2026-05-03T00:00:01Z".to_string(),
            delete_on_exit: false,
        };
        storage.insert(alpha.clone()).await.unwrap();
        storage.insert(beta.clone()).await.unwrap();

        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ContainerApiState::new(runtime).with_standalone_storage(storage.clone());

        // Cache is empty before repopulate.
        assert_eq!(state.containers.read().await.len(), 0);

        state.repopulate_cache_from_storage().await.unwrap();

        let cache = state.containers.read().await;
        assert_eq!(cache.len(), 2);
        assert!(
            cache.contains_key("standalone-alpha"),
            "cache must be keyed by ContainerId::service, not by the full \
             '{{service}}-rep-{{replica}}' storage key"
        );
        assert!(cache.contains_key("standalone-beta"));
        assert_eq!(cache["standalone-alpha"].image, "alpine:latest");
        assert_eq!(cache["standalone-beta"].image, "alpine:3.20");
    }

    // -----------------------------------------------------------------------
    // Phase 1 §1.3.3 — `resolve_container_lookup` accepts hex, hex prefix,
    // and the legacy service-name storage key.
    // -----------------------------------------------------------------------

    /// Build a `ContainerApiState` with a fixed daemon UUID and a single
    /// pre-registered container so the helper has something to find. The
    /// state is keyed by the service-name string the way `create_container`
    /// keys it in production.
    async fn make_state_with_container(
        service: &str,
        replica: u32,
        daemon_uuid: &str,
    ) -> (ContainerApiState, ContainerId, String) {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ContainerApiState::with_daemon_uuid(runtime, daemon_uuid.to_string());
        let cid = ContainerId::new(service.to_string(), replica);
        let hex = state.id_map.register(state.id_map.daemon_uuid(), &cid);

        let standalone = StandaloneContainer {
            container_id: cid.clone(),
            image: "alpine:latest".to_string(),
            name: Some(service.to_string()),
            labels: HashMap::new(),
            created_at: "2026-05-03T00:00:00Z".to_string(),
            delete_on_exit: false,
        };
        state
            .containers
            .write()
            .await
            .insert(service.to_string(), standalone);
        (state, cid, hex)
    }

    #[tokio::test]
    async fn resolve_container_id_accepts_hex() {
        let (state, cid, hex) =
            make_state_with_container("standalone-demo", 0, "test-daemon-uuid").await;
        let resolved = resolve_container_lookup(&state, &hex)
            .await
            .expect("full hex must resolve");
        assert_eq!(resolved.container_id, cid);
        assert_eq!(resolved.storage_key, "standalone-demo");
        assert_eq!(hex.len(), 64);
    }

    #[tokio::test]
    async fn resolve_container_id_accepts_short_hex() {
        let (state, cid, hex) =
            make_state_with_container("standalone-demo", 0, "test-daemon-uuid").await;
        let prefix: String = hex.chars().take(12).collect();
        let resolved = resolve_container_lookup(&state, &prefix)
            .await
            .expect("12-char hex prefix must resolve uniquely");
        assert_eq!(resolved.container_id, cid);
        assert_eq!(resolved.storage_key, "standalone-demo");
    }

    #[tokio::test]
    async fn resolve_container_id_falls_back_to_name() {
        let (state, cid, _hex) =
            make_state_with_container("standalone-demo", 0, "test-daemon-uuid").await;
        let resolved = resolve_container_lookup(&state, "standalone-demo")
            .await
            .expect("legacy service-name lookup must still work");
        assert_eq!(resolved.container_id, cid);
        assert_eq!(resolved.storage_key, "standalone-demo");
    }

    #[tokio::test]
    async fn resolve_container_id_returns_none_for_unknown() {
        let (state, _cid, _hex) =
            make_state_with_container("standalone-demo", 0, "test-daemon-uuid").await;
        assert!(resolve_container_lookup(&state, "no-such-container")
            .await
            .is_none());
    }

    // -----------------------------------------------------------------------
    // Tasks 5.1.1-3 / 5.2.1-4 — POST /wait?condition= and /rename?name=
    //
    // These tests exercise the daemon-side handlers directly with a
    // MockRuntime-backed `ContainerApiState`. The MockRuntime's default
    // `wait_outcome_with_condition` falls through to `wait_outcome` →
    // `wait_container` (which returns 0), so the wait test pins the
    // exit-code-zero path through the Docker-shaped response. The rename
    // test uses a tiny custom runtime that records the call.
    // -----------------------------------------------------------------------

    /// `POST /api/v1/containers/{id}/wait` happy path: a known container,
    /// no `condition` param, `MockRuntime` returns 0 → response carries
    /// `status_code: 0` and no `error` envelope.
    /// Lightweight runtime override used by the wait/rename handler tests.
    /// Delegates everything to `MockRuntime` except `wait_outcome` /
    /// `wait_outcome_with_condition`, which return a fixed exit code so
    /// the test can pin the response shape without seeding the runtime's
    /// internal container map.
    #[derive(Default)]
    struct FixedExitRuntime {
        inner: zlayer_agent::MockRuntime,
        exit_code: i32,
    }

    #[async_trait::async_trait]
    impl Runtime for FixedExitRuntime {
        async fn pull_image(&self, image: &str) -> zlayer_agent::error::Result<()> {
            self.inner.pull_image(image).await
        }
        async fn pull_image_with_policy(
            &self,
            image: &str,
            policy: zlayer_spec::PullPolicy,
            auth: Option<&zlayer_spec::RegistryAuth>,
        ) -> zlayer_agent::error::Result<()> {
            self.inner.pull_image_with_policy(image, policy, auth).await
        }
        async fn create_container(
            &self,
            id: &ContainerId,
            spec: &zlayer_spec::ServiceSpec,
        ) -> zlayer_agent::error::Result<()> {
            self.inner.create_container(id, spec).await
        }
        async fn start_container(&self, id: &ContainerId) -> zlayer_agent::error::Result<()> {
            self.inner.start_container(id).await
        }
        async fn stop_container(
            &self,
            id: &ContainerId,
            t: std::time::Duration,
        ) -> zlayer_agent::error::Result<()> {
            self.inner.stop_container(id, t).await
        }
        async fn remove_container(&self, id: &ContainerId) -> zlayer_agent::error::Result<()> {
            self.inner.remove_container(id).await
        }
        async fn container_state(
            &self,
            id: &ContainerId,
        ) -> zlayer_agent::error::Result<zlayer_agent::runtime::ContainerState> {
            self.inner.container_state(id).await
        }
        async fn container_logs(
            &self,
            id: &ContainerId,
            tail: usize,
        ) -> zlayer_agent::error::Result<Vec<zlayer_observability::logs::LogEntry>> {
            self.inner.container_logs(id, tail).await
        }
        async fn exec(
            &self,
            id: &ContainerId,
            cmd: &[String],
        ) -> zlayer_agent::error::Result<(i32, String, String)> {
            self.inner.exec(id, cmd).await
        }
        async fn get_container_stats(
            &self,
            id: &ContainerId,
        ) -> zlayer_agent::error::Result<zlayer_agent::cgroups_stats::ContainerStats> {
            self.inner.get_container_stats(id).await
        }
        async fn wait_container(&self, _id: &ContainerId) -> zlayer_agent::error::Result<i32> {
            Ok(self.exit_code)
        }
        async fn get_logs(
            &self,
            id: &ContainerId,
        ) -> zlayer_agent::error::Result<Vec<zlayer_observability::logs::LogEntry>> {
            self.inner.get_logs(id).await
        }
        async fn get_container_pid(
            &self,
            id: &ContainerId,
        ) -> zlayer_agent::error::Result<Option<u32>> {
            self.inner.get_container_pid(id).await
        }
        async fn get_container_ip(
            &self,
            id: &ContainerId,
        ) -> zlayer_agent::error::Result<Option<std::net::IpAddr>> {
            self.inner.get_container_ip(id).await
        }
    }

    /// Helper: build a `ContainerApiState` backed by a `FixedExitRuntime`
    /// whose `wait_container` always returns `exit_code`. Mirrors
    /// `make_state_with_container` but swaps the runtime so we don't have
    /// to thread a real `ServiceSpec` through `MockRuntime::create_container`.
    async fn make_wait_state(
        service: &str,
        exit_code: i32,
    ) -> (ContainerApiState, ContainerId, String) {
        let runtime = Arc::new(FixedExitRuntime {
            exit_code,
            ..FixedExitRuntime::default()
        });
        let runtime_dyn: Arc<dyn Runtime + Send + Sync> = runtime;
        let state = ContainerApiState::with_daemon_uuid(runtime_dyn, "wait-fixed-uuid".to_string());
        let cid = ContainerId::new(service.to_string(), 0);
        let hex = state.id_map.register(state.id_map.daemon_uuid(), &cid);
        let standalone = StandaloneContainer {
            container_id: cid.clone(),
            image: "alpine:latest".to_string(),
            name: Some(service.to_string()),
            labels: HashMap::new(),
            created_at: "2026-05-03T00:00:00Z".to_string(),
            delete_on_exit: false,
        };
        state
            .containers
            .write()
            .await
            .insert(service.to_string(), standalone);
        (state, cid, hex)
    }

    #[tokio::test]
    async fn wait_container_post_returns_status_code_for_known_container() {
        use crate::auth::Claims;
        use std::time::Duration as StdDuration;

        let (state, _cid, hex) = make_wait_state("standalone-wait", 137).await;

        let user = AuthUser {
            claims: Claims::new("test", StdDuration::from_secs(60), vec![], None),
        };

        let resp = wait_container_post(
            user,
            State(state),
            Path(hex),
            Query(WaitContainerQuery { condition: None }),
        )
        .await
        .expect("wait must succeed");

        // `FixedExitRuntime` returns `exit_code = 137`; the handler must
        // forward it onto the Docker `StatusCode` field exactly.
        assert_eq!(resp.0.status_code, 137);
        assert!(resp.0.error.is_none());
    }

    /// `POST /api/v1/containers/{id}/wait` with an invalid condition string
    /// must reject with 400 `BadRequest`, never reaching the runtime. This is
    /// the contract that the docker-compat shim relies on for the
    /// `condition=` translation.
    #[tokio::test]
    async fn wait_container_post_rejects_invalid_condition() {
        use crate::auth::Claims;
        use std::time::Duration as StdDuration;

        let (state, _cid, hex) = make_wait_state("standalone-bad", 0).await;
        let user = AuthUser {
            claims: Claims::new("test", StdDuration::from_secs(60), vec![], None),
        };

        let err = wait_container_post(
            user,
            State(state),
            Path(hex),
            Query(WaitContainerQuery {
                condition: Some("not-a-real-condition".to_string()),
            }),
        )
        .await
        .expect_err("invalid condition must be rejected");

        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    /// `POST /api/v1/containers/{id}/rename?name=<new>` must update the
    /// in-memory container metadata so the next `inspect` sees the new name
    /// AND must invoke the runtime's `rename_container`. We use a custom
    /// runtime that records the call so we can verify both ends.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn rename_container_updates_metadata_and_calls_runtime() {
        use crate::auth::Claims;
        use std::sync::Mutex;
        use std::time::Duration as StdDuration;
        use zlayer_agent::error::Result as AgentResult;

        // Custom runtime that captures rename_container calls so the test
        // can assert both the daemon-side metadata mutation and the
        // runtime forwarding.
        #[derive(Default)]
        struct RecordingRuntime {
            inner: zlayer_agent::MockRuntime,
            renames: Mutex<Vec<(ContainerId, String)>>,
        }

        #[async_trait::async_trait]
        impl Runtime for RecordingRuntime {
            // Every required method delegates to MockRuntime.
            async fn pull_image(&self, image: &str) -> AgentResult<()> {
                self.inner.pull_image(image).await
            }
            async fn pull_image_with_policy(
                &self,
                image: &str,
                policy: zlayer_spec::PullPolicy,
                auth: Option<&zlayer_spec::RegistryAuth>,
            ) -> AgentResult<()> {
                self.inner.pull_image_with_policy(image, policy, auth).await
            }
            async fn create_container(
                &self,
                id: &ContainerId,
                spec: &zlayer_spec::ServiceSpec,
            ) -> AgentResult<()> {
                self.inner.create_container(id, spec).await
            }
            async fn start_container(&self, id: &ContainerId) -> AgentResult<()> {
                self.inner.start_container(id).await
            }
            async fn stop_container(
                &self,
                id: &ContainerId,
                t: std::time::Duration,
            ) -> AgentResult<()> {
                self.inner.stop_container(id, t).await
            }
            async fn remove_container(&self, id: &ContainerId) -> AgentResult<()> {
                self.inner.remove_container(id).await
            }
            async fn container_state(
                &self,
                id: &ContainerId,
            ) -> AgentResult<zlayer_agent::runtime::ContainerState> {
                self.inner.container_state(id).await
            }
            async fn container_logs(
                &self,
                id: &ContainerId,
                tail: usize,
            ) -> AgentResult<Vec<zlayer_observability::logs::LogEntry>> {
                self.inner.container_logs(id, tail).await
            }
            async fn exec(
                &self,
                id: &ContainerId,
                cmd: &[String],
            ) -> AgentResult<(i32, String, String)> {
                self.inner.exec(id, cmd).await
            }
            async fn get_container_stats(
                &self,
                id: &ContainerId,
            ) -> AgentResult<zlayer_agent::cgroups_stats::ContainerStats> {
                self.inner.get_container_stats(id).await
            }
            async fn wait_container(&self, id: &ContainerId) -> AgentResult<i32> {
                self.inner.wait_container(id).await
            }
            async fn get_logs(
                &self,
                id: &ContainerId,
            ) -> AgentResult<Vec<zlayer_observability::logs::LogEntry>> {
                self.inner.get_logs(id).await
            }
            async fn get_container_pid(&self, id: &ContainerId) -> AgentResult<Option<u32>> {
                self.inner.get_container_pid(id).await
            }
            async fn get_container_ip(
                &self,
                id: &ContainerId,
            ) -> AgentResult<Option<std::net::IpAddr>> {
                self.inner.get_container_ip(id).await
            }
            async fn rename_container(&self, id: &ContainerId, new_name: &str) -> AgentResult<()> {
                self.renames
                    .lock()
                    .unwrap()
                    .push((id.clone(), new_name.to_string()));
                Ok(())
            }
        }

        let runtime: Arc<RecordingRuntime> = Arc::new(RecordingRuntime::default());
        let runtime_dyn: Arc<dyn Runtime + Send + Sync> = runtime.clone();
        let state =
            ContainerApiState::with_daemon_uuid(runtime_dyn, "rename-test-uuid".to_string());
        let cid = ContainerId::new("standalone-rename".to_string(), 0);
        let hex = state.id_map.register(state.id_map.daemon_uuid(), &cid);

        let standalone = StandaloneContainer {
            container_id: cid.clone(),
            image: "alpine:latest".to_string(),
            name: Some("standalone-rename".to_string()),
            labels: HashMap::new(),
            created_at: "2026-05-03T00:00:00Z".to_string(),
            delete_on_exit: false,
        };
        state
            .containers
            .write()
            .await
            .insert("standalone-rename".to_string(), standalone);

        // Caller must have the operator role.
        let user = AuthUser {
            claims: Claims::new(
                "test",
                StdDuration::from_secs(60),
                vec!["operator".to_string()],
                None,
            ),
        };

        let status = rename_container(
            user,
            State(state.clone()),
            Path(hex),
            Query(RenameContainerQuery {
                name: Some("renamed-display".to_string()),
            }),
        )
        .await
        .expect("rename must succeed");

        assert_eq!(status, axum::http::StatusCode::NO_CONTENT);

        // Runtime call recorded.
        let recorded = {
            let renames = runtime.renames.lock().unwrap();
            renames.clone()
        };
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, cid);
        assert_eq!(recorded[0].1, "renamed-display");

        // In-memory metadata updated.
        let g = state.containers.read().await;
        let meta = g
            .get("standalone-rename")
            .expect("container meta should remain in cache after rename");
        assert_eq!(meta.name.as_deref(), Some("renamed-display"));
    }

    /// Tasks 5.3.1-4: `POST /api/v1/containers/{id}/update` happy path.
    ///
    /// Pins three contracts at once:
    ///
    /// 1. The handler resolves the container by its 64-char hex id
    ///    (registered in the id map) before talking to the runtime.
    /// 2. The handler builds a [`ContainerResourceUpdate`] from the
    ///    request body and forwards it to
    ///    `Runtime::update_container_resources` verbatim.
    /// 3. The handler returns Docker's `{"Warnings": [...]}` shape so
    ///    `zlayer-docker` can pass the body through unchanged.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn update_container_forwards_to_runtime_and_returns_warnings() {
        use crate::auth::Claims;
        use std::sync::Mutex;
        use std::time::Duration as StdDuration;
        use zlayer_agent::error::Result as AgentResult;
        use zlayer_agent::runtime::{ContainerResourceUpdate, ContainerUpdateOutcome};
        use zlayer_types::api::containers::{ContainerUpdateRequest, ContainerUpdateRestartPolicy};

        // Custom runtime that captures every `update_container_resources`
        // call so the test can assert both that the handler called it
        // and that every body field threaded through unchanged.
        #[derive(Default)]
        struct UpdateRecordingRuntime {
            inner: zlayer_agent::MockRuntime,
            updates: Mutex<Vec<(ContainerId, ContainerResourceUpdate)>>,
        }

        #[async_trait::async_trait]
        impl Runtime for UpdateRecordingRuntime {
            async fn pull_image(&self, image: &str) -> AgentResult<()> {
                self.inner.pull_image(image).await
            }
            async fn pull_image_with_policy(
                &self,
                image: &str,
                policy: zlayer_spec::PullPolicy,
                auth: Option<&zlayer_spec::RegistryAuth>,
            ) -> AgentResult<()> {
                self.inner.pull_image_with_policy(image, policy, auth).await
            }
            async fn create_container(
                &self,
                id: &ContainerId,
                spec: &zlayer_spec::ServiceSpec,
            ) -> AgentResult<()> {
                self.inner.create_container(id, spec).await
            }
            async fn start_container(&self, id: &ContainerId) -> AgentResult<()> {
                self.inner.start_container(id).await
            }
            async fn stop_container(
                &self,
                id: &ContainerId,
                t: std::time::Duration,
            ) -> AgentResult<()> {
                self.inner.stop_container(id, t).await
            }
            async fn remove_container(&self, id: &ContainerId) -> AgentResult<()> {
                self.inner.remove_container(id).await
            }
            async fn container_state(
                &self,
                id: &ContainerId,
            ) -> AgentResult<zlayer_agent::runtime::ContainerState> {
                self.inner.container_state(id).await
            }
            async fn container_logs(
                &self,
                id: &ContainerId,
                tail: usize,
            ) -> AgentResult<Vec<zlayer_observability::logs::LogEntry>> {
                self.inner.container_logs(id, tail).await
            }
            async fn exec(
                &self,
                id: &ContainerId,
                cmd: &[String],
            ) -> AgentResult<(i32, String, String)> {
                self.inner.exec(id, cmd).await
            }
            async fn get_container_stats(
                &self,
                id: &ContainerId,
            ) -> AgentResult<zlayer_agent::cgroups_stats::ContainerStats> {
                self.inner.get_container_stats(id).await
            }
            async fn wait_container(&self, id: &ContainerId) -> AgentResult<i32> {
                self.inner.wait_container(id).await
            }
            async fn get_logs(
                &self,
                id: &ContainerId,
            ) -> AgentResult<Vec<zlayer_observability::logs::LogEntry>> {
                self.inner.get_logs(id).await
            }
            async fn get_container_pid(&self, id: &ContainerId) -> AgentResult<Option<u32>> {
                self.inner.get_container_pid(id).await
            }
            async fn get_container_ip(
                &self,
                id: &ContainerId,
            ) -> AgentResult<Option<std::net::IpAddr>> {
                self.inner.get_container_ip(id).await
            }
            async fn update_container_resources(
                &self,
                id: &ContainerId,
                update: &ContainerResourceUpdate,
            ) -> AgentResult<ContainerUpdateOutcome> {
                self.updates
                    .lock()
                    .unwrap()
                    .push((id.clone(), update.clone()));
                Ok(ContainerUpdateOutcome {
                    warnings: vec!["test-warning".to_string()],
                })
            }
        }

        let runtime: Arc<UpdateRecordingRuntime> = Arc::new(UpdateRecordingRuntime::default());
        let runtime_dyn: Arc<dyn Runtime + Send + Sync> = runtime.clone();
        let state =
            ContainerApiState::with_daemon_uuid(runtime_dyn, "update-test-uuid".to_string());
        let cid = ContainerId::new("standalone-update".to_string(), 0);
        let hex = state.id_map.register(state.id_map.daemon_uuid(), &cid);

        let standalone = StandaloneContainer {
            container_id: cid.clone(),
            image: "alpine:latest".to_string(),
            name: Some("standalone-update".to_string()),
            labels: HashMap::new(),
            created_at: "2026-05-03T00:00:00Z".to_string(),
            delete_on_exit: false,
        };
        state
            .containers
            .write()
            .await
            .insert("standalone-update".to_string(), standalone);

        let user = AuthUser {
            claims: Claims::new(
                "test",
                StdDuration::from_secs(60),
                vec!["operator".to_string()],
                None,
            ),
        };

        let body = ContainerUpdateRequest {
            cpu_shares: Some(512),
            memory: Some(536_870_912),
            cpu_period: Some(100_000),
            cpu_quota: Some(50_000),
            cpuset_cpus: Some("0-3".to_string()),
            memory_swap: Some(1_073_741_824),
            blkio_weight: Some(500),
            pids_limit: Some(2048),
            restart_policy: Some(ContainerUpdateRestartPolicy {
                name: Some("on-failure".to_string()),
                maximum_retry_count: Some(5),
            }),
            ..Default::default()
        };

        let resp = update_container(user, State(state.clone()), Path(hex), Json(body))
            .await
            .expect("update must succeed");

        // Response body propagated from the runtime outcome.
        assert_eq!(resp.0.warnings, vec!["test-warning".to_string()]);

        let recorded = runtime.updates.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1);
        let (rec_cid, rec_update) = &recorded[0];
        assert_eq!(rec_cid, &cid);

        // Every body field must have threaded through to the runtime
        // ContainerResourceUpdate unchanged. This is the contract the
        // Docker compat shim relies on.
        assert_eq!(rec_update.cpu_shares, Some(512));
        assert_eq!(rec_update.memory, Some(536_870_912));
        assert_eq!(rec_update.cpu_period, Some(100_000));
        assert_eq!(rec_update.cpu_quota, Some(50_000));
        assert_eq!(rec_update.cpuset_cpus.as_deref(), Some("0-3"));
        assert_eq!(rec_update.memory_swap, Some(1_073_741_824));
        assert_eq!(rec_update.blkio_weight, Some(500));
        assert_eq!(rec_update.pids_limit, Some(2048));
        let rp = rec_update.restart_policy.as_ref().expect("restart_policy");
        assert_eq!(rp.name.as_deref(), Some("on-failure"));
        assert_eq!(rp.maximum_retry_count, Some(5));
    }

    /// `POST /api/v1/containers/{id}/update` against a runtime that
    /// returns `Unsupported` must surface a `501 Not Implemented` to
    /// the caller — not a `500`. This is the contract Docker clients
    /// rely on to detect when a backend cannot honour the call.
    #[tokio::test]
    async fn update_container_maps_unsupported_runtime_to_501() {
        use crate::auth::Claims;
        use std::time::Duration as StdDuration;
        use zlayer_types::api::containers::ContainerUpdateRequest;

        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state =
            ContainerApiState::with_daemon_uuid(runtime, "update-unsupported-uuid".to_string());
        let cid = ContainerId::new("standalone-unsupported".to_string(), 0);
        let hex = state.id_map.register(state.id_map.daemon_uuid(), &cid);

        let standalone = StandaloneContainer {
            container_id: cid,
            image: "alpine:latest".to_string(),
            name: None,
            labels: HashMap::new(),
            created_at: "2026-05-03T00:00:00Z".to_string(),
            delete_on_exit: false,
        };
        state
            .containers
            .write()
            .await
            .insert("standalone-unsupported".to_string(), standalone);

        let user = AuthUser {
            claims: Claims::new(
                "test",
                StdDuration::from_secs(60),
                vec!["operator".to_string()],
                None,
            ),
        };

        let body = ContainerUpdateRequest {
            cpu_shares: Some(256),
            ..Default::default()
        };

        let err = update_container(user, State(state), Path(hex), Json(body))
            .await
            .expect_err("MockRuntime returns Unsupported by default");
        assert!(
            matches!(err, ApiError::NotImplemented(_)),
            "expected NotImplemented, got {err:?}"
        );
    }

    #[tokio::test]
    async fn create_container_returns_hex_id_via_id_map() {
        // `create_container` itself is integration-heavy (pulls images,
        // attaches to networks). The contract this test pins is narrower:
        // when `ContainerApiState::id_map.register` is called with the
        // ContainerId minted by `generate_container_id`, the resulting
        // string is a 64-char lowercase hex stamped with the daemon UUID.
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ContainerApiState::with_daemon_uuid(runtime, "deterministic-uuid".to_string());
        let (_id_str, cid) = generate_container_id(Some("demo"));
        let hex = state.id_map.register(state.id_map.daemon_uuid(), &cid);
        assert_eq!(hex.len(), 64);
        assert!(hex
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

        // The hex form must equal the canonical `compute_hex` output so
        // Docker compat layer clients can predict the value.
        let expected =
            crate::handlers::container_id_map::compute_hex(state.id_map.daemon_uuid(), &cid);
        assert_eq!(hex, expected);

        // And the round-trip lookup must return the same ContainerId.
        assert_eq!(state.id_map.lookup_hex(&hex), Some(cid));
    }

    // -----------------------------------------------------------------------
    // §3.2.2 / §3.3.2 — REST `/logs` and `/stats` streaming handlers backed
    // by `Runtime::logs_stream` and `Runtime::stats_stream`.
    // -----------------------------------------------------------------------

    /// `logs` namespace fixture: build a `LogsStreamOptions` from a
    /// representative `ContainerLogQuery` and assert each field maps to the
    /// expected runtime option. Pins the "neither stdout nor stderr set ->
    /// both default to true" default and the `tail==0 -> None` mapping.
    mod logs {
        use super::*;
        use bytes::Bytes;
        use chrono::TimeZone;
        use futures_util::StreamExt;
        use zlayer_agent::runtime::{LogChannel, LogChunk, MockRuntime, Runtime};

        #[test]
        fn handler_builds_logs_stream_options_from_query() {
            // Empty query: stdout/stderr both default to true; follow off.
            let empty: ContainerLogQuery = serde_json::from_str("{}").unwrap();
            let opts = logs_stream_options_from_query(&empty);
            assert!(!opts.follow);
            assert!(opts.stdout);
            assert!(opts.stderr);
            assert_eq!(opts.tail, Some(100));
            assert!(opts.since.is_none());
            assert!(opts.until.is_none());
            assert!(!opts.timestamps);

            // Fully-populated query: every field is plumbed through.
            let full: ContainerLogQuery = serde_json::from_str(
                r#"{
                    "tail": 25,
                    "follow": true,
                    "since": 1700000000,
                    "until": 1700001000,
                    "timestamps": true,
                    "stdout": false,
                    "stderr": true
                }"#,
            )
            .unwrap();
            let opts = logs_stream_options_from_query(&full);
            assert!(opts.follow);
            assert!(!opts.stdout);
            assert!(opts.stderr);
            assert_eq!(opts.tail, Some(25));
            assert_eq!(opts.since, Some(1_700_000_000));
            assert_eq!(opts.until, Some(1_700_001_000));
            assert!(opts.timestamps);

            // tail=0 maps to "all logs" (`None`).
            let no_tail: ContainerLogQuery = serde_json::from_str(r#"{"tail": 0}"#).unwrap();
            assert!(logs_stream_options_from_query(&no_tail).tail.is_none());

            // Both explicitly-false collapses to "include both" rather than
            // "stream nothing".
            let both_false: ContainerLogQuery =
                serde_json::from_str(r#"{"stdout": false, "stderr": false}"#).unwrap();
            let opts = logs_stream_options_from_query(&both_false);
            assert!(opts.stdout);
            assert!(opts.stderr);
        }

        /// `format=json` (the default) emits one NDJSON line per `LogChunk`
        /// with the documented shape. Drives the format converter directly
        /// via `ndjson_line_for_chunk` so the test is independent of axum
        /// routing.
        #[tokio::test]
        async fn json_format_emits_ndjson_per_chunk() {
            let runtime = MockRuntime::new();
            let id = ContainerId::new("logs-json".to_string(), 0);
            // Pre-script three chunks: one stdout (UTF-8), one stderr (with
            // timestamp), and one stdout containing non-UTF8 bytes that must
            // be base64-encoded.
            let ts = chrono::Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap();
            runtime
                .enqueue_log_chunk(
                    &id,
                    LogChunk {
                        stream: LogChannel::Stdout,
                        bytes: Bytes::from_static(b"hello\n"),
                        timestamp: None,
                    },
                )
                .await;
            runtime
                .enqueue_log_chunk(
                    &id,
                    LogChunk {
                        stream: LogChannel::Stderr,
                        bytes: Bytes::from_static(b"oops\n"),
                        timestamp: Some(ts),
                    },
                )
                .await;
            runtime
                .enqueue_log_chunk(
                    &id,
                    LogChunk {
                        stream: LogChannel::Stdout,
                        bytes: Bytes::from_static(&[0xFF, 0xFE, 0xFD]),
                        timestamp: None,
                    },
                )
                .await;

            let mut stream = runtime
                .logs_stream(&id, LogsStreamOptions::default())
                .await
                .expect("logs_stream succeeds");

            let mut chunks = Vec::new();
            while let Some(item) = stream.next().await {
                chunks.push(item.expect("MockRuntime queue items are Ok"));
            }
            assert_eq!(chunks.len(), 3);

            let l1 = ndjson_line_for_chunk(&chunks[0]);
            let s1 = std::str::from_utf8(&l1).unwrap();
            assert!(s1.ends_with('\n'));
            let v1: serde_json::Value = serde_json::from_str(s1.trim_end()).unwrap();
            assert_eq!(v1["stream"], "stdout");
            assert_eq!(v1["data"], "hello\n");
            assert!(v1.get("timestamp").is_none());
            assert!(v1.get("data_b64").is_none());

            let l2 = ndjson_line_for_chunk(&chunks[1]);
            let v2: serde_json::Value =
                serde_json::from_str(std::str::from_utf8(&l2).unwrap().trim_end()).unwrap();
            assert_eq!(v2["stream"], "stderr");
            assert_eq!(v2["data"], "oops\n");
            assert!(v2["timestamp"].as_str().is_some());

            let l3 = ndjson_line_for_chunk(&chunks[2]);
            let v3: serde_json::Value =
                serde_json::from_str(std::str::from_utf8(&l3).unwrap().trim_end()).unwrap();
            assert_eq!(v3["stream"], "stdout");
            // Non-UTF8 bytes -> base64-encoded under `data_b64` instead of
            // `data` so the line stays valid JSON.
            assert!(v3.get("data").is_none());
            assert!(v3["data_b64"].as_str().is_some());
        }
    }

    /// `stats` namespace fixture: drives the `Runtime::stats_stream`-backed
    /// handler through the streaming branches via the `MockRuntime` queue.
    mod stats {
        use super::*;
        use chrono::TimeZone;
        use futures_util::StreamExt;
        use zlayer_agent::runtime::{MockRuntime, Runtime, StatsSample};

        fn sample(cpu_total_ns: u64) -> StatsSample {
            StatsSample {
                cpu_total_ns,
                cpu_system_ns: 0,
                online_cpus: 1,
                mem_used_bytes: 0,
                mem_limit_bytes: 0,
                net_rx_bytes: 0,
                net_tx_bytes: 0,
                blkio_read_bytes: 0,
                blkio_write_bytes: 0,
                pids_current: 0,
                pids_limit: None,
                timestamp: chrono::Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap(),
            }
        }

        /// `stream=false` (Docker default): the body must contain exactly one
        /// `StatsSample` line, regardless of how many samples the runtime
        /// produced. Drives the same `take(1)` logic the handler uses.
        #[tokio::test]
        async fn stream_false_emits_exactly_one_sample() {
            let runtime = MockRuntime::new();
            let id = ContainerId::new("stats-once".to_string(), 0);
            // Enqueue three samples; only the first must reach the body.
            runtime.enqueue_stats_sample(&id, sample(1)).await;
            runtime.enqueue_stats_sample(&id, sample(2)).await;
            runtime.enqueue_stats_sample(&id, sample(3)).await;

            let stream = runtime.stats_stream(&id).await.expect("stats_stream ok");
            let body: Vec<_> = stream.take(1).collect().await;
            assert_eq!(body.len(), 1);
            let s = body
                .into_iter()
                .next()
                .unwrap()
                .expect("queued samples are Ok");
            let line = ndjson_line_for_stats(&s);
            let text = std::str::from_utf8(&line).unwrap();
            assert!(text.ends_with('\n'));
            let v: serde_json::Value = serde_json::from_str(text.trim_end()).unwrap();
            assert_eq!(v["cpu_total_ns"], 1);
        }

        /// `stream=true`: the runtime's full sample sequence must be visible
        /// to the consumer, in order.
        #[tokio::test]
        async fn stream_true_emits_multiple_samples() {
            let runtime = MockRuntime::new();
            let id = ContainerId::new("stats-many".to_string(), 0);
            for n in 1u64..=4 {
                runtime.enqueue_stats_sample(&id, sample(n)).await;
            }

            let mut stream = runtime.stats_stream(&id).await.expect("stats_stream ok");
            let mut seen = Vec::new();
            while let Some(item) = stream.next().await {
                let s = item.expect("queued samples are Ok");
                let line = ndjson_line_for_stats(&s);
                let v: serde_json::Value =
                    serde_json::from_str(std::str::from_utf8(&line).unwrap().trim_end()).unwrap();
                seen.push(v["cpu_total_ns"].as_u64().unwrap());
            }
            assert_eq!(seen, vec![1, 2, 3, 4]);
        }
    }

    #[test]
    fn validate_dns_entries_accepts_v4_and_v6() {
        validate_dns_entries(&["8.8.8.8".to_string(), "2001:4860:4860::8888".to_string()])
            .expect("valid dns entries");
    }

    #[test]
    fn validate_dns_entries_rejects_garbage() {
        assert!(validate_dns_entries(&[String::new()]).is_err());
        assert!(validate_dns_entries(&["not-an-ip".to_string()]).is_err());
        assert!(validate_dns_entries(&["1.2.3.4".to_string(), "nope".to_string()]).is_err());
    }

    #[test]
    fn validate_extra_hosts_accepts_plain_ip_and_host_gateway() {
        validate_extra_hosts(&[
            "host.docker.internal:host-gateway".to_string(),
            "myhost:10.0.0.1".to_string(),
            "ipv6host:fe80::1".to_string(),
        ])
        .expect("valid extra_hosts");
    }

    #[test]
    fn validate_extra_hosts_rejects_malformed() {
        assert!(validate_extra_hosts(&["no-colon".to_string()]).is_err());
        assert!(validate_extra_hosts(&[":10.0.0.1".to_string()]).is_err());
        assert!(validate_extra_hosts(&["host:".to_string()]).is_err());
        assert!(validate_extra_hosts(&["host:not-an-ip".to_string()]).is_err());
    }

    #[test]
    fn build_service_spec_threads_hostname_dns_extra_hosts() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            hostname: Some("my-host".to_string()),
            dns: vec!["8.8.8.8".to_string()],
            extra_hosts: vec!["host.docker.internal:host-gateway".to_string()],
            ..CreateContainerRequest::default()
        };
        let spec = build_service_spec(&request).expect("valid spec");
        assert_eq!(spec.hostname.as_deref(), Some("my-host"));
        assert_eq!(spec.dns, vec!["8.8.8.8".to_string()]);
        assert_eq!(
            spec.extra_hosts,
            vec!["host.docker.internal:host-gateway".to_string()]
        );
    }

    #[test]
    fn volume_mount_bind_default_translates_to_storage_bind() {
        // `mount_type: None` means legacy behavior: a host-path bind mount.
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            volumes: vec![VolumeMount {
                mount_type: None,
                source: Some("/srv/data".to_string()),
                target: "/data".to_string(),
                readonly: true,
            }],
            ..CreateContainerRequest::default()
        };
        let spec = build_service_spec(&request).expect("bind default should build");
        assert_eq!(spec.storage.len(), 1);
        match &spec.storage[0] {
            zlayer_spec::StorageSpec::Bind {
                source,
                target,
                readonly,
            } => {
                assert_eq!(source, "/srv/data");
                assert_eq!(target, "/data");
                assert!(*readonly);
            }
            other => panic!("expected Bind, got {other:?}"),
        }
    }

    #[test]
    fn volume_mount_bind_rejects_relative_source() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            volumes: vec![VolumeMount {
                mount_type: Some(VolumeMountType::Bind),
                source: Some("relative/path".to_string()),
                target: "/data".to_string(),
                readonly: false,
            }],
            ..CreateContainerRequest::default()
        };
        assert!(build_service_spec(&request).is_err());
    }

    #[test]
    fn volume_mount_volume_translates_to_storage_named() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            volumes: vec![VolumeMount {
                mount_type: Some(VolumeMountType::Volume),
                source: Some("pg-data".to_string()),
                target: "/var/lib/postgresql".to_string(),
                readonly: false,
            }],
            ..CreateContainerRequest::default()
        };
        let spec = build_service_spec(&request).expect("volume should build");
        assert_eq!(spec.storage.len(), 1);
        match &spec.storage[0] {
            zlayer_spec::StorageSpec::Named {
                name,
                target,
                readonly,
                ..
            } => {
                assert_eq!(name, "pg-data");
                assert_eq!(target, "/var/lib/postgresql");
                assert!(!*readonly);
            }
            other => panic!("expected Named, got {other:?}"),
        }
    }

    #[test]
    fn volume_mount_volume_rejects_invalid_name() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            volumes: vec![VolumeMount {
                mount_type: Some(VolumeMountType::Volume),
                source: Some("Bad Name!".to_string()),
                target: "/data".to_string(),
                readonly: false,
            }],
            ..CreateContainerRequest::default()
        };
        assert!(build_service_spec(&request).is_err());
    }

    #[test]
    fn volume_mount_tmpfs_translates_to_storage_tmpfs() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            volumes: vec![VolumeMount {
                mount_type: Some(VolumeMountType::Tmpfs),
                source: None,
                target: "/tmp-mem".to_string(),
                readonly: false,
            }],
            ..CreateContainerRequest::default()
        };
        let spec = build_service_spec(&request).expect("tmpfs should build");
        assert_eq!(spec.storage.len(), 1);
        match &spec.storage[0] {
            zlayer_spec::StorageSpec::Tmpfs { target, .. } => {
                assert_eq!(target, "/tmp-mem");
            }
            other => panic!("expected Tmpfs, got {other:?}"),
        }
    }

    #[test]
    fn volume_mount_tmpfs_rejects_source() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            volumes: vec![VolumeMount {
                mount_type: Some(VolumeMountType::Tmpfs),
                source: Some("/something".to_string()),
                target: "/tmp-mem".to_string(),
                readonly: false,
            }],
            ..CreateContainerRequest::default()
        };
        assert!(build_service_spec(&request).is_err());
    }

    #[test]
    fn build_service_spec_rejects_bad_dns() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            dns: vec!["not-an-ip".to_string()],
            ..CreateContainerRequest::default()
        };
        assert!(build_service_spec(&request).is_err());
    }

    #[test]
    fn build_service_spec_threads_restart_policy() {
        // Happy path: an on_failure policy with bounded retries and a
        // humantime delay survives translation into `ServiceSpec`.
        let policy = zlayer_spec::ContainerRestartPolicy {
            kind: zlayer_spec::ContainerRestartKind::OnFailure,
            max_attempts: Some(5),
            delay: Some("500ms".to_string()),
        };
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            restart_policy: Some(policy.clone()),
            ..CreateContainerRequest::default()
        };
        let spec = build_service_spec(&request).expect("valid restart policy");
        assert_eq!(spec.restart_policy, Some(policy));
    }

    #[test]
    fn build_service_spec_rejects_bad_restart_delay() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            restart_policy: Some(zlayer_spec::ContainerRestartPolicy {
                kind: zlayer_spec::ContainerRestartKind::Always,
                max_attempts: None,
                delay: Some("not-a-duration".to_string()),
            }),
            ..CreateContainerRequest::default()
        };
        let err = build_service_spec(&request).expect_err("must reject bad delay");
        assert!(
            matches!(err, ApiError::BadRequest(_)),
            "expected BadRequest, got {err:?}"
        );
    }

    // -- §3.10: registry auth resolution precedence -------------------------

    #[tokio::test]
    async fn resolve_registry_auth_prefers_inline_over_store() {
        // When both `registry_auth` (inline) and `registry_credential_id`
        // are supplied, the inline value wins without ever consulting the
        // store. We prove that by passing `None` for the store: if the
        // resolver touched the store it would error with BadRequest.
        let inline = zlayer_spec::RegistryAuth {
            username: "inline-user".to_string(),
            password: "inline-pass".to_string(),
            auth_type: zlayer_spec::RegistryAuthType::Basic,
        };
        let resolved = resolve_registry_auth(Some(&inline), Some("some-id"), None)
            .await
            .expect("inline wins without store");
        let resolved = resolved.expect("must resolve to inline auth");
        assert_eq!(resolved.username, "inline-user");
        assert_eq!(resolved.password, "inline-pass");
        assert_eq!(resolved.auth_type, zlayer_spec::RegistryAuthType::Basic);
    }

    #[tokio::test]
    async fn resolve_registry_auth_returns_none_when_neither_supplied() {
        let resolved = resolve_registry_auth(None, None, None)
            .await
            .expect("no-auth path must succeed");
        assert!(
            resolved.is_none(),
            "no inline, no id -> runtime fallback (None)"
        );
    }

    #[tokio::test]
    async fn resolve_registry_auth_rejects_id_without_store() {
        let err = resolve_registry_auth(None, Some("some-id"), None)
            .await
            .expect_err("must reject missing store");
        assert!(
            matches!(err, ApiError::BadRequest(_)),
            "expected BadRequest when credential_id is set but no store is configured, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Network-attachment request + validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn network_attachment_request_deserialize_full() {
        let json = r#"{
            "network": "web",
            "aliases": ["api", "backend"],
            "ipv4_address": "10.0.0.5"
        }"#;
        let got: NetworkAttachmentRequest = serde_json::from_str(json).unwrap();
        assert_eq!(got.network, "web");
        assert_eq!(got.aliases, vec!["api".to_string(), "backend".to_string()]);
        assert_eq!(got.ipv4_address.as_deref(), Some("10.0.0.5"));
    }

    #[test]
    fn network_attachment_request_deserialize_minimal() {
        let json = r#"{"network": "w"}"#;
        let got: NetworkAttachmentRequest = serde_json::from_str(json).unwrap();
        assert_eq!(got.network, "w");
        assert!(got.aliases.is_empty());
        assert!(got.ipv4_address.is_none());
    }

    #[test]
    fn network_attachment_request_serialize_skips_empty() {
        let req = NetworkAttachmentRequest {
            network: "w".to_string(),
            aliases: Vec::new(),
            ipv4_address: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        // Only `network` should appear — `aliases` (empty) and `ipv4_address`
        // (None) are skipped per the serde attributes on the struct.
        assert_eq!(json, serde_json::json!({"network": "w"}));
    }

    #[test]
    fn create_container_request_deserialize_with_networks() {
        let json = r#"{
            "image": "nginx:latest",
            "networks": [
                {"network": "web"},
                {"network": "db", "aliases": ["primary"], "ipv4_address": "10.0.0.7"}
            ]
        }"#;
        let got: CreateContainerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(got.networks.len(), 2);
        assert_eq!(got.networks[0].network, "web");
        assert!(got.networks[0].aliases.is_empty());
        assert_eq!(got.networks[1].network, "db");
        assert_eq!(
            got.networks[1].aliases,
            vec!["primary".to_string()],
            "aliases deserialize"
        );
        assert_eq!(got.networks[1].ipv4_address.as_deref(), Some("10.0.0.7"));
    }

    #[test]
    fn validate_network_attachments_rejects_when_registry_absent() {
        let attach = NetworkAttachmentRequest {
            network: "web".to_string(),
            aliases: Vec::new(),
            ipv4_address: None,
        };
        let err = validate_network_attachments(&[attach], None)
            .expect_err("must reject when no registry is plumbed in");
        assert!(
            matches!(err, ApiError::BadRequest(_)),
            "expected BadRequest, got {err:?}"
        );
    }

    #[test]
    fn validate_network_attachments_rejects_unknown_network() {
        let registry = BridgeNetworkApiState::new();
        let attach = NetworkAttachmentRequest {
            network: "does-not-exist".to_string(),
            aliases: Vec::new(),
            ipv4_address: None,
        };
        let err = validate_network_attachments(&[attach], Some(&registry))
            .expect_err("must reject unknown network");
        assert!(
            matches!(err, ApiError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
    }

    #[test]
    fn validate_network_attachments_rejects_bad_ipv4() {
        let registry = BridgeNetworkApiState::new();
        let net = zlayer_spec::BridgeNetwork {
            id: "n1".to_string(),
            name: "web".to_string(),
            driver: zlayer_spec::BridgeNetworkDriver::Bridge,
            subnet: None,
            labels: HashMap::new(),
            internal: false,
            created_at: chrono::Utc::now(),
        };
        registry.networks.insert(net.id.clone(), net);
        let attach = NetworkAttachmentRequest {
            network: "web".to_string(),
            aliases: Vec::new(),
            ipv4_address: Some("definitely-not-an-ip".to_string()),
        };
        let err = validate_network_attachments(&[attach], Some(&registry))
            .expect_err("must reject bad ipv4");
        assert!(
            matches!(err, ApiError::BadRequest(_)),
            "expected BadRequest, got {err:?}"
        );
    }

    #[test]
    fn validate_network_attachments_accepts_known_network_with_valid_ipv4() {
        let registry = BridgeNetworkApiState::new();
        let net = zlayer_spec::BridgeNetwork {
            id: "n1".to_string(),
            name: "web".to_string(),
            driver: zlayer_spec::BridgeNetworkDriver::Bridge,
            subnet: None,
            labels: HashMap::new(),
            internal: false,
            created_at: chrono::Utc::now(),
        };
        registry.networks.insert(net.id.clone(), net);
        let attach = NetworkAttachmentRequest {
            network: "web".to_string(),
            aliases: vec!["alpha".to_string()],
            ipv4_address: Some("10.0.0.5".to_string()),
        };
        assert!(validate_network_attachments(&[attach], Some(&registry)).is_ok());
    }

    #[test]
    fn runtime_container_name_matches_docker_convention() {
        let id = ContainerId::new("svc".to_string(), 3);
        assert_eq!(runtime_container_name(&id), "zlayer-svc-3");
    }

    // -----------------------------------------------------------------------
    // Rollback behaviour — uses the mock Runtime from earlier in the file
    // plus a tiny mock BridgeNetworkRuntime so we can assert that a failing
    // attach tears the container down.
    // -----------------------------------------------------------------------

    /// A `BridgeNetworkRuntime` that succeeds `connect` the first N times
    /// then fails. Used by the rollback tests below.
    struct CountingRuntime {
        /// Connect calls that should succeed before we start failing.
        succeed_until: std::sync::atomic::AtomicUsize,
        /// Disconnect calls observed (for rollback assertion).
        disconnect_calls: std::sync::Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl crate::handlers::container_networks::BridgeNetworkRuntime for CountingRuntime {
        async fn create(
            &self,
            _spec: &zlayer_spec::BridgeNetwork,
        ) -> std::result::Result<(), crate::handlers::container_networks::RuntimeError> {
            Ok(())
        }
        async fn delete(
            &self,
            _id: &str,
        ) -> std::result::Result<(), crate::handlers::container_networks::RuntimeError> {
            Ok(())
        }
        async fn connect(
            &self,
            _network: &str,
            _attachment: &zlayer_spec::BridgeNetworkAttachment,
        ) -> std::result::Result<(), crate::handlers::container_networks::RuntimeError> {
            let prev = self
                .succeed_until
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            if prev == 0 {
                Err(crate::handlers::container_networks::RuntimeError::Failed(
                    "injected failure".to_string(),
                ))
            } else {
                Ok(())
            }
        }
        async fn disconnect(
            &self,
            network: &str,
            container: &str,
        ) -> std::result::Result<(), crate::handlers::container_networks::RuntimeError> {
            self.disconnect_calls
                .lock()
                .unwrap()
                .push((network.to_string(), container.to_string()));
            Ok(())
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines, clippy::items_after_statements)]
    async fn rollback_on_partial_attach_failure_removes_container_and_disconnects() {
        // Build a registry with two networks (web, db). Plumb in a
        // CountingRuntime that succeeds the first `connect` and then fails.
        let counting = std::sync::Arc::new(CountingRuntime {
            succeed_until: std::sync::atomic::AtomicUsize::new(1),
            disconnect_calls: std::sync::Mutex::new(Vec::new()),
        });
        let registry = BridgeNetworkApiState::new().with_runtime(counting.clone());
        for name in ["web", "db"] {
            let net = zlayer_spec::BridgeNetwork {
                id: name.to_string(),
                name: name.to_string(),
                driver: zlayer_spec::BridgeNetworkDriver::Bridge,
                subnet: None,
                labels: HashMap::new(),
                internal: false,
                created_at: chrono::Utc::now(),
            };
            registry.networks.insert(net.id.clone(), net);
        }

        // A mock Runtime that records what rollback tries to stop / remove.
        #[derive(Default)]
        struct TracingRuntime {
            stopped: std::sync::Mutex<Vec<String>>,
            removed: std::sync::Mutex<Vec<String>>,
        }
        #[async_trait::async_trait]
        impl Runtime for TracingRuntime {
            async fn pull_image(
                &self,
                _image: &str,
            ) -> std::result::Result<(), zlayer_agent::AgentError> {
                Ok(())
            }
            async fn pull_image_with_policy(
                &self,
                _image: &str,
                _policy: zlayer_spec::PullPolicy,
                _auth: Option<&zlayer_spec::RegistryAuth>,
            ) -> std::result::Result<(), zlayer_agent::AgentError> {
                Ok(())
            }
            async fn create_container(
                &self,
                _id: &ContainerId,
                _spec: &zlayer_spec::ServiceSpec,
            ) -> std::result::Result<(), zlayer_agent::AgentError> {
                Ok(())
            }
            async fn start_container(
                &self,
                _id: &ContainerId,
            ) -> std::result::Result<(), zlayer_agent::AgentError> {
                Ok(())
            }
            async fn stop_container(
                &self,
                id: &ContainerId,
                _timeout: std::time::Duration,
            ) -> std::result::Result<(), zlayer_agent::AgentError> {
                self.stopped.lock().unwrap().push(id.to_string());
                Ok(())
            }
            async fn remove_container(
                &self,
                id: &ContainerId,
            ) -> std::result::Result<(), zlayer_agent::AgentError> {
                self.removed.lock().unwrap().push(id.to_string());
                Ok(())
            }
            async fn container_state(
                &self,
                _id: &ContainerId,
            ) -> std::result::Result<ContainerState, zlayer_agent::AgentError> {
                Ok(ContainerState::Running)
            }
            async fn container_logs(
                &self,
                _id: &ContainerId,
                _tail: usize,
            ) -> std::result::Result<
                Vec<zlayer_observability::logs::LogEntry>,
                zlayer_agent::AgentError,
            > {
                Ok(Vec::new())
            }
            async fn exec(
                &self,
                _id: &ContainerId,
                _cmd: &[String],
            ) -> std::result::Result<(i32, String, String), zlayer_agent::AgentError> {
                Ok((0, String::new(), String::new()))
            }
            async fn get_container_stats(
                &self,
                _id: &ContainerId,
            ) -> std::result::Result<
                zlayer_agent::cgroups_stats::ContainerStats,
                zlayer_agent::AgentError,
            > {
                Ok(zlayer_agent::cgroups_stats::ContainerStats {
                    cpu_usage_usec: 0,
                    memory_bytes: 0,
                    memory_limit: u64::MAX,
                    timestamp: std::time::Instant::now(),
                })
            }
            async fn wait_container(
                &self,
                _id: &ContainerId,
            ) -> std::result::Result<i32, zlayer_agent::AgentError> {
                Ok(0)
            }
            async fn get_logs(
                &self,
                _id: &ContainerId,
            ) -> std::result::Result<
                Vec<zlayer_observability::logs::LogEntry>,
                zlayer_agent::AgentError,
            > {
                Ok(Vec::new())
            }
            async fn get_container_pid(
                &self,
                _id: &ContainerId,
            ) -> std::result::Result<Option<u32>, zlayer_agent::AgentError> {
                Ok(Some(1))
            }
            async fn get_container_ip(
                &self,
                _id: &ContainerId,
            ) -> std::result::Result<Option<std::net::IpAddr>, zlayer_agent::AgentError>
            {
                Ok(None)
            }
        }

        // Keep a concrete Arc so we can inspect the `stopped` / `removed`
        // vecs after the call returns, AND hand the same Arc to
        // `attach_container_to_networks` via the Runtime trait object.
        let tracing_concrete = Arc::new(TracingRuntime::default());
        let tracing: Arc<dyn Runtime + Send + Sync> = tracing_concrete.clone();
        let cid = ContainerId::new("svc".to_string(), 0);

        let attachments = vec![
            NetworkAttachmentRequest {
                network: "web".to_string(),
                aliases: Vec::new(),
                ipv4_address: None,
            },
            NetworkAttachmentRequest {
                network: "db".to_string(),
                aliases: Vec::new(),
                ipv4_address: None,
            },
        ];

        let err = attach_container_to_networks(
            Some(&registry),
            &tracing,
            &cid,
            Some("demo"),
            &attachments,
        )
        .await
        .expect_err("second attach is injected to fail");
        assert!(matches!(err, ApiError::Internal(_)));

        // The runtime saw a rollback for the first (successful) attach.
        let disconnected = counting.disconnect_calls.lock().unwrap().clone();
        assert_eq!(
            disconnected,
            vec![("web".to_string(), "zlayer-svc-0".to_string())],
            "rollback should disconnect the only completed attach"
        );

        // And the fake runtime was asked to stop + remove the container.
        let stopped = tracing_concrete.stopped.lock().unwrap().clone();
        assert_eq!(stopped, vec!["svc-rep-0".to_string()]);
        let removed = tracing_concrete.removed.lock().unwrap().clone();
        assert_eq!(removed, vec!["svc-rep-0".to_string()]);
    }

    // -----------------------------------------------------------------------
    // Tasks 5.5.1–5.5.6: pause / unpause / top / changes / port / prune
    //
    // Each test pins one endpoint by driving the handler with a
    // `LifecycleRuntime` mock that records the runtime call and returns a
    // pre-canned response. The mocks delegate every other Runtime method to
    // the inner `MockRuntime` so we don't have to reimplement the whole
    // trait.
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct LifecycleRuntime {
        inner: zlayer_agent::MockRuntime,
        pause_calls: std::sync::Mutex<Vec<ContainerId>>,
        unpause_calls: std::sync::Mutex<Vec<ContainerId>>,
        top_calls: std::sync::Mutex<Vec<(ContainerId, Vec<String>)>>,
        changes_calls: std::sync::Mutex<Vec<ContainerId>>,
        port_calls: std::sync::Mutex<Vec<ContainerId>>,
        prune_calls: std::sync::Mutex<u32>,
        // Archive endpoints (Tasks 5.4.1-6) — record calls + return canned bytes.
        archive_get_calls: std::sync::Mutex<Vec<(ContainerId, String)>>,
        archive_put_calls:
            std::sync::Mutex<Vec<(ContainerId, String, bytes::Bytes, ArchivePutOptions)>>,
        archive_head_calls: std::sync::Mutex<Vec<(ContainerId, String)>>,
    }

    #[async_trait::async_trait]
    impl Runtime for LifecycleRuntime {
        async fn pull_image(&self, image: &str) -> zlayer_agent::error::Result<()> {
            self.inner.pull_image(image).await
        }
        async fn pull_image_with_policy(
            &self,
            image: &str,
            policy: zlayer_spec::PullPolicy,
            auth: Option<&zlayer_spec::RegistryAuth>,
        ) -> zlayer_agent::error::Result<()> {
            self.inner.pull_image_with_policy(image, policy, auth).await
        }
        async fn create_container(
            &self,
            id: &ContainerId,
            spec: &zlayer_spec::ServiceSpec,
        ) -> zlayer_agent::error::Result<()> {
            self.inner.create_container(id, spec).await
        }
        async fn start_container(&self, id: &ContainerId) -> zlayer_agent::error::Result<()> {
            self.inner.start_container(id).await
        }
        async fn stop_container(
            &self,
            id: &ContainerId,
            t: std::time::Duration,
        ) -> zlayer_agent::error::Result<()> {
            self.inner.stop_container(id, t).await
        }
        async fn remove_container(&self, id: &ContainerId) -> zlayer_agent::error::Result<()> {
            self.inner.remove_container(id).await
        }
        async fn container_state(
            &self,
            id: &ContainerId,
        ) -> zlayer_agent::error::Result<ContainerState> {
            self.inner.container_state(id).await
        }
        async fn container_logs(
            &self,
            id: &ContainerId,
            tail: usize,
        ) -> zlayer_agent::error::Result<Vec<zlayer_observability::logs::LogEntry>> {
            self.inner.container_logs(id, tail).await
        }
        async fn exec(
            &self,
            id: &ContainerId,
            cmd: &[String],
        ) -> zlayer_agent::error::Result<(i32, String, String)> {
            self.inner.exec(id, cmd).await
        }
        async fn get_container_stats(
            &self,
            id: &ContainerId,
        ) -> zlayer_agent::error::Result<zlayer_agent::cgroups_stats::ContainerStats> {
            self.inner.get_container_stats(id).await
        }
        async fn wait_container(&self, id: &ContainerId) -> zlayer_agent::error::Result<i32> {
            self.inner.wait_container(id).await
        }
        async fn get_logs(
            &self,
            id: &ContainerId,
        ) -> zlayer_agent::error::Result<Vec<zlayer_observability::logs::LogEntry>> {
            self.inner.get_logs(id).await
        }
        async fn get_container_pid(
            &self,
            id: &ContainerId,
        ) -> zlayer_agent::error::Result<Option<u32>> {
            self.inner.get_container_pid(id).await
        }
        async fn get_container_ip(
            &self,
            id: &ContainerId,
        ) -> zlayer_agent::error::Result<Option<std::net::IpAddr>> {
            self.inner.get_container_ip(id).await
        }

        async fn pause_container(&self, id: &ContainerId) -> zlayer_agent::error::Result<()> {
            self.pause_calls.lock().unwrap().push(id.clone());
            Ok(())
        }
        async fn unpause_container(&self, id: &ContainerId) -> zlayer_agent::error::Result<()> {
            self.unpause_calls.lock().unwrap().push(id.clone());
            Ok(())
        }
        async fn top_container(
            &self,
            id: &ContainerId,
            ps_args: &[String],
        ) -> zlayer_agent::error::Result<zlayer_agent::runtime::ContainerTopOutput> {
            self.top_calls
                .lock()
                .unwrap()
                .push((id.clone(), ps_args.to_vec()));
            Ok(zlayer_agent::runtime::ContainerTopOutput {
                titles: vec!["UID".to_string(), "PID".to_string(), "CMD".to_string()],
                processes: vec![vec![
                    "0".to_string(),
                    "1234".to_string(),
                    "/bin/sleep 1000".to_string(),
                ]],
            })
        }
        async fn changes_container(
            &self,
            id: &ContainerId,
        ) -> zlayer_agent::error::Result<Vec<zlayer_agent::runtime::FilesystemChangeEntry>>
        {
            self.changes_calls.lock().unwrap().push(id.clone());
            Ok(vec![
                zlayer_agent::runtime::FilesystemChangeEntry {
                    path: "/etc/hosts".to_string(),
                    kind: zlayer_agent::runtime::FilesystemChangeKind::Modified,
                },
                zlayer_agent::runtime::FilesystemChangeEntry {
                    path: "/tmp/new".to_string(),
                    kind: zlayer_agent::runtime::FilesystemChangeKind::Added,
                },
            ])
        }
        async fn port_mappings_container(
            &self,
            id: &ContainerId,
        ) -> zlayer_agent::error::Result<Vec<zlayer_agent::runtime::PortMappingEntry>> {
            self.port_calls.lock().unwrap().push(id.clone());
            Ok(vec![zlayer_agent::runtime::PortMappingEntry {
                container_port: 80,
                protocol: "tcp".to_string(),
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some(8080),
            }])
        }
        async fn prune_containers(
            &self,
        ) -> zlayer_agent::error::Result<zlayer_agent::runtime::ContainerPruneResult> {
            *self.prune_calls.lock().unwrap() += 1;
            Ok(zlayer_agent::runtime::ContainerPruneResult {
                deleted: vec!["abc".to_string(), "def".to_string()],
                space_reclaimed: 1024,
            })
        }
        async fn archive_get(
            &self,
            id: &ContainerId,
            path: &str,
        ) -> zlayer_agent::error::Result<zlayer_agent::runtime::ArchiveStream> {
            self.archive_get_calls
                .lock()
                .unwrap()
                .push((id.clone(), path.to_string()));
            // Return a deterministic 4-byte chunk so the test can assert
            // that the streaming body actually delivers it.
            let chunks = vec![Ok(bytes::Bytes::from_static(b"TAR1"))];
            Ok(Box::pin(futures_util::stream::iter(chunks)))
        }
        async fn archive_put(
            &self,
            id: &ContainerId,
            path: &str,
            tar_bytes: bytes::Bytes,
            opts: zlayer_agent::runtime::ArchivePutOptions,
        ) -> zlayer_agent::error::Result<()> {
            self.archive_put_calls.lock().unwrap().push((
                id.clone(),
                path.to_string(),
                tar_bytes,
                opts,
            ));
            Ok(())
        }
        async fn archive_head(
            &self,
            id: &ContainerId,
            path: &str,
        ) -> zlayer_agent::error::Result<zlayer_agent::runtime::PathStat> {
            self.archive_head_calls
                .lock()
                .unwrap()
                .push((id.clone(), path.to_string()));
            Ok(zlayer_agent::runtime::PathStat {
                name: "etc".to_string(),
                size: 4096,
                mode: 0o040_755,
                mtime: Some("2026-05-03T00:00:00Z".to_string()),
                link_target: String::new(),
            })
        }
    }

    /// Build a test state pre-loaded with one standalone container backed
    /// by a `LifecycleRuntime` so the lifecycle handlers (pause / top /
    /// changes / port) have something to resolve against. Returns the state
    /// plus the container's hex id and the runtime handle for assertions.
    async fn make_lifecycle_state() -> (ContainerApiState, String, Arc<LifecycleRuntime>) {
        let runtime: Arc<LifecycleRuntime> = Arc::new(LifecycleRuntime::default());
        let runtime_dyn: Arc<dyn Runtime + Send + Sync> = runtime.clone();
        let state =
            ContainerApiState::with_daemon_uuid(runtime_dyn, "lifecycle-test-uuid".to_string());
        let cid = ContainerId::new("standalone-life".to_string(), 0);
        let hex = state.id_map.register(state.id_map.daemon_uuid(), &cid);
        let standalone = StandaloneContainer {
            container_id: cid.clone(),
            image: "alpine:latest".to_string(),
            name: Some("standalone-life".to_string()),
            labels: HashMap::new(),
            created_at: "2026-05-03T00:00:00Z".to_string(),
            delete_on_exit: false,
        };
        state
            .containers
            .write()
            .await
            .insert("standalone-life".to_string(), standalone);
        (state, hex, runtime)
    }

    fn operator_user() -> AuthUser {
        use crate::auth::Claims;
        use std::time::Duration as StdDuration;
        AuthUser {
            claims: Claims::new(
                "test",
                StdDuration::from_secs(60),
                vec!["operator".to_string()],
                None,
            ),
        }
    }

    #[tokio::test]
    async fn pause_container_handler_invokes_runtime_and_returns_204() {
        let (state, hex, runtime) = make_lifecycle_state().await;
        let status = pause_container(operator_user(), State(state), Path(hex))
            .await
            .expect("pause must succeed");
        assert_eq!(status, axum::http::StatusCode::NO_CONTENT);
        let calls = runtime.pause_calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].service, "standalone-life");
    }

    #[tokio::test]
    async fn unpause_container_handler_invokes_runtime_and_returns_204() {
        let (state, hex, runtime) = make_lifecycle_state().await;
        let status = unpause_container(operator_user(), State(state), Path(hex))
            .await
            .expect("unpause must succeed");
        assert_eq!(status, axum::http::StatusCode::NO_CONTENT);
        let calls = runtime.unpause_calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
    }

    #[tokio::test]
    async fn top_container_handler_returns_runtime_output() {
        let (state, hex, runtime) = make_lifecycle_state().await;
        let resp = top_container(
            operator_user(),
            State(state),
            Path(hex),
            Query(ContainerTopQuery {
                ps_args: Some("aux".to_string()),
            }),
        )
        .await
        .expect("top must succeed");
        let body = resp.0;
        assert_eq!(body.titles, vec!["UID", "PID", "CMD"]);
        assert_eq!(body.processes.len(), 1);
        assert_eq!(body.processes[0][1], "1234");

        let calls = runtime.top_calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, vec!["aux".to_string()]);
    }

    #[tokio::test]
    async fn changes_container_handler_returns_docker_kind_integers() {
        let (state, hex, runtime) = make_lifecycle_state().await;
        let resp = changes_container(operator_user(), State(state), Path(hex))
            .await
            .expect("changes must succeed");
        let body = resp.0;
        assert_eq!(body.len(), 2);
        // Modified == 0, Added == 1
        assert_eq!(body[0].path, "/etc/hosts");
        assert_eq!(body[0].kind, 0);
        assert_eq!(body[1].path, "/tmp/new");
        assert_eq!(body[1].kind, 1);

        assert_eq!(runtime.changes_calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn port_container_handler_groups_bindings_by_port_protocol_key() {
        let (state, hex, runtime) = make_lifecycle_state().await;
        let resp = port_container(operator_user(), State(state), Path(hex))
            .await
            .expect("port must succeed");
        let body = resp.0;
        let bucket = body
            .ports
            .get("80/tcp")
            .expect("80/tcp key must be present")
            .as_ref()
            .expect("bucket must contain bindings");
        assert_eq!(bucket.len(), 1);
        assert_eq!(bucket[0].host_ip.as_deref(), Some("0.0.0.0"));
        assert_eq!(bucket[0].host_port.as_deref(), Some("8080"));

        assert_eq!(runtime.port_calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn prune_containers_handler_returns_docker_shape() {
        let runtime: Arc<LifecycleRuntime> = Arc::new(LifecycleRuntime::default());
        let runtime_dyn: Arc<dyn Runtime + Send + Sync> = runtime.clone();
        let state = ContainerApiState::with_daemon_uuid(runtime_dyn, "prune-test-uuid".to_string());

        let resp = prune_containers(operator_user(), State(state))
            .await
            .expect("prune must succeed");
        let body = resp.0;
        assert_eq!(body.containers_deleted, vec!["abc", "def"]);
        assert_eq!(body.space_reclaimed, 1024);
        assert_eq!(*runtime.prune_calls.lock().unwrap(), 1);
    }

    // -------------------------------------------------------------------------
    // Tasks 5.4.1-6 — container archive endpoints (`docker cp`)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn archive_get_handler_streams_runtime_bytes_with_path_stat_header() {
        use axum::body::to_bytes;
        use base64::Engine;

        let (state, hex, runtime) = make_lifecycle_state().await;
        let resp = archive_get(
            operator_user(),
            State(state),
            Path(hex),
            Query(ArchivePathQuery {
                path: Some("/etc".to_string()),
            }),
        )
        .await
        .expect("archive_get must succeed");

        // Header should be base64-encoded JSON describing /etc.
        let header = resp
            .headers()
            .get("X-Docker-Container-Path-Stat")
            .expect("path-stat header must be present")
            .to_str()
            .unwrap()
            .to_string();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&header)
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(v["name"], "etc");
        assert_eq!(v["size"], 4096);
        assert_eq!(v["mode"], 0o040_755);

        // Body must include the canned bytes from the runtime.
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        assert_eq!(body.as_ref(), b"TAR1");

        // Both archive_head (for the header) and archive_get (for the body)
        // must have been called once each.
        assert_eq!(runtime.archive_head_calls.lock().unwrap().len(), 1);
        let get_calls = runtime.archive_get_calls.lock().unwrap();
        assert_eq!(get_calls.len(), 1);
        assert_eq!(get_calls[0].1, "/etc");
    }

    #[tokio::test]
    async fn archive_put_handler_forwards_body_and_options() {
        let (state, hex, runtime) = make_lifecycle_state().await;
        let body = bytes::Bytes::from_static(&[0u8, 1, 2, 3]);
        let status = archive_put(
            operator_user(),
            State(state),
            Path(hex),
            Query(ArchivePutQuery {
                path: Some("/tmp".to_string()),
                no_overwrite_dir_non_dir: Some("1".to_string()),
                copy_uid_gid: Some("true".to_string()),
            }),
            body.clone(),
        )
        .await
        .expect("archive_put must succeed");
        assert_eq!(status, axum::http::StatusCode::OK);

        let calls = runtime.archive_put_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "/tmp");
        assert_eq!(calls[0].2, body);
        assert!(calls[0].3.no_overwrite_dir_non_dir);
        assert!(calls[0].3.copy_uid_gid);
    }

    #[tokio::test]
    async fn archive_head_handler_only_returns_path_stat_header() {
        use axum::body::to_bytes;

        let (state, hex, runtime) = make_lifecycle_state().await;
        let resp = archive_head(
            operator_user(),
            State(state),
            Path(hex),
            Query(ArchivePathQuery {
                path: Some("/etc".to_string()),
            }),
        )
        .await
        .expect("archive_head must succeed");

        // Body must be empty.
        let body = to_bytes(resp.into_body(), 16).await.unwrap();
        assert!(body.is_empty());

        assert_eq!(runtime.archive_head_calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn archive_get_handler_rejects_missing_path() {
        let (state, hex, _runtime) = make_lifecycle_state().await;
        let err = archive_get(
            operator_user(),
            State(state),
            Path(hex),
            Query(ArchivePathQuery { path: None }),
        )
        .await
        .expect_err("missing path must yield BadRequest");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }
}
