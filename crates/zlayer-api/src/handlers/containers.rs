//! Raw container lifecycle endpoints
//!
//! Provides direct container management endpoints for use by CI runners and
//! other tooling that needs to manage containers independently of the
//! deployment/service abstraction.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info};
use utoipa::{IntoParams, ToSchema};

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use crate::event_bus::{ContainerEvent, ContainerEventBus};
use crate::handlers::container_networks::BridgeNetworkApiState;
use zlayer_agent::runtime::{
    ContainerId, ContainerInspectDetails, ContainerState, ExecEvent, HealthDetail,
    NetworkAttachmentDetail, Runtime,
};
use zlayer_spec::BridgeNetworkAttachment;

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
}

impl ContainerApiState {
    /// Create a new container API state with a runtime and a fresh event bus.
    pub fn new(runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        Self {
            runtime,
            containers: Arc::new(RwLock::new(HashMap::new())),
            event_bus: ContainerEventBus::new(),
            bridge_networks: None,
            registry_store: None,
        }
    }

    /// Create a new container API state with a runtime and an existing event
    /// bus. Used when the event bus must be shared across multiple route
    /// groups (e.g. `/api/v1/containers` and `/api/v1/events`).
    pub fn with_event_bus(
        runtime: Arc<dyn Runtime + Send + Sync>,
        event_bus: ContainerEventBus,
    ) -> Self {
        Self {
            runtime,
            containers: Arc::new(RwLock::new(HashMap::new())),
            event_bus,
            bridge_networks: None,
            registry_store: None,
        }
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
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Resource limits for a container
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct ContainerResourceLimits {
    /// CPU limit in cores (e.g., 0.5, 1.0, 2.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<f64>,
    /// Memory limit (e.g., "256Mi", "1Gi")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
}

/// Volume mount kind discriminator.
///
/// Selects which [`zlayer_spec::StorageSpec`] variant [`VolumeMount`] is
/// translated into by [`build_service_spec`]. When omitted on the wire,
/// defaults to [`VolumeMountType::Bind`] (legacy behavior).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VolumeMountType {
    /// Host-path bind mount. `source` is an absolute host path.
    Bind,
    /// Named persistent volume. `source` is the volume name (managed by
    /// `/api/v1/volumes`), not a host path.
    Volume,
    /// Memory-backed tmpfs mount. `source` must be empty/omitted.
    Tmpfs,
}

/// Volume mount specification.
///
/// The `type` field (a Docker-compatible discriminator) selects how `source`
/// is interpreted:
/// - `"bind"` (default): `source` is an absolute host path.
/// - `"volume"`: `source` is a named-volume identifier.
/// - `"tmpfs"`: no `source`; a memory-backed mount is provisioned.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct VolumeMount {
    /// Mount kind. Omit (or `"bind"`) for legacy host-path binds.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub mount_type: Option<VolumeMountType>,
    /// Host path (bind), volume name (volume), or unused (tmpfs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Container mount path
    pub target: String,
    /// Mount as read-only
    #[serde(default)]
    pub readonly: bool,
}

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

/// Container health check request.
///
/// Mirrors the on-disk `HealthCheck` enum (see `zlayer_spec::HealthCheck`) as a
/// discriminated union keyed on `type`. Translated to `zlayer_spec::HealthSpec`
/// by `HealthCheckRequest::to_health_spec`. Durations are humantime strings
/// (for example `"10s"`, `"500ms"`, `"1m"`).
///
/// ## Variants
/// - `type: "tcp"` — requires `port` (1-65535).
/// - `type: "http"` — requires `url`; `expect_status` defaults to 200.
/// - `type: "command"` — requires `command` (array of argv tokens; joined with
///   spaces and passed to `sh -c` by the health monitor, matching the existing
///   compose-to-ZLayer conversion in `zlayer-docker`).
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct HealthCheckRequest {
    /// Check variant: `"tcp"`, `"http"`, or `"command"`.
    #[serde(rename = "type")]
    pub check_type: String,
    /// TCP port (required when `type == "tcp"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// HTTP URL (required when `type == "http"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP status code expected from `url` (defaults to 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_status: Option<u16>,
    /// Command argv (required when `type == "command"`). Joined with spaces
    /// and passed to `sh -c`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Interval between checks, humantime format (e.g. `"30s"`). Defaults to 30s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    /// Timeout per individual check, humantime format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    /// Number of consecutive failures before marking unhealthy. Defaults to 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
    /// Grace period before the first check runs, humantime format. Maps to
    /// `HealthSpec::start_grace`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_period: Option<String>,
}

impl HealthCheckRequest {
    /// Translate the wire-format request into the internal `HealthSpec`.
    ///
    /// # Errors
    /// Returns `ApiError::BadRequest` for:
    /// - Unknown `type` (must be `"tcp"`, `"http"`, or `"command"`).
    /// - Missing required fields per variant (e.g. `port == 0` for tcp, empty `url`
    ///   for http, missing/empty `command` for command).
    /// - Malformed humantime strings on `interval`, `timeout`, or `start_period`.
    pub fn to_health_spec(&self) -> Result<zlayer_spec::HealthSpec> {
        use zlayer_spec::{HealthCheck, HealthSpec};

        let check = match self.check_type.as_str() {
            "tcp" => {
                let port = self.port.ok_or_else(|| {
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
                let url = self
                    .url
                    .as_ref()
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| {
                        ApiError::BadRequest(
                            "health_check.url is required when type == \"http\"".to_string(),
                        )
                    })?
                    .clone();
                let expect_status = self.expect_status.unwrap_or(200);
                HealthCheck::Http { url, expect_status }
            }
            "command" => {
                let argv = self
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

        let interval = parse_optional_duration(self.interval.as_deref(), "health_check.interval")?
            .or_else(|| Some(Duration::from_secs(30)));
        let timeout = parse_optional_duration(self.timeout.as_deref(), "health_check.timeout")?;
        let start_grace =
            parse_optional_duration(self.start_period.as_deref(), "health_check.start_period")?;
        let retries = self.retries.unwrap_or(3);

        Ok(HealthSpec {
            start_grace,
            interval,
            timeout,
            retries,
            check,
        })
    }
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

/// Request to create and start a container
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateContainerRequest {
    /// OCI image reference (e.g., "nginx:latest", "ubuntu:22.04")
    pub image: String,
    /// Optional human-readable name
    #[serde(default)]
    pub name: Option<String>,
    /// Image pull policy: "always", "`if_not_present`", or "never"
    #[serde(default)]
    pub pull_policy: Option<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Command to run (overrides image entrypoint)
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// Labels for filtering and grouping
    #[serde(default)]
    pub labels: HashMap<String, String>,
    /// Resource limits (CPU, memory)
    #[serde(default)]
    pub resources: Option<ContainerResourceLimits>,
    /// Volume mounts
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,
    /// Published ports (Docker's `-p host:container/proto`). When omitted,
    /// the container is created without any host port publishing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<zlayer_spec::PortMapping>,
    /// Working directory inside the container
    #[serde(default)]
    pub work_dir: Option<String>,
    /// Optional health check. When omitted, the daemon installs a no-op
    /// placeholder (`HealthCheck::Tcp { port: 0 }`) matching the current
    /// default; the health monitor treats `port == 0` as "skip".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check: Option<HealthCheckRequest>,
    /// Optional container hostname (maps to Docker's `--hostname`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    /// Additional DNS servers (maps to Docker's `--dns`). Each entry must be
    /// a plausible IPv4 or IPv6 address.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns: Vec<String>,
    /// Extra `hostname:ip` entries appended to `/etc/hosts` (maps to Docker's
    /// `--add-host`). The special literal `host-gateway` is accepted as the
    /// `ip` half.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_hosts: Vec<String>,
    /// Container restart policy (Docker-style). When omitted, the runtime
    /// applies no explicit restart policy (Docker default: `"no"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_policy: Option<zlayer_spec::ContainerRestartPolicy>,
    /// User-defined bridge/overlay networks to attach the newly-created
    /// container to. Each entry references a network by id or name and is
    /// attached after the container is successfully started. If any
    /// attachment fails, the partially-started container is rolled back
    /// (stopped + removed) and the request is failed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<NetworkAttachmentRequest>,
    // -- §3.10: registry auth ------------------------------------------------
    /// Id of a persisted registry credential (from
    /// `POST /api/v1/credentials/registry`) to use when pulling the image.
    /// Ignored when [`Self::registry_auth`] is also supplied (inline auth
    /// wins). Requires the daemon to be configured with a credential store
    /// — otherwise the request is rejected with `400`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_credential_id: Option<String>,
    /// Inline Docker/OCI registry credentials used for this pull only. Not
    /// persisted, never logged, never echoed back on a response. When both
    /// `registry_credential_id` and `registry_auth` are set, this field
    /// takes precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_auth: Option<zlayer_spec::RegistryAuth>,
}

/// A request to attach a freshly-created container to a user-defined bridge
/// or overlay network, mirroring the wire-shape used by `POST
/// /api/v1/container-networks/{id_or_name}/connect`.
///
/// Included on [`CreateContainerRequest::networks`] so callers can wire up
/// every attachment in a single call instead of issuing a separate connect
/// request per network after container create.
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct NetworkAttachmentRequest {
    /// Bridge-network id or name to attach to.
    pub network: String,
    /// Optional DNS aliases for this container on the network.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Optional static IPv4 to pin this container to. Validated as
    /// [`std::net::Ipv4Addr`] before the runtime is called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4_address: Option<String>,
}

/// Container information returned by the API
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerInfo {
    /// Container identifier
    pub id: String,
    /// Human-readable name (if set)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// OCI image reference
    pub image: String,
    /// Container state (pending, running, exited, failed)
    pub state: String,
    /// Labels
    pub labels: HashMap<String, String>,
    /// Creation timestamp (ISO 8601)
    pub created_at: String,
    /// Process ID (if running)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    // -- §3.15: rich inspect fields -----------------------------------------
    /// Published port mappings (container → host). Populated from the
    /// runtime's inspect response; empty when the runtime doesn't expose
    /// port-level detail or the container has no published ports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<zlayer_spec::PortMapping>,
    /// Networks this container is attached to, with per-network aliases
    /// and IPv4. Empty when the runtime doesn't surface network detail.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<NetworkAttachmentInfo>,
    /// Primary IPv4 address (first non-empty IP across attached networks).
    /// Docker's `bridge` network is preferred when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<String>,
    /// Runtime-native health status, when the container image declares a
    /// `HEALTHCHECK` (or equivalent). `None` when the runtime doesn't track
    /// health for this container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<ContainerHealthInfo>,
    /// Most-recent exit code. `None` for containers still running and for
    /// containers that have never exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// Per-network attachment entry on [`ContainerInfo::networks`].
///
/// Populated from the runtime's inspect response — mirrors the subset of
/// bollard's `EndpointSettings` that API clients need to correlate a container
/// with its `container_networks` entries.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct NetworkAttachmentInfo {
    /// Network name as reported by the runtime. Matches the `name` field on
    /// entries returned by `GET /api/v1/container-networks`.
    pub network: String,
    /// DNS aliases the container answers to on this network.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Assigned IPv4 on this network, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<String>,
}

/// Runtime-native health snapshot on [`ContainerInfo::health`].
///
/// Sourced from bollard's `ContainerState.health` for Docker-backed
/// containers. The internal `HealthMonitor` in
/// `crates/zlayer-agent/src/health.rs` drives service-level health events
/// against user-configured health specs; for standalone containers the API
/// reports the runtime-native status instead so images with a baked-in
/// `HEALTHCHECK` still surface correctly.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ContainerHealthInfo {
    /// One of `"none"`, `"starting"`, `"healthy"`, `"unhealthy"` (Docker
    /// `HealthStatusEnum`). Empty / missing upstream values normalise to
    /// `"none"`.
    pub status: String,
    /// Consecutive failing probe count, when the runtime tracks it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failing_streak: Option<u32>,
    /// Output from the most recent failing probe, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output: Option<String>,
}

impl From<NetworkAttachmentDetail> for NetworkAttachmentInfo {
    fn from(d: NetworkAttachmentDetail) -> Self {
        Self {
            network: d.network,
            aliases: d.aliases,
            ipv4: d.ipv4,
        }
    }
}

impl From<HealthDetail> for ContainerHealthInfo {
    fn from(d: HealthDetail) -> Self {
        Self {
            status: d.status,
            failing_streak: d.failing_streak,
            last_output: d.last_output,
        }
    }
}

/// Query parameters for listing containers
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListContainersQuery {
    /// Filter by label (key=value format)
    #[serde(default)]
    pub label: Option<String>,
}

/// Query parameters for container logs
#[derive(Debug, Deserialize, IntoParams)]
pub struct ContainerLogQuery {
    /// Number of tail lines to return
    #[serde(default = "default_tail")]
    pub tail: usize,
    /// Follow logs (SSE stream)
    #[serde(default)]
    pub follow: bool,
}

fn default_tail() -> usize {
    100
}

/// Exec request for running a command in a container
#[derive(Debug, Deserialize, ToSchema)]
pub struct ContainerExecRequest {
    /// Command and arguments to execute
    pub command: Vec<String>,
}

/// Query parameters for the exec endpoint.
///
/// When `stream=true` the handler returns a Server-Sent Events stream with
/// one `stdout` / `stderr` event per line of output and a final `exit` event
/// carrying the exit code as JSON. When `stream=false` (the default) the
/// handler buffers the whole output and returns a single JSON
/// [`ContainerExecResponse`] body.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct ExecQuery {
    /// Stream exec events as SSE instead of returning a buffered JSON body.
    #[serde(default)]
    pub stream: bool,
}

/// Exec response with command output
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerExecResponse {
    /// Exit code from the command
    pub exit_code: i32,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
}

/// Request body for stopping a container. Matches the Docker-compat
/// `POST /containers/{id}/stop` shape.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct StopContainerRequest {
    /// Graceful shutdown timeout in seconds before the runtime force-kills
    /// the container. Defaults to 30 seconds when omitted.
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Request body for restarting a container. Matches the Docker-compat
/// `POST /containers/{id}/restart` shape.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct RestartContainerRequest {
    /// Graceful shutdown timeout in seconds before the runtime force-kills
    /// the container. Defaults to 30 seconds when omitted.
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Request body for killing (sending a signal to) a container. Matches the
/// Docker-compat `POST /containers/{id}/kill` shape.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct KillContainerRequest {
    /// Signal name to send (e.g. `"SIGTERM"`, `"SIGINT"`). Accepts both the
    /// `SIG`-prefixed and bare forms. When omitted, defaults to `SIGKILL`.
    #[serde(default)]
    pub signal: Option<String>,
}

/// Wait response with container exit code plus optional classification
/// fields (added in §3.12 of the SDK-fixes spec).
///
/// The three optional fields (`reason`, `signal`, `finished_at`) are
/// additive — clients that only read `exit_code` keep working unchanged.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerWaitResponse {
    /// Container identifier
    pub id: String,
    /// Exit code (0 = success). When the container was killed by signal
    /// `N`, this is typically `128 + N`.
    pub exit_code: i32,
    /// Classification of the exit. One of `"exited"`, `"signal"`,
    /// `"oom_killed"`, or `"runtime_error"`. Absent when the runtime
    /// didn't classify the exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Signal name when `reason == "signal"`, e.g. `"SIGKILL"`. Absent
    /// when the runtime couldn't determine it (or the exit wasn't a
    /// signal death).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    /// RFC3339 timestamp of when the container exited, if reported by
    /// the runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

/// Container resource statistics
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerStatsResponse {
    /// Container identifier
    pub id: String,
    /// CPU usage in microseconds
    pub cpu_usage_usec: u64,
    /// Current memory usage in bytes
    pub memory_bytes: u64,
    /// Memory limit in bytes (`u64::MAX` if unlimited)
    pub memory_limit: u64,
    /// Memory usage as percentage of limit
    pub memory_percent: f64,
}

/// Query parameters for container stats.
///
/// When `stream=false` (default), the handler returns a single JSON
/// [`ContainerStatsResponse`]. When `stream=true`, the handler switches to
/// Server-Sent Events and emits one `ContainerStatsResponse` sample per
/// `interval` seconds until the container exits or the client disconnects.
///
/// `interval` is clamped to `[1, 60]` seconds. Default interval is `2`.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct StatsQuery {
    /// Stream periodic samples as SSE events instead of a one-shot JSON
    /// response.
    #[serde(default)]
    pub stream: bool,
    /// Sample cadence in seconds (only used when `stream=true`). Clamped to
    /// `[1, 60]`. Defaults to `2` seconds.
    #[serde(default, alias = "interval_seconds")]
    pub interval: Option<u32>,
}

impl StatsQuery {
    /// Clamp the requested interval into `[1, 60]` seconds, defaulting to 2.
    fn clamped_interval(&self) -> Duration {
        let secs = self.interval.unwrap_or(2).clamp(1, 60);
        Duration::from_secs(u64::from(secs))
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
    let cid = ContainerId {
        service: service_name.clone(),
        replica: 0,
    };
    (service_name, cid)
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

    let resources = match &request.resources {
        Some(r) => ResourcesSpec {
            cpu: r.cpu,
            memory: r.memory.clone(),
            gpu: None,
        },
        None => ResourcesSpec::default(),
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
        Some(hc) => hc.to_health_spec()?,
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
            name: request.image.clone(),
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
        devices: Vec::new(),
        storage,
        port_mappings: request.ports.clone(),
        capabilities: Vec::new(),
        privileged: false,
        node_mode: NodeMode::default(),
        node_selector: None,
        platform: None,
        service_type: ServiceType::default(),
        wasm: None,
        logs: None,
        host_network: false,
        hostname: request.hostname.clone(),
        dns: request.dns.clone(),
        extra_hosts: request.extra_hosts.clone(),
        restart_policy: request.restart_policy.clone(),
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
                .map(NetworkAttachmentInfo::from)
                .collect(),
            ipv4,
            health: health.map(ContainerHealthInfo::from),
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
    let spec = build_service_spec(&request)?;

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

    // Track the container
    let standalone = StandaloneContainer {
        container_id: container_id.clone(),
        image: request.image.clone(),
        name: request.name.clone(),
        labels: request.labels.clone(),
        created_at: now.clone(),
    };

    state
        .containers
        .write()
        .await
        .insert(id_str.clone(), standalone);

    // Emit container.start event on the daemon-wide bus.
    state.event_bus.publish(ContainerEvent::start(
        id_str.clone(),
        request.labels.clone(),
    ));

    let info = ContainerInfo {
        id: id_str,
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

    for (id_str, meta) in containers.iter() {
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

        results.push(ContainerInfo {
            id: id_str.clone(),
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
    let containers = state.containers.read().await;
    let meta = containers
        .get(&id)
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

    Ok(Json(ContainerInfo {
        id: id.clone(),
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

    let (container_id, labels) = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        (meta.container_id.clone(), meta.labels.clone())
    };

    info!(container_id = %container_id, "Stopping and removing standalone container");

    // Stop with 30s timeout (ignore errors if container is already stopped)
    let stop_result = state
        .runtime
        .stop_container(&container_id, Duration::from_secs(30))
        .await;
    if let Err(ref e) = stop_result {
        debug!(error = %e, "Stop returned error (container may already be stopped)");
    }

    // Remove the container
    state
        .runtime
        .remove_container(&container_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to remove container: {e}")))?;

    // Remove from tracking
    state.containers.write().await.remove(&id);

    // Emit container.die event after successful stop+remove.
    state.event_bus.publish(ContainerEvent::die(
        id.clone(),
        labels,
        None,
        Some("deleted".to_string()),
    ));

    Ok(axum::http::StatusCode::NO_CONTENT)
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

    let (container_id, labels) = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        (meta.container_id.clone(), meta.labels.clone())
    };

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
        id.clone(),
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

    let (container_id, labels) = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        (meta.container_id.clone(), meta.labels.clone())
    };

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
        .publish(ContainerEvent::start(id.clone(), labels));

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

    let (container_id, labels) = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        (meta.container_id.clone(), meta.labels.clone())
    };

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
            id.clone(),
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
        .publish(ContainerEvent::start(id.clone(), labels));

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

    let (container_id, labels) = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        (meta.container_id.clone(), meta.labels.clone())
    };

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
        .publish(ContainerEvent::die(id.clone(), labels, None, Some(reason)));

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Get container logs.
///
/// Returns the last N lines of container logs as plain text, or streams
/// logs as Server-Sent Events when `follow=true`.
///
/// # Errors
///
/// Returns an error if the container is not found or log retrieval fails.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/logs",
    params(
        ("id" = String, Path, description = "Container identifier"),
        ContainerLogQuery,
    ),
    responses(
        (status = 200, description = "Container logs (plain text or SSE stream)", body = String),
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
    let container_id = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.container_id.clone()
    };

    if query.follow {
        // SSE streaming mode
        let stream = container_log_follow_stream(state.runtime.clone(), container_id, query.tail);
        let sse = Sse::new(stream).keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        );
        Ok(sse.into_response())
    } else {
        // Plain text mode
        let entries = state
            .runtime
            .container_logs(&container_id, query.tail)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to get logs: {e}")))?;
        let logs: String = entries
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        Ok(logs.into_response())
    }
}

/// Internal state for the container log-follow polling stream.
struct ContainerLogFollowState {
    runtime: Arc<dyn Runtime + Send + Sync>,
    container_id: ContainerId,
    tail: usize,
    seen_line_count: usize,
    initial: bool,
    poll_interval: Duration,
    poll_counter: u64,
    terminated: bool,
}

/// Build an SSE stream that polls for new log output.
fn container_log_follow_stream(
    runtime: Arc<dyn Runtime + Send + Sync>,
    container_id: ContainerId,
    tail: usize,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    futures_util::stream::unfold(
        ContainerLogFollowState {
            runtime,
            container_id,
            tail,
            seen_line_count: 0,
            initial: true,
            poll_interval: Duration::from_millis(500),
            poll_counter: 0,
            terminated: false,
        },
        |mut state| async move {
            // Once terminated on a previous iteration, close the stream.
            if state.terminated {
                return None;
            }

            if !state.initial {
                tokio::time::sleep(state.poll_interval).await;
            }

            let fetch_tail = if state.initial { state.tail } else { 10_000 };

            let entries = state
                .runtime
                .container_logs(&state.container_id, fetch_tail)
                .await
                .unwrap_or_default();

            let all_lines: Vec<String> = entries.iter().map(ToString::to_string).collect();
            let total = all_lines.len();

            let was_initial = state.initial;
            let new_lines = if state.initial {
                state.initial = false;
                state.seen_line_count = total;
                all_lines
            } else if total > state.seen_line_count {
                let new = all_lines[state.seen_line_count..].to_vec();
                state.seen_line_count = total;
                new
            } else {
                vec![]
            };

            let mut events: Vec<std::result::Result<Event, Infallible>> = new_lines
                .into_iter()
                .map(|line| Ok(Event::default().data(line)))
                .collect();

            // Decide whether to poll container state this tick. Always check on
            // the first iteration (catches already-exited containers); after
            // that, check every fourth poll to keep overhead low.
            let should_check = was_initial || state.poll_counter % 4 == 0;
            state.poll_counter = state.poll_counter.wrapping_add(1);

            if should_check {
                let exit_observed = matches!(
                    state.runtime.container_state(&state.container_id).await,
                    Ok(ContainerState::Exited { .. } | ContainerState::Failed { .. })
                );

                if exit_observed {
                    // Final log drain: fetch a large tail and append any lines
                    // we haven't already emitted to the current batch.
                    let final_entries = state
                        .runtime
                        .container_logs(&state.container_id, 10_000)
                        .await
                        .unwrap_or_default();
                    let all: Vec<String> = final_entries.iter().map(ToString::to_string).collect();
                    if all.len() > state.seen_line_count {
                        for line in &all[state.seen_line_count..] {
                            events.push(Ok(Event::default().data(line.clone())));
                        }
                        state.seen_line_count = all.len();
                    }

                    state.terminated = true;

                    debug!(
                        container_id = %state.container_id,
                        new_events = events.len(),
                        total_seen = state.seen_line_count,
                        "container log follow terminating after exit"
                    );

                    return Some((futures_util::stream::iter(events), state));
                }
            }

            debug!(
                container_id = %state.container_id,
                new_events = events.len(),
                total_seen = state.seen_line_count,
                "container log follow poll"
            );

            Some((futures_util::stream::iter(events), state))
        },
    )
    .flatten()
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

    let container_id = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.container_id.clone()
    };

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
    let (container_id, labels) = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        (meta.container_id.clone(), meta.labels.clone())
    };

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
        id.clone(),
        labels.clone(),
        Some(outcome.exit_code),
        reason_wire.clone(),
    ));
    if outcome.reason == zlayer_agent::runtime::WaitReason::OomKilled {
        state.event_bus.publish(ContainerEvent::oom(
            id.clone(),
            labels,
            Some(outcome.exit_code),
            Some("oom_killed".to_string()),
        ));
    }

    Ok(Json(ContainerWaitResponse {
        id,
        exit_code: outcome.exit_code,
        reason: reason_wire,
        signal: outcome.signal,
        finished_at: finished_at_wire,
    }))
}

/// Get container resource statistics.
///
/// Returns CPU and memory usage statistics for the specified container.
/// By default this is a one-shot JSON response. When the query string
/// contains `stream=true`, the handler switches to Server-Sent Events and
/// emits one `ContainerStatsResponse` sample every `interval` seconds
/// (default 2, clamped to `[1, 60]`). The stream ends with a final
/// `event: close` when the container exits or the runtime reports an
/// unrecoverable error.
///
/// # Errors
///
/// Returns an error if the container is not found or stats retrieval fails.
#[utoipa::path(
    get,
    path = "/api/v1/containers/{id}/stats",
    params(
        ("id" = String, Path, description = "Container identifier"),
        StatsQuery,
    ),
    responses(
        (status = 200, description = "Container statistics (JSON one-shot or SSE stream)", body = ContainerStatsResponse),
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
    let container_id = {
        let containers = state.containers.read().await;
        let meta = containers
            .get(&id)
            .ok_or_else(|| ApiError::NotFound(format!("Container '{id}' not found")))?;
        meta.container_id.clone()
    };

    if query.stream {
        // SSE streaming mode: emit one sample per tick until the container
        // exits or the runtime returns an error.
        let interval = query.clamped_interval();
        let stream =
            container_stats_follow_stream(state.runtime.clone(), container_id, id, interval);
        let sse = Sse::new(stream).keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        );
        return Ok(sse.into_response());
    }

    let cstats = state
        .runtime
        .get_container_stats(&container_id)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Container not found: {reason}"))
            }
            other => ApiError::Internal(format!("Failed to get stats: {other}")),
        })?;

    Ok(Json(ContainerStatsResponse {
        id,
        cpu_usage_usec: cstats.cpu_usage_usec,
        memory_bytes: cstats.memory_bytes,
        memory_limit: cstats.memory_limit,
        memory_percent: cstats.memory_percent(),
    })
    .into_response())
}

/// Internal state for the stats SSE follow stream.
struct ContainerStatsFollowState {
    runtime: Arc<dyn Runtime + Send + Sync>,
    container_id: ContainerId,
    /// Public-facing container id (the key clients use in the URL).
    public_id: String,
    tick: tokio::time::Interval,
    terminated: bool,
}

/// Build an SSE stream that emits a fresh `ContainerStatsResponse` on a
/// fixed cadence. The stream terminates when the runtime returns a fatal
/// error (the container is gone / exited), emitting a final
/// `event: close` with a small JSON reason payload.
fn container_stats_follow_stream(
    runtime: Arc<dyn Runtime + Send + Sync>,
    container_id: ContainerId,
    public_id: String,
    interval: Duration,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    // `MissedTickBehavior::Delay` means if we fall behind (slow subscriber)
    // we just resume ticking from now rather than replaying a backlog of
    // samples. This is the graceful "drop slow subscribers" behavior.
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    futures_util::stream::unfold(
        ContainerStatsFollowState {
            runtime,
            container_id,
            public_id,
            tick,
            terminated: false,
        },
        |mut state| async move {
            if state.terminated {
                return None;
            }

            // First tick on a `tokio::time::interval` fires immediately so
            // the client gets a sample right away; subsequent ticks wait
            // for the full interval.
            state.tick.tick().await;

            match state.runtime.get_container_stats(&state.container_id).await {
                Ok(cstats) => {
                    let sample = ContainerStatsResponse {
                        id: state.public_id.clone(),
                        cpu_usage_usec: cstats.cpu_usage_usec,
                        memory_bytes: cstats.memory_bytes,
                        memory_limit: cstats.memory_limit,
                        memory_percent: cstats.memory_percent(),
                    };

                    let event = match serde_json::to_string(&sample) {
                        Ok(data) => Event::default().event("stats").data(data),
                        Err(err) => {
                            debug!(
                                container_id = %state.container_id,
                                error = %err,
                                "failed to serialize stats sample; closing stream"
                            );
                            state.terminated = true;
                            Event::default()
                                .event("close")
                                .data(r#"{"reason":"serialize_error"}"#)
                        }
                    };

                    Some((Ok(event), state))
                }
                Err(err) => {
                    // Detect "container is gone" vs a transient failure so
                    // we can surface a specific reason. Either way we stop
                    // streaming — the container won't be producing stats
                    // again on this id.
                    let reason = match &err {
                        zlayer_agent::AgentError::NotFound { .. } => "exited",
                        _ => "runtime_error",
                    };
                    debug!(
                        container_id = %state.container_id,
                        error = %err,
                        reason,
                        "container stats follow terminating"
                    );
                    state.terminated = true;
                    let payload = format!(r#"{{"reason":"{reason}"}}"#);
                    let event = Event::default().event("close").data(payload);
                    Some((Ok(event), state))
                }
            }
        },
    )
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
        let id = ContainerId {
            service: "test-svc".to_string(),
            replica: 0,
        };
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
        // Empty query -> one-shot mode, default 2s cadence once clamped.
        let query: StatsQuery = serde_json::from_str("{}").unwrap();
        assert!(!query.stream);
        assert!(query.interval.is_none());
        assert_eq!(query.clamped_interval(), Duration::from_secs(2));
    }

    #[test]
    fn test_stats_query_parses_stream_and_interval() {
        // `stream=true` + explicit `interval`.
        let query: StatsQuery =
            serde_json::from_str(r#"{"stream": true, "interval": 5}"#).expect("parse stats query");
        assert!(query.stream);
        assert_eq!(query.interval, Some(5));
        assert_eq!(query.clamped_interval(), Duration::from_secs(5));

        // Legacy `interval_seconds` alias is honored by serde.
        let alias: StatsQuery = serde_json::from_str(r#"{"stream": true, "interval_seconds": 7}"#)
            .expect("parse stats query alias");
        assert!(alias.stream);
        assert_eq!(alias.interval, Some(7));
        assert_eq!(alias.clamped_interval(), Duration::from_secs(7));
    }

    #[test]
    fn test_stats_query_clamps_interval() {
        // Below the floor clamps up to 1s.
        let low = StatsQuery {
            stream: true,
            interval: Some(0),
        };
        assert_eq!(low.clamped_interval(), Duration::from_secs(1));

        // Above the ceiling clamps down to 60s.
        let high = StatsQuery {
            stream: true,
            interval: Some(9_999),
        };
        assert_eq!(high.clamped_interval(), Duration::from_secs(60));
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
            name: None,
            pull_policy: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: Vec::new(),
            ports: ports.clone(),
            work_dir: None,
            health_check: None,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: None,
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
        };
        let spec = build_service_spec(&request).expect("spec should build");
        assert_eq!(spec.port_mappings, ports);
    }

    #[test]
    fn test_build_service_spec_minimal() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            name: None,
            pull_policy: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: Vec::new(),
            ports: Vec::new(),
            work_dir: None,
            health_check: None,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: None,
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
        };
        let spec = build_service_spec(&request).expect("minimal spec should build");
        assert_eq!(spec.image.name, "alpine:latest");
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
            pull_policy: None,
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
            ports: Vec::new(),
            work_dir: Some("/app".to_string()),
            health_check: None,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: None,
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
        };
        let spec = build_service_spec(&request).expect("full spec should build");
        assert_eq!(spec.image.name, "node:20");
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
        let spec = req.to_health_spec().expect("valid tcp spec");
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
        let spec = req.to_health_spec().expect("valid http spec");
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
        let spec = req.to_health_spec().expect("valid command spec");
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
        let err = req.to_health_spec().expect_err("unknown type must error");
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
        assert!(req.to_health_spec().is_err());
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
        assert!(req.to_health_spec().is_err());
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
        assert!(req.to_health_spec().is_err());
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
        let err = req
            .to_health_spec()
            .expect_err("invalid humantime must error");
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

    // -----------------------------------------------------------------------
    // Log-follow stream: termination-on-exit test
    // -----------------------------------------------------------------------

    /// State backing `MockStateRuntime`.
    struct MockState {
        logs: Vec<zlayer_observability::logs::LogEntry>,
        state: ContainerState,
        state_calls: u32,
    }

    /// Minimal runtime used exclusively by
    /// `container_log_follow_stream_terminates_on_exit`. Only `container_state`
    /// and `container_logs` are ever called; every other method traps.
    struct MockStateRuntime {
        inner: std::sync::Arc<std::sync::Mutex<MockState>>,
    }

    #[async_trait::async_trait]
    impl zlayer_agent::runtime::Runtime for MockStateRuntime {
        async fn pull_image(&self, _image: &str) -> zlayer_agent::error::Result<()> {
            unimplemented!()
        }

        async fn pull_image_with_policy(
            &self,
            _image: &str,
            _policy: zlayer_spec::PullPolicy,
            _auth: Option<&zlayer_spec::RegistryAuth>,
        ) -> zlayer_agent::error::Result<()> {
            unimplemented!()
        }

        async fn create_container(
            &self,
            _id: &ContainerId,
            _spec: &zlayer_spec::ServiceSpec,
        ) -> zlayer_agent::error::Result<()> {
            unimplemented!()
        }

        async fn start_container(&self, _id: &ContainerId) -> zlayer_agent::error::Result<()> {
            unimplemented!()
        }

        async fn stop_container(
            &self,
            _id: &ContainerId,
            _timeout: Duration,
        ) -> zlayer_agent::error::Result<()> {
            unimplemented!()
        }

        async fn remove_container(&self, _id: &ContainerId) -> zlayer_agent::error::Result<()> {
            unimplemented!()
        }

        async fn container_state(
            &self,
            _id: &ContainerId,
        ) -> zlayer_agent::error::Result<ContainerState> {
            let mut guard = self.inner.lock().expect("mock state mutex poisoned");
            guard.state_calls += 1;
            // First 5 calls: Running. Sixth call: append a post-exit log line
            // and flip to Exited. Subsequent calls (if any) see the already-
            // flipped Exited state.
            if guard.state_calls == 6 {
                guard.logs.push(zlayer_observability::logs::LogEntry {
                    timestamp: chrono::Utc::now(),
                    stream: zlayer_observability::logs::LogStream::Stdout,
                    message: "post-exit line".to_string(),
                    source: zlayer_observability::logs::LogSource::Container("mock".to_string()),
                    service: None,
                    deployment: None,
                });
                guard.state = ContainerState::Exited { code: 0 };
            }
            Ok(guard.state.clone())
        }

        async fn container_logs(
            &self,
            _id: &ContainerId,
            tail: usize,
        ) -> zlayer_agent::error::Result<Vec<zlayer_observability::logs::LogEntry>> {
            let guard = self.inner.lock().expect("mock state mutex poisoned");
            let skip = guard.logs.len().saturating_sub(tail);
            Ok(guard.logs.iter().skip(skip).cloned().collect())
        }

        async fn exec(
            &self,
            _id: &ContainerId,
            _cmd: &[String],
        ) -> zlayer_agent::error::Result<(i32, String, String)> {
            unimplemented!()
        }

        async fn get_container_stats(
            &self,
            _id: &ContainerId,
        ) -> zlayer_agent::error::Result<zlayer_agent::cgroups_stats::ContainerStats> {
            unimplemented!()
        }

        async fn wait_container(&self, _id: &ContainerId) -> zlayer_agent::error::Result<i32> {
            unimplemented!()
        }

        async fn get_logs(
            &self,
            _id: &ContainerId,
        ) -> zlayer_agent::error::Result<Vec<zlayer_observability::logs::LogEntry>> {
            unimplemented!()
        }

        async fn get_container_pid(
            &self,
            _id: &ContainerId,
        ) -> zlayer_agent::error::Result<Option<u32>> {
            unimplemented!()
        }

        async fn get_container_ip(
            &self,
            _id: &ContainerId,
        ) -> zlayer_agent::error::Result<Option<std::net::IpAddr>> {
            unimplemented!()
        }
    }

    fn make_log_entry(message: &str) -> zlayer_observability::logs::LogEntry {
        zlayer_observability::logs::LogEntry {
            timestamp: chrono::Utc::now(),
            stream: zlayer_observability::logs::LogStream::Stdout,
            message: message.to_string(),
            source: zlayer_observability::logs::LogSource::Container("mock".to_string()),
            service: None,
            deployment: None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn container_log_follow_stream_terminates_on_exit() {
        use futures_util::StreamExt;

        let mock_state = std::sync::Arc::new(std::sync::Mutex::new(MockState {
            logs: vec![
                make_log_entry("line 1"),
                make_log_entry("line 2"),
                make_log_entry("line 3"),
            ],
            state: ContainerState::Running,
            state_calls: 0,
        }));

        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockStateRuntime {
            inner: mock_state.clone(),
        });

        let container_id = ContainerId {
            service: "test".to_string(),
            replica: 0,
        };

        let stream = container_log_follow_stream(runtime, container_id, 100);

        // With `start_paused = true`, tokio auto-advances time when the
        // runtime is otherwise idle, so the 500ms poll sleeps collapse. Wrap
        // in a wall-clock timeout so a regression that fails to terminate
        // surfaces as a test failure instead of a hang.
        let collected = tokio::time::timeout(Duration::from_secs(30), stream.collect::<Vec<_>>())
            .await
            .expect("stream should terminate within timeout");

        // 3 initial lines + 1 post-exit line = at least 4 events.
        assert!(
            collected.len() >= 4,
            "expected at least 4 events, got {}",
            collected.len()
        );

        // Every element is Ok since `Event`'s error type is Infallible.
        let payloads: Vec<String> = collected
            .into_iter()
            .map(|r| r.expect("Infallible"))
            .map(|e| format!("{e:?}"))
            .collect();
        let joined = payloads.join("\n");
        assert!(joined.contains("line 1"), "missing 'line 1' in {joined}");
        assert!(joined.contains("line 2"), "missing 'line 2' in {joined}");
        assert!(joined.contains("line 3"), "missing 'line 3' in {joined}");
        assert!(
            joined.contains("post-exit line"),
            "missing 'post-exit line' in {joined}"
        );
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
            name: None,
            pull_policy: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: Vec::new(),
            ports: Vec::new(),
            work_dir: None,
            health_check: None,
            hostname: Some("my-host".to_string()),
            dns: vec!["8.8.8.8".to_string()],
            extra_hosts: vec!["host.docker.internal:host-gateway".to_string()],
            restart_policy: None,
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
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
            name: None,
            pull_policy: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: vec![VolumeMount {
                mount_type: None,
                source: Some("/srv/data".to_string()),
                target: "/data".to_string(),
                readonly: true,
            }],
            ports: Vec::new(),
            work_dir: None,
            health_check: None,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: None,
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
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
            name: None,
            pull_policy: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: vec![VolumeMount {
                mount_type: Some(VolumeMountType::Bind),
                source: Some("relative/path".to_string()),
                target: "/data".to_string(),
                readonly: false,
            }],
            ports: Vec::new(),
            work_dir: None,
            health_check: None,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: None,
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
        };
        assert!(build_service_spec(&request).is_err());
    }

    #[test]
    fn volume_mount_volume_translates_to_storage_named() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            name: None,
            pull_policy: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: vec![VolumeMount {
                mount_type: Some(VolumeMountType::Volume),
                source: Some("pg-data".to_string()),
                target: "/var/lib/postgresql".to_string(),
                readonly: false,
            }],
            ports: Vec::new(),
            work_dir: None,
            health_check: None,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: None,
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
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
            name: None,
            pull_policy: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: vec![VolumeMount {
                mount_type: Some(VolumeMountType::Volume),
                source: Some("Bad Name!".to_string()),
                target: "/data".to_string(),
                readonly: false,
            }],
            ports: Vec::new(),
            work_dir: None,
            health_check: None,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: None,
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
        };
        assert!(build_service_spec(&request).is_err());
    }

    #[test]
    fn volume_mount_tmpfs_translates_to_storage_tmpfs() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            name: None,
            pull_policy: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: vec![VolumeMount {
                mount_type: Some(VolumeMountType::Tmpfs),
                source: None,
                target: "/tmp-mem".to_string(),
                readonly: false,
            }],
            ports: Vec::new(),
            work_dir: None,
            health_check: None,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: None,
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
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
            name: None,
            pull_policy: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: vec![VolumeMount {
                mount_type: Some(VolumeMountType::Tmpfs),
                source: Some("/something".to_string()),
                target: "/tmp-mem".to_string(),
                readonly: false,
            }],
            ports: Vec::new(),
            work_dir: None,
            health_check: None,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: None,
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
        };
        assert!(build_service_spec(&request).is_err());
    }

    #[test]
    fn build_service_spec_rejects_bad_dns() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            name: None,
            pull_policy: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: Vec::new(),
            ports: Vec::new(),
            work_dir: None,
            health_check: None,
            hostname: None,
            dns: vec!["not-an-ip".to_string()],
            extra_hosts: Vec::new(),
            restart_policy: None,
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
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
            name: None,
            pull_policy: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: Vec::new(),
            ports: Vec::new(),
            work_dir: None,
            health_check: None,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: Some(policy.clone()),
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
        };
        let spec = build_service_spec(&request).expect("valid restart policy");
        assert_eq!(spec.restart_policy, Some(policy));
    }

    #[test]
    fn build_service_spec_rejects_bad_restart_delay() {
        let request = CreateContainerRequest {
            image: "alpine:latest".to_string(),
            name: None,
            pull_policy: None,
            env: HashMap::new(),
            command: None,
            labels: HashMap::new(),
            resources: None,
            volumes: Vec::new(),
            ports: Vec::new(),
            work_dir: None,
            health_check: None,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: Some(zlayer_spec::ContainerRestartPolicy {
                kind: zlayer_spec::ContainerRestartKind::Always,
                max_attempts: None,
                delay: Some("not-a-duration".to_string()),
            }),
            registry_credential_id: None,
            registry_auth: None,
            networks: Vec::new(),
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
        let id = ContainerId {
            service: "svc".to_string(),
            replica: 3,
        };
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
        let cid = ContainerId {
            service: "svc".to_string(),
            replica: 0,
        };

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
}
