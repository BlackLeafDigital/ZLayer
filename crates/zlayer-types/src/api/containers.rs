//! Raw container lifecycle API DTOs.
//!
//! Wire-format types shared between the daemon's `/api/v1/containers`
//! endpoints and SDK clients. Moved out of `zlayer-api` so SDK crates can
//! depend on them without pulling in the full server stack.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Inline serde shim for `Option<Duration>` ↔ humantime strings.
///
/// Mirrors the `duration::option` module in `spec/types.rs` so the request
/// types here can accept the same wire format (e.g. `"30s"`, `"500ms"`,
/// `"1m"`) as the spec's [`crate::spec::ServiceSpec::stop_grace_period`]
/// without taking on a `humantime_serde` dependency.
mod duration_opt {
    use humantime::format_duration;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    #[allow(clippy::ref_option)]
    pub fn serialize<S>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match duration {
            Some(d) => serializer.serialize_str(&format_duration(*d).to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;
        let s: Option<String> = Option::deserialize(deserializer)?;
        match s {
            Some(s) => humantime::parse_duration(&s)
                .map(Some)
                .map_err(|e| D::Error::custom(format!("invalid duration: {e}"))),
            None => Ok(None),
        }
    }
}

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

/// Request to create and start a container
#[derive(Debug, Default, Deserialize, Serialize, ToSchema)]
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

    // -- Docker lifecycle / security ----------------------------------------
    /// Run the container in privileged mode (Docker `--privileged`). When
    /// omitted, defaults to `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privileged: Option<bool>,
    /// Linux capabilities to add (Docker `--cap-add`). Maps to
    /// `ServiceSpec::capabilities`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cap_add: Vec<String>,
    /// Linux capabilities to drop (Docker `--cap-drop`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cap_drop: Vec<String>,
    /// Host devices to expose to the container (Docker `--device`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<crate::spec::DeviceSpec>,
    /// Network mode (Docker `--network`). Accepts `"default"`, `"host"`,
    /// `"none"`, `"bridge"`, `"bridge:<name>"`, or `"container:<id>"`. When
    /// omitted, defaults to [`crate::spec::NetworkMode::Default`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_mode: Option<crate::spec::NetworkMode>,
    /// Security options such as `apparmor=...`, `seccomp=...`,
    /// `no-new-privileges:true` (Docker `--security-opt`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security_opt: Vec<String>,
    /// PID namespace mode (Docker `--pid`). Accepts e.g. `"host"` or
    /// `"container:<id>"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_mode: Option<String>,
    /// IPC namespace mode (Docker `--ipc`). Accepts e.g. `"host"`,
    /// `"shareable"`, `"private"`, or `"container:<id>"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipc_mode: Option<String>,
    /// Mount the container's root filesystem read-only (Docker `--read-only`).
    #[serde(default)]
    pub read_only_root_fs: bool,
    /// Run a Docker-supplied init process (PID 1) inside the container
    /// (Docker `--init`). Distinct from `ZLayer`'s pre-start init actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_container: Option<bool>,

    // -- Docker metadata ----------------------------------------------------
    /// User and group override for the container's main process
    /// (Docker `--user uid:gid`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Signal sent to the container's main process to request a graceful
    /// shutdown (Docker `--stop-signal`). Accepts e.g. `"SIGTERM"` or `"15"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_signal: Option<String>,
    /// Grace period to wait between the stop signal and a forced kill
    /// (Docker `--stop-timeout`). Wire format is a humantime string
    /// (e.g. `"30s"`, `"500ms"`, `"1m"`).
    #[serde(
        default,
        with = "duration_opt",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<String>, example = "30s")]
    pub stop_grace_period: Option<std::time::Duration>,
    /// Kernel sysctl overrides (Docker `--sysctl`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sysctls: HashMap<String, String>,
    /// Per-process ulimits (Docker `--ulimit`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub ulimits: HashMap<String, crate::spec::UlimitSpec>,
    /// Additional groups to add to the container process
    /// (Docker `--group-add`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_groups: Vec<String>,

    // -- Docker resource knobs (folded into `ServiceSpec::resources`) -------
    /// Maximum number of processes the container may spawn
    /// (Docker `--pids-limit`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids_limit: Option<i64>,
    /// CPUs that the container is allowed to execute on
    /// (Docker `--cpuset-cpus`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpuset: Option<String>,
    /// Relative CPU shares (Docker `--cpu-shares`). Default weight is 1024.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_shares: Option<u32>,
    /// Total memory limit including swap (Docker `--memory-swap`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_swap: Option<String>,
    /// Soft memory limit (Docker `--memory-reservation`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_reservation: Option<String>,
    /// Container memory swappiness, 0-100 (Docker `--memory-swappiness`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_swappiness: Option<u8>,
    /// OOM-killer score adjustment (Docker `--oom-score-adj`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oom_score_adj: Option<i32>,
    /// Disable the OOM killer for the container (Docker `--oom-kill-disable`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oom_kill_disable: Option<bool>,
    /// Block IO weight, 10-1000 (Docker `--blkio-weight`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blkio_weight: Option<u16>,

    // -- Lifecycle ----------------------------------------------------------
    /// Container lifecycle policy. Carries the `delete_on_exit` knob (Docker
    /// `--rm` / `HostConfig.AutoRemove`) so the daemon can remove terminated
    /// container records and bundles once they exit. Defaults to
    /// [`crate::spec::LifecycleSpec::default()`] (i.e. retain on exit), which
    /// matches the historical behavior for callers that omit the field.
    #[serde(default)]
    pub lifecycle: crate::spec::LifecycleSpec,

    // -- Placement ----------------------------------------------------------
    /// Node selection constraints (required / preferred labels). When set on a
    /// daemon that has a cluster handle, the leader places the container on a
    /// node whose labels satisfy the required set; otherwise the field is
    /// ignored and the container is created locally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_selector: Option<crate::spec::NodeSelector>,
    /// Target platform (OS + arch) the container must run on, e.g.
    /// `darwin/arm64`. When set on a clustered daemon, the leader places the
    /// container on a node whose reported platform matches; when no node
    /// matches, the request is rejected. Ignored on single-node daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<crate::spec::TargetPlatform>,

    // -- Lifecycle: start-on-create -----------------------------------------
    /// Whether the daemon should start the container immediately after
    /// creating it.
    ///
    /// `None` (the default, and what `..Default::default()` / an omitted wire
    /// field both produce) and `Some(true)` both mean "create and start",
    /// preserving the historical `zlayer run`-style one-shot behaviour for the
    /// native REST API and every existing caller.
    ///
    /// The Docker-compat shim sets this to `Some(false)`: Docker's
    /// `POST /containers/create` is create-only and the client is expected to
    /// follow up with an explicit `POST /containers/{id}/start`. Without
    /// honouring this, a Docker `create` would auto-start the container,
    /// leaving it in `running` state when the Docker contract requires
    /// `created`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<bool>,
}

impl CreateContainerRequest {
    /// Whether the daemon should start the container right after creating it.
    ///
    /// Returns `true` unless the request explicitly set `start: false`, so the
    /// native one-shot behaviour is the default and only the Docker-compat
    /// create-only path opts out.
    #[must_use]
    pub fn should_start_on_create(&self) -> bool {
        self.start != Some(false)
    }
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

/// Query parameters for listing containers
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListContainersQuery {
    /// Filter by label (key=value format)
    #[serde(default)]
    pub label: Option<String>,
}

/// Query parameters for container logs.
///
/// Mirrors the Docker Engine API `GET /containers/{id}/logs` query string so
/// the streaming handler can pass options through to
/// [`zlayer_agent::runtime::Runtime::logs_stream`] with minimal translation.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct ContainerLogQuery {
    /// Number of tail lines to return. `0` and "all" map to "everything
    /// available"; otherwise the runtime ships the last `tail` lines before
    /// the live stream begins.
    #[serde(default = "default_tail")]
    pub tail: usize,
    /// Follow logs after the current end-of-buffer marker.
    #[serde(default)]
    pub follow: bool,
    /// Earliest log timestamp to include (Unix seconds). `None` means no
    /// lower bound.
    #[serde(default)]
    pub since: Option<i64>,
    /// Latest log timestamp to include (Unix seconds). `None` means no upper
    /// bound.
    #[serde(default)]
    pub until: Option<i64>,
    /// When `true`, the runtime is asked to populate per-chunk timestamps so
    /// the wire-format includes them.
    #[serde(default)]
    pub timestamps: bool,
    /// Include stdout chunks. When neither `stdout` nor `stderr` is set, the
    /// handler defaults both to `true` (Docker parity).
    #[serde(default)]
    pub stdout: Option<bool>,
    /// Include stderr chunks. See [`ContainerLogQuery::stdout`] for the
    /// "neither set" default behavior.
    #[serde(default)]
    pub stderr: Option<bool>,
    /// Wire format for the streamed body. `"json"` (the default) emits one
    /// NDJSON `LogChunk` per line; `"raw"` emits Docker's multiplexed stdcopy
    /// frames (`application/vnd.docker.raw-stream`).
    #[serde(default)]
    pub format: Option<ContainerLogFormat>,
}

/// Wire format for [`ContainerLogQuery::format`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ContainerLogFormat {
    /// Newline-delimited JSON, one `LogChunk` per line. The default.
    #[default]
    Json,
    /// Docker multiplexed stdcopy framing.
    Raw,
}

fn default_tail() -> usize {
    100
}

/// Exec request for running a command in a container
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ContainerExecRequest {
    /// Command and arguments to execute
    pub command: Vec<String>,
    /// Optional `user[:group]` to run the command as (Docker `--user`). A NAME
    /// (e.g. `git`) is resolved against the container's `/etc/passwd` by the
    /// runtime; numeric `uid` / `uid:gid` are used directly. `None` keeps the
    /// container's configured user (root by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Optional working directory inside the container (Docker `-w`/`--workdir`).
    /// `None` keeps the container's default workdir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// Extra environment variables in `KEY=VALUE` form (Docker `-e`/`--env`),
    /// merged on top of the container's env (later entries win).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
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

/// Restart policy entry for [`ContainerUpdateRequest`].
///
/// Mirrors Docker's `HostConfig.RestartPolicy` shape so the Docker compat
/// layer can pass the wire payload through unchanged. `name` accepts the
/// same set of strings as `docker run --restart`: `""`, `"no"`, `"always"`,
/// `"unless-stopped"`, or `"on-failure"`. `maximum_retry_count` is only
/// honoured when `name == "on-failure"`.
#[derive(Debug, Default, Clone, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
pub struct ContainerUpdateRestartPolicy {
    /// `"no"`, `"always"`, `"unless-stopped"`, or `"on-failure"`.
    #[serde(rename = "Name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Maximum number of retries before giving up (only used with
    /// `on-failure`). When `0` or omitted, retries are unbounded.
    #[serde(
        rename = "MaximumRetryCount",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub maximum_retry_count: Option<i64>,
}

/// Request body for `POST /api/v1/containers/{id}/update`.
///
/// Mirrors Docker Engine's `POST /containers/{id}/update` body 1:1 so the
/// `zlayer-docker` compatibility shim can pass the wire payload straight
/// through. Every field is optional — only the fields present on the wire
/// are applied; unset fields are left untouched on the running container.
///
/// Field naming uses Docker's `PascalCase` on the wire (`CpuShares`,
/// `Memory`, ...) and `snake_case` on the Rust side. Subset of the full
/// Docker schema: `ZLayer` supports the resource knobs (cpu, memory, pids,
/// blkio) plus `RestartPolicy`. Windows-only fields (`CpuCount`,
/// `IOMaximumIOps`) and ulimits/devices are accepted on the wire but
/// silently ignored by the Linux runtimes.
#[derive(Debug, Default, Clone, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
pub struct ContainerUpdateRequest {
    /// Relative CPU weight (cgroup `cpu.weight` or `cpu.shares`). Range
    /// 2-262144 on cgroup v2; 2-262144 mapped from 1-10000 on v1.
    #[serde(rename = "CpuShares", default, skip_serializing_if = "Option::is_none")]
    pub cpu_shares: Option<i64>,

    /// Memory limit in bytes. Set `0` to remove the limit.
    #[serde(rename = "Memory", default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<i64>,

    /// CPU CFS period in microseconds.
    #[serde(rename = "CpuPeriod", default, skip_serializing_if = "Option::is_none")]
    pub cpu_period: Option<i64>,

    /// CPU CFS quota in microseconds. Together with `cpu_period` defines
    /// the fraction of a CPU the container may use.
    #[serde(rename = "CpuQuota", default, skip_serializing_if = "Option::is_none")]
    pub cpu_quota: Option<i64>,

    /// CPU real-time period in microseconds.
    #[serde(
        rename = "CpuRealtimePeriod",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cpu_realtime_period: Option<i64>,

    /// CPU real-time runtime in microseconds.
    #[serde(
        rename = "CpuRealtimeRuntime",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cpu_realtime_runtime: Option<i64>,

    /// CPUs allowed for execution (e.g. `"0-3"`, `"0,1"`).
    #[serde(
        rename = "CpusetCpus",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cpuset_cpus: Option<String>,

    /// Memory nodes (NUMA) allowed for execution (e.g. `"0-3"`).
    #[serde(
        rename = "CpusetMems",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cpuset_mems: Option<String>,

    /// Soft memory limit in bytes. The kernel reclaims pages above this
    /// reservation when the host comes under memory pressure.
    #[serde(
        rename = "MemoryReservation",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub memory_reservation: Option<i64>,

    /// Total memory limit (memory + swap) in bytes. `-1` removes the swap
    /// limit, matching Docker semantics.
    #[serde(
        rename = "MemorySwap",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub memory_swap: Option<i64>,

    /// Kernel memory limit in bytes (deprecated upstream; accepted for
    /// wire compatibility).
    #[serde(
        rename = "KernelMemory",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub kernel_memory: Option<i64>,

    /// Block IO weight (relative weight, range 10-1000).
    #[serde(
        rename = "BlkioWeight",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub blkio_weight: Option<u16>,

    /// PIDs limit. Set `0` or `-1` for unlimited.
    #[serde(rename = "PidsLimit", default, skip_serializing_if = "Option::is_none")]
    pub pids_limit: Option<i64>,

    /// New restart policy. When present, replaces the container's stored
    /// restart policy. Docker applies this asynchronously: the next time
    /// the supervisor decides whether to restart, it consults the new
    /// policy.
    #[serde(
        rename = "RestartPolicy",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub restart_policy: Option<ContainerUpdateRestartPolicy>,
}

/// Response body for `POST /api/v1/containers/{id}/update`.
///
/// Mirrors Docker's `{"Warnings": [...]}` shape so the compat layer
/// passes the body through verbatim. `Warnings` is always present (even
/// if empty) for wire compatibility with clients that match the field
/// presence, not just its contents.
#[derive(Debug, Default, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct ContainerUpdateResponse {
    /// Human-readable warnings emitted by the runtime while applying the
    /// update — e.g. `"kernel memory limit is deprecated"`.
    #[serde(rename = "Warnings", default)]
    pub warnings: Vec<String>,
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

/// Docker-shaped wait response returned by
/// `POST /api/v1/containers/{id}/wait`.
///
/// Mirrors Docker Engine's `/containers/{id}/wait` body 1:1: a
/// `StatusCode` field plus an optional `Error` envelope. Used by the
/// `zlayer-docker` compatibility shim and any SDK callers that consume
/// the Docker shape directly. The richer
/// [`ContainerWaitResponse`] (returned by the legacy `GET` endpoint) is
/// preserved for clients that need the `reason` / `signal` / `finished_at`
/// classification fields.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerWaitDockerResponse {
    /// Container exit code (0 = success). When killed by signal `N`,
    /// this is typically `128 + N`, matching Docker's convention.
    #[serde(rename = "StatusCode")]
    pub status_code: i64,
    /// Optional error envelope surfaced when the wait itself failed
    /// (e.g. the container was removed before reaching `not-running`
    /// when `condition=not-running` was requested). Absent on a normal
    /// exit.
    #[serde(rename = "Error", default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ContainerWaitDockerError>,
}

/// Error envelope nested inside [`ContainerWaitDockerResponse`].
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerWaitDockerError {
    /// Human-readable description of why the wait failed.
    #[serde(rename = "Message")]
    pub message: String,
}

/// Query parameters for `POST /api/v1/containers/{id}/wait` —
/// Docker's `condition=` query string.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct WaitContainerQuery {
    /// One of `"not-running"` (default), `"next-exit"`, or `"removed"`.
    /// Matches Docker's `/containers/{id}/wait` semantics. Omitted
    /// values default to `"not-running"`.
    #[serde(default)]
    pub condition: Option<String>,
}

/// Query parameters for `POST /api/v1/containers/{id}/rename` —
/// Docker's `name=<new-name>` query string.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct RenameContainerQuery {
    /// New human-readable name to assign to the container. Required.
    #[serde(default)]
    pub name: Option<String>,
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

/// Query parameters for `GET /api/v1/containers/{id}/top` —
/// Docker's `ps_args=<...>` query string. Defaults to the runtime's
/// own column set when omitted or empty.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct ContainerTopQuery {
    /// `ps`-style argument string, e.g. `"aux"` or `"-eo pid,user,cmd"`.
    /// Empty / omitted means "use the runtime's defaults".
    #[serde(default)]
    pub ps_args: Option<String>,
}

/// Response body for `GET /api/v1/containers/{id}/top` (Docker compat shape).
///
/// Wire field names use Docker's `Titles` / `Processes` casing so the
/// shim can pass the body through untouched.
#[derive(Debug, Default, Serialize, Deserialize, ToSchema)]
pub struct ContainerTopResponse {
    /// `ps` column titles — e.g. `["UID", "PID", "PPID", "C", "STIME",
    /// "TTY", "TIME", "CMD"]`.
    #[serde(rename = "Titles")]
    pub titles: Vec<String>,
    /// One row per process inside the container. Each row has the same
    /// length as `titles`.
    #[serde(rename = "Processes")]
    pub processes: Vec<Vec<String>>,
}

/// One row of `GET /api/v1/containers/{id}/changes` (Docker compat shape).
///
/// Mirrors Docker's `{"Path": "/foo", "Kind": 0}` body:
/// `Kind` is a numeric enum where `0 = Modified`, `1 = Added`, `2 = Deleted`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerChangeEntry {
    /// Path inside the container that changed (absolute, e.g. `/etc/hosts`).
    #[serde(rename = "Path")]
    pub path: String,
    /// `0` = Modified, `1` = Added, `2` = Deleted (Docker's wire integer).
    #[serde(rename = "Kind")]
    pub kind: u8,
}

/// Response body for `GET /api/v1/containers/{id}/port` (Docker compat shape).
///
/// Mirrors Docker's `{"Ports": {"80/tcp": [{"HostIp":"...","HostPort":"..."}]}}`
/// body. Each key is `<container_port>/<protocol>` and the value is the list
/// of host bindings for that port (or `null` when the port is exposed but not
/// published).
#[derive(Debug, Default, Serialize, Deserialize, ToSchema)]
pub struct ContainerPortResponse {
    /// Map of `"<port>/<protocol>"` to host bindings.
    #[serde(rename = "Ports")]
    pub ports: HashMap<String, Option<Vec<ContainerPortBinding>>>,
}

/// One host binding inside a [`ContainerPortResponse`] entry.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContainerPortBinding {
    /// Host IP that maps to the container port. Empty / `"0.0.0.0"` means
    /// "any IPv4 address".
    #[serde(rename = "HostIp", default, skip_serializing_if = "Option::is_none")]
    pub host_ip: Option<String>,
    /// Host port (always serialised as a string in Docker's wire format).
    #[serde(rename = "HostPort", default, skip_serializing_if = "Option::is_none")]
    pub host_port: Option<String>,
}

/// Response body for `POST /api/v1/containers/prune` (Docker compat shape).
///
/// Docker uses `ContainersDeleted` / `SpaceReclaimed` `PascalCase` fields, so
/// SDK consumers (and the docker shim) can read the body verbatim.
#[derive(Debug, Default, Serialize, Deserialize, ToSchema)]
pub struct ContainerPruneResponse {
    /// Container IDs that were removed.
    #[serde(rename = "ContainersDeleted")]
    pub containers_deleted: Vec<String>,
    /// Bytes reclaimed from the runtime's container storage.
    #[serde(rename = "SpaceReclaimed")]
    pub space_reclaimed: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{DeviceSpec, NetworkMode, UlimitSpec};
    use std::time::Duration;

    /// Build a baseline request with only the required `image` field so each
    /// round-trip test can override exactly the slice of fields it cares
    /// about without listing the dozens of unrelated optional fields.
    fn baseline_request() -> CreateContainerRequest {
        CreateContainerRequest {
            image: "nginx:latest".to_string(),
            ..CreateContainerRequest::default()
        }
    }

    #[test]
    fn create_request_round_trips_placement_fields() {
        use crate::spec::{ArchKind, NodeSelector, OsKind, TargetPlatform};

        let mut req = baseline_request();
        req.platform = Some(TargetPlatform::new(OsKind::Macos, ArchKind::Arm64));
        req.node_selector = Some(NodeSelector {
            labels: [("zone".to_string(), "us-east".to_string())]
                .into_iter()
                .collect(),
            prefer_labels: std::collections::HashMap::new(),
        });

        let json = serde_json::to_string(&req).expect("serialize");
        let back: CreateContainerRequest =
            serde_json::from_str(&json).expect("deserialize round-trip");

        let platform = back.platform.expect("platform present");
        assert_eq!(platform.os, OsKind::Macos);
        assert_eq!(platform.arch, ArchKind::Arm64);
        let selector = back.node_selector.expect("node_selector present");
        assert_eq!(
            selector.labels.get("zone").map(String::as_str),
            Some("us-east")
        );

        // Omitted placement fields must round-trip as None (skip_serializing_if).
        let bare: CreateContainerRequest =
            serde_json::from_str(r#"{"image":"nginx:latest"}"#).expect("deserialize bare");
        assert!(bare.platform.is_none());
        assert!(bare.node_selector.is_none());
    }

    #[test]
    fn start_on_create_defaults_to_true_and_honours_explicit_false() {
        // Omitted `start` (the common case for native callers and the SDK)
        // must mean "create and start" — anything else regresses
        // `zlayer run`-style one-shots.
        let bare: CreateContainerRequest =
            serde_json::from_str(r#"{"image":"nginx:latest"}"#).expect("deserialize bare");
        assert_eq!(bare.start, None);
        assert!(
            bare.should_start_on_create(),
            "omitted start must default to start-on-create"
        );

        // `..Default::default()` (used by the CLI / compose run paths and by
        // every in-process struct construction) must also start.
        let dflt = CreateContainerRequest {
            image: "nginx:latest".to_string(),
            ..CreateContainerRequest::default()
        };
        assert!(
            dflt.should_start_on_create(),
            "Default::default() must default to start-on-create"
        );

        // Explicit `start: true` starts.
        let yes: CreateContainerRequest =
            serde_json::from_str(r#"{"image":"nginx:latest","start":true}"#)
                .expect("deserialize start=true");
        assert!(yes.should_start_on_create());

        // Only an explicit `start: false` (the Docker-compat create-only path)
        // suppresses the auto-start.
        let no: CreateContainerRequest =
            serde_json::from_str(r#"{"image":"nginx:latest","start":false}"#)
                .expect("deserialize start=false");
        assert_eq!(no.start, Some(false));
        assert!(
            !no.should_start_on_create(),
            "explicit start=false must suppress the auto-start"
        );
    }

    #[test]
    fn buffered_exec_response_parses_native_wire_shape() {
        // The native buffered exec handler emits
        // `Json(ContainerExecResponse { exit_code, stdout, stderr })`. Lock the
        // exact wire shape so a future rename can't silently regress the
        // `DaemonClient::exec_in_container` parse path (which fails with
        // "missing field 'exit_code'" when pointed at the wrong endpoint).
        let resp = ContainerExecResponse {
            exit_code: 42,
            stdout: "hello".to_string(),
            stderr: "oops".to_string(),
        };
        let wire = serde_json::to_string(&resp).expect("serialize");
        assert_eq!(
            wire, r#"{"exit_code":42,"stdout":"hello","stderr":"oops"}"#,
            "buffered exec wire shape must stay snake_case exit_code/stdout/stderr"
        );
        let back: ContainerExecResponse = serde_json::from_str(&wire).expect("round-trip");
        assert_eq!(back.exit_code, 42);
        assert_eq!(back.stdout, "hello");
        assert_eq!(back.stderr, "oops");

        // Guard the bug we fixed: the *interactive* create-exec endpoint
        // returns `{"Id":"<64-hex>"}`, which must NOT parse as a buffered
        // exec result. (This is the `missing field 'exit_code' at line 1
        // column 73` failure observed when the buffered client pointed at the
        // create-exec route.)
        let create_exec_body =
            r#"{"Id":"abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"}"#;
        assert!(
            serde_json::from_str::<ContainerExecResponse>(create_exec_body).is_err(),
            "create-exec `{{Id}}` body must not deserialize as a buffered exec result"
        );
    }

    #[test]
    fn create_request_round_trips_security_fields() {
        let mut req = baseline_request();
        req.privileged = Some(true);
        req.cap_add = vec!["NET_ADMIN".to_string(), "SYS_PTRACE".to_string()];
        req.cap_drop = vec!["MKNOD".to_string()];
        req.devices = vec![DeviceSpec {
            path: "/dev/kvm".to_string(),
            read: true,
            write: true,
            mknod: false,
        }];
        req.network_mode = Some(NetworkMode::Host);
        req.security_opt = vec!["no-new-privileges:true".to_string()];
        req.pid_mode = Some("host".to_string());
        req.ipc_mode = Some("shareable".to_string());
        req.read_only_root_fs = true;
        req.init_container = Some(true);

        let json = serde_json::to_string(&req).expect("serialize");
        let back: CreateContainerRequest =
            serde_json::from_str(&json).expect("deserialize round-trip");

        assert_eq!(back.privileged, Some(true));
        assert_eq!(back.cap_add, vec!["NET_ADMIN", "SYS_PTRACE"]);
        assert_eq!(back.cap_drop, vec!["MKNOD"]);
        assert_eq!(back.devices.len(), 1);
        assert_eq!(back.devices[0].path, "/dev/kvm");
        assert!(back.devices[0].read);
        assert!(back.devices[0].write);
        assert!(!back.devices[0].mknod);
        assert_eq!(back.network_mode, Some(NetworkMode::Host));
        assert_eq!(back.security_opt, vec!["no-new-privileges:true"]);
        assert_eq!(back.pid_mode.as_deref(), Some("host"));
        assert_eq!(back.ipc_mode.as_deref(), Some("shareable"));
        assert!(back.read_only_root_fs);
        assert_eq!(back.init_container, Some(true));
    }

    #[test]
    fn create_request_round_trips_metadata_fields() {
        let mut req = baseline_request();
        req.labels.insert("env".to_string(), "prod".to_string());
        req.labels.insert("team".to_string(), "core".to_string());
        req.user = Some("1000:1000".to_string());
        req.stop_signal = Some("SIGTERM".to_string());
        req.stop_grace_period = Some(Duration::from_secs(45));
        req.sysctls
            .insert("net.core.somaxconn".to_string(), "1024".to_string());
        req.ulimits.insert(
            "nofile".to_string(),
            UlimitSpec {
                soft: 4096,
                hard: 8192,
            },
        );
        req.extra_groups = vec!["docker".to_string(), "audio".to_string()];

        let json = serde_json::to_string(&req).expect("serialize");
        // Confirm the humantime wire format is a string.
        assert!(
            json.contains("\"stop_grace_period\":\"45s\""),
            "expected humantime stop_grace_period in JSON, got: {json}"
        );

        let back: CreateContainerRequest =
            serde_json::from_str(&json).expect("deserialize round-trip");

        assert_eq!(back.labels.get("env").map(String::as_str), Some("prod"));
        assert_eq!(back.labels.get("team").map(String::as_str), Some("core"));
        assert_eq!(back.user.as_deref(), Some("1000:1000"));
        assert_eq!(back.stop_signal.as_deref(), Some("SIGTERM"));
        assert_eq!(back.stop_grace_period, Some(Duration::from_secs(45)));
        assert_eq!(
            back.sysctls.get("net.core.somaxconn").map(String::as_str),
            Some("1024")
        );
        let nofile = back.ulimits.get("nofile").expect("nofile ulimit present");
        assert_eq!(nofile.soft, 4096);
        assert_eq!(nofile.hard, 8192);
        assert_eq!(back.extra_groups, vec!["docker", "audio"]);
    }

    #[test]
    fn create_request_round_trips_resource_knobs() {
        let mut req = baseline_request();
        req.pids_limit = Some(2048);
        req.cpuset = Some("0-3".to_string());
        req.cpu_shares = Some(1024);
        req.memory_swap = Some("2Gi".to_string());
        req.memory_reservation = Some("256Mi".to_string());
        req.memory_swappiness = Some(10);
        req.oom_score_adj = Some(-500);
        req.oom_kill_disable = Some(false);
        req.blkio_weight = Some(500);

        let json = serde_json::to_string(&req).expect("serialize");
        let back: CreateContainerRequest =
            serde_json::from_str(&json).expect("deserialize round-trip");

        assert_eq!(back.pids_limit, Some(2048));
        assert_eq!(back.cpuset.as_deref(), Some("0-3"));
        assert_eq!(back.cpu_shares, Some(1024));
        assert_eq!(back.memory_swap.as_deref(), Some("2Gi"));
        assert_eq!(back.memory_reservation.as_deref(), Some("256Mi"));
        assert_eq!(back.memory_swappiness, Some(10));
        assert_eq!(back.oom_score_adj, Some(-500));
        assert_eq!(back.oom_kill_disable, Some(false));
        assert_eq!(back.blkio_weight, Some(500));
    }

    #[test]
    fn create_request_round_trips_network_mode_strings() {
        // The spec's `NetworkMode` deserialization happens via
        // `deserialize_network_mode`, but that helper is only attached to
        // `ServiceSpec.network_mode` — at the request layer we want the
        // derived `Deserialize` for `NetworkMode` (lowercase enum) to
        // accept the same wire shapes. Confirm each of the five Docker
        // forms round-trips through the request body.
        //
        // Note: at the request layer, `network_mode` accepts the
        // externally-tagged enum form (e.g. `{"bridge": {"name": "..."}}`),
        // matching what the derived `Serialize` for `NetworkMode` emits.
        let cases: &[(&str, NetworkMode)] = &[
            (r#""default""#, NetworkMode::Default),
            (r#""host""#, NetworkMode::Host),
            (r#""none""#, NetworkMode::None),
            (
                r#"{"bridge":{"name":null}}"#,
                NetworkMode::Bridge { name: None },
            ),
            (
                r#"{"bridge":{"name":"custom_net"}}"#,
                NetworkMode::Bridge {
                    name: Some("custom_net".to_string()),
                },
            ),
            (
                r#"{"container":{"id":"abc"}}"#,
                NetworkMode::Container {
                    id: "abc".to_string(),
                },
            ),
        ];

        for (literal, expected) in cases {
            let body = format!(r#"{{"image":"nginx:latest","network_mode":{literal}}}"#);
            let req: CreateContainerRequest = serde_json::from_str(&body)
                .unwrap_or_else(|e| panic!("deserialize {literal}: {e}"));
            assert_eq!(
                req.network_mode.as_ref(),
                Some(expected),
                "wire form {literal} did not round-trip",
            );

            // Re-serialize and parse again to confirm the emitted form
            // also round-trips back into the same variant.
            let reser = serde_json::to_string(&req).expect("re-serialize");
            let again: CreateContainerRequest =
                serde_json::from_str(&reser).expect("re-deserialize");
            assert_eq!(again.network_mode.as_ref(), Some(expected));
        }
    }

    /// `ContainerUpdateRequest` must accept Docker Engine's `PascalCase`
    /// wire shape verbatim (`CpuShares`, `Memory`, `RestartPolicy`, ...)
    /// and round-trip every documented field. This pins the contract
    /// `zlayer-docker` relies on when forwarding `POST /containers/{id}/update`.
    #[test]
    fn container_update_request_round_trips_docker_wire_shape() {
        let body = serde_json::json!({
            "CpuShares": 512,
            "Memory": 314_572_800_i64,
            "CpuPeriod": 100_000,
            "CpuQuota": 50_000,
            "CpuRealtimePeriod": 1_000_000,
            "CpuRealtimeRuntime": 950_000,
            "CpusetCpus": "0-3",
            "CpusetMems": "0,1",
            "MemoryReservation": 268_435_456_i64,
            "MemorySwap": 629_145_600_i64,
            "KernelMemory": 67_108_864_i64,
            "BlkioWeight": 500,
            "PidsLimit": 2048,
            "RestartPolicy": {
                "Name": "on-failure",
                "MaximumRetryCount": 5
            }
        });

        let req: ContainerUpdateRequest =
            serde_json::from_value(body.clone()).expect("deserialize update body");

        assert_eq!(req.cpu_shares, Some(512));
        assert_eq!(req.memory, Some(314_572_800));
        assert_eq!(req.cpu_period, Some(100_000));
        assert_eq!(req.cpu_quota, Some(50_000));
        assert_eq!(req.cpu_realtime_period, Some(1_000_000));
        assert_eq!(req.cpu_realtime_runtime, Some(950_000));
        assert_eq!(req.cpuset_cpus.as_deref(), Some("0-3"));
        assert_eq!(req.cpuset_mems.as_deref(), Some("0,1"));
        assert_eq!(req.memory_reservation, Some(268_435_456));
        assert_eq!(req.memory_swap, Some(629_145_600));
        assert_eq!(req.kernel_memory, Some(67_108_864));
        assert_eq!(req.blkio_weight, Some(500));
        assert_eq!(req.pids_limit, Some(2048));
        let rp = req.restart_policy.as_ref().expect("restart_policy");
        assert_eq!(rp.name.as_deref(), Some("on-failure"));
        assert_eq!(rp.maximum_retry_count, Some(5));

        // Round-trip through the wire shape unchanged: every field must
        // serialize back with its PascalCase Docker name.
        let reser = serde_json::to_value(&req).expect("re-serialize");
        assert_eq!(reser, body);
    }

    /// An empty body must deserialize successfully — Docker accepts
    /// `POST /containers/{id}/update` with `{}` (a no-op update).
    #[test]
    fn container_update_request_empty_body_deserializes_to_default() {
        let req: ContainerUpdateRequest =
            serde_json::from_str("{}").expect("empty body must deserialize");
        assert_eq!(req, ContainerUpdateRequest::default());
        assert!(req.cpu_shares.is_none());
        assert!(req.memory.is_none());
        assert!(req.restart_policy.is_none());
    }

    /// `ContainerUpdateResponse` must always emit `Warnings` (even empty)
    /// so clients that match on field presence don't break.
    #[test]
    fn container_update_response_always_emits_warnings_field() {
        let resp = ContainerUpdateResponse::default();
        let json = serde_json::to_value(&resp).expect("serialize");
        assert!(json.get("Warnings").is_some(), "Warnings must be present");
        assert_eq!(json["Warnings"], serde_json::json!([]));
    }
}
