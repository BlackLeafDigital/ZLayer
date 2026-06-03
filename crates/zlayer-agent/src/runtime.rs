//! Abstract container runtime interface
//!
//! Defines the Runtime trait that can be implemented for different container runtimes
//! (containerd, CRI-O, etc.)

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use futures_util::Stream;
use std::collections::VecDeque;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use zlayer_observability::logs::{LogEntry, LogSource, LogStream};
use zlayer_spec::{PullPolicy, RegistryAuth, ServiceSpec};

/// Container state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    /// Container is being pulled/created
    Pending,
    /// Init actions are running
    Initializing,
    /// Container is running
    Running,
    /// Container is stopping
    Stopping,
    /// Container has exited
    Exited { code: i32 },
    /// Container failed
    Failed { reason: String },
}

impl ContainerState {
    /// Stable lowercase string representation of the state.
    ///
    /// Used when surfacing container state through the API / `ps` output.
    /// `Running` stringifies to `"running"` (matched case-insensitively by the
    /// raft e2e harness when counting healthy replicas).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Initializing => "initializing",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Exited { .. } => "exited",
            Self::Failed { .. } => "failed",
        }
    }
}

impl std::fmt::Display for ContainerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Container identifier.
///
/// Identifies a container by `(service, replica)` for the legacy single-group
/// case, and extends with `role` + `node_id` for cluster-aware
/// multi-group services and cross-node identification.
///
/// Defaults: `role = "default"`, `node_id = 0`. Existing constructors
/// (`ContainerId::new(service, replica)`) produce these defaults. Use
/// `ContainerId::with_role_and_node(...)` when the new fields matter.
///
/// Display:
/// - With defaults: `{service}-rep-{replica}` (backward compat).
/// - Otherwise: `{service}-{role}-{replica}-on-{node_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ContainerId {
    pub service: String,
    pub replica: u32,
    /// Role within `replica_groups`. `"default"` for services without groups.
    #[serde(default = "default_container_role")]
    pub role: String,
    /// Cluster node that owns this container. `0` in single-node deployments
    /// or before the cluster is initialized.
    #[serde(default)]
    pub node_id: u64,
}

fn default_container_role() -> String {
    "default".to_string()
}

impl ContainerId {
    /// Build a legacy `{service, replica}` `ContainerId` with default `role`
    /// and `node_id`. Used by all existing callsites — behavior is unchanged.
    #[must_use]
    pub fn new(service: impl Into<String>, replica: u32) -> Self {
        Self {
            service: service.into(),
            replica,
            role: default_container_role(),
            node_id: 0,
        }
    }

    /// Build a cluster-aware `ContainerId` with explicit `role` and `node_id`.
    /// Used by `ServiceManager` when a service has `replica_groups` or when
    /// the daemon participates in a multi-node cluster.
    #[must_use]
    pub fn with_role_and_node(
        service: impl Into<String>,
        replica: u32,
        role: impl Into<String>,
        node_id: u64,
    ) -> Self {
        Self {
            service: service.into(),
            replica,
            role: role.into(),
            node_id,
        }
    }

    /// True when both `role` and `node_id` are at their defaults — i.e.
    /// this is a legacy-shape `ContainerId`.
    #[must_use]
    pub fn is_legacy_shape(&self) -> bool {
        self.role == "default" && self.node_id == 0
    }
}

impl std::fmt::Display for ContainerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_legacy_shape() {
            write!(f, "{}-rep-{}", self.service, self.replica)
        } else {
            write!(
                f,
                "{}-{}-{}-on-{}",
                self.service, self.role, self.replica, self.node_id
            )
        }
    }
}

/// Container handle
pub struct Container {
    pub id: ContainerId,
    /// Image reference this container was created from (canonical form, e.g.
    /// `docker.io/library/nginx:1.29-alpine`). Surfaced through the API/`ps`.
    pub image: String,
    pub state: ContainerState,
    pub pid: Option<u32>,
    pub task: Option<JoinHandle<std::io::Result<()>>>,
    /// Overlay network IP address assigned to this container
    pub overlay_ip: Option<IpAddr>,
    /// Health monitor task handle for this container
    pub health_monitor: Option<JoinHandle<()>>,
    /// Runtime-assigned port override (used by macOS sandbox where all
    /// containers share the host network and need unique ports).
    /// When `Some(port)`, the proxy should use this port instead of the
    /// spec-declared endpoint port for this specific container's backend address.
    pub port_override: Option<u16>,
}

/// Summary information about a cached image on the host runtime.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImageInfo {
    /// Canonical image reference (e.g. `zachhandley/zlayer-manager:latest`).
    pub reference: String,
    /// Content-addressed digest if known (`sha256:...`). `None` when the
    /// backend only tracks images by tag.
    pub digest: Option<String>,
    /// Total on-disk / in-cache size in bytes, when available.
    pub size_bytes: Option<u64>,
}

/// Result of a prune operation.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PruneResult {
    /// Image references that were removed.
    pub deleted: Vec<String>,
    /// Bytes reclaimed from the cache. `0` when the backend cannot report.
    pub space_reclaimed: u64,
}

/// Reason a container stopped running, as reported by [`Runtime::wait_outcome`].
///
/// Serialized as `snake_case` strings on the wire (`exited`, `signal`,
/// `oom_killed`, `runtime_error`) so the API DTO can emit the reason as-is
/// without a second translation layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    /// Container exited normally.
    Exited,
    /// Container was killed by a signal (e.g. `SIGKILL`, `SIGTERM`).
    Signal,
    /// Container was killed by the OOM killer.
    OomKilled,
    /// Runtime-side failure (pre-start error, runtime crash, etc.).
    RuntimeError,
}

/// Wait condition mirroring Docker's `POST /containers/{id}/wait?condition=`.
///
/// Maps 1:1 to the wire form (`not-running`, `next-exit`, `removed`) via
/// kebab-case serde so the daemon and the Docker compat shim can deserialize
/// the query parameter in a single step. The default condition is
/// [`WaitCondition::NotRunning`], matching Docker's behaviour when the
/// `condition` query param is omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitCondition {
    /// Wait until the container is not running. This is the default when
    /// the caller doesn't specify a condition. Returns immediately if the
    /// container has already exited; otherwise blocks until it does.
    #[default]
    NotRunning,
    /// Wait for the next container exit, even if the container is already
    /// stopped at the time of the call. Restarts the wait loop on each
    /// observed exit.
    NextExit,
    /// Wait until the container is removed. Useful in conjunction with
    /// `--rm` / `AutoRemove` lifecycle wiring.
    Removed,
}

impl WaitCondition {
    /// Return the wire string this variant serializes to (`"not-running"`,
    /// `"next-exit"`, `"removed"`), matching Docker's `condition=` query
    /// parameter spelling.
    #[must_use]
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::NotRunning => "not-running",
            Self::NextExit => "next-exit",
            Self::Removed => "removed",
        }
    }

    /// Parse a Docker-style condition string (`"not-running"`, `"next-exit"`,
    /// `"removed"`). Returns `None` for unknown values so callers can
    /// distinguish "default" (omitted) from "rejected".
    #[must_use]
    pub fn from_wire_str(s: &str) -> Option<Self> {
        match s {
            "not-running" | "" => Some(Self::NotRunning),
            "next-exit" => Some(Self::NextExit),
            "removed" => Some(Self::Removed),
            _ => None,
        }
    }
}

/// Richer wait result returned by [`Runtime::wait_outcome`].
///
/// Backwards-compatible with [`Runtime::wait_container`] (which returns just
/// `exit_code`). The API handler uses this to populate the extended
/// `ContainerWaitResponse` fields (`reason`, `signal`, `finished_at`) while
/// existing callers that only need the exit code can keep using
/// `wait_container`.
#[derive(Debug, Clone)]
pub struct WaitOutcome {
    /// Process exit code (0 = success). When the container was killed by
    /// signal `N`, this is typically `128 + N`.
    pub exit_code: i32,
    /// Classification of the exit.
    pub reason: WaitReason,
    /// Signal name when `reason == WaitReason::Signal`, e.g. `"SIGKILL"`.
    /// Derived from `exit_code - 128` on a best-effort basis.
    pub signal: Option<String>,
    /// Time the container exited, if the runtime reports it.
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl WaitOutcome {
    /// Build a plain `Exited` outcome with no signal/timestamp metadata — the
    /// default that matches the pre-§3.12 behaviour.
    #[must_use]
    pub fn exited(exit_code: i32) -> Self {
        Self {
            exit_code,
            reason: WaitReason::Exited,
            signal: None,
            finished_at: None,
        }
    }
}

/// Map a signal-style exit code (`128 + N`) to a canonical signal name.
///
/// Recognises common POSIX signals and falls back to `signal_<n>` for
/// unknown numbers so the caller always gets *something* readable.
#[must_use]
pub fn signal_name_from_exit_code(exit_code: i32) -> Option<String> {
    if exit_code <= 128 {
        return None;
    }
    let n = exit_code - 128;
    let name = match n {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        10 => "SIGUSR1",
        11 => "SIGSEGV",
        12 => "SIGUSR2",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        17 => "SIGSTOP",
        18 => "SIGCONT",
        _ => return Some(format!("signal_{n}")),
    };
    Some(name.to_string())
}

/// One streaming event emitted by [`Runtime::exec_stream`].
///
/// Runtimes push these events as the exec'd command produces output. The final
/// event for any successful stream is always an [`ExecEvent::Exit`] carrying
/// the process's exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecEvent {
    /// A chunk of stdout from the running command. Emitted line-by-line by
    /// Docker; other runtimes may emit the full buffered output in one event.
    Stdout(String),
    /// A chunk of stderr from the running command.
    Stderr(String),
    /// The command has exited with this exit code. Always the final event.
    Exit(i32),
}

/// Boxed async stream of [`ExecEvent`]s returned by [`Runtime::exec_stream`].
pub type ExecEventStream = Pin<Box<dyn Stream<Item = ExecEvent> + Send>>;

/// Options accepted by [`Runtime::exec_pty`].
///
/// Mirrors the union of fields exposed by Docker's `POST /containers/{id}/exec`
/// (`ExecConfig`) so the daemon can pass them through with minimal translation.
/// Unlike [`Runtime::exec`] / [`Runtime::exec_stream`] (which capture stdout
/// and stderr separately and return only after the process exits), `exec_pty`
/// is the interactive entry point: it allocates a PTY when `tty` is set,
/// streams I/O bidirectionally over a single duplex byte stream, and returns
/// an [`ExecHandle`] that the caller drives concurrently with the running
/// process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // mirrors Docker's `ExecConfig` 1:1
pub struct ExecOptions {
    /// The argv vector for the exec'd process (`command[0]` is the binary).
    pub command: Vec<String>,
    /// Extra environment variables, in `KEY=VALUE` form. Merged into the
    /// container's existing env on the runtime side.
    pub env: Vec<String>,
    /// Optional working directory inside the container. `None` keeps the
    /// container's default `WORKDIR`.
    pub working_dir: Option<String>,
    /// Optional `user[:group]` override. `None` keeps the container's
    /// configured user.
    pub user: Option<String>,
    /// Run the exec with privileged capabilities (Docker `Privileged`).
    pub privileged: bool,
    /// Allocate a TTY for the exec'd process (Docker `Tty`). When `true`, the
    /// runtime should set up a pseudo-terminal pair and the duplex stream on
    /// the returned [`ExecHandle`] carries multiplexed PTY traffic; when
    /// `false`, the stream carries raw stdout/stderr without PTY framing.
    pub tty: bool,
    /// Attach stdin so the caller can write to the process (Docker
    /// `AttachStdin`). When `false`, the writable half of the duplex stream
    /// is effectively a no-op.
    pub attach_stdin: bool,
    /// Attach stdout so the caller receives the process's stdout on the
    /// readable half of the duplex stream (Docker `AttachStdout`).
    pub attach_stdout: bool,
    /// Attach stderr so the caller receives the process's stderr on the
    /// readable half of the duplex stream (Docker `AttachStderr`).
    pub attach_stderr: bool,
}

/// Marker supertrait combining [`AsyncRead`] + [`AsyncWrite`] so they can be
/// used together as a single trait object. Rust forbids stacking two
/// non-auto traits directly in `dyn`, so [`ExecPtyStream`] is built on top
/// of this helper instead.
///
/// A blanket impl below covers every type that already satisfies the four
/// component bounds, so callers never need to implement `ExecDuplex`
/// manually — they just hand the runtime any concrete duplex stream that's
/// `AsyncRead + AsyncWrite + Send + Unpin`.
pub trait ExecDuplex: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> ExecDuplex for T where T: AsyncRead + AsyncWrite + Send + Unpin + ?Sized {}

/// Duplex byte stream used by [`ExecHandle`] to shuttle stdin/stdout (and,
/// when [`ExecOptions::tty`] is set, multiplexed PTY traffic) between the
/// caller and the exec'd process.
///
/// `Unpin` is required (via [`ExecDuplex`]) so callers can poll the trait
/// object directly via the usual `tokio::io::AsyncReadExt` /
/// `AsyncWriteExt` extension methods without having to pin the box
/// themselves.
pub type ExecPtyStream = Box<dyn ExecDuplex + 'static>;

/// Future returned by [`ExecHandle`] that resolves with the exec'd process's
/// exit code once the runtime observes it has terminated.
pub type ExecExitFuture = Pin<Box<dyn Future<Output = Result<i32>> + Send>>;

/// Runtime-side handle returned by [`Runtime::exec_pty`].
///
/// Bundles everything a long-lived interactive exec session needs:
///
/// 1. A duplex [`ExecPtyStream`] for shuttling stdin/stdout (or full PTY
///    traffic when `tty` is set) between the caller and the running process.
/// 2. A `tokio::sync::mpsc::Sender<(rows, cols)>` so the caller can resize
///    the allocated PTY in response to terminal-size changes (mirrors
///    Docker's `POST /exec/{id}/resize` endpoint). Runtimes that don't allocate
///    a PTY should still accept the channel and treat resize messages as
///    no-ops; the channel is dropped on the runtime side once the process
///    exits.
/// 3. A boxed [`ExecExitFuture`] that resolves with the exit code once the
///    runtime detects the process has terminated.
///
/// `ExecHandle` deliberately does not implement `Debug` / `Clone` because
/// every field holds a trait object (or, in the exit future's case, an opaque
/// boxed future). Consumers move the fields out by destructuring and then
/// drive the I/O stream, the resize channel, and the exit future
/// independently.
pub struct ExecHandle {
    /// Bidirectional byte channel between the caller and the exec'd process.
    pub stream: ExecPtyStream,
    /// Channel for sending `(rows, cols)` resize requests for the PTY
    /// allocated to the exec session. Bounded so a stuck runtime can't make
    /// the caller buffer unbounded resize events; senders should drop the
    /// most recent excess size on backpressure.
    pub resize: tokio::sync::mpsc::Sender<(u16, u16)>,
    /// Future that resolves with the process's exit code once the runtime
    /// observes the exec has terminated. Consumers typically `await` this on
    /// a dedicated task while pumping the duplex stream on another.
    pub exit: ExecExitFuture,
}

/// Which standard stream a [`LogChunk`] originated from.
///
/// Mirrors the three POSIX file descriptors. `Stdin` is included for
/// completeness — Docker's multiplexed log header carries a `stdin` channel
/// for attached interactive containers — but most container runtimes only
/// emit `Stdout` / `Stderr` chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogChannel {
    /// Standard input (rarely emitted; included for completeness with
    /// Docker's stdcopy framing).
    Stdin,
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// One chunk of container log output emitted by [`Runtime::logs_stream`].
///
/// Streams emit one `LogChunk` per line (or per Docker stdcopy frame) as
/// data is produced by the container. Backends that expose timestamps
/// populate `timestamp`; ones that don't leave it `None`.
#[derive(Debug, Clone)]
pub struct LogChunk {
    /// Which standard stream produced this chunk.
    pub stream: LogChannel,
    /// Raw bytes of the chunk. Not necessarily UTF-8 — container output is
    /// arbitrary binary data and consumers must handle invalid UTF-8.
    pub bytes: bytes::Bytes,
    /// When the runtime reported this chunk, when known.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

/// Options accepted by [`Runtime::logs_stream`].
///
/// Mirrors the `GET /containers/{id}/logs` query parameters in the Docker
/// Engine API so backends can pass them through with minimal translation.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // mirrors Docker's logs query params 1:1
pub struct LogsStreamOptions {
    /// Continue streaming after the current end-of-log marker. When `false`,
    /// the stream terminates once the runtime has emitted all currently
    /// buffered logs.
    pub follow: bool,
    /// Tail the last N lines before starting to stream. `None` means "all
    /// available logs from the start".
    pub tail: Option<u64>,
    /// Earliest timestamp (Unix seconds) to include. `None` means "no
    /// lower bound".
    pub since: Option<i64>,
    /// Latest timestamp (Unix seconds) to include. `None` means "no upper
    /// bound".
    pub until: Option<i64>,
    /// When `true`, the runtime should populate [`LogChunk::timestamp`] for
    /// every chunk. Backends that always carry timestamps may ignore this
    /// flag.
    pub timestamps: bool,
    /// Include stdout in the stream.
    pub stdout: bool,
    /// Include stderr in the stream.
    pub stderr: bool,
}

/// One periodic resource-usage sample emitted by [`Runtime::stats_stream`].
///
/// Mirrors the union of fields exposed by Docker's `/containers/{id}/stats`
/// endpoint and Linux cgroup stat files so downstream consumers (autoscaler,
/// `docker stats`-compat HTTP shim) can read a single shape regardless of
/// backend. Counters that the runtime cannot report are left at zero —
/// missing data is signalled separately via the surrounding stream
/// metadata.
#[derive(Debug, Clone)]
pub struct StatsSample {
    /// Cumulative container CPU time consumed, in nanoseconds.
    pub cpu_total_ns: u64,
    /// Cumulative system CPU time observed at the same moment, in
    /// nanoseconds. Used to compute relative CPU percentage between
    /// successive samples (Docker's classic `cpu_delta / system_delta`
    /// formula).
    pub cpu_system_ns: u64,
    /// Number of CPUs currently online for this container. Used as the
    /// final scaling factor in the CPU-percentage calculation.
    pub online_cpus: u32,
    /// Resident memory currently in use, in bytes (cgroup
    /// `memory.usage_in_bytes` minus inactive page cache for v1, or
    /// `memory.current` for v2).
    pub mem_used_bytes: u64,
    /// Memory limit configured on the container, in bytes. `0` when no
    /// limit is set (cgroup reports its sentinel value).
    pub mem_limit_bytes: u64,
    /// Cumulative bytes received across all attached network interfaces.
    pub net_rx_bytes: u64,
    /// Cumulative bytes transmitted across all attached network interfaces.
    pub net_tx_bytes: u64,
    /// Cumulative bytes read from block devices.
    pub blkio_read_bytes: u64,
    /// Cumulative bytes written to block devices.
    pub blkio_write_bytes: u64,
    /// Number of process IDs currently running inside the container's pid
    /// namespace.
    pub pids_current: u64,
    /// Configured pids limit, if any. `None` means unlimited.
    pub pids_limit: Option<u64>,
    /// Wallclock time the sample was taken.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// One progress event emitted by [`Runtime::pull_image_stream`].
///
/// Backends emit a series of `Status` events as layers are downloaded /
/// extracted, followed by exactly one `Done` event when the pull completes
/// successfully. Errors mid-pull are propagated as the stream's `Err`
/// variant and terminate the stream.
#[derive(Debug, Clone)]
pub enum PullProgress {
    /// Progress update for an in-flight layer or stage.
    Status {
        /// Layer ID or other backend-specific identifier, when available.
        id: Option<String>,
        /// Human-readable status text, e.g. `"Pulling fs layer"`,
        /// `"Downloading"`, `"Extracting"`. Always present; may be empty
        /// when the backend has nothing to report this tick.
        status: String,
        /// Pre-formatted progress bar (Docker emits a string like
        /// `"[========>          ] 12.3MB/45.6MB"`). `None` when the
        /// backend reports raw `current`/`total` only.
        progress: Option<String>,
        /// Bytes transferred so far for this layer, when reported.
        current: Option<u64>,
        /// Expected total bytes for this layer, when reported.
        total: Option<u64>,
    },
    /// Pull completed successfully.
    Done {
        /// Resolved canonical image reference (typically the same as the
        /// requested reference, but may include a digest the backend
        /// resolved).
        reference: String,
        /// Content-addressed digest, when the backend reports one.
        digest: Option<String>,
    },
}

/// Boxed async stream of `Result<LogChunk, AgentError>` items returned by
/// [`Runtime::logs_stream`].
///
/// `'static` lifetime so handlers can hold the stream past the trait method's
/// borrow of `self`.
pub type LogsStream = Pin<Box<dyn Stream<Item = Result<LogChunk>> + Send + 'static>>;

/// Boxed async stream of `Result<StatsSample, AgentError>` items returned by
/// [`Runtime::stats_stream`].
pub type StatsStream = Pin<Box<dyn Stream<Item = Result<StatsSample>> + Send + 'static>>;

/// Boxed async stream of `Result<PullProgress, AgentError>` items returned by
/// [`Runtime::pull_image_stream`].
pub type PullProgressStream = Pin<Box<dyn Stream<Item = Result<PullProgress>> + Send + 'static>>;

/// Boxed async stream of TAR-archive byte chunks returned by
/// [`Runtime::archive_get`].
///
/// Each yielded `Bytes` is a contiguous slice of the TAR archive that the
/// runtime is producing for the requested container path. The stream ends
/// once the archive is fully written. Mid-stream errors map to
/// [`AgentError`] variants.
pub type ArchiveStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes>> + Send + 'static>>;

/// Stat metadata for a single path inside a container, returned by
/// [`Runtime::archive_head`].
///
/// Mirrors Docker's `X-Docker-Container-Path-Stat` header payload (a
/// base64-encoded JSON object with `name`, `size`, `mode`, `mtime`, and
/// `linkTarget` fields). The API layer serializes this back into the same
/// header for the `HEAD /containers/{id}/archive` response.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PathStat {
    /// Base name of the path (e.g. `"foo.txt"` for `/etc/foo.txt`).
    pub name: String,
    /// File size in bytes. For directories this is the size reported by the
    /// runtime (typically the directory entry size, not the recursive sum).
    pub size: i64,
    /// Unix file mode bits (`S_IFMT | S_IRWXU | ...`). Encoded as a `u32` so
    /// it round-trips through the Docker JSON header losslessly.
    pub mode: u32,
    /// Last-modification time as an RFC 3339 string. `None` when the runtime
    /// cannot report it.
    pub mtime: Option<String>,
    /// Target of a symbolic link, when the path is a symlink. Empty string
    /// for non-symlink paths (matching Docker's wire shape).
    pub link_target: String,
}

/// Options accepted by [`Runtime::archive_put`].
///
/// Mirrors the query parameters Docker accepts on
/// `PUT /containers/{id}/archive`:
///
/// * `noOverwriteDirNonDir=1` — refuse to replace a non-directory with a
///   directory or vice versa. Default `false`.
/// * `copyUIDGID=1` — preserve UID/GID of files in the archive instead of
///   chown'ing them to the container's user. Default `false`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArchivePutOptions {
    /// When `true`, the runtime must reject puts that would replace a
    /// non-directory with a directory (or vice versa) at the destination.
    pub no_overwrite_dir_non_dir: bool,
    /// When `true`, preserve UID/GID of files in the archive verbatim.
    pub copy_uid_gid: bool,
}

/// Per-network attachment reported by [`Runtime::inspect_detailed`].
///
/// Mirrors the subset of bollard's `EndpointSettings` that the API needs to
/// populate `ContainerInfo.networks` for standalone containers. Kept in
/// `zlayer-agent` (rather than `zlayer-spec`) because it's a runtime-level
/// inspect result, not a deployment specification.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkAttachmentDetail {
    /// Network name as reported by the runtime (Docker's key in
    /// `NetworkSettings.Networks`, e.g. `"bridge"` or a user-defined network
    /// name).
    pub network: String,
    /// DNS aliases the container answers to on this network. Empty when the
    /// runtime doesn't surface aliases.
    pub aliases: Vec<String>,
    /// Assigned IPv4 address on this network, if any. Empty strings are
    /// normalised to `None`.
    pub ipv4: Option<String>,
}

/// Per-container health detail reported by [`Runtime::inspect_detailed`].
///
/// Sourced directly from bollard's `ContainerState.health` (Docker's native
/// healthcheck tracking). Our internal `HealthMonitor` in
/// `crates/zlayer-agent/src/health.rs` drives service-level health events; for
/// standalone containers the API reports the runtime-native status instead so
/// that images with a baked-in `HEALTHCHECK` show up correctly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HealthDetail {
    /// One of `"none"`, `"starting"`, `"healthy"`, `"unhealthy"` (Docker's
    /// `HealthStatusEnum`). Empty string is normalised to `"none"` upstream.
    pub status: String,
    /// Consecutive failing probe count, if the runtime reports it.
    pub failing_streak: Option<u32>,
    /// Output from the most recent failing probe, when available.
    pub last_output: Option<String>,
}

/// Rich inspect details for a single container, returned by
/// [`Runtime::inspect_detailed`].
///
/// Carries the fields `ContainerInfo` needs on top of the bare
/// [`ContainerState`] reported by [`Runtime::container_state`]:
/// published ports, attached networks, first IPv4, health, and `exit_code`.
///
/// Default is an all-empty record — that's what the default trait method
/// returns for runtimes that don't (yet) implement rich inspect, and the API
/// layer treats all fields as purely additive, so a default record still
/// produces a backwards-compatible `ContainerInfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerInspectDetails {
    /// Published port mappings (container → host), translated back from the
    /// runtime's internal port-binding map.
    pub ports: Vec<zlayer_spec::PortMapping>,
    /// Networks the container is attached to, plus the aliases + IPv4 for each.
    pub networks: Vec<NetworkAttachmentDetail>,
    /// First non-empty IPv4 address found across the container's networks,
    /// useful as a "primary" IP for simple clients that don't want to iterate
    /// `networks`. `None` when the container isn't on any network with an IP.
    pub ipv4: Option<String>,
    /// Health status when the container has a Docker-native `HEALTHCHECK` or
    /// the runtime otherwise reports a health state.
    pub health: Option<HealthDetail>,
    /// Most recent exit code, when the runtime reports one. `None` for
    /// containers that are still running and have never exited.
    pub exit_code: Option<i32>,
}

/// Lightweight summary of a single container reported by
/// [`Runtime::list_containers`].
///
/// Reconciliation only needs to match runtime containers against
/// `ZLayer`'s own metadata, which lives in the `com.zlayer.container_id`
/// label (see `zlayer-api::handlers::container_id_map::ZLAYER_CONTAINER_ID_LABEL`).
/// Carrying that label value plus the runtime-native id is sufficient for
/// the standalone-container reconcile pass; richer fields can be added
/// when concrete callers need them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeContainerSummary {
    /// Backend-native container handle (Docker's 64-char hex, the youki
    /// state-dir name, etc.). Opaque to the reconciler — only used for
    /// logging and to disambiguate listings.
    pub runtime_id: String,
    /// Value of the `com.zlayer.container_id` label, if the runtime
    /// reports one for this container. Containers without this label are
    /// foreign (not ZLayer-managed) and should be ignored by reconcile.
    pub zlayer_container_id_label: Option<String>,
}

/// One row of process information returned by [`Runtime::top_container`].
///
/// Mirrors Docker's `GET /containers/{id}/top` response shape: a `Titles`
/// vector that names each column, plus a `Processes` matrix where each row is
/// the per-process column values. The runtime decides which `ps` fields to
/// emit; the API/Docker shim forwards them verbatim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerTopOutput {
    /// Column titles (e.g. `["UID", "PID", "PPID", "C", "STIME", "TTY", "TIME", "CMD"]`).
    pub titles: Vec<String>,
    /// One row per process; each row has the same length as `titles`.
    pub processes: Vec<Vec<String>>,
}

/// Filesystem change kind reported by [`Runtime::changes_container`].
///
/// Matches Docker's numeric encoding: `0 = Modified`, `1 = Added`, `2 = Deleted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemChangeKind {
    /// File or directory was modified in the container's writable layer.
    Modified,
    /// File or directory was added.
    Added,
    /// File or directory was deleted.
    Deleted,
}

impl FilesystemChangeKind {
    /// Numeric wire value used by Docker's `/containers/{id}/changes`.
    #[must_use]
    pub const fn as_docker_kind(self) -> u8 {
        match self {
            Self::Modified => 0,
            Self::Added => 1,
            Self::Deleted => 2,
        }
    }
}

/// One filesystem change reported by [`Runtime::changes_container`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FilesystemChangeEntry {
    /// Path inside the container that changed (absolute, e.g. `/etc/hosts`).
    pub path: String,
    /// Kind of change.
    pub kind: FilesystemChangeKind,
}

/// One published port mapping entry returned by [`Runtime::port_mappings_container`].
///
/// Mirrors one entry of Docker's `/containers/{id}/port` map. A single
/// container port may bind multiple host endpoints (e.g. IPv4 + IPv6); each
/// such binding yields one [`PortMappingEntry`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PortMappingEntry {
    /// Container port number that's published.
    pub container_port: u16,
    /// Protocol (`"tcp"`, `"udp"`, `"sctp"`).
    pub protocol: String,
    /// Host IP address that the container's port is mapped to.
    pub host_ip: Option<String>,
    /// Host port number that the container's port is mapped to.
    pub host_port: Option<u16>,
}

/// Result of a [`Runtime::prune_containers`] call.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContainerPruneResult {
    /// Container IDs that were removed (runtime-native or hex form).
    pub deleted: Vec<String>,
    /// Bytes reclaimed from the runtime's container storage. `0` when the
    /// backend cannot report.
    pub space_reclaimed: u64,
}

/// Runtime-level container restart policy attached to a
/// [`ContainerResourceUpdate`].
///
/// Mirrors Docker's `HostConfig.RestartPolicy`. `name` is one of `""`,
/// `"no"`, `"always"`, `"unless-stopped"`, or `"on-failure"`. Backends that
/// can persist a restart policy (Docker via bollard, Youki via the
/// supervisor's stored spec) honour it; backends that cannot
/// (WASM, Mock) leave it unmodified.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerRestartPolicyUpdate {
    /// Restart policy name.
    pub name: Option<String>,
    /// Maximum retry count (only honoured when `name == "on-failure"`).
    pub maximum_retry_count: Option<i64>,
}

/// Resource-update payload passed to [`Runtime::update_container_resources`].
///
/// Mirrors the resource subset of Docker's `POST /containers/{id}/update`
/// body. Every field is `Option<...>`; backends apply only the fields that
/// are `Some`. A request with all fields `None` is a no-op (consistent with
/// Docker's behaviour).
///
/// Field semantics match Docker's wire shape: see [`ContainerUpdateRequest`]
/// in `zlayer-types::api::containers` for the JSON encoding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerResourceUpdate {
    /// CPU shares (cgroup `cpu.weight` / `cpu.shares`).
    pub cpu_shares: Option<i64>,
    /// Memory limit in bytes.
    pub memory: Option<i64>,
    /// CPU CFS period in microseconds.
    pub cpu_period: Option<i64>,
    /// CPU CFS quota in microseconds.
    pub cpu_quota: Option<i64>,
    /// CPU real-time period in microseconds.
    pub cpu_realtime_period: Option<i64>,
    /// CPU real-time runtime in microseconds.
    pub cpu_realtime_runtime: Option<i64>,
    /// CPUs allowed for execution (e.g. `"0-3"`).
    pub cpuset_cpus: Option<String>,
    /// Memory nodes (NUMA) allowed for execution.
    pub cpuset_mems: Option<String>,
    /// Soft memory limit in bytes.
    pub memory_reservation: Option<i64>,
    /// Total memory limit (memory + swap) in bytes. `-1` removes swap.
    pub memory_swap: Option<i64>,
    /// Kernel memory limit in bytes (deprecated upstream).
    pub kernel_memory: Option<i64>,
    /// Block IO weight (10-1000).
    pub blkio_weight: Option<u16>,
    /// PIDs limit. `0` or `-1` for unlimited.
    pub pids_limit: Option<i64>,
    /// Replacement restart policy. `None` leaves the policy unchanged.
    pub restart_policy: Option<ContainerRestartPolicyUpdate>,
}

impl ContainerResourceUpdate {
    /// Returns `true` when this update would not change anything — every
    /// field is `None`. Backends short-circuit no-op updates rather than
    /// touching the cgroup hierarchy.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cpu_shares.is_none()
            && self.memory.is_none()
            && self.cpu_period.is_none()
            && self.cpu_quota.is_none()
            && self.cpu_realtime_period.is_none()
            && self.cpu_realtime_runtime.is_none()
            && self.cpuset_cpus.is_none()
            && self.cpuset_mems.is_none()
            && self.memory_reservation.is_none()
            && self.memory_swap.is_none()
            && self.kernel_memory.is_none()
            && self.blkio_weight.is_none()
            && self.pids_limit.is_none()
            && self.restart_policy.is_none()
    }
}

/// Result of a [`Runtime::update_container_resources`] call. Mirrors
/// Docker's `{"Warnings": [...]}` response shape: backends append a string
/// per "we accepted this but did not apply it" or "field deprecated"
/// warning.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerUpdateOutcome {
    /// Human-readable warnings emitted while applying the update.
    pub warnings: Vec<String>,
}

/// Detailed image inspect record returned by [`Runtime::inspect_image_native`].
///
/// Mirrors the union of fields exposed by Docker's `GET /images/{name}/json`
/// (bollard's `ImageInspect`) so the API / Docker compat shim can surface a
/// Docker-shaped JSON response without re-translating later. Optional fields
/// remain `None` when the backend cannot provide them.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageInspectInfo {
    /// Content-addressed image id (`sha256:...`), when known.
    pub id: Option<String>,
    /// All tags (`repo:tag`) currently pointing at this image.
    pub repo_tags: Vec<String>,
    /// Manifest digests this image is known under (`repo@sha256:...`).
    pub repo_digests: Vec<String>,
    /// Parent image id (`sha256:...`), when the image was built locally.
    pub parent: Option<String>,
    /// Human-readable comment recorded at commit/import time.
    pub comment: Option<String>,
    /// Creation timestamp in RFC 3339 form.
    pub created: Option<String>,
    /// Container id this image was committed from, when applicable.
    pub container: Option<String>,
    /// Daemon version that built / imported this image.
    pub docker_version: Option<String>,
    /// Author recorded on the image (e.g. `MAINTAINER` instruction).
    pub author: Option<String>,
    /// Hardware architecture the image targets (`amd64`, `arm64`, ...).
    pub architecture: Option<String>,
    /// Operating system the image targets (`linux`, `windows`, ...).
    pub os: Option<String>,
    /// Total on-disk size in bytes, when known.
    pub size: Option<u64>,
    /// Layer order: list of `sha256:...` digests, root-most first.
    pub layers: Vec<String>,
    /// Container env (`KEY=VALUE`).
    pub env: Vec<String>,
    /// Default command vector.
    pub cmd: Vec<String>,
    /// Default entrypoint vector.
    pub entrypoint: Vec<String>,
    /// Working directory inside the image.
    pub working_dir: Option<String>,
    /// User the image runs as by default.
    pub user: Option<String>,
    /// Image labels.
    pub labels: std::collections::BTreeMap<String, String>,
}

/// One row of an image's history, returned by [`Runtime::image_history`].
///
/// Mirrors Docker's `GET /images/{name}/history` response.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageHistoryEntry {
    /// Layer / image id (`sha256:...`). May be `<missing>` for layers that
    /// were dropped during a squash.
    pub id: String,
    /// Unix-seconds timestamp when this layer was created.
    pub created: i64,
    /// Dockerfile-style instruction that produced this layer.
    pub created_by: String,
    /// Tags that point at this specific layer.
    pub tags: Vec<String>,
    /// Layer size in bytes.
    pub size: u64,
    /// Optional comment recorded with the layer.
    pub comment: String,
}

/// One result returned by [`Runtime::search_images`].
///
/// Mirrors Docker's `GET /images/search` response items.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageSearchResult {
    /// Image name (e.g. `library/nginx`).
    pub name: String,
    /// Free-text description of the image.
    pub description: String,
    /// Number of stars on the source registry, when reported.
    pub star_count: u64,
    /// Whether the image is officially curated.
    pub official: bool,
    /// Whether the image was produced by an automated build (deprecated by
    /// Docker but still surfaced for compatibility).
    pub automated: bool,
}

/// Result of a [`Runtime::commit_container`] call.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommitOutcome {
    /// Content-addressed image id of the freshly created image.
    pub id: String,
}

/// Options accepted by [`Runtime::commit_container`].
///
/// Mirrors Docker's `POST /commit` query parameters. All fields are optional
/// — when `repo`/`tag` are both empty the runtime creates an untagged image.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitOptions {
    /// Repository name to apply to the committed image (e.g. `myapp`).
    pub repo: Option<String>,
    /// Tag to apply (defaults to `latest` when `repo` is set and tag is empty).
    pub tag: Option<String>,
    /// Free-form comment to record on the committed image.
    pub comment: Option<String>,
    /// Author to record on the committed image.
    pub author: Option<String>,
    /// Whether to pause the container before committing (defaults to `true`).
    pub pause: bool,
    /// Dockerfile-style instructions to apply during commit.
    pub changes: Option<String>,
}

/// Boxed async stream of TAR-archive byte chunks returned by image save /
/// container export endpoints. Each yielded `Bytes` is a contiguous slice
/// of an uncompressed TAR archive.
pub type ImageExportStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes>> + Send + 'static>>;

/// One progress event emitted by [`Runtime::load_images`].
///
/// `Status` events carry per-line progress reported by the daemon while
/// the tar is being unpacked; `Done` is emitted exactly once when load
/// completes successfully.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoadProgress {
    /// Mid-load progress entry.
    Status {
        /// Layer or image id when reported.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Human-readable status text.
        status: String,
    },
    /// Load completed. `references` lists the image references that were
    /// loaded into the cache.
    Done {
        /// Loaded image references (`repo:tag` or `sha256:...`).
        references: Vec<String>,
    },
}

/// Boxed async stream of [`LoadProgress`] events returned by
/// [`Runtime::load_images`].
pub type LoadProgressStream = Pin<Box<dyn Stream<Item = Result<LoadProgress>> + Send + 'static>>;

/// Abstract container runtime trait
///
/// This trait abstracts over different container runtimes (containerd, CRI-O, etc.)
#[async_trait::async_trait]
pub trait Runtime: Send + Sync {
    /// Pull an image to local storage
    async fn pull_image(&self, image: &str) -> Result<()>;

    /// Pull an image to local storage with a specific policy.
    ///
    /// When `auth` is `Some`, the runtime uses those inline credentials for
    /// the pull (§3.10 of `ZLAYER_SDK_FIXES.md`). When `auth` is `None`, the
    /// runtime falls back to its existing credential-store lookup keyed by
    /// registry hostname (or anonymous access when no match exists).
    ///
    /// Non-Docker runtimes may accept but ignore the `auth` argument — their
    /// OCI puller (`zlayer-registry`) already resolves credentials from the
    /// store by hostname, and inline auth is primarily a Docker-backend
    /// concern. Ignoring it is safe: callers that need inline auth should use
    /// the Docker runtime.
    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: PullPolicy,
        auth: Option<&RegistryAuth>,
    ) -> Result<()>;

    /// Create a container
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()>;

    /// Start a container
    async fn start_container(&self, id: &ContainerId) -> Result<()>;

    /// Stop a container
    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()>;

    /// Remove a container
    async fn remove_container(&self, id: &ContainerId) -> Result<()>;

    /// Get container state
    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState>;

    /// Get container logs as structured entries
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>>;

    /// Execute command in container
    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)>;

    /// Execute a command in a container and stream stdout / stderr / exit
    /// events as they are produced.
    ///
    /// The default implementation calls the buffered [`Runtime::exec`] and
    /// emits everything as a single `Stdout` event, a single `Stderr` event,
    /// and a final `Exit` event. Runtimes that support true streaming (e.g.
    /// Docker via bollard) override this to produce line-by-line events as
    /// the command runs.
    ///
    /// The returned stream always terminates with exactly one
    /// [`ExecEvent::Exit`] as the final item on success. Errors that occur
    /// before the stream is returned (e.g. container not found, failure to
    /// create the exec) are surfaced via the outer `Result`; errors that
    /// occur mid-stream are logged by the runtime and the stream closes.
    async fn exec_stream(&self, id: &ContainerId, cmd: &[String]) -> Result<ExecEventStream> {
        let (exit, stdout, stderr) = self.exec(id, cmd).await?;
        let mut events: Vec<ExecEvent> = Vec::with_capacity(3);
        if !stdout.is_empty() {
            events.push(ExecEvent::Stdout(stdout));
        }
        if !stderr.is_empty() {
            events.push(ExecEvent::Stderr(stderr));
        }
        events.push(ExecEvent::Exit(exit));
        Ok(Box::pin(futures_util::stream::iter(events)))
    }

    /// Start an interactive exec session against a container, returning an
    /// [`ExecHandle`] the caller drives concurrently with the running process.
    ///
    /// Unlike [`Runtime::exec`] (which buffers stdout/stderr and returns only
    /// after the process exits) and [`Runtime::exec_stream`] (which streams
    /// line-by-line events one-way), `exec_pty` is the long-lived bidirectional
    /// entry point: when [`ExecOptions::tty`] is set the runtime allocates a
    /// pseudo-terminal pair and the returned [`ExecHandle::stream`] shuttles
    /// raw PTY bytes; when `tty` is false the stream still carries
    /// stdin/stdout/stderr but without PTY framing. The handle's
    /// [`ExecHandle::resize`] channel mirrors Docker's
    /// `POST /exec/{id}/resize` and the [`ExecHandle::exit`] future resolves
    /// with the process exit code once the runtime detects termination.
    ///
    /// The default implementation returns [`AgentError::Unsupported`].
    /// Backends that can host interactive execs (Docker via bollard's
    /// `start_exec` with hijacked stream, Youki via libcontainer's exec API,
    /// HCS via the Windows console) override this. Runtimes that have no
    /// notion of an interactive exec (WASM in-process, mocks that don't need
    /// PTY traffic) should leave the default in place — callers then surface
    /// a clear error rather than degrade silently to a buffered exec.
    async fn exec_pty(&self, _id: &ContainerId, _opts: ExecOptions) -> Result<ExecHandle> {
        Err(AgentError::Unsupported(
            "exec_pty is not supported by this runtime".into(),
        ))
    }

    /// Get container resource statistics from cgroups
    ///
    /// Returns CPU and memory statistics for the specified container.
    /// Used for metrics collection and autoscaling decisions.
    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats>;

    /// Wait for a container to exit and return its exit code
    ///
    /// This method blocks until the container exits or an error occurs.
    /// Used primarily for job execution to implement run-to-completion semantics.
    async fn wait_container(&self, id: &ContainerId) -> Result<i32>;

    /// Wait for a container to exit and return a [`WaitOutcome`] with richer
    /// classification (exit code + reason + signal + `finished_at` timestamp).
    ///
    /// The default implementation delegates to [`Runtime::wait_container`] and
    /// synthesizes a [`WaitReason::Exited`] result with no signal/timestamp.
    /// Runtimes that can distinguish OOM kills, signal deaths, or report a
    /// finished-at time (e.g. the Docker runtime, which has
    /// `ContainerInspectResponse.state.oom_killed` / `.finished_at`) should
    /// override this.
    ///
    /// This is the legacy entry point that always uses
    /// [`WaitCondition::NotRunning`]. Callers that need to honour Docker's
    /// `condition=` query parameter should use
    /// [`Runtime::wait_outcome_with_condition`] instead.
    async fn wait_outcome(&self, id: &ContainerId) -> Result<WaitOutcome> {
        let exit_code = self.wait_container(id).await?;
        Ok(WaitOutcome::exited(exit_code))
    }

    /// Wait for a container to reach a [`WaitCondition`] and return a
    /// [`WaitOutcome`].
    ///
    /// Mirrors Docker's `POST /containers/{id}/wait?condition=<...>` semantics:
    ///
    /// * [`WaitCondition::NotRunning`] (the default) — block until the
    ///   container is no longer running. Returns the exit-code outcome.
    /// * [`WaitCondition::NextExit`] — wait for the next observed exit, even
    ///   if the container is already stopped at call time. The default
    ///   implementation cannot distinguish "already stopped" from "next exit",
    ///   so it falls back to the same wait as `NotRunning`. Backends that can
    ///   subscribe to runtime events (Docker via bollard's wait stream)
    ///   override this to honour the semantic.
    /// * [`WaitCondition::Removed`] — block until the container has been
    ///   removed. The default implementation again falls back to a normal
    ///   wait; the Docker runtime overrides it via bollard's `condition`
    ///   parameter.
    ///
    /// Default implementation delegates to [`Runtime::wait_outcome`] for all
    /// conditions, ignoring the condition argument. This keeps existing
    /// runtimes (Youki, WASM, mocks) working without code changes.
    async fn wait_outcome_with_condition(
        &self,
        id: &ContainerId,
        _condition: WaitCondition,
    ) -> Result<WaitOutcome> {
        self.wait_outcome(id).await
    }

    /// Rename a container. Mirrors Docker's
    /// `POST /containers/{id}/rename?name=<new>` endpoint.
    ///
    /// `new_name` is the requested human-readable name (without any leading
    /// `/`). Backends are expected to validate the name against their own
    /// constraints (e.g. uniqueness, allowed characters) and return an
    /// appropriate [`AgentError`] on rejection.
    ///
    /// The default implementation returns [`AgentError::Unsupported`].
    /// Runtimes that can rename a live container override this:
    ///
    /// * Docker — calls bollard's `rename_container` with
    ///   `RenameContainerOptions { name }`.
    /// * Youki — currently returns `Unsupported` because the libcontainer
    ///   state-dir is keyed off the immutable `ContainerId` and renaming the
    ///   on-disk layout safely while a container is alive would require
    ///   coordination with the supervisor that owns the bundle path.
    /// * Other backends (WASM, HCS, mocks) inherit the `Unsupported` default.
    async fn rename_container(&self, _id: &ContainerId, _new_name: &str) -> Result<()> {
        Err(AgentError::Unsupported(
            "rename_container is not supported by this runtime".into(),
        ))
    }

    /// Get container logs (stdout/stderr combined)
    ///
    /// Returns logs as structured entries.
    /// Used to capture job output after completion.
    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>>;

    /// Get the PID of a container's main process
    ///
    /// Returns:
    /// - `Ok(Some(pid))` for runtimes with real processes (Youki, Docker)
    /// - `Ok(None)` for runtimes without separate PIDs (WASM in-process)
    /// - `Err` if the container doesn't exist or there's an error
    ///
    /// Used for overlay network attachment and process management.
    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>>;

    /// Get the IP address of a container
    ///
    /// Returns:
    /// - `Ok(Some(ip))` if the container has a known IP address
    /// - `Ok(None)` if the container exists but has no IP assigned yet
    /// - `Err` if the container doesn't exist or there's an error
    ///
    /// Used for proxy backend registration when overlay networking is unavailable.
    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>>;

    /// Get a runtime-assigned port override for a container.
    ///
    /// Returns:
    /// - `Ok(Some(port))` if the runtime assigned a dynamic port to this container
    /// - `Ok(None)` if the container should use the spec-declared endpoint port
    ///
    /// This exists for runtimes where all containers share the host network stack
    /// (e.g., macOS sandbox). Without network namespaces, multiple replicas of
    /// the same service would conflict on the same port. The runtime assigns
    /// each replica a unique port and passes it via the `PORT` environment variable.
    /// The proxy then routes to `container_ip:override_port` instead of
    /// `container_ip:spec_port`.
    ///
    /// Runtimes with per-container networking (overlay, VMs, Docker) return `None`.
    async fn get_container_port_override(&self, _id: &ContainerId) -> Result<Option<u16>> {
        Ok(None)
    }

    /// Get the HCN namespace GUID of a Windows container.
    ///
    /// Windows-only. Linux/macOS runtimes have no HCN namespace concept and
    /// return `Ok(None)`. The `HcsRuntime` overrides this to return the
    /// namespace GUID attached during `create_container`; `OverlayManager`
    /// then uses the GUID to register the container's assigned overlay IP
    /// against the right HCN compartment (analogous to how Linux uses PID
    /// to enter the netns via `/proc/{pid}/ns/net`).
    #[cfg(target_os = "windows")]
    async fn get_container_namespace_id(
        &self,
        _id: &ContainerId,
    ) -> Result<Option<windows::core::GUID>> {
        Ok(None)
    }

    /// Sync all named volumes associated with this container to S3.
    ///
    /// Called after a container is stopped but before it is removed, giving
    /// the runtime a chance to flush persistent volume data to remote storage.
    ///
    /// The default implementation is a no-op. Runtimes that support S3-backed
    /// volume sync (e.g., Youki with the `s3` feature) override this.
    async fn sync_container_volumes(&self, _id: &ContainerId) -> Result<()> {
        Ok(())
    }

    /// Stream container logs as raw byte chunks tagged with their channel.
    ///
    /// Mirrors the `GET /containers/{id}/logs` endpoint of the Docker Engine
    /// API: callers can request `follow`, `tail`, `since`/`until` time
    /// windows, per-channel filtering (`stdout` / `stderr`), and inline
    /// timestamps via [`LogsStreamOptions`]. Backends that demultiplex
    /// Docker's stdcopy framing emit one [`LogChunk`] per frame; line-based
    /// runtimes emit one chunk per line.
    ///
    /// The default implementation returns [`AgentError::Unsupported`].
    /// Concrete runtimes override this with backend-specific streaming
    /// (bollard's `logs` for Docker, log-file tailing for Youki/HCS, etc.).
    /// The stream is `'static` so HTTP handlers can drive it independently
    /// of the trait-method borrow.
    async fn logs_stream(&self, _id: &ContainerId, _opts: LogsStreamOptions) -> Result<LogsStream> {
        Err(AgentError::Unsupported(
            "logs_stream is not supported by this runtime".into(),
        ))
    }

    /// Stream periodic resource-usage samples for a container.
    ///
    /// Mirrors the streaming form of `GET /containers/{id}/stats` in the
    /// Docker Engine API: each yielded [`StatsSample`] is one full snapshot
    /// of CPU / memory / network / block-IO / pids counters at the moment
    /// it was taken. Sampling cadence is backend-defined (Docker emits one
    /// sample per second by default).
    ///
    /// The default implementation returns [`AgentError::Unsupported`].
    /// Backends that can produce this data implement it directly: Docker
    /// via bollard's `stats`, Youki by polling cgroup stat files,
    /// `MockRuntime` by emitting a deterministic single sample.
    async fn stats_stream(&self, _id: &ContainerId) -> Result<StatsStream> {
        Err(AgentError::Unsupported(
            "stats_stream is not supported by this runtime".into(),
        ))
    }

    /// Pull an image, streaming progress events as layers are downloaded.
    ///
    /// Mirrors the streaming form of `POST /images/create` in the Docker
    /// Engine API. Backends emit a series of [`PullProgress::Status`]
    /// events for in-flight layers, followed by exactly one
    /// [`PullProgress::Done`] event on success. Errors that occur mid-pull
    /// surface as `Err` items on the stream and terminate it.
    ///
    /// `auth` carries inline credentials for this pull. When `None`, the
    /// runtime falls back to its credential-store lookup keyed by registry
    /// hostname (matching the semantics of [`Runtime::pull_image_with_policy`]).
    ///
    /// The default implementation returns [`AgentError::Unsupported`].
    /// Backends override this with their native streaming pull
    /// (bollard's `create_image` for Docker, `zlayer-registry` for Youki).
    async fn pull_image_stream(
        &self,
        _image: &str,
        _auth: Option<&RegistryAuth>,
    ) -> Result<PullProgressStream> {
        Err(AgentError::Unsupported(
            "pull_image_stream is not supported by this runtime".into(),
        ))
    }

    /// List all images managed by this runtime's image storage.
    ///
    /// The default implementation returns `AgentError::Unsupported` — individual
    /// runtimes override this with backend-specific logic (bollard for Docker,
    /// zlayer-registry cache walk for Youki, etc.).
    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        Err(AgentError::Unsupported(
            "list_images is not supported by this runtime".into(),
        ))
    }

    /// Remove an image by reference from local storage.
    ///
    /// When `force` is true, also removes the image even when other containers
    /// reference it. The default implementation returns `AgentError::Unsupported`.
    async fn remove_image(&self, _image: &str, _force: bool) -> Result<()> {
        Err(AgentError::Unsupported(
            "remove_image is not supported by this runtime".into(),
        ))
    }

    /// Prune dangling / unused images from local storage.
    ///
    /// Returns a [`PruneResult`] describing what was removed. The default
    /// implementation returns `AgentError::Unsupported`.
    async fn prune_images(&self) -> Result<PruneResult> {
        Err(AgentError::Unsupported(
            "prune_images is not supported by this runtime".into(),
        ))
    }

    /// Send a signal to a running container.
    ///
    /// When `signal` is `None`, the runtime sends `SIGKILL` (matching Docker's
    /// `docker kill` default). Backends validate the signal name and reject
    /// anything outside the standard POSIX set (`SIGKILL`, `SIGTERM`, `SIGINT`,
    /// `SIGHUP`, `SIGUSR1`, `SIGUSR2`).
    ///
    /// Used by `POST /api/v1/containers/{id}/kill` and Docker-compat
    /// `docker kill`. The default implementation returns
    /// [`AgentError::Unsupported`].
    async fn kill_container(&self, _id: &ContainerId, _signal: Option<&str>) -> Result<()> {
        Err(AgentError::Unsupported(
            "kill_container is not supported by this runtime".into(),
        ))
    }

    /// Create a new tag pointing at an existing image.
    ///
    /// `source` is the reference to an already-cached image. `target` is the
    /// new reference to create — it must be a full reference (repository + tag).
    ///
    /// Used by `POST /api/v1/images/tag` and Docker-compat `docker tag`. The
    /// default implementation returns [`AgentError::Unsupported`].
    async fn tag_image(&self, _source: &str, _target: &str) -> Result<()> {
        Err(AgentError::Unsupported(
            "tag_image is not supported by this runtime".into(),
        ))
    }

    /// Inspect an image and return a Docker-shaped detail record.
    ///
    /// Mirrors Docker's `GET /images/{name}/json`. Backends translate their
    /// native inspect output into [`ImageInspectInfo`]; the API/Docker
    /// compat shim emits the JSON body. The default implementation returns
    /// [`AgentError::Unsupported`] so non-Docker backends keep compiling.
    async fn inspect_image_native(&self, _image: &str) -> Result<ImageInspectInfo> {
        Err(AgentError::Unsupported(
            "inspect_image_native is not supported by this runtime".into(),
        ))
    }

    /// Return the parent-layer history for an image.
    ///
    /// Mirrors Docker's `GET /images/{name}/history`. The default
    /// implementation returns [`AgentError::Unsupported`].
    async fn image_history(&self, _image: &str) -> Result<Vec<ImageHistoryEntry>> {
        Err(AgentError::Unsupported(
            "image_history is not supported by this runtime".into(),
        ))
    }

    /// Search a registry for images matching `term`.
    ///
    /// Mirrors Docker's `GET /images/search`. `limit` caps the number of
    /// returned items; `0` means "let the registry decide". The default
    /// implementation returns [`AgentError::Unsupported`].
    async fn search_images(&self, _term: &str, _limit: u32) -> Result<Vec<ImageSearchResult>> {
        Err(AgentError::Unsupported(
            "search_images is not supported by this runtime".into(),
        ))
    }

    /// Stream a tar archive containing one or more images.
    ///
    /// Mirrors Docker's `GET /images/get?names=...`. Multi-image archives
    /// dedupe shared layers. The default implementation returns
    /// [`AgentError::Unsupported`].
    async fn save_images(&self, _names: &[String]) -> Result<ImageExportStream> {
        Err(AgentError::Unsupported(
            "save_images is not supported by this runtime".into(),
        ))
    }

    /// Load images from a tar archive.
    ///
    /// Mirrors Docker's `POST /images/load`. `tar_bytes` is the
    /// uncompressed (or gzip-compressed) tar produced by [`Self::save_images`].
    /// `quiet` suppresses progress events when set. The default
    /// implementation returns [`AgentError::Unsupported`].
    async fn load_images(
        &self,
        _tar_bytes: bytes::Bytes,
        _quiet: bool,
    ) -> Result<LoadProgressStream> {
        Err(AgentError::Unsupported(
            "load_images is not supported by this runtime".into(),
        ))
    }

    /// Import a single image from a tar root filesystem.
    ///
    /// Mirrors the `fromSrc=`-mode of Docker's `POST /images/create`.
    /// `tar_bytes` is a tar of the root filesystem; `repo`/`tag` are the
    /// reference to apply to the resulting image. The default
    /// implementation returns [`AgentError::Unsupported`].
    async fn import_image(
        &self,
        _tar_bytes: bytes::Bytes,
        _repo: Option<&str>,
        _tag: Option<&str>,
    ) -> Result<String> {
        Err(AgentError::Unsupported(
            "import_image is not supported by this runtime".into(),
        ))
    }

    /// Stream a tar archive of the container's filesystem.
    ///
    /// Mirrors Docker's `GET /containers/{id}/export`. The default
    /// implementation returns [`AgentError::Unsupported`].
    async fn export_container_fs(&self, _id: &ContainerId) -> Result<ImageExportStream> {
        Err(AgentError::Unsupported(
            "export_container_fs is not supported by this runtime".into(),
        ))
    }

    /// Commit a container's filesystem state to a new image.
    ///
    /// Mirrors Docker's `POST /commit?container=...`. `opts` carries the
    /// optional repo/tag/comment/author/pause/changes parameters. The
    /// default implementation returns [`AgentError::Unsupported`].
    async fn commit_container(
        &self,
        _id: &ContainerId,
        _opts: &CommitOptions,
    ) -> Result<CommitOutcome> {
        Err(AgentError::Unsupported(
            "commit_container is not supported by this runtime".into(),
        ))
    }

    /// Return rich inspect details for a container: published ports, attached
    /// networks, first IPv4, health, and most-recent exit code.
    ///
    /// Runtimes implement this by translating the backend's native inspect
    /// response (bollard's `ContainerInspectResponse` for Docker) into the
    /// runtime-level [`ContainerInspectDetails`] struct. The API layer merges
    /// these fields into `ContainerInfo` on `GET /api/v1/containers` and
    /// `GET /api/v1/containers/{id}` (§3.15 of `ZLAYER_SDK_FIXES.md`).
    ///
    /// The default implementation returns [`ContainerInspectDetails::default`]
    /// — an all-empty record, which the API layer treats as "this runtime
    /// doesn't support rich inspect; skip all the extra fields". This keeps
    /// non-Docker runtimes (Youki, WASM, Mock) backwards compatible; they can
    /// override this later if they gain equivalent inspect capability.
    async fn inspect_detailed(&self, _id: &ContainerId) -> Result<ContainerInspectDetails> {
        Ok(ContainerInspectDetails::default())
    }

    /// Pause all processes in the container by freezing its cgroup.
    ///
    /// Mirrors Docker's `POST /containers/{id}/pause`. After pause, the
    /// container's processes are suspended in the kernel via the cgroup
    /// freezer; calls to [`Runtime::container_state`] still report
    /// `Running` but no instructions execute until [`Runtime::unpause_container`].
    ///
    /// The default implementation returns [`AgentError::Unsupported`].
    /// Backends override this with their native pause API (bollard's
    /// `pause_container` for Docker, libcontainer's `Container::pause` for
    /// Youki).
    async fn pause_container(&self, _id: &ContainerId) -> Result<()> {
        Err(AgentError::Unsupported(
            "pause_container is not supported by this runtime".into(),
        ))
    }

    /// Resume a previously-paused container by thawing its cgroup freezer.
    ///
    /// Mirrors Docker's `POST /containers/{id}/unpause`. The default
    /// implementation returns [`AgentError::Unsupported`].
    async fn unpause_container(&self, _id: &ContainerId) -> Result<()> {
        Err(AgentError::Unsupported(
            "unpause_container is not supported by this runtime".into(),
        ))
    }

    /// Update a running container's resource limits and restart policy.
    ///
    /// Mirrors Docker's `POST /containers/{id}/update`. The fields on
    /// [`ContainerResourceUpdate`] are individually optional: backends
    /// apply only the fields that are `Some` and leave the rest of the
    /// container's runtime configuration untouched. A fully-empty update
    /// short-circuits to a no-op (no cgroup writes, no warnings).
    ///
    /// Returns a [`ContainerUpdateOutcome`] whose `warnings` vector
    /// surfaces non-fatal issues (e.g. "kernel memory limit is
    /// deprecated", or "real-time scheduling not supported on this
    /// kernel"). Empty `warnings` ⇒ every requested field was applied.
    ///
    /// The default implementation returns [`AgentError::Unsupported`].
    /// Backends override this:
    ///
    /// * Docker — calls bollard's `update_container` with a
    ///   `ContainerUpdateBody` populated from the input.
    /// * Youki — writes the corresponding cgroup v2 files
    ///   (`cpu.weight`, `memory.max`, `pids.max`, `io.weight`,
    ///   `cpuset.cpus`, `cpuset.mems`) under
    ///   `<container_root>/cgroup` and persists the new restart policy
    ///   in the on-disk supervisor state.
    /// * Other backends (WASM, mocks) inherit the `Unsupported` default.
    async fn update_container_resources(
        &self,
        _id: &ContainerId,
        _update: &ContainerResourceUpdate,
    ) -> Result<ContainerUpdateOutcome> {
        Err(AgentError::Unsupported(
            "update_container_resources is not supported by this runtime".into(),
        ))
    }

    /// List the processes running inside a container (`docker top`).
    ///
    /// `ps_args` is forwarded to the runtime as the `ps(1)` argument list when
    /// supported; an empty slice means "use the runtime's default columns".
    /// Mirrors Docker's `GET /containers/{id}/top?ps_args=<...>`.
    ///
    /// The default implementation returns [`AgentError::Unsupported`].
    async fn top_container(
        &self,
        _id: &ContainerId,
        _ps_args: &[String],
    ) -> Result<ContainerTopOutput> {
        Err(AgentError::Unsupported(
            "top_container is not supported by this runtime".into(),
        ))
    }

    /// Report changes to a container's filesystem since it was created.
    ///
    /// Mirrors Docker's `GET /containers/{id}/changes`. Returns one
    /// [`FilesystemChangeEntry`] per added / modified / deleted path in the
    /// container's writable layer. Runtimes that don't compute layer diffs
    /// (e.g. youki, which uses raw bundle rootfs without a layered FS) return
    /// [`AgentError::Unsupported`].
    async fn changes_container(&self, _id: &ContainerId) -> Result<Vec<FilesystemChangeEntry>> {
        Err(AgentError::Unsupported(
            "changes_container is not supported by this runtime".into(),
        ))
    }

    /// Report the published port mappings for a container.
    ///
    /// Mirrors Docker's `GET /containers/{id}/port`. Returns one
    /// [`PortMappingEntry`] per (container-port, protocol, host-binding)
    /// triple. Containers with no published ports return an empty vector.
    ///
    /// The default implementation returns [`AgentError::Unsupported`].
    async fn port_mappings_container(&self, _id: &ContainerId) -> Result<Vec<PortMappingEntry>> {
        Err(AgentError::Unsupported(
            "port_mappings_container is not supported by this runtime".into(),
        ))
    }

    /// Prune stopped containers from the runtime.
    ///
    /// Mirrors Docker's `POST /containers/prune`. Returns the IDs of
    /// containers that were removed plus the bytes reclaimed. The default
    /// implementation returns [`AgentError::Unsupported`].
    async fn prune_containers(&self) -> Result<ContainerPruneResult> {
        Err(AgentError::Unsupported(
            "prune_containers is not supported by this runtime".into(),
        ))
    }

    /// Enumerate all containers known to this runtime, including stopped /
    /// exited ones (Docker's `list_containers(all=true)` semantics).
    ///
    /// Used by `zlayer-api::handlers::standalone_reconcile` on daemon boot
    /// to match persisted standalone-container records against the
    /// runtime's actual inventory: entries the runtime no longer reports
    /// are pruned, surviving entries are re-registered in the
    /// `ContainerIdMap`, and runtime containers carrying a
    /// `com.zlayer.container_id` label that has no storage match are
    /// counted as orphans (logged but otherwise left alone).
    ///
    /// The default implementation returns an empty list, which makes
    /// reconcile degrade to a label-blind pass: storage entries can still
    /// be probed individually via [`Runtime::container_state`], but orphan
    /// detection is disabled until the backend overrides this method
    /// (Docker via `bollard::list_containers`, youki via state-dir walk).
    async fn list_containers(&self) -> Result<Vec<RuntimeContainerSummary>> {
        Ok(Vec::new())
    }

    /// Stream a TAR archive of the file or directory at `path` inside the
    /// container.
    ///
    /// Mirrors Docker's `GET /containers/{id}/archive?path=<...>`. The
    /// returned [`ArchiveStream`] yields raw `application/x-tar` bytes.
    /// Backends decide whether to materialize the archive in memory or
    /// stream it on the fly:
    ///
    /// * Docker — bollard's `download_from_container` produces a chunked
    ///   stream of TAR bytes; we forward it verbatim.
    /// * Youki — a rootfs walk under `<bundle>/rootfs<path>` produces a
    ///   TAR archive in a worker task and streams it through an mpsc.
    /// * Other backends (WASM, mocks) inherit the `Unsupported` default.
    ///
    /// The default implementation returns [`AgentError::Unsupported`].
    async fn archive_get(&self, _id: &ContainerId, _path: &str) -> Result<ArchiveStream> {
        Err(AgentError::Unsupported(
            "archive_get is not supported by this runtime".into(),
        ))
    }

    /// Extract a TAR archive into the container at `path`.
    ///
    /// Mirrors Docker's `PUT /containers/{id}/archive?path=<...>`. The
    /// runtime must extract `tar_bytes` (an uncompressed TAR archive) into
    /// `path` inside the container, honouring [`ArchivePutOptions`].
    ///
    /// `path` must already exist inside the container and must be a
    /// directory; if it does not exist the runtime returns
    /// [`AgentError::NotFound`]. Mismatched directory/non-directory
    /// replacements with `no_overwrite_dir_non_dir=true` return
    /// [`AgentError::InvalidSpec`].
    ///
    /// The default implementation returns [`AgentError::Unsupported`].
    async fn archive_put(
        &self,
        _id: &ContainerId,
        _path: &str,
        _tar_bytes: bytes::Bytes,
        _opts: ArchivePutOptions,
    ) -> Result<()> {
        Err(AgentError::Unsupported(
            "archive_put is not supported by this runtime".into(),
        ))
    }

    /// Return stat metadata for the file or directory at `path` inside the
    /// container.
    ///
    /// Mirrors Docker's `HEAD /containers/{id}/archive?path=<...>`, which
    /// answers with the metadata that `GET /archive` *would* expose without
    /// materializing the TAR. Used by `docker cp` and the API layer to
    /// short-circuit on missing paths.
    ///
    /// The default implementation returns [`AgentError::Unsupported`].
    async fn archive_head(&self, _id: &ContainerId, _path: &str) -> Result<PathStat> {
        Err(AgentError::Unsupported(
            "archive_head is not supported by this runtime".into(),
        ))
    }
}

/// Validate a signal name for [`Runtime::kill_container`].
///
/// Accepts both the `SIG`-prefixed form (`"SIGKILL"`) and the bare form
/// (`"KILL"`). Returns the canonical uppercase `SIG`-prefixed name on success.
///
/// # Errors
///
/// Returns [`AgentError::InvalidSpec`] when `signal` is not one of the
/// supported signals: `SIGKILL`, `SIGTERM`, `SIGINT`, `SIGHUP`, `SIGUSR1`,
/// `SIGUSR2`.
pub fn validate_signal(signal: &str) -> Result<String> {
    let trimmed = signal.trim();
    if trimmed.is_empty() {
        return Err(AgentError::InvalidSpec(
            "signal must not be empty".to_string(),
        ));
    }
    let upper = trimmed.to_ascii_uppercase();
    let canonical = if upper.starts_with("SIG") {
        upper
    } else {
        format!("SIG{upper}")
    };
    match canonical.as_str() {
        "SIGKILL" | "SIGTERM" | "SIGINT" | "SIGHUP" | "SIGUSR1" | "SIGUSR2" => Ok(canonical),
        other => Err(AgentError::InvalidSpec(format!(
            "unsupported signal '{other}'; allowed: SIGKILL, SIGTERM, SIGINT, SIGHUP, SIGUSR1, SIGUSR2"
        ))),
    }
}

/// Auth context injected into every container so it can talk back to the host
/// API without needing external credentials.
#[derive(Debug, Clone)]
pub struct ContainerAuthContext {
    /// Base URL of the `ZLayer` API, e.g. `"http://127.0.0.1:3669"`.
    pub api_url: String,
    /// JWT signing secret — used to mint per-container tokens at start time.
    pub jwt_secret: String,
    /// Absolute path of the Unix socket on the host (bind-mounted into Linux
    /// containers; added to `writable_dirs` for macOS sandbox).
    pub socket_path: String,
}

/// In-memory mock runtime for testing and development.
///
/// In addition to tracking container lifecycle in memory, the mock exposes
/// per-container queues for streaming method outputs so unit tests can
/// pre-script the events that [`Runtime::logs_stream`],
/// [`Runtime::stats_stream`], and [`Runtime::pull_image_stream`] should
/// yield. See [`MockRuntime::enqueue_log_chunk`],
/// [`MockRuntime::enqueue_stats_sample`], and
/// [`MockRuntime::enqueue_pull_progress`].
pub struct MockRuntime {
    containers: tokio::sync::RwLock<std::collections::HashMap<ContainerId, Container>>,
    /// Pre-scripted log chunks per container. Each call to
    /// [`Runtime::logs_stream`] drains this queue in order; once empty, the
    /// stream either terminates (when `follow=false`) or hangs forever
    /// (when `follow=true`) so tests can exercise both branches.
    pub logs_to_yield:
        Arc<Mutex<std::collections::HashMap<ContainerId, VecDeque<Result<LogChunk>>>>>,
    /// Pre-scripted stats samples per container. Drained in order by
    /// [`Runtime::stats_stream`].
    pub stats_to_yield:
        Arc<Mutex<std::collections::HashMap<ContainerId, VecDeque<Result<StatsSample>>>>>,
    /// Pre-scripted pull progress events keyed by image reference. Drained
    /// in order by [`Runtime::pull_image_stream`].
    pub pull_progress_to_yield:
        Arc<Mutex<std::collections::HashMap<String, VecDeque<Result<PullProgress>>>>>,
}

impl MockRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self {
            containers: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            logs_to_yield: Arc::new(Mutex::new(std::collections::HashMap::new())),
            stats_to_yield: Arc::new(Mutex::new(std::collections::HashMap::new())),
            pull_progress_to_yield: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Push a single [`LogChunk`] onto the queue for `id`. Subsequent calls to
    /// [`Runtime::logs_stream`] will yield enqueued chunks in FIFO order.
    pub async fn enqueue_log_chunk(&self, id: &ContainerId, chunk: LogChunk) {
        self.logs_to_yield
            .lock()
            .await
            .entry(id.clone())
            .or_default()
            .push_back(Ok(chunk));
    }

    /// Push a pre-built error onto the log queue for `id`. The next
    /// [`Runtime::logs_stream`] call drains this as the next yielded item.
    pub async fn enqueue_log_error(&self, id: &ContainerId, err: AgentError) {
        self.logs_to_yield
            .lock()
            .await
            .entry(id.clone())
            .or_default()
            .push_back(Err(err));
    }

    /// Push a single [`StatsSample`] onto the queue for `id`. Subsequent calls
    /// to [`Runtime::stats_stream`] will yield enqueued samples in FIFO order.
    pub async fn enqueue_stats_sample(&self, id: &ContainerId, sample: StatsSample) {
        self.stats_to_yield
            .lock()
            .await
            .entry(id.clone())
            .or_default()
            .push_back(Ok(sample));
    }

    /// Push a single [`PullProgress`] event onto the queue for `image`.
    /// Subsequent calls to [`Runtime::pull_image_stream`] for the same image
    /// reference will yield enqueued events in FIFO order.
    pub async fn enqueue_pull_progress(&self, image: &str, progress: PullProgress) {
        self.pull_progress_to_yield
            .lock()
            .await
            .entry(image.to_string())
            .or_default()
            .push_back(Ok(progress));
    }
}

impl Default for MockRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Runtime for MockRuntime {
    async fn pull_image(&self, _image: &str) -> Result<()> {
        self.pull_image_with_policy(_image, PullPolicy::IfNotPresent, None)
            .await
    }

    async fn pull_image_with_policy(
        &self,
        _image: &str,
        _policy: PullPolicy,
        _auth: Option<&RegistryAuth>,
    ) -> Result<()> {
        // Mock: always succeeds
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    }

    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let mut containers = self.containers.write().await;
        containers.insert(
            id.clone(),
            Container {
                id: id.clone(),
                image: spec.image.name.to_string(),
                state: ContainerState::Pending,
                pid: None,
                task: None,
                overlay_ip: None,
                health_monitor: None,
                port_override: None,
            },
        );
        Ok(())
    }

    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let mut containers = self.containers.write().await;
        if let Some(container) = containers.get_mut(id) {
            container.state = ContainerState::Running;
            container.pid = Some(std::process::id()); // Mock PID
        }
        Ok(())
    }

    async fn stop_container(&self, id: &ContainerId, _timeout: Duration) -> Result<()> {
        let mut containers = self.containers.write().await;
        if let Some(container) = containers.get_mut(id) {
            container.state = ContainerState::Exited { code: 0 };
        }
        Ok(())
    }

    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let mut containers = self.containers.write().await;
        containers.remove(id);
        Ok(())
    }

    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let containers = self.containers.read().await;
        containers
            .get(id)
            .map(|c| c.state.clone())
            .ok_or_else(|| AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
    }

    async fn container_logs(&self, _id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>> {
        let entries = vec![
            LogEntry {
                timestamp: chrono::Utc::now(),
                stream: LogStream::Stdout,
                message: "mock log line 1".to_string(),
                source: LogSource::Container("mock".to_string()),
                service: None,
                deployment: None,
            },
            LogEntry {
                timestamp: chrono::Utc::now(),
                stream: LogStream::Stderr,
                message: "mock error line".to_string(),
                source: LogSource::Container("mock".to_string()),
                service: None,
                deployment: None,
            },
        ];
        let skip = entries.len().saturating_sub(tail);
        Ok(entries.into_iter().skip(skip).collect())
    }

    async fn exec(&self, _id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        Ok((0, cmd.join(" "), String::new()))
    }

    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        // Mock: return dummy stats
        let containers = self.containers.read().await;
        if containers.contains_key(id) {
            Ok(ContainerStats {
                cpu_usage_usec: 1_000_000,       // 1 second
                memory_bytes: 50 * 1024 * 1024,  // 50 MB
                memory_limit: 256 * 1024 * 1024, // 256 MB
                timestamp: std::time::Instant::now(),
            })
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        // Mock: simulate waiting for container to exit
        let containers = self.containers.read().await;
        if let Some(container) = containers.get(id) {
            match &container.state {
                ContainerState::Exited { code } => Ok(*code),
                ContainerState::Failed { .. } => Ok(1),
                _ => {
                    // Simulate a brief wait and then return success
                    drop(containers);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(0)
                }
            }
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
        // Mock: return dummy structured log entries
        let containers = self.containers.read().await;
        if containers.contains_key(id) {
            let container_name = id.to_string();
            Ok(vec![
                LogEntry {
                    timestamp: chrono::Utc::now(),
                    stream: LogStream::Stdout,
                    message: format!("[{container_name}] Container started"),
                    source: LogSource::Container(container_name.clone()),
                    service: None,
                    deployment: None,
                },
                LogEntry {
                    timestamp: chrono::Utc::now(),
                    stream: LogStream::Stdout,
                    message: format!("[{container_name}] Executing command..."),
                    source: LogSource::Container(container_name.clone()),
                    service: None,
                    deployment: None,
                },
                LogEntry {
                    timestamp: chrono::Utc::now(),
                    stream: LogStream::Stdout,
                    message: format!("[{container_name}] Command completed successfully"),
                    source: LogSource::Container(container_name),
                    service: None,
                    deployment: None,
                },
            ])
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        let containers = self.containers.read().await;
        if let Some(container) = containers.get(id) {
            Ok(container.pid)
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        let containers = self.containers.read().await;
        if containers.contains_key(id) {
            // Mock: deterministic IP based on replica number (172.17.0.{replica+2})
            #[allow(clippy::cast_possible_truncation)]
            let last_octet = (id.replica + 2) as u8;
            Ok(Some(IpAddr::V4(std::net::Ipv4Addr::new(
                172, 17, 0, last_octet,
            ))))
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        Ok(Vec::new())
    }

    async fn remove_image(&self, _image: &str, _force: bool) -> Result<()> {
        Ok(())
    }

    async fn prune_images(&self) -> Result<PruneResult> {
        Ok(PruneResult::default())
    }

    async fn kill_container(&self, id: &ContainerId, signal: Option<&str>) -> Result<()> {
        // Validate signal even in the mock so callers exercise the same error
        // path. Default to SIGKILL when omitted.
        let _canonical = validate_signal(signal.unwrap_or("SIGKILL"))?;
        let mut containers = self.containers.write().await;
        let container = containers.get_mut(id).ok_or_else(|| AgentError::NotFound {
            container: id.to_string(),
            reason: "container not found".to_string(),
        })?;
        container.state = ContainerState::Exited { code: 137 };
        Ok(())
    }

    async fn tag_image(&self, _source: &str, _target: &str) -> Result<()> {
        // The in-memory mock doesn't store images; treat tag as a no-op success.
        Ok(())
    }

    async fn logs_stream(&self, id: &ContainerId, opts: LogsStreamOptions) -> Result<LogsStream> {
        use futures_util::StreamExt;

        // Drain the per-container queue once at call time; the resulting
        // iterator is owned by the stream so the lock isn't held while the
        // consumer reads.
        let queued: Vec<Result<LogChunk>> = {
            let mut guard = self.logs_to_yield.lock().await;
            guard.remove(id).map(Vec::from).unwrap_or_default()
        };
        let head = futures_util::stream::iter(queued);
        if opts.follow {
            // After the queued items are drained, hang forever so callers can
            // exercise the "still-following, no more data" branch and cancel
            // by dropping the stream.
            let tail = futures_util::stream::pending::<Result<LogChunk>>();
            Ok(Box::pin(head.chain(tail)))
        } else {
            Ok(Box::pin(head))
        }
    }

    async fn stats_stream(&self, id: &ContainerId) -> Result<StatsStream> {
        let queued: Vec<Result<StatsSample>> = {
            let mut guard = self.stats_to_yield.lock().await;
            guard.remove(id).map(Vec::from).unwrap_or_default()
        };
        // `stats_stream` has no `follow` flag on the trait. The mock yields
        // exactly what was pre-loaded and then closes — tests that want a
        // forever-pending stream can simulate it by holding the receiver and
        // never enqueueing more. Closing on drain keeps tests bounded so a
        // forgotten consumer never deadlocks the test runner.
        Ok(Box::pin(futures_util::stream::iter(queued)))
    }

    async fn pull_image_stream(
        &self,
        image: &str,
        _auth: Option<&RegistryAuth>,
    ) -> Result<PullProgressStream> {
        let queued: Vec<Result<PullProgress>> = {
            let mut guard = self.pull_progress_to_yield.lock().await;
            guard.remove(image).map(Vec::from).unwrap_or_default()
        };
        // Pulls are inherently bounded: the real backends emit a final `Done`
        // event and close the stream. The mock follows the same shape — it
        // just yields whatever the test pre-loaded and ends.
        Ok(Box::pin(futures_util::stream::iter(queued)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_runtime() {
        let runtime = MockRuntime::new();
        let id = ContainerId::new("test".to_string(), 1);

        runtime.pull_image("test:latest").await.unwrap();
        runtime.create_container(&id, &mock_spec()).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        let state = runtime.container_state(&id).await.unwrap();
        assert_eq!(state, ContainerState::Running);
    }

    #[test]
    fn validate_signal_accepts_known_signals() {
        // SIG-prefixed form
        assert_eq!(validate_signal("SIGKILL").unwrap(), "SIGKILL");
        assert_eq!(validate_signal("SIGTERM").unwrap(), "SIGTERM");
        assert_eq!(validate_signal("SIGINT").unwrap(), "SIGINT");
        assert_eq!(validate_signal("SIGHUP").unwrap(), "SIGHUP");
        assert_eq!(validate_signal("SIGUSR1").unwrap(), "SIGUSR1");
        assert_eq!(validate_signal("SIGUSR2").unwrap(), "SIGUSR2");

        // Bare form (no "SIG" prefix) should be canonicalised.
        assert_eq!(validate_signal("KILL").unwrap(), "SIGKILL");
        assert_eq!(validate_signal("term").unwrap(), "SIGTERM");
        // Whitespace around the name is tolerated.
        assert_eq!(validate_signal(" INT ").unwrap(), "SIGINT");
    }

    #[test]
    fn validate_signal_rejects_unknown_or_empty() {
        assert!(matches!(
            validate_signal(""),
            Err(AgentError::InvalidSpec(_))
        ));
        assert!(matches!(
            validate_signal("   "),
            Err(AgentError::InvalidSpec(_))
        ));
        assert!(matches!(
            validate_signal("SIGSEGV"),
            Err(AgentError::InvalidSpec(_))
        ));
        assert!(matches!(
            validate_signal("NOPE"),
            Err(AgentError::InvalidSpec(_))
        ));
        // Signals outside the POSIX allowlist are rejected even if real.
        assert!(matches!(
            validate_signal("SIGPIPE"),
            Err(AgentError::InvalidSpec(_))
        ));
    }

    #[tokio::test]
    async fn mock_kill_container_defaults_to_sigkill() {
        let runtime = MockRuntime::new();
        let id = ContainerId::new("kill-me".to_string(), 0);
        runtime.create_container(&id, &mock_spec()).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // `None` -> defaults to SIGKILL; returns Ok and marks the container
        // as exited.
        runtime.kill_container(&id, None).await.unwrap();
        let state = runtime.container_state(&id).await.unwrap();
        assert!(
            matches!(state, ContainerState::Exited { code: 137 }),
            "expected Exited(137), got {state:?}"
        );
    }

    #[test]
    fn wait_reason_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&WaitReason::Exited).unwrap(),
            "\"exited\""
        );
        assert_eq!(
            serde_json::to_string(&WaitReason::Signal).unwrap(),
            "\"signal\""
        );
        assert_eq!(
            serde_json::to_string(&WaitReason::OomKilled).unwrap(),
            "\"oom_killed\""
        );
        assert_eq!(
            serde_json::to_string(&WaitReason::RuntimeError).unwrap(),
            "\"runtime_error\""
        );
    }

    #[test]
    fn wait_reason_deserialize_roundtrip() {
        for variant in [
            WaitReason::Exited,
            WaitReason::Signal,
            WaitReason::OomKilled,
            WaitReason::RuntimeError,
        ] {
            let s = serde_json::to_string(&variant).unwrap();
            let back: WaitReason = serde_json::from_str(&s).unwrap();
            assert_eq!(variant, back, "roundtrip failed for {variant:?}");
        }
    }

    #[test]
    fn signal_name_from_exit_code_known_signals() {
        assert_eq!(signal_name_from_exit_code(137).as_deref(), Some("SIGKILL"));
        assert_eq!(signal_name_from_exit_code(143).as_deref(), Some("SIGTERM"));
        assert_eq!(signal_name_from_exit_code(130).as_deref(), Some("SIGINT"));
        assert_eq!(signal_name_from_exit_code(129).as_deref(), Some("SIGHUP"));
        assert_eq!(signal_name_from_exit_code(139).as_deref(), Some("SIGSEGV"));
    }

    #[test]
    fn signal_name_from_exit_code_handles_unknown_and_normal() {
        // Normal exits (<= 128) return None.
        assert_eq!(signal_name_from_exit_code(0), None);
        assert_eq!(signal_name_from_exit_code(1), None);
        assert_eq!(signal_name_from_exit_code(128), None);

        // Unknown signals produce a stable string form.
        assert_eq!(
            signal_name_from_exit_code(128 + 99).as_deref(),
            Some("signal_99")
        );
    }

    #[tokio::test]
    async fn default_wait_outcome_delegates_to_wait_container() {
        let runtime = MockRuntime::new();
        let id = ContainerId::new("wait-test".to_string(), 0);
        runtime.create_container(&id, &mock_spec()).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        let outcome = runtime.wait_outcome(&id).await.unwrap();
        // MockRuntime::wait_container returns 0 for running containers.
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.reason, WaitReason::Exited);
        assert!(outcome.signal.is_none());
        assert!(outcome.finished_at.is_none());
    }

    #[tokio::test]
    async fn mock_kill_container_rejects_bogus_signal() {
        let runtime = MockRuntime::new();
        let id = ContainerId::new("kill-me".to_string(), 0);
        runtime.create_container(&id, &mock_spec()).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        let err = runtime
            .kill_container(&id, Some("SIGFOO"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, AgentError::InvalidSpec(_)),
            "expected InvalidSpec, got {err:?}"
        );
    }

    // The default trait impls of `logs_stream`, `stats_stream`, and
    // `pull_image_stream` still return `AgentError::Unsupported`. `MockRuntime`
    // overrides all three so tests can pre-script stream output (see
    // `mock_logs_stream_yields_queued_items_in_order` and friends below).
    // A trivial `BareRuntime` exercises the default trait impls without
    // dragging in MockRuntime's overrides.

    /// Minimal `Runtime` implementation used to exercise the default trait
    /// impls of `logs_stream` / `stats_stream` / `pull_image_stream`. Every
    /// non-default method panics — the bare runtime is only ever called for
    /// the three streaming methods the tests care about.
    struct BareRuntime;

    #[async_trait::async_trait]
    impl Runtime for BareRuntime {
        async fn pull_image(&self, _image: &str) -> Result<()> {
            unimplemented!()
        }
        async fn pull_image_with_policy(
            &self,
            _image: &str,
            _policy: PullPolicy,
            _auth: Option<&RegistryAuth>,
        ) -> Result<()> {
            unimplemented!()
        }
        async fn create_container(&self, _id: &ContainerId, _spec: &ServiceSpec) -> Result<()> {
            unimplemented!()
        }
        async fn start_container(&self, _id: &ContainerId) -> Result<()> {
            unimplemented!()
        }
        async fn stop_container(&self, _id: &ContainerId, _timeout: Duration) -> Result<()> {
            unimplemented!()
        }
        async fn remove_container(&self, _id: &ContainerId) -> Result<()> {
            unimplemented!()
        }
        async fn container_state(&self, _id: &ContainerId) -> Result<ContainerState> {
            unimplemented!()
        }
        async fn container_logs(&self, _id: &ContainerId, _tail: usize) -> Result<Vec<LogEntry>> {
            unimplemented!()
        }
        async fn exec(&self, _id: &ContainerId, _cmd: &[String]) -> Result<(i32, String, String)> {
            unimplemented!()
        }
        async fn get_container_stats(&self, _id: &ContainerId) -> Result<ContainerStats> {
            unimplemented!()
        }
        async fn wait_container(&self, _id: &ContainerId) -> Result<i32> {
            unimplemented!()
        }
        async fn get_logs(&self, _id: &ContainerId) -> Result<Vec<LogEntry>> {
            unimplemented!()
        }
        async fn get_container_pid(&self, _id: &ContainerId) -> Result<Option<u32>> {
            unimplemented!()
        }
        async fn get_container_ip(&self, _id: &ContainerId) -> Result<Option<IpAddr>> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn default_logs_stream_is_unsupported() {
        let runtime = BareRuntime;
        let id = ContainerId::new("stream-test".to_string(), 0);
        // The success-side `LogsStream` is not `Debug`, so we can't call
        // `unwrap_err`; pattern-match on the Result directly instead.
        match runtime.logs_stream(&id, LogsStreamOptions::default()).await {
            Err(AgentError::Unsupported(_)) => {}
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(_) => panic!("expected Err(Unsupported), got Ok"),
        }
    }

    #[tokio::test]
    async fn default_stats_stream_is_unsupported() {
        let runtime = BareRuntime;
        let id = ContainerId::new("stream-test".to_string(), 0);
        match runtime.stats_stream(&id).await {
            Err(AgentError::Unsupported(_)) => {}
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(_) => panic!("expected Err(Unsupported), got Ok"),
        }
    }

    #[tokio::test]
    async fn default_pull_image_stream_is_unsupported() {
        let runtime = BareRuntime;
        match runtime.pull_image_stream("alpine:latest", None).await {
            Err(AgentError::Unsupported(_)) => {}
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(_) => panic!("expected Err(Unsupported), got Ok"),
        }
    }

    #[tokio::test]
    async fn default_archive_get_is_unsupported() {
        let runtime = BareRuntime;
        let id = ContainerId::new("archive-test".to_string(), 0);
        match runtime.archive_get(&id, "/etc/hosts").await {
            Err(AgentError::Unsupported(_)) => {}
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(_) => panic!("expected Err(Unsupported), got Ok"),
        }
    }

    #[tokio::test]
    async fn default_archive_put_is_unsupported() {
        let runtime = BareRuntime;
        let id = ContainerId::new("archive-test".to_string(), 0);
        let err = runtime
            .archive_put(
                &id,
                "/tmp",
                bytes::Bytes::from_static(&[]),
                ArchivePutOptions::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::Unsupported(_)));
    }

    #[tokio::test]
    async fn default_archive_head_is_unsupported() {
        let runtime = BareRuntime;
        let id = ContainerId::new("archive-test".to_string(), 0);
        let err = runtime.archive_head(&id, "/etc/hosts").await.unwrap_err();
        assert!(matches!(err, AgentError::Unsupported(_)));
    }

    #[tokio::test]
    async fn default_exec_pty_is_unsupported() {
        let runtime = BareRuntime;
        let id = ContainerId::new("exec-pty".to_string(), 0);
        // The success-side `ExecHandle` is not `Debug`, so we can't call
        // `unwrap_err`; pattern-match on the Result directly instead.
        match runtime.exec_pty(&id, ExecOptions::default()).await {
            Err(AgentError::Unsupported(_)) => {}
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(_) => panic!("expected Err(Unsupported), got Ok"),
        }
    }

    #[tokio::test]
    async fn default_inspect_image_native_is_unsupported() {
        let runtime = BareRuntime;
        let err = runtime.inspect_image_native("alpine").await.unwrap_err();
        assert!(matches!(err, AgentError::Unsupported(_)));
    }

    #[tokio::test]
    async fn default_image_history_is_unsupported() {
        let runtime = BareRuntime;
        let err = runtime.image_history("alpine").await.unwrap_err();
        assert!(matches!(err, AgentError::Unsupported(_)));
    }

    #[tokio::test]
    async fn default_search_images_is_unsupported() {
        let runtime = BareRuntime;
        let err = runtime.search_images("nginx", 10).await.unwrap_err();
        assert!(matches!(err, AgentError::Unsupported(_)));
    }

    #[tokio::test]
    async fn default_save_images_is_unsupported() {
        let runtime = BareRuntime;
        // The success-side stream isn't `Debug`, so pattern-match the Result.
        match runtime.save_images(&["alpine".to_string()]).await {
            Err(AgentError::Unsupported(_)) => {}
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(_) => panic!("expected Err(Unsupported), got Ok"),
        }
    }

    #[tokio::test]
    async fn default_load_images_is_unsupported() {
        let runtime = BareRuntime;
        match runtime
            .load_images(bytes::Bytes::from_static(&[]), false)
            .await
        {
            Err(AgentError::Unsupported(_)) => {}
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(_) => panic!("expected Err(Unsupported), got Ok"),
        }
    }

    #[tokio::test]
    async fn default_import_image_is_unsupported() {
        let runtime = BareRuntime;
        let err = runtime
            .import_image(bytes::Bytes::from_static(&[]), None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::Unsupported(_)));
    }

    #[tokio::test]
    async fn default_export_container_fs_is_unsupported() {
        let runtime = BareRuntime;
        let id = ContainerId::new("export".to_string(), 0);
        match runtime.export_container_fs(&id).await {
            Err(AgentError::Unsupported(_)) => {}
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(_) => panic!("expected Err(Unsupported), got Ok"),
        }
    }

    #[tokio::test]
    async fn default_commit_container_is_unsupported() {
        let runtime = BareRuntime;
        let id = ContainerId::new("commit".to_string(), 0);
        let err = runtime
            .commit_container(&id, &CommitOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::Unsupported(_)));
    }

    #[test]
    fn load_progress_serializes_with_kind_discriminator() {
        let status = LoadProgress::Status {
            id: Some("abc".to_string()),
            status: "Loading layer".to_string(),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["kind"], "status");
        assert_eq!(json["status"], "Loading layer");

        let done = LoadProgress::Done {
            references: vec!["alpine:latest".to_string()],
        };
        let json = serde_json::to_value(&done).unwrap();
        assert_eq!(json["kind"], "done");
        assert_eq!(json["references"], serde_json::json!(["alpine:latest"]));
    }

    #[test]
    fn commit_options_default_is_no_op_pause_false() {
        let opts = CommitOptions::default();
        assert!(opts.repo.is_none());
        assert!(opts.tag.is_none());
        assert!(opts.comment.is_none());
        assert!(opts.author.is_none());
        assert!(!opts.pause);
        assert!(opts.changes.is_none());
    }

    #[test]
    fn image_inspect_info_default_round_trips_via_serde() {
        let info = ImageInspectInfo::default();
        let json = serde_json::to_string(&info).unwrap();
        let back: ImageInspectInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
    }

    #[tokio::test]
    async fn mock_logs_stream_yields_queued_items_in_order() {
        use futures_util::StreamExt;

        let runtime = MockRuntime::new();
        let id = ContainerId::new("logs-order".to_string(), 0);

        let make_chunk = |s: &str, ch: LogChannel| LogChunk {
            stream: ch,
            bytes: bytes::Bytes::copy_from_slice(s.as_bytes()),
            timestamp: None,
        };

        runtime
            .enqueue_log_chunk(&id, make_chunk("first", LogChannel::Stdout))
            .await;
        runtime
            .enqueue_log_chunk(&id, make_chunk("second", LogChannel::Stderr))
            .await;
        runtime
            .enqueue_log_chunk(&id, make_chunk("third", LogChannel::Stdout))
            .await;

        // `follow=false` so the stream ends once the queue is drained.
        let opts = LogsStreamOptions {
            follow: false,
            ..LogsStreamOptions::default()
        };
        let mut stream = runtime.logs_stream(&id, opts).await.unwrap();

        let mut got = Vec::new();
        while let Some(item) = stream.next().await {
            let chunk = item.unwrap();
            got.push((
                chunk.stream,
                String::from_utf8(chunk.bytes.to_vec()).unwrap(),
            ));
        }
        assert_eq!(
            got,
            vec![
                (LogChannel::Stdout, "first".to_string()),
                (LogChannel::Stderr, "second".to_string()),
                (LogChannel::Stdout, "third".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn mock_logs_stream_empty_queue_ends_immediately_when_not_follow() {
        use futures_util::StreamExt;

        let runtime = MockRuntime::new();
        let id = ContainerId::new("logs-empty".to_string(), 0);

        let opts = LogsStreamOptions {
            follow: false,
            ..LogsStreamOptions::default()
        };
        let mut stream = runtime.logs_stream(&id, opts).await.unwrap();

        // Empty queue + follow=false => stream is closed on first poll.
        // Wrap in a short timeout so a regression that hangs would surface
        // as a test failure rather than the test runner stalling.
        let next = tokio::time::timeout(Duration::from_millis(500), stream.next())
            .await
            .expect("stream did not terminate; expected immediate close on empty queue");
        assert!(
            next.is_none(),
            "expected stream to be exhausted, got Some(_)"
        );
    }

    #[tokio::test]
    async fn mock_stats_stream_yields_queued_samples_in_order() {
        use futures_util::StreamExt;

        let runtime = MockRuntime::new();
        let id = ContainerId::new("stats-order".to_string(), 0);

        let now = chrono::Utc::now();
        let mk = |cpu: u64| StatsSample {
            cpu_total_ns: cpu,
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
            timestamp: now,
        };

        runtime.enqueue_stats_sample(&id, mk(100)).await;
        runtime.enqueue_stats_sample(&id, mk(200)).await;
        runtime.enqueue_stats_sample(&id, mk(300)).await;

        let mut stream = runtime.stats_stream(&id).await.unwrap();

        let mut cpus = Vec::new();
        while let Some(item) = stream.next().await {
            cpus.push(item.unwrap().cpu_total_ns);
        }
        assert_eq!(cpus, vec![100, 200, 300]);
    }

    #[tokio::test]
    async fn mock_pull_image_stream_yields_queued_progress_in_order() {
        use futures_util::StreamExt;

        let runtime = MockRuntime::new();
        let image = "alpine:latest";

        runtime
            .enqueue_pull_progress(
                image,
                PullProgress::Status {
                    id: Some("layer-1".to_string()),
                    status: "Pulling fs layer".to_string(),
                    progress: None,
                    current: None,
                    total: None,
                },
            )
            .await;
        runtime
            .enqueue_pull_progress(
                image,
                PullProgress::Status {
                    id: Some("layer-1".to_string()),
                    status: "Downloading".to_string(),
                    progress: Some("[==>  ] 1MB/4MB".to_string()),
                    current: Some(1024 * 1024),
                    total: Some(4 * 1024 * 1024),
                },
            )
            .await;
        runtime
            .enqueue_pull_progress(
                image,
                PullProgress::Done {
                    reference: image.to_string(),
                    digest: Some("sha256:deadbeef".to_string()),
                },
            )
            .await;

        let mut stream = runtime.pull_image_stream(image, None).await.unwrap();
        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            events.push(item.unwrap());
        }

        assert_eq!(events.len(), 3);
        match &events[0] {
            PullProgress::Status { status, .. } => assert_eq!(status, "Pulling fs layer"),
            done @ PullProgress::Done { .. } => panic!("expected Status, got {done:?}"),
        }
        match &events[1] {
            PullProgress::Status {
                status,
                current,
                total,
                ..
            } => {
                assert_eq!(status, "Downloading");
                assert_eq!(*current, Some(1024 * 1024));
                assert_eq!(*total, Some(4 * 1024 * 1024));
            }
            done @ PullProgress::Done { .. } => panic!("expected Status, got {done:?}"),
        }
        match &events[2] {
            PullProgress::Done { reference, digest } => {
                assert_eq!(reference, image);
                assert_eq!(digest.as_deref(), Some("sha256:deadbeef"));
            }
            status @ PullProgress::Status { .. } => panic!("expected Done, got {status:?}"),
        }
    }

    #[test]
    fn log_channel_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&LogChannel::Stdin).unwrap(),
            "\"stdin\""
        );
        assert_eq!(
            serde_json::to_string(&LogChannel::Stdout).unwrap(),
            "\"stdout\""
        );
        assert_eq!(
            serde_json::to_string(&LogChannel::Stderr).unwrap(),
            "\"stderr\""
        );
    }

    fn mock_spec() -> ServiceSpec {
        use zlayer_spec::*;
        serde_yaml::from_str::<DeploymentSpec>(
            r"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
",
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }
}
