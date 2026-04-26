//! Raw container lifecycle API DTOs.
//!
//! Wire-format types shared between the daemon's `/api/v1/containers`
//! endpoints and SDK clients. Moved out of `zlayer-api` so SDK crates can
//! depend on them without pulling in the full server stack.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
#[cfg(feature = "utoipa")]
use utoipa::{IntoParams, ToSchema};

/// Resource limits for a container
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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

/// Request to create and start a container
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
    pub ports: Vec<crate::spec::PortMapping>,
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
    pub restart_policy: Option<crate::spec::ContainerRestartPolicy>,
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
    pub registry_auth: Option<crate::spec::RegistryAuth>,
}

/// A request to attach a freshly-created container to a user-defined bridge
/// or overlay network, mirroring the wire-shape used by `POST
/// /api/v1/container-networks/{id_or_name}/connect`.
///
/// Included on [`CreateContainerRequest::networks`] so callers can wire up
/// every attachment in a single call instead of issuing a separate connect
/// request per network after container create.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
    pub ports: Vec<crate::spec::PortMapping>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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

/// Query parameters for listing containers
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(IntoParams))]
pub struct ListContainersQuery {
    /// Filter by label (key=value format)
    #[serde(default)]
    pub label: Option<String>,
}

/// Query parameters for container logs
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(IntoParams))]
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
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(IntoParams))]
pub struct ExecQuery {
    /// Stream exec events as SSE instead of returning a buffered JSON body.
    #[serde(default)]
    pub stream: bool,
}

/// Exec response with command output
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct StopContainerRequest {
    /// Graceful shutdown timeout in seconds before the runtime force-kills
    /// the container. Defaults to 30 seconds when omitted.
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Request body for restarting a container. Matches the Docker-compat
/// `POST /containers/{id}/restart` shape.
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
pub struct RestartContainerRequest {
    /// Graceful shutdown timeout in seconds before the runtime force-kills
    /// the container. Defaults to 30 seconds when omitted.
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Request body for killing (sending a signal to) a container. Matches the
/// Docker-compat `POST /containers/{id}/kill` shape.
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(ToSchema))]
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
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(IntoParams))]
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
